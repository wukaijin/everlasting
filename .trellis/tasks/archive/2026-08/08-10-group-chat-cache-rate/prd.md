# 群聊参与者/主持人最近一次缓存率显示

## Goal

在群聊会话中,让用户看到每个参与者(含主持人)各自**最近一次 LLM 调用**的缓存率(cache_rate = 该次调用返回 usage 的 `cache_read_input_tokens / context_input_tokens`),用于直观判断多轮讨论中各模型/供应商的 prompt 缓存命中情况与成本效率。

缓存率是**单次**语义(用户明确):某一次 API 调用返回的 usage 计算得出,不是多轮聚合。参与者之间不可直接比较(供应商缓存能力不同),但"TA 最近一次发言时的缓存命中"对用户有意义。

## Background / 已确认事实(代码证据)

- 缓存率口径(用户定义):`cache_read_input_tokens / context_input_tokens`。分母必须用 `context_input_tokens`(跨 provider 归一化的总输入:Anthropic = input+cc+cr,OpenAI = prompt_tokens)—— 见 `llm/types/usage.rs:44-53`,两 provider 的 `input_tokens` 口径不一致,直接用 `input_tokens` 会让命中率失真。
- 群聊每轮调用(不论主持人还是参与者)的 usage 独立写入 `turn_trace(session_id, seq, token_usage_json)`:`db/trace.rs:55` `upsert_turn_trace_token`;写入点在 `chat_loop/drive.rs:801`,群聊走 persist=true(`group_chat_loop.rs:320,336`),所以主持人和参与者每轮都写。
- seq 全局连续:每次 `run_chat_loop` 从 DB `max(seq)+1` 起(`chat_loop/init.rs:100-107`),群聊多 speaker 调用共享同一 session,seq 不冲突。
- assistant 消息行带 `speaker` 列:`chat_loop.rs:817`;主持人 = `"moderator"`,参与者 = `participant.name`(`group_chat_loop.rs:54,336-338`)。普通聊天/worker 路径 speaker 为 NULL。
- assistant 消息行的 seq == 该轮 turn 的 seq(`drive.rs` 中 `messages.push` 用当前 seq 后 `seq += 1`),因此 `messages (session_id, seq, speaker)` ↔ `turn_trace (session_id, seq, token_usage_json)` 可直接 join。
- 取"最近一次":每个 speaker 的 `max(seq)` assistant 行对应那一轮。
- moderator 的 model = `sessions.model_id`(fallback `model` 列):`agent/group_chat.rs:128-132`;参与者 model = `sessions.metadata.participants[].model`。**两者前端 `SessionSummary` / `groupChatParticipants` 均已有**,后端聚合命令无需返回 model。
- 群聊参与者 UI 现状:无常驻参与者列表;仅有标题行"群聊 (N 参与者)"chip(点击打开编辑弹窗,`ChatPanel.vue:618-629`)与 `GroupChatConfigModal` 编辑弹窗(参与者 name + model 下拉)。
- `turn_trace` 会被 `clear_session_trace` 整表清掉(与"回看"功能共用数据):缓存率是会话存活期内有效;清 trace 后为空,属预期行为。
- OpenAI 归一化后无 `cache_creation_input_tokens`(`llm/types/usage.rs:37-38`),但 `cache_read` 有(`cached_tokens`);兼容代理可能两者皆 0 → 缓存率 0%。
- 取消/出错轮次 `Done{usage: None}` 不写 turn_trace(`drive.rs:759-806`),该轮不参与统计。

## Requirements

- R1: 群聊会话中,展示主持人 + 每个参与者的缓存率,语义为"TA 最近一次 LLM 调用"的 `cache_read_input_tokens / context_input_tokens`。
- R2: 某 speaker 尚无任何有 usage 的轮次(如讨论刚开始、全部轮次取消/出错)时,显示占位(如 "—"),不显示误导性数字。
- R3: 缓存率仅群聊会话显示;普通聊天不显示(普通聊天只有一个模型,chip tooltip 已覆盖 cache_read 明细,不需要新 UI)。
- R4: 数据获取不依赖新增存储;基于现有 `turn_trace` + `messages.speaker` join 派生。
- R5: 展示时需要能区分参与者身份(主持人 vs 参与者);model 信息前端已有,不需要后端返回。
- R6 (UX 已定): 展示位置 = `GroupChatConfigModal` 编辑弹窗(`mode="edit"`):每个参与者行追加只读缓存率行;弹窗底部新增只读"主持人"区(主持人行 + 缓存率)。`mode="create"` 不显示(新群聊无历史轮次)。
- R7 (刷新时机): 弹窗每次打开时拉取一次缓存率数据;不实时订阅。

## Acceptance Criteria

- [ ] AC1: 群聊会话中,主持人 + 每个参与者各显示一个缓存率百分比,数值 = 该 speaker **最近一次 assistant 轮**(若该轮有 usage)的 `cache_read_input_tokens / context_input_tokens`(后端聚合查询的单元测试覆盖正确性,含多 speaker、多轮、最新轮无 usage → 整行不返回不回退到更早轮次)。
- [ ] AC2: 从未有 usage 的 speaker 显示 "—" 占位。
- [ ] AC3: 普通聊天会话不出现该展示。
- [ ] AC4: `clear_session_trace` 清掉 turn_trace 后,群聊缓存率区域显示空态(不报错)。
- [ ] AC5: 前端展示有对应的单元测试(纯计算函数或组件测试,不依赖真实 IPC)。

## Out of Scope

- 不做多轮聚合缓存率(用户确认单次语义)。
- 不做按 cost 加权的缓存率展示。
- 不修改 `turn_trace` / `messages` schema。
- 不改变普通聊天的 chip / tooltip 现状。
- 不做缓存率的历史趋势/轮次明细展示。

## Open Questions

- (无阻塞问题。展示位置与刷新时机已定:R6/R7。)
