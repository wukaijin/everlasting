# C3 现状代码分析(2026-08-18,本仓库 main)

> 任务 `08-18-llm-context-compaction` 的本地基线。所有行号以当前 main 为准。

## 1. 现有实现:`agent/context.rs`

纯机械贪心丢组,零 LLM 参与:

- **触发/目标**:`TRIGGER_RATIO = 0.80` / `TARGET_RATIO = 0.50`(context.rs:47-50),cl100k 估算(`estimate_messages_tokens`,图片按 6400 token 固定 pad 估)。
- **保护优先级**(模块 doc):
  1. `messages[0..=1]` — B5 memory 合成对(instructions user + assistant ack),`PROTECTED_HEAD = 2`,永不丢;
  2. 最后一条(当前 turn 输入)永不丢;
  3. thinking 块靠"整组原子丢"保护(不拆 turn 就不会孤儿签名);
  4. 旧 runtime tool_result;
  5. 旧 user/assistant turn(从旧到新)。
- **组原子性**:`group_droppable_turns` 把 middle 切成 `(assistant(tool_use), user(tool_result))` 配对组或单例组;RULE-A-001(2026-06-14)修复过孤儿 tool_use bug —— 配对不完整的 assistant(tool_use) 隐式保护,不产组。
- **失败信号**:`DegradationKind::StillOver`(RULE-A-002,2026-06-14)— 丢完所有安全候选仍超 target 时,drive.rs **fail-fast**(emit Error + abort turn),绝不发超预算请求。

## 2. 接线点:`agent/chat_loop/drive.rs:172-248`

- 挂在**每 turn 入口**(RULE-A-005 system prompt 刷新之后、B12 checklist 注入之前)。
- 压缩结果 `messages = compacted.messages` —— **loop 生命周期内粘性**(本请求后续 turn 都用压缩后的 Vec)。
- E2 trace:`record_compaction` always-on 落 `turn_trace.compaction_json` + `ChatEvent::ContextCompacted { seq, tokens_before, tokens_after, dropped_count, degradation }`(event.rs:217)— TracePanel 已能实时/回看。

## 3. 关键架构事实:历史 SoT 与请求流

- **前端每请求全量发送 messages**:`POST /api/v1/agent/chat` 请求体 `ChatRequest.messages`(daemon/routes/agent.rs:37),前端 streamController `history` wire 数组由 chat.ts `toPayloadContent` 组装。
- **DB 是 SoT**:loop 每 turn 持久化到 `messages` 表(skip_persist=true 仅 worker);前端 reload 从 DB 重建。
- **推论(本任务第一适配点)**:C3 压缩是 per-request 内存态 —— 请求结束后压缩状态即消失,下一请求前端又发全量历史。机械丢组是确定性的,每请求重丢一次无成本;**LLM 摘要若不持久化,每个请求都要重付一次摘要调用**。⇒ 摘要必须落 DB 并在历史重建时生效。
- `messages` 表已有 `metadata` JSON 列(B1 图片附件引用在用)—— synthetic 摘要消息可用 metadata 标记 kind。

## 4. 与相邻机制的交互(设计约束)

| 机制 | 交互点 |
|------|--------|
| B5 memory 头对 + `cache_control: Ephemeral` 断点 | 摘要 synthetic 消息必须插在**头对之后**(位置 ≥ 2),否则 bust memory cache(B12 注释里的 load-bearing 论证同款) |
| B12 checklist 每轮 ephemeral 注入 | 挂在 C3 之后 append,不受影响;checklist SoT 在 DB tool_result 里,replay 从 DB 还原 —— **不能删 DB 行** |
| C7D stub / memory digest | resident 层每请求重注入,天然等价 Claude Code "磁盘重注入" —— 摘要无需覆盖 memory/tools |
| L1a 后台 shell 通知 | drain 后 append user message,在保留区外时会被压缩 —— 摘要需涵盖(或通知本身短,无碍) |
| D3 edit_user_message | in-place 改写 + cascade 截断后续;cutoff 水位若指向被截断区间需自愈 |
| B1 图片 | 保留区图片随消息保留;被摘要区的图片消失(摘要应提及图片存在过);images_token 口径 = 请求内全部图块 |
| loop_detection / C2+ | 独立窗口,不共享 messages;不受影响 |
| MAX_TURNS=200(单聊/worker)、群聊 30 轮 | 本任务不动;去硬卡是后续任务,以本任务为前置 |

## 5. Scope 现状(三路径)

- 单聊:`MAX_TURNS = 200`(agent/mod.rs:91,chat.rs 调用点传 None)。注释里"50"是陈旧的。
- 群聊:`MAX_ORCHESTRATION_ROUNDS = 30`(group_chat_loop.rs:88);moderator `max_turns=Some(1)`、参与者 `Some(20)`;per-speaker `role_history` 隔离。
- worker:`SUBAGENT_MAX_TURNS = 200`(subagent/dispatch.rs:79);`Incomplete` 软终态 + C1 resume;worker messages 不进主 DB(skip_persist),transcript 落 `subagent_runs`。

## 6. 可复用资产清单

- `compact_messages` / `group_droppable_turns` / `estimate_messages_tokens` — 保留区计算与组原子性直接复用;
- `CompactResult` + `DegradationKind` + `record_compaction` + `ContextCompacted` — 观测管道现成;
- `retry_open`(A5+,llm/retry.rs)— 摘要 LLM 调用可直接套;
- `turn_trace` 三 token 切片 + turn-smoke — 压缩前后窗口变化可度量;
- session provider 解析(chat.rs `lookup_provider_for_session`)— 摘要调用模型来源。
