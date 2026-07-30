# 交错思考渲染 — 方案设计

> **背景**:同类产品(Claude.ai / Cursor)在前端把 LLM 的一次完整 agent run 渲染成**一条连续流动的消息**:想 → 调工具 → 结果 → 想 → 调工具 → 结果 → 文本。而 EverLasting 当前把每个 LLM turn 渲染成**独立气泡**,气泡内思考/工具/文本分桶固定排序,观感上像是"思考和文本扎堆",形态上不一样。业界把这种连续形态称作"交错思考"(interleaved thinking)。
>
> **结论先行**:**形态差异不是 DB 收集方式导致的**。`messages.content` 列存的是 `Vec<ContentBlock>` 的 JSON 数组,物理上完全能保留块顺序;真正把"交错"压平的是后端攒块落库 + 前端 rehydrate + 渲染这条管线里的两次"分桶"。而且实时流式阶段其实**已经是"一条流"**,问题只集中在 reload 之后。

---

## 0. 全景数据流(现状 vs 目标)

```
现状:
  实时流式:  [user] → [assistant placeholder(整个 run 堆叠)]   ← 已经是一条流
                          ↓ finalize → reloadAfterFinalize
  reload 后:  [user] [asst t1] [user(tr)] [asst t2] [user(tr)] [asst t3]   ← 散成 N 个气泡
  渲染:       每行一个 MessageItem,气泡内 think→tool→text 固定排序

目标:
  实时流式:  [user] → [assistant placeholder(一条流)]          ← 不变
                          ↓ finalize → reloadAfterFinalize
  reload 后:  [user] [asst t1] [user(tr)] [asst t2] [user(tr)] [asst t3]   ← DB 行不变
                          ↓ 渲染分组(新增,纯前端 computed)
  渲染:       [user 气泡]  [assistant run 容器: t1(想→工具→结果) t2(...) t3(文本)]
```

关键点:**DB 行和底层 message 数组都不变**,分组只发生在渲染层,因此 2013 wire-history 不变量(`send()` 从 message 数组重建 Anthropic 历史的 tool_use/tool_result 配对)不受影响。

---

## 1. 根因分析(为什么当前形态不是"交错")

### 1.1 DB 层:能存顺序,没有限制

`messages` 表(`db/migrations.rs:210-242`):

```sql
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  role TEXT NOT NULL,                 -- 'user' | 'assistant'
  content TEXT NOT NULL,              -- JSON: Vec<ContentBlock>
  text TEXT NOT NULL,                 -- 去规范化的可见文本(不含 thinking)
  has_tool_calls INTEGER NOT NULL DEFAULT 0,
  has_tool_results INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  seq INTEGER NOT NULL,               -- per-session 单调递增 turn 索引
  metadata TEXT,                      -- 可空 JSON(injections / edited_at)
  UNIQUE(session_id, seq)
)
```

`ContentBlock` 是个枚举,文本/思考/工具调用/工具结果都是同一个 `content` 数组里的并列成员(`llm/types.rs:73-109`):

```rust
pub enum ContentBlock {
    Text { text, cache_control },
    Thinking { thinking, signature },       // signature 必须原样回传
    RedactedThinking { data },
    ToolUse { id, name, input },
    ToolResult { tool_use_id, content, is_error },
}
```

**结论**:DB schema 物理上完全可以保留下来的块顺序,没有任何限制。一个 assistant turn 落一条行(`persist_turn` 单次 INSERT,`db/sessions.rs`,函数 `persist_turn`)。

#### ⚠ 硬约束:ToolResult 永远不进 assistant message(否则触发 2013)

这是 Anthropic Messages API 的 wire 规范,改动 `ordered_blocks` 时必须守住:

- `ContentBlock::ToolResult` **只能**出现在 **user-role** message 里。
- `ContentBlock::ToolUse` **只能**出现在 **assistant-role** message 里。
- 配对关系(`tool_use_id`)由 **role** 决定,不依赖块在数组内的顺序。

证据见前端 wire 重建函数 `toPayloadContent`(`chat.ts`,assistant 分支显式注释 "Intentionally omit `m.toolResults` — they're for the UI, not the wire";user 分支才放 `tool_result`)。后端 `persist_turn` 落库时同理:`assistant_blocks` 只装 `Thinking`/`Text`/`ToolUse`/`RedactedThinking`,绝不装 `ToolResult`;tool_result 单独包成 user-role 的 `ChatMessage` 喂给下一轮 LLM(`chat_loop.rs` 里 `messages.push(tool_result_msg)`)。

**这是为什么本方案只讨论"assistant turn 内部 thinking/text/tool_use 的顺序",ToolResult 的归属(永远在独立 user 行)从一开始就钉死,不在 `ordered_blocks` 的讨论范围内。**

### 1.2 第一处压平:后端攒块(真正的元凶之一)

`chat_loop.rs:1660-1666` 每个 turn 用的是**按类型分桶的累加器**:

```rust
let mut text_parts: Vec<String> = Vec::new();
let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
let mut finalized_thinking: Vec<(String, String)> = Vec::new();   // (text, signature)
let mut redacted_thinking_data: Vec<String> = Vec::new();
```

然后在 `chat_loop.rs:1976-2019` 拼块时**硬编码顺序**:先全部 thinking → 再一个 text → 再全部 tool_use → 再 redacted。哪怕模型实际流式输出是 `[think → tool → think → tool]`,落库时也会被重排成 `[think, think, tool, tool]`。**流式顺序在这里被丢掉了。**

### 1.3 第二处压平:前端类型本来就是分桶的

`chat.types.ts:173-264` 的 `ChatMessage` 是**平行的类型化数组**,根本没有一个有序的 `content_blocks`:

```ts
export interface ChatMessage {
  content: string;                       // 只有文本
  toolCalls?: ToolCallInfo[];
  toolResults?: ToolResultInfo[];
  thinkingBlocks?: ThinkingBlockInfo[];
  redactedThinkingData?: string[];
  // ... 没有 ordered content_blocks
}
```

`rehydrateMessages`(`streamController.ts:424-688`)把 DB 的块数组按类型 push 进各自桶里;`MessageItem.vue:1050-1292` 再以固定顺序渲染:**思考折叠块 → redacted 计数 → 工具卡列表 → 文本气泡**。所以单条气泡内永远是"思考扎堆在上、工具扎堆在中、文本在下",不会出现"想一下、调一下"的流水形态。

### 1.4 关键洞察:实时流式阶段已经是"一条流"

`handleChatEvent`(`streamController.ts:877-1366`)在整个 run 期间始终 mutate 同一个 assistant placeholder(`msgs[len-1]`),turn1/turn2/turnN 的 think/tool/result/text 全部堆叠上去:

- `case "start"`(`streamController.ts:886-906`):turn2/turn3 只 `req.currentTurnIndex++`,**不新建 message**。
- `case "delta"` / `thinking_delta` / `tool:call` / `tool:result`:全部 append 到同一个 placeholder 的各桶。

**所以实时态已经是一条流。** 问题只在 finalize:`reloadAfterFinalize`(`streamController.ts:1726-1852`)调 `load_session` 把单个流式占位替换成 N 条 per-turn DB 行,于是"一条流"散成了多个气泡。

### 1.5 为什么单 turn 内"保留流序"几乎不改变观感

Anthropic 一旦发出 `tool_use`,本轮 `stop_reason` 即为 `tool_use`,本轮结束(`chat_loop.rs:2201`)。所以单 turn 内根本到不了 `[think→tool→think→tool]`。真正让 Claude.ai/Cursor 看起来"想一下调一下"的,是**跨 turn 的连续时间轴渲染**——而这正是方案要解决的。

---

## 2. 约束与边界

- **DB 表不动**:不加 `request_id` 列。reload 后无法靠 request_id 分组,改用**邻接启发式分组**(见 §3.4)。
- **后端落库保留流序**:把 `chat_loop.rs` 的"分桶硬编码排序"(函数 `run_chat_loop` 内的块拼装)改成按真实流序 push。DB schema 不变(content 本就是 JSON 数组)。
- **2013 wire-history 不变量必须保住**:`send()` 从 message 数组重建 Anthropic 历史时,`tool_use`/`tool_result` 配对必须仍可还原(`chat.ts` 函数 `toPayloadContent`)。分组只发生在**渲染层**,底层 message 数组仍保留每条独立行。注意:`toPayloadContent` 内部也是硬编码顺序(thinking→text→tool_use→redacted),即使落库按流序,wire 重建仍会被压平——**这是预期行为,无需改 `toPayloadContent`**(渲染层用 `contentBlocks` 字段,wire 层用旧分桶,互不影响)。
- **request_id 不持久化(首版决策)**:`request_id`(`rid`)在前端生成,整个 agent loop 的多 turn 共享同一个 rid,但**从不写进 DB**(`persist_turn` 不接收它,`metadata` 只存 `injections`/`edited_at`)。首版选邻接启发式而非 rid 分组。rid 方案见 §3.6(可选增强)。

---

## 2.5 MVP 选项:只改实时态(成本最低)

在动手做完整方案前,值得单独评估这个最低成本路径:

- **实时态已经是"一条流"**(§1.4 已论证,`case "start"` 不新建 message,整个 run 堆到一个 placeholder)。
- 用户最常看的是**当前 run**,历史会话 reload 后"散成多泡"是次要体验。
- 若只保留实时态连续、reload 后维持现状,**几乎零后端改动**:只需确认 `case "start"` 不破坏 placeholder(已确认),无需改落库顺序、无需改 rehydrate、无需新组件。

这是真正的最小可行版本。若用户反馈"历史会话也要交错",再上完整方案(§3)。**建议先验证这个 MVP 的用户感知差异是否可接受**,再决定是否投入完整改造。

## 3. 方案(全 agent run 合并成一条流 + 后端保留流序 + DB 不改表)

### 3.1 后端:落库保留流序(1 处核心改动)

**文件**:`app/src-tauri/src/agent/chat_loop.rs:1660-1666, 1976-2019`

**现状**:4 个分桶累加器,在 `:1976` 按固定顺序拼装。

**改为**:增加一个 `Vec<ContentBlock>` 的 `ordered_blocks` 累加器,在每个事件处理点(`:1761-1805`)按真实流序 push。保留 4 个分桶累加器供现有逻辑(text join / cancel marker / tool 执行)使用,但最终落库时用 `ordered_blocks`。

关键点:
- `Delta` 事件:现有逻辑是逐段 push 到 `text_parts`。需改成在 `ordered_blocks` 追加/更新一个 Text 块(相邻文本合并,或保留多块——Anthropic 都接受)。
- `ToolCall` / thinking flush:按到达顺序 push 进 `ordered_blocks`。
- cancel/error marker(`:1984-2003`):仍需拼到末尾——在 `ordered_blocks` 的最后一个 Text 块或新增 Text 块上追加 marker,而非单独分桶。
- 落库时(`:1976`):用 `ordered_blocks` 替代手工拼装的 `assistant_blocks`。

**为何安全**:Anthropic 的 signature 是 per-block 的,顺序不影响 round-trip;`to_text()`(`llm/types.rs:133`)遍历所有 Text 块求和,顺序无关。

**测试影响**:`tests_agent_loop.rs` 的多 turn 测试只断言"persist 了几行 + TurnComplete.seq",不锁块顺序——应能通过。`db/messages_tests.rs:59-99` 的 canonical 示例手工构造块,顺序由调用方决定,不受影响。

### 3.2 前端 A:`rehydrateMessages` 输出有序结构

**文件**:`app/src/stores/streamController.ts:424-688`

**现状**:把 DB 的 `content` 数组按类型分桶到平行数组。

**改为**:在分桶之外,**额外保留一个有序 `contentBlocks` 字段**(按 DB content 数组原序透传)。平行数组保留(现有组件依赖),只是新增一个顺序源。这样 reload 后的渲染就能拿到真实顺序。

### 3.3 前端 B:`ChatMessage` 类型加 `contentBlocks`

**文件**:`app/src/stores/chat.types.ts:173-264`

新增字段(可选,有则用):

```ts
/** 有序内容块(reload 后从 DB content 数组透传,用于交错渲染)。
 *  缺省时回退到分桶数组的固定排序。 */
contentBlocks?: ContentBlockView[];
```

`ContentBlockView` = `{ kind: "text"|"thinking"|"tool_use"|"tool_result"|"redacted"; ... }` 的判别联合。

### 3.4 前端 C:`MessageList.vue` 渲染分组(核心 UX 改动)

**文件**:`app/src/components/chat/MessageList.vue:41-50, 207-213`

**新增 computed `renderGroups`**:把 `visibleMessages` 按 run 分组。基础规则:
- 一条**真·用户输入**消息 → 开启新 run。
- 紧随其后的 assistant 行 + 后续的 ghost/orphan user 行 → 归入同一 run。
- 下一条真·用户输入 → 新 run。

```
输入: [user(text)] [asst] [user(tr)] [asst] [user(tr)] [asst] [user(text)] [asst]
分组:  └── run 1 ─────────────────────────┘  └── run 2 ──┘
```

#### ⚠ 关键判据:如何区分"真·用户输入" vs "ghost user(tool_result)"

reload 后 user-role 行有两种来源,形态上很像,必须用**可靠判据**区分,不能只用 `content === ""`:

| 行类型 | 产生位置 | 判据 |
|---|---|---|
| **真·用户输入** | 用户回车发送 | 开启新 run |
| **ghost user(tool_result)** | DB 里 assistant tool_use 后跟随的 user(tool_result) 行 | `m.toolResults?.length > 0` → 归入前一个 assistant 的 run |
| **orphan repair synthetic** | rehydrate 的 orphan-repair 步骤 splice 进来 | `m.id.endsWith("-orphan-repair")` → 归入前一个 assistant 的 run |

要点:
1. **`m.toolResults?.length > 0` 是 ghost 的充分判据**。merge step(`rehydrateMessages` 里的 merge 循环)是**复制不是移动**——把 user 行的 toolResults 复制到前一个 assistant 后,**user 行自身的 toolResults 仍在**。所以 reload 后 ghost user 行仍带 toolResults。真·用户输入不会有 toolResults。
2. orphan-repair synthetic 行的 id 形如 `${m.id}-orphan-repair`,`content === ""` 且带 isError 的 toolResults,必须识别后归入同 run。
3. `content === ""` **不可靠**(用户理论上可能发空消息后被 prompt 填充),故以 `toolResults` / `id` 后缀为准。

> 误判后果清单(均为**渲染层回退**,底层 message 数组不变,不会丢数据):
> - resend 后:DB 新增独立 user 行 + 完整新 run,正确开启新 run(符合预期)。
> - edit 后:同 seq 的 assistant 后继仍在原 run,分组不断裂。
> - orphan-repair 归错:最坏多分一个 run 气泡,回退现状观感。

**渲染**:`<MessageItem v-for>` 改成 `<MessageRunGroup v-for>`(新组件),内部把同 run 的多个 assistant 行的 `contentBlocks` 按序拼接渲染成一条流:

```
想(2.3s) → 🔧 read 0.3s✓ → 结果 → 想(1.1s) → 🔧 grep 0.5s✓ → 结果 → 最终文本
```

复用现有组件:`ThinkingBlock`、`ToolCallCard`、气泡——只是排列方式从"气泡内分桶"变成"按 contentBlocks 顺序流"。

### 3.5 实时流式阶段:已基本就绪

`handleChatEvent` 已把整个 run 堆到一个 placeholder 上。需确认:`finalize` 后不要破坏——即 `reloadAfterFinalize` 替换成 DB 行后,新的 `renderGroups` computed 能把它们重新归并成同一条流。**所以实时态的"一条流"会被 reload 短暂打散,再由渲染层重新合并**——视觉上是连续的。

### 3.6 可选增强:metadata.rid 分组(非首版)

首版用邻接启发式。若后续发现 reload 历史会话分组误判率不可接受,可升级为 rid 分组:

- `messages.metadata` 列已是 free-form JSON(向前兼容,无需改 schema),可塞 `{request_id: rid}`。
- rid 是 agent loop 跨 turn 共享的同一字符串,是天然分组 key——resend 天然新 run、orphan-repair 同 rid、无需识别 synthetic id。

**但首版不上的原因**(评审高估了它的收益):
1. rid **只改善 reload 历史会话的分组准确性**,不解决实时态——而实时态本就是一条流。
2. rid **不解决 §6.4 的 UX 闪屏**(finalize→reload→重分组,与分组 key 无关)。
3. 成本是**后端核心路径散弹改动**:`persist_turn` 加 rid 参数 + 所有 persist 站点(7+ 处)plumb rid + metadata 写入 + 前端 rehydrate 读取。
4. 邻接启发式 + §3.4 的明确判据(`toolResults`/`orphan-repair` 后缀)已覆盖已知误判场景。

**取舍:rid 为主 + 启发式为旧数据 fallback 的组合是理想终态,但首版优先启发式(改动面小一个数量级),rid 留作后续增强。**

---

## 4. 数据流总览(改造后)

```
实时流式:  [user] → [assistant placeholder(整个 run 堆叠)]   ← 已存在,不变
                          ↓ finalize
reload/rehydrate:  [user] [asst t1] [user(tr)] [asst t2] [user(tr)] [asst t3]   ← DB 行
                          ↓ 渲染分组(新增,纯前端 computed)
渲染层:    [user 气泡]  [assistant run 容器: t1(想→工具→结果) t2(想→工具→结果) t3(文本)]
```

渲染分组是**纯前端 computed**,不污染底层 message 数组,2013 不变量不受影响。

---

## 5. 测试改动

> 行号会随 commit 偏移,**下面用 describe/it 标题作语义定位**,行号仅作辅助锚点。

| 测试块(按标题定位) | 影响 | 处理 |
|---|---|---|
| `describe("finalizeRequest (… step-4 follow-up — 2013 wire invariant)")` | **不破**——测的是 buffer eviction + diff cache 清理(`it("evicts the in-memory message buffer…")` / `"invalidates the chat store's diff cache…"`),标题虽含 "2013" 但**不锁 wire 重建形态**。分组只在渲染层,wire history 重建不受影响 | 无需改 |
| `describe("rehydrateMessages — orphan tool_use repair (BUG FIX 2013)")` + `describe("… existing merge step is preserved")` | **不破**——这些操作仍在 message 数组层(merge 是复制非移动;orphan-repair splice synthetic) | 新增 case 应落在 rehydrate 测试段,断言"reload 后 contentBlocks 有序透传" |
| `tests_agent_loop.rs`(per-turn persist 系列:`agent_loop_tool_use_triggers_tool_result_turn` 等) | **不破**——只锁"persist 了几行 + TurnComplete.seq",不锁块顺序 | 若后续加了"块顺序"断言,需改为按流序 |
| 新增:`MessageList` 分组单测 | 新增 | 覆盖:单 run、多 run、ghost user 判据、orphan-repair 归属 |
| 新增:`MessageRunGroup` 组件测试 | 新增 | 覆盖交错渲染顺序 |

---

## 6. 退路与风险

1. **启发式分组可能误判**:resend/edit/中间穿插的 user 消息可能切断 run。缓解:分组逻辑只影响**渲染顺序**,底层 message 数组不变,误判最坏只是"多分了几个 run 气泡",回退到现状观感,不会丢数据(详见 §3.4 误判后果清单)。
2. **后端流序改动触及落库核心路径**:`persist_turn` 是 RULE-A-003(失败必须 emit Error + abort)的落点。改动后需回归 cancel/error/max_turns 三条异常路径,确认 marker 仍正确拼到流序末尾。**error/cancel marker 应作为 `ordered_blocks` 里的一个独立 Text 块**(单独显示,不混进前文),而非 append 到最后一个 Text 块——对齐现有 `MessageItemFooter` 的 retry 提示范式。
3. **DB 兼容**:旧数据(content 数组是旧分桶顺序)在新渲染下会按旧顺序显示——视觉与现状一致,不会更差。新对话享受交错。
4. **UX 闪屏(已知,需设计过渡)**:实时态结束后 `reloadAfterFinalize` 用 N 条 DB 行替换流式 buffer,`<TransitionGroup key="m.id">` 会触发"1 大泡 → N 小泡 → run 容器"的三段视觉闪。缓解选项(任选其一,非阻塞):
   - finalize 后用 run 容器骨架占位作过渡;
   - 给流式 placeholder 预置 `${sid}-${seq}` 形态的 id,让 TransitionGroup 感知"同组";
   - 在 finalize 前就把 placeholder 视为"未来 N 个 turn 的临时 run 容器",reload 后无缝替换。
5. **灰度可行**:渲染分组是 computed,可用 feature flag 包裹(建议命名 `interleaved_thinking_render`);后端流序改动可独立 ship(reload 后旧 session 不受影响)。回滚点:`renderGroups` computed 短路、回到 `visibleMessages` 直接 `v-for MessageItem`。
6. **subagent 路径无需特殊处理**:worker 用 `skip_persist=true`,中间 turn 不落库,父进程只看 `dispatch_subagent` 的 tool_use/result,与普通工具同等处理。

---

## 7. 执行顺序(建议)

0. **先验证 MVP(§2.5)**:确认实时态已是"一条流"、`case "start"` 不破坏 placeholder。评估"只改实时态"的用户感知差异是否可接受——若可接受,可能根本不需要完整方案。
1. **后端流序**(独立可 ship,风险集中):改 `chat_loop.rs` 累加器 + 落库,回归 3 条异常路径(cancel/error/max_turns)+ 跑 agent loop 测试。
2. **前端类型 + rehydrate**:`contentBlocks` 字段 + 透传,加单测。
3. **渲染分组**:`renderGroups` computed + `MessageRunGroup` 组件,加单测(覆盖 §3.4 的 ghost/orphan 判据)。
4. **验收**:发一个多 turn 工具调用的对话,实时看"一条流",reload 后看"一条流"。

每步独立可验证、可回滚。

---

## 8. 关键文件索引

**后端(落库流序)**
- `app/src-tauri/src/agent/chat_loop.rs:1660-1666` — per-turn 分桶累加器
- `app/src-tauri/src/agent/chat_loop.rs:1761-1805` — 事件 switch(accumulate)
- `app/src-tauri/src/agent/chat_loop.rs:1976-2098` — 块拼装 + persist(**THE flatten**)
- `app/src-tauri/src/agent/thinking.rs` — `PendingThinking` + `flush_pending_thinking`
- `app/src-tauri/src/db/sessions.rs:692-764` — `persist_turn`(单次 INSERT)
- `app/src-tauri/src/llm/types.rs:73-109` — `ContentBlock` 枚举
- `app/src-tauri/src/llm/types.rs:133` — `to_text()`(Text 块求和,顺序无关)

**前端(渲染分组)**
- `app/src/stores/streamController.ts:424-688` — `rehydrateMessages`(分桶)
- `app/src/stores/streamController.ts:877-1366` — `handleChatEvent`(实时态已合并)
- `app/src/stores/streamController.ts:1726-1852` — `reloadAfterFinalize`(打散成 per-turn 行)
- `app/src/stores/chat.types.ts:173-264` — `ChatMessage`(平行数组,无有序字段)
- `app/src/components/chat/MessageList.vue:41-50, 207-213` — flat v-for,无分组
- `app/src/components/chat/MessageItem.vue:1050-1292` — 渲染顺序(think→tool→text 固定)

**测试**
- `app/src/stores/streamController.test.ts` — `describe("finalizeRequest (… step-4 follow-up — 2013 wire invariant)")`(测 buffer eviction / diff cache,标题含 2013 但**不锁 wire 重建**)+ `describe("rehydrateMessages — orphan tool_use repair")` / `describe("… existing merge step is preserved")`(orphan/merge,与分组逻辑相关)
- `app/src-tauri/src/agent/tests_agent_loop.rs` — 多 turn persist 测试(`agent_loop_tool_use_triggers_tool_result_turn` 等,只锁行数 + seq,不锁块顺序)
