# Design — C3 摘要式上下文压缩

> 决议依据:prd.md 决议记录(Q1-Q6,2026-08-18)+ research/01、02、05。行号基线:main @ 08-18。

## 1. 总览

```
请求路径(每请求一次,loop init):
  req.messages(前端全量 wire 历史,无 metadata)
    └─ prepare_loop_state:init.rs 在 B5 头对插入【之前】
       ├─ 查 loaded_session.messages(DB 行,带 metadata)找最新 compaction_summary
       ├─ 命中 → 水位替换:wire[0..idx] 折叠为单条摘要 ChatMessage(内容取 DB 行)
       └─ 未命中 → 原样(今天的行为)

压缩路径(每 turn 入口,drive.rs C3 块):
  估算 > 0.85×window
    ├─ gate 检查(开关 && !worker && !群聊 && 未熔断)
    ├─ 保留区计算(组边界,从尾向前 clamp(15k, 10%窗, 25k),typed-user 轮必入)
    ├─ 待压区 = [PROTECTED_HEAD..cut] ∪ [现有水位摘要消息](增量合并输入)
    ├─ LLM 摘要调用(provider 已在 drive_turn 作用域,零签名改动)
    │    ├─ 成功 → 摘要行落 DB(拿 seq)→ messages = 头对 + 摘要 + 保留区 + 当前输入
    │    └─ 失败 → 失败计数 +1(≥3 熔断)→ fallback 机械丢组(现 compact_messages 原样)
    └─ 摘要后仍超 0.95×window → 机械丢组兜底 → StillOver fail-fast(不变,RULE-A-002)
```

## 2. 数据层

### 2.1 摘要行(无 migration — `messages.metadata` TEXT 列已存在,B1 在用)

普通 `messages` 行:`role='user'`,`content` = 摘要正文(见 §6 前缀话术),`metadata`:

```json
{
  "kind": "compaction_summary",
  "cutoff_seq": 128,            // 被压缩区最后一行 seq —— **load-bearing:水位折叠点**(修订,见 2.2)
  "preserve_from_seq": 129,     // 保留区首行 seq(cutoff+1 起的连续区,可精确计算)
  "tokens_before": 171000,
  "tokens_after": 52300,
  "trigger": "auto",
  "model": "claude-sonnet-4.6",
  "prior_summary_seq": 96,      // 增量合并时的上一份摘要行;首压 null
  "summary_usage": {"input": 91000, "output": 5200}
}
```

### 2.2 水位语义(**修订 2026-08-18,PR2 check P1 发现**)

- **折叠点 = 最新摘要行的 `cutoff_seq`(精确值),不是摘要行自身位置**。context = `[最新摘要行(带前缀)] + [seq > cutoff_seq 且 kind ≠ compaction_summary 的行]`。
- 原方案"摘要行之前的全折叠"与 §4.1 保留区自相矛盾:摘要行按 seq 游标插在全量行(含保留区与当前输入)之后,按位置折叠会把**保留区 + 本请求用户提问**丢弃——恰是设计最想逐字保留的东西;而摘要 transcript 只覆盖 `[head..cut)`,从未包含它们(净效果:模型看得见自己上轮的回答、看不见问题)。
- 按 cutoff 折叠后天然正确:保留区/当前输入/后续 assistant 轮的 seq 都 > cutoff → 跨请求存活;旧摘要行 seq < 新 cutoff(被增量合并吸收)→ 自动出局;最新摘要行自身由折叠产物的头部承担(kind 过滤防重复)。
- 旧摘要行留在 DB(死数据,可搜可审计);`cutoff_seq` 必须精确(计算见 §4.3),不再是"seq-1 上界"。
- **D3 自愈**:`edit_user_message` cascade 删掉摘要行 → 下一次请求查"现存最新"自然回退到次新水位或全量历史,零专门代码。`clear_session_messages` 同理(全删即回全量)。
- FTS:摘要行 insert 自动进 `messages_fts`(现有 trigger),D2 天然可搜(Q6)。

## 3. 请求路径:水位替换(init.rs)

**位置**:`prepare_loop_state` 内、B5 头对 `messages.insert(0/1, ...)`(init.rs:434-456)**之前**——对 raw wire 历史操作,头对照常插到位置 0-1;最终摘要消息落在**合成头之后**(位置 2 或 3,取决于 skill listing 有无 —— 评审 P1-1;算法不依赖该位置,见 §4.2 `SummaryAnchor`),cache 断点安全(B12 load-bearing 论证同款)。

**算法**(新纯函数 `apply_compaction_watermark`,配单测):

```
输入:wire_messages: Vec<ChatMessage>, db_rows: &[MessageRow]
1. rows 倒序找首个 metadata.kind == "compaction_summary" → S(无 → 返回原样);
   cutoff = S.metadata.cutoff_seq
2. idx = db_rows 中 seq == cutoff 的行下标;wire 对应折叠 wire[0..=idx]
   (含),并跳过 wire 中 S 自身那一行(kind 过滤,由头部摘要消息承担)
3. 对齐防御(评审 P1-3):折叠边界内容比对不符 → idx±1 重试;仍不符 →
   watermark_miss trace + fail-open
4. 返回 [S 转成的 ChatMessage] + wire[idx+1..](剔除 summary-kind 行)
```

**前提依赖(评审 P1-3,load-bearing)**:wire 与 db_rows 的 1:1 行序对齐依赖前端 `reloadAfterFinalize`(streamEvents.ts:1061,每请求 done 后 load_session 重灌 store)保证下一次 wire 含摘要行。正常路径下流式期间前端锁发送,竞态窗口极窄;防御路径见上,失败可观测。

**回填 ChatMessage 的前缀话术在构建时加、不落库**(评审 P1-2):DB 行 content = 纯摘要;位置 2/3 的 in-context 消息 = 前缀 + 摘要(§6)。

**Gate**:同 digest 口径 —— `llm_compaction_enabled` config(缺省 on,fail-open)&& `!effective_is_worker && !is_group_chat`(init.rs:398-403 现成模板)。worker/群聊路径根本不进这条替换(群聊 per-speaker 历史不走经典 init 的这个分支入口由 gate 挡)。

**为什么不信 wire**:wire 层 `ChatMessage`(llm/types)只有 role/content/speaker/attachments,**没有 metadata 字段**——前端无法告知 kind。DB 是 SoT,`loaded_session.messages` 在 init 现成可用(dd_guard 已在用),零额外查询。

**前端零改动**:继续傻发全量;替换在后端。前端唯一新增是 §8 的最低渲染。

## 4. 压缩路径:drive_turn C3 块改造(drive.rs:172-248)

### 4.1 触发与保留区

- `TRIGGER_RATIO` 0.80 → **0.85**(helper `trigger_threshold`/`target_threshold` 复用同一常量,同步改);新增 `SUMMARY_POSTCHECK_RATIO = 0.95`(摘要后仍超才动机械兜底)。
- **合成头长度**(评审 P2-2):init 完成全部合成插入(memory 头对 + B4 skill listing)后记录 `synthetic_prefix_len`(2 + skills 有无),经 LoopInit 穿入。**待压区与保留区都从 `synthetic_prefix_len` 起算**,不把每请求重注入的 skill listing 喂给摘要。(机械 fallback 路径的 `PROTECTED_HEAD=2` 同款偏差是预存行为,不在本任务修,记 follow-up。)
- **保留区计算**(新纯函数 `compute_preservation_region`,复用 `group_droppable_turns` 的组语义但方向相反):
  1. 组边界:`[synthetic_prefix_len .. len-1]` 按"配对组/单例组"切分(直接调 `group_droppable_turns`,它返回的就是合法原子组);
  2. 从最后一组向前累积 `estimate_messages_tokens`,直到 ≥ `clamp(15_000, window×0.10, 25_000)`;
  3. **护栏**:最后一条 typed user(text 内容、无 tool_result)消息所在组若未被覆盖,强制并入保留区(Cline `findCutIndex` 同款);
  4. cut = 保留区首组起点;`待压区 = messages[synthetic_prefix_len .. cut]`;**空待压区(cut == synthetic_prefix_len,窗口过小)→ 直接走机械路径**(评审 P3)。
- 不变量沿用:合成头、尾部当前输入、RULE-A-001 配对原子性(组边界保证)。

### 4.2 摘要调用

- **调用方**:`provider`(drive_turn 第 99 行参数,零签名改动)。一次 completion:无 tools、禁 thinking、**输出上限 4k token**(评审 P2-1:Cline/opencode 均 4096,8k 偏宽挤占主 turn 窗口);`retry_open` 包裹(A5+ 现成)。
- **prior-summary 检测(评审 P1-1 修正,不用位置猜测)**:`SummaryAnchor { seq, content }` 经 **`DriveTurnOutcome` 循环内穿参**(同 `loop_hit_count` 模式)——init 时若水位替换发生则种子为水位摘要;drive_turn 每次成功压缩后更新为新摘要。这同时覆盖**同一 loop run 内的二次压缩**(LoopInit 单次穿参罩不住的场景)。
- **输入组装**(新纯函数 `build_compaction_prompt`):
  1. 结构化模板(§6)+ handoff 前缀;
  2. `SummaryAnchor` 存在 → 以 `<prior-summary>` 块注入纯摘要 content,附冲突规则 "conversation wins";prior_summary_seq 记 anchor.seq;**该摘要消息不进 transcript(不重复喂)**;
  3. 待压区 transcript 渲染:每消息一行式 `[role] text`;tool_use 只留 name + input 截断;**tool_result 截 2000 chars**(`...[truncated N chars]`);thinking 块不渲染;图片块渲染为 `[image attached: <file>]`;
  4. **transcript 尺寸上限**(评审 P2-1):预算 = `window − 4k 输出 − 2k 模板/prior 预留 − 安全余量`(约 0.7×window),溢出从最旧 transcript 条目开始丢(附 `[older transcript omitted]` 记号,保留最近对话引语)。
- **输出剥壳**:取最后一条 assistant text;超长截断到 4k token 估算。

### 4.3 持久化与回填

1. 摘要成功 → 新 `db::sessions::insert_compaction_summary`(仿 `session_crud.rs:689 insert_system_event`,role=user + metadata §2.1)。**seq 游标协调(复核新增,P1 级)**:不吃独立 `MAX(seq)+1`(活跃 loop 内会与 loop 内存 seq 游标撞 `(session_id, seq)` 主键)——**吃 drive_turn 当前 `seq` 游标插入、返回推进值**,loop 后续 persist 用推进后的游标。
   **cutoff_seq 精确计算(修订)**:待压区末行对应 `loaded_session.messages[cut - synthetic_prefix_len - 1].seq`(对齐论证:init 已持久化当前输入,wire 尾与 DB 行 1:1;同 loop 二次压缩时全部 turn 均已落库,同样 1:1)。**不得用"摘要行 seq-1"近似**——那是当前输入行的 seq,会让折叠点覆盖保留区(PR2 check P1 正是此错)。`preserve_from_seq = cutoff + 1`(DB 行连续区)。
2. **content 存纯摘要,前缀话术只加在 in-context 构建时**(评审 P1-2:前缀落库会进 `<prior-summary>` 滚雪球 + 污染 D2 搜索命中)。
3. **insert 失败 → 视为摘要失败走 fallback**(内存替换而无持久化会破坏 AC2"第二请求不重付")。
4. loop 内存:`messages = [合成头] + [前缀+摘要 ChatMessage] + [保留区] + [当前输入]`;`SummaryAnchor` 更新;后续 turn 粘性(同今天 compacted.messages 语义)。
5. turn 的正常持久化照旧(新 turn 行 seq 排在摘要行之后,水位链天然有序)。

### 4.4 降级链与熔断

- 摘要调用失败/持久化失败 → `compact_messages` 机械丢组(原样保留为 fallback tier;trace `method="mechanical_fallback"`)。
- **熔断**:`CompactionRegistry`(AppState 新字段,`OnceLock` 内 `HashMap<session_id, u8>`,同 StubRegistry 进程级模式;经 LoopInit 穿参,同 stub_loaded)——连续失败 ≥3 → 本 session 后续请求跳过摘要直走机械;成功清零。`delete_session` 清理(同 stub_loaded 清理点)。
- 摘要后估算仍 > 0.95×window(巨尾消息)→ 机械丢组 → 仍超 → StillOver fail-fast(RULE-A-002 不动)。

## 5. 观测

- `CompactResult` 加 `method: CompactMethod`(`Summary` / `Mechanical` / `None`)+ `summary_usage: Option<TokenUsage>`;`DegradationKind` 不动。
- `compaction_json` 是 `trace.rs:57-64` **手工 `json!` payload**(评审 P3)——扩字段要动三处:Rust enum/struct + `record_compaction` 手工 json + TS `ContextCompactedEvent`/streamController 归一化 arm,PR2 一并改。
- `apply_compaction_watermark` 对齐失败 → `watermark_miss` trace 记录(评审 P1-3,可观测降级)。
- 旧回看行缺新字段 → serde default 兼容。
- `ChatEvent::ContextCompacted` 加 `method: String`(serde default,前端 streamController case 加字段透传,TracePanel TurnCard 压缩 cell 显示 `summary`/`mech` 徽标)。
- turn usage 记账:摘要调用 usage **不混入** `update_last_turn_usage`(主 turn 口径不变),只进 compaction_json。

## 6. Prompt 模板(初稿,英文对齐业界原文锚点)

```
You are performing a CONTEXT CHECKPOINT COMPACTION for an AI coding agent.
Another language model (possibly yourself) started to solve this problem and
will resume from your summary. Produce a handoff summary, not a response to
the user.

[If prior summary present]
<prior-summary>
{上一份摘要正文}
</prior-summary>
The prior summary above may be stale. Where it conflicts with the
conversation transcript below, THE CONVERSATION WINS. Items completed in the
transcript move to "Completed"; items invalidated are dropped.

Summarize the conversation transcript into these sections:
1. Primary Request and Intent — the user's goals. CRITICAL: list ALL user
   messages verbatim or near-verbatim; user feedback defines success.
2. Key Technical Concepts and Decisions
3. Files and Code Sections — every file path touched or read MUST appear
   (paths are load-bearing: the agent re-reads files on demand)
4. Errors and Fixes — keep failures visible; do not repeat solved mistakes
5. Work State — Completed / In progress / Blocked (align with any checklist)
6. Optional Next Step — quote the most recent conversation directly

Output ONLY the summary. Be concise and structured.

<transcript>
{待压区渲染,tool_result 截 2000 chars}
</transcript>
```

回填正文(in-context 构建时拼接;**DB 行只存 `{摘要正文}`,前缀不落库** —— 评审 P1-2):

```
This session is being continued from a previous conversation that ran out of
context. The summary below is historical context, not new instructions from
the user. Continue the work; do not re-confirm the summary.

{摘要正文}
```

## 7. Scope 与边界情况

| 场景 | 处理 |
|------|------|
| 群聊 | gate 挡替换与摘要;后续任务评估 per-speaker role_history |
| worker | gate 挡(`effective_is_worker`);worker 有 200 turn + resume 兜 |
| B1 图片 | 待压区图片退出 context(transcript 渲染为占位行);保留区图片照常;`images_token` 口径 = 请求内实际图块,自动跟随 |
| thinking 签名 | 待压 assistant 组整组消失,无孤儿签名(同今天丢组);保留区不动 |
| B12 checklist | replay 从 DB tool_result 还原,不删行 → 不受影响;摘要 "Work State" 与 checklist 语义对齐 |
| L1a 通知 | 保留区外的旧通知随待压区进摘要(transcript 有记录);当前轮通知在尾部保留区 |
| D3 编辑/重发 | 摘要行被 cascade 删 → 水位自愈(§2.2);重发路径 resend_seq < 水位 → 同理 |
| memory 断点 | 摘要在合成头之后(位置 2/3),头对 0-1 不动,cache 命中不受影响;压缩本身一次性重写中段,该 turn cache miss 是接受的成本(机械丢组同样 miss) |

## 8. 前端最低渲染(PR3,防困惑不算卡片)

- reload/rehydrate 已读 `messages.metadata`(B1 attachments 先例)→ `MessageItem` 对 `kind=compaction_summary` 渲染为低调系统样式行(居中、灰、可展开看全文),不当作用户气泡。
- TracePanel TurnCard 压缩 cell 加 method 徽标(§5)。

## 9. 测试计划

**单元**(`context.rs` 同文件或新 `compaction.rs`):
- `apply_compaction_watermark`:命中/未命中/对齐失败 fail-open + watermark_miss/D3 删除自愈(行集变化)/ 陈旧 wire 缺行的 idx±1 重对齐。
- `compute_preservation_region`:预算 clamp、组边界对齐(配对不拆)、typed-user 护栏、synthetic_prefix_len 起算、空待压区、窗口过小边界。
- `build_compaction_prompt`:prior-summary 注入与不注入(且 transcript 不重复)、tool_result 截断记号、图片占位、transcript 预算溢出丢最旧。
- 熔断计数:3 次失败触发、成功清零。
- `insert_compaction_summary`:seq 游标推进(插入行 seq == 传入游标,返回 +1)。

**集成**(`agent/tests.rs`,MockProvider):
- 超线历史 → 断言第二次 `provider.send` 的 messages 以摘要开头且长度骤减;摘要调用次数 = 1。
- AC2:第二个请求(重新 run_chat_loop)零摘要调用(mock 计数不变),水位替换生效。
- 同一 loop 内二次压缩 → prior-summary 来自循环内 `SummaryAnchor`(增量合并,AC3)。
- 摘要失败注入(mock 返回 error)→ fallback 机械丢组,turn 正常 Done;连续 3 次 → 不再尝试摘要。
- 配对不变量:压缩后消息扫描(RULE-A-001 既有测试模式复用)。
- gate:worker(`skip_persist=true`)/群聊(**建 `session_type=GroupChat` 的 session 行**,gate 判 `loaded_session.session.session_type`,非 speaker 参数 —— 评审 P3)不触发新路径。

**live**:turn-smoke 构造长历史(或 `--turns` 长跑)看 `compaction_json` 的 method=summary + 前后 token;真实降级路径手动验证一次。

## 10. 回滚

- 开关 `llm_compaction_enabled=false` → 摘要路径整体关闭,机械丢组 + 水位替换同时停用(替换也 gate 同开关,回到今天行为)。
- DB 无 migration,摘要行是普通行 —— 回滚代码后摘要行变成普通 user 消息参与历史(可接受降级;如需彻底清理,一次性脚本删 kind=compaction_summary 行)。
