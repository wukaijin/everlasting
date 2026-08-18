# Review — C3 摘要式上下文压缩任务评审(2026-08-18)

> 评审人:ZCode(对照 main @ 08-18 实际代码逐条核实)
> 评审对象:prd.md / design.md / implement.md / research/01-05 / check.jsonl / implement.jsonl

## 总评

文档链完整、调研扎实,方案骨架与现有代码高度吻合,可直接进入实现。抽查的约 20 个技术断言里 18 个与代码逐一对上(行号基本准确)。发现的问题集中在两个 P1(design 内部的"位置 2"假设与摘要存储形态),属实现前需修订的文档缺陷,不推翻方案;另有几个 P2/P3 建议顺手处理。

## 一、实证核对(抽样结论)

| 文档断言 | 代码证据 | 结论 |
|---|---|---|
| TRIGGER_RATIO 0.80 / TARGET 0.50 / PROTECTED_HEAD=2 | `app/src-tauri/src/agent/context.rs:47/50/55` | ✓ |
| C3 挂 drive.rs turn 入口,~172-248 | `agent/chat_loop/drive.rs:172-211` | ✓ |
| gate 模板 = digest 口径(开关缺省 on + !worker + !群聊) | `agent/chat_loop/init.rs:392-403`(memory_digest_enabled 模式现成) | ✓ |
| 前端每请求全量发 messages,wire 无 metadata | `daemon/routes/agent.rs:34-37`;`llm/types/chat.rs:19-48`(只有 role/content/speaker/attachments) | ✓ |
| messages 表已有 metadata 列 + FTS trigger 自动索引新行 | `db/migrations/schema.rs:166` / `1051-1070` | ✓ |
| B5 头对 insert(0/1) 在 prepare_loop_state 内 | `agent/chat_loop/init.rs:435/444` | ✓ |
| record_compaction + ContextCompacted(event.rs:217)+ compaction_json 手工 payload | `agent/trace.rs:30-68`、`llm/types/event.rs:217`、`agent/chat_loop/drive.rs:208` | ✓ |
| 单聊/worker/群聊三路径都走 run_chat_loop → prepare_loop_state **单一入口** | `agent/chat.rs:466` / `agent/subagent/dispatch/drive.rs:111` / `agent/group_chat_loop.rs:299,497`;`agent/chat_loop.rs:622` 是 prepare_loop_state 唯一调用点 | ✓(gate 在 init 一处即可罩全,设计选点正确) |
| retry_open 现成 | `llm/retry.rs:186` | ✓ |
| 摘要行 insert 前需先成功(AC2) | `db/sessions/messages.rs:26` persist_turn **无 metadata 列**;带 metadata 的插入是 `db/sessions/session_crud.rs:689` insert_system_event(role='user'、MAX(seq)+1,结构几乎正是摘要行所需) | ⚠ 见 P2-3 |
| 前端每请求完成后 reloadAfterFinalize 从 DB 重灌 store(wire 含摘要行的前提) | `app/src/stores/streamEvents.ts:1061`(调用点 :1040) | ✓(但 design 未写明此依赖,见 P1-3) |

## 二、P1 — 需先修订 design/implement 再 start

### P1-1:prior-summary 检测的"位置 2"假设在 3 种布局里错 2 种

design §4.2 和 implement PR3 都写"`messages[2]` 是摘要消息 → 注入 `<prior-summary>`",但 init 里摘要实际落在:

- 有 memory + 有 skills:`[mem-u][mem-a][skills][summary][...]` → summary 在 **index 3**(B4 skill listing 插到位置 2,见 `init.rs:497` `skill_pos = has_memory ? 2 : 0`);
- 有 memory + 无 skills:index 2(唯一对的情形);
- 无 memory + 有 skills:`[skills][summary][...]` → summary 在 **index 1**。

drive.rs 的 C3 里 `messages[2]` 就是 skills。按位置检测会**静默漏掉增量合并**(AC3 对 skill-bearing session 失效,不报错)。

**建议**:别按位置猜。设计本来就经 LoopInit 穿 CompactionRegistry,顺手把"当前水位摘要行(或其 content/seq)"穿进 drive;或按回填前缀做内容匹配。

### P1-2:摘要行 content 应存"纯摘要",回填前缀在请求构建时加,别落库

design §6 的回填正文 = Codex 前缀 + 摘要;若前缀随 content 落 DB,增量合并时 `<prior-summary>` 会带着 "This session is being continued…" 前缀被二次压缩(滚雪球噪音),D2 搜索也会命中它。

另外 §4.2 的"待压区 … ∪ [现有水位摘要消息]"要明确写成"作为 `<prior-summary>` 注入、**不在 transcript 里重复渲染**",否则同一份摘要被喂两遍。

### P1-3(AC2 地基,建议写进设计):apply_compaction_watermark 的行序 1:1 对齐依赖 reloadAfterFinalize

摘要行只在后端内存注入、从不流向 UI;前端每请求完成后 `reloadAfterFinalize`(streamEvents.ts:1061)从 DB 重灌 store,下一次 wire 才含摘要行,对齐才成立 —— 这条链路已核实,当前代码满足,AC2 实证可达。

但 design §3 的对齐防御是"内容不符 → fail-open 跳替换":一旦 reload 机制被改/绕过、或用户在下一次 reload 完成前抢发,失败路径 = 全量历史重发 → C3 再触发 → 重付摘要,AC2 静默破。

**建议**:① 把"依赖 reloadAfterFinalize 保证 wire 含摘要行"写进算法前提注释;② 防御降级改为"跳过 compaction_summary 行后重新对齐",实在不对齐时记一条 watermark_miss trace 再 fail-open(可观测,而不是哑失败)。

## 三、P2 — 建议实现时处理

### P2-1:摘要调用输入没有 transcript 尺寸上限

待压区可到 ≈0.83×window,design 只约束输出 8k、tool_result 截 2k、thinking 不渲染;Gemini/opencode 对 transcript 也设预算。建议给 `build_compaction_prompt` 的 transcript 设 `window − 输出预留` 上限,溢出截最旧(保留最近对话引语)或降级机械。

附:输出上限 8k 偏宽(业界 Cline/opencode 都是 4k),4k 更稳,高输出会占主 turn 窗口。

### P2-2:待压区起点会卷进合成头

待压区 = `messages[PROTECTED_HEAD..cut]`,而 index 2 是每请求重注入的 B4 skill listing —— 把"下个请求还会重新注入"的东西喂给摘要 = 浪费 token + 摘要噪音。建议从合成头之后起算。

### P2-3:实现锚点纠正

implement.md 写"db/messages.rs"—— 该文件不存在,消息插入在 `db/sessions/messages.rs`(`persist_turn`,且**无 metadata 列**);带 metadata 的插入先例是 `db/sessions/session_crud.rs:689 insert_system_event`(role='user'、MAX(seq)+1、手工 json 内容)。要么给 persist_turn 加 metadata 参数,要么仿 insert_system_event 写 `insert_compaction_summary`,PR1 写清即可。

### P2-4:AC5 措辞

"DB 全量 messages 行数不变"不实 —— 摘要路径会**追加**摘要行与后续 turn 行。真正的不变量是"被压缩区原始行不删",建议改为"压缩区原始行数不变(仅新增摘要行)"。

## 四、P3 / nits

- 群聊 gate 测试口径:init 的 gate 判定是 `loaded_session.session.session_type == GroupChat`(init.rs:398),不是 speaker 参数;集成测试需建 GroupChat 类型 session 行才能让 gate 生效,test plan 的"speaker Some"描述不准确(worker 侧 `skip_persist=true` 是准的)。
- `record_compaction` 的 compaction payload 是**手工 json!**(trace.rs:57-64),扩 `method`/`summary_usage` 要动 Rust enum + record_compaction + TS `ContextCompactedEvent`/streamController 归一化 arm 三处 —— implement PR2 已覆盖,但手工 json 这处最易漏。
- context.rs:365-371 的 trigger/target helper 与常量要同步调 0.85(design §4.1 写了,确认 helper 复用同一常量)。
- 空待压区(cut == head,窗口过小)行为未定义:建议直接跳过摘要走机械路径(现机械路径天然处理)。

## 五、值得保留的设计强点

- **单一入口选点正确**:worker/群聊都复用 run_chat_loop(dispatch/drive.rs:111、group_chat_loop.rs:299/497),init 一处 gate 罩住三路径,与 digest gate 完全同构,可复制粘贴级落地。
- **AC2 的另一半依赖恰好已存在**:`reloadAfterFinalize` 每请求回灌 DB,让"摘要落库 + 水位替换"链路闭环 —— 设计没显式写它,但代码已满足,补进 design 即可。
- 组原子性复用 `group_droppable_turns` 反方向算保留区,RULE-A-001 零新增不变量;回滚只要单开关关全链(design §10),降级链与熔断参数有 Claude Code 实证背书。

## 结论

PRD 的 AC 列表在动手前把 P1-1/P1-2 两行修掉,其余按 P2 列表顺手处理即可。全程未发现需推翻方案骨架的问题。

---

## 复核(2026-08-18,主会话独立核实后采纳)

评审关键断言逐条对照代码复核:**全部成立**——

- P1-1:✓ 实证 `init.rs:497`(`skill_pos = if has_memory { 2 } else { 0 }`),摘要消息实际落位 3(有 memory+skills)/ 2(仅 memory)/ 1(无 memory+skills),"位置 2"三种布局错两种。
- P2-3:✓ `db/messages.rs` 不存在(实际 `db/sessions/messages.rs`);`insert_system_event` 在 `session_crud.rs:689`,MAX(seq)+1 模式。
- P1-3:✓ `reloadAfterFinalize` 在 `streamEvents.ts:1061`(load_session → rehydrateMessages → putMessages,wire 含摘要行的前提成立)。
- 单一入口:✓ `prepare_loop_state` 唯一调用点 `chat_loop.rs:622`。

**处置**:P1×3 + P2×4 + P3×4 全采纳,已修订 design.md / prd.md / implement.md。

**复核补充(评审未覆盖,主会话新增两条)**:

1. **P1-1 修法升级**:评审建议"经 LoopInit 穿水位摘要"只覆盖请求入口检出的摘要;**同一 loop run 内的二次压缩**(turn N 建摘要 → turn M 再超线)检不到。改为 `DriveTurnOutcome` 循环内穿参 `SummaryAnchor`(init 种子 + drive_turn 每次成功压缩后更新,同 `loop_hit_count` 线程模式),位置猜测和内容匹配都不需要。
2. **P1 级新增:摘要行 insert 的 seq 游标协调**——`insert_system_event` 的独立 `MAX(seq)+1` 在**活跃 loop 内**会与 loop 的内存 seq 游标撞号(messages 主键 `(session_id, seq)`,loop 下一次 persist 即碰撞)。`insert_compaction_summary` 必须吃 loop 当前 `seq` 游标、插入后返回推进值,不走独立 MAX+1。

---

## PR2 check 追记(2026-08-18,check 子代理发现 + 主会话独立核实采纳)

- check 修复 2 处实现 bug:摘要采集循环漏 `Ok(ChatEvent::Error)` 吞错(部分文本会被当完整摘要落库)、`dropped_count` 口径(摘要折叠数漏记)。
- **P1 级设计缺陷(主会话核实成立,已修订 design §2.2/§3/§4.3 + prd AC2)**:摘要行按 seq 游标插在全量行(含保留区+当前输入)之后,原水位语义"摘要行之前的全折叠"会把**保留区与本请求用户提问**一并丢弃,而摘要 transcript 从未覆盖它们——净效果是模型看得见自己上轮回答、看不见问题,最近 15k 逐字区静默消失。修复采方案 (b):**按 `cutoff_seq`(精确)折叠 + kind 过滤摘要行**;否决方案 (a)(副本行重持久化会污染 D2/FTS/B1 附件引用/edit cascade 语义)。`preserve_from_seq` 随之恢复为可精确写入。偏离裁决 ①-⑤ 全部接受。