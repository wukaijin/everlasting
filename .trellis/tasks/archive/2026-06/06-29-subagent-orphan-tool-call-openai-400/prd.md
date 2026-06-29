# subagent orphan tool_call(OpenAI 400)修复与并发上限提升

## Goal

修复 subagent 并发场景下 OpenAI provider 报 400 `"An assistant message with 'tool_calls' must be followed by tool messages responding to each 'tool_call_id'"` 的 orphan tool_call bug;同时把 subagent 并发上限 `DELEGATION_MAX_CONCURRENT_CHILDREN` 默认值从 3 提到 10(用户诉求)。两者强相关:提上限会放大 orphan bug 的触发面,必须先修后提。

## What I already know

### 现象(日志,2026-06-28)
- 同一 parent `o778xe897apmqy3nclj` 的 3 个并发 subagent worker(`sub-call_00/01/03`)各自在 turn 6/8/9 撞同一个 OpenAI 400。
- 父 agent 先发 5 个 dispatch_subagent 被 L3a 硬拒(`count=5 max_concurrent=3`),重发成 3 个并发。
- "3/3 全中"=系统性;"turn 6/8/9 各异"=内容相关、非固定轮次结构;"首次踩坑后持续 400"。

### 已排除(代码确认正确)
- wire fan-out `chat_message_to_wire_messages`(`wire.rs:259`):每个 `ToolResult` block 拆成独立 `WireMessage::Tool`,不丢。
- `strip_unsupported`(`wire.rs:464`):`Tool` 透传、`ToolUse` 保留。
- L3a hard reject(`chat_loop.rs:1840-1850`):每个被拒 tool_use 补 `is_error:true` ToolResult。
- loop detection(`chat_loop.rs:1565`):绝不跳过执行,只注入 hint Text block。
- OpenAI 流式累积(`openai.rs:596/830/864`):`build_tool_call_event` 返回 None = 丢弃(少记),不可能多出 orphan;assistant tool_use 数 ≡ emit ToolCall 数 ≡ 执行 tool 数,1:1。

### 根因(已确认)
`chat_loop.rs:1377-1484` 的 **error 路径** push 了含 `tool_use` 的 assistant 消息,却**没有**像 cancel 路径那样补 synthetic tool_result:

- `chat_loop.rs:1453` `messages.push(msg)` —— 只要 `assistant_blocks` 非空(含 tool_use)就 push。
- `chat_loop.rs:1457` `if cancelled` —— **只有 cancel 分支**调 `build_synthetic_tool_result_message`(`helpers.rs:79`)补占位 result。
- `had_error` 路径:assistant 已 emit tool_use → push assistant(tool_use) → 既不执行 tool、也不补 synthetic → orphan → 下轮起持续 400。

诱因:GLM 走 OpenAI 兼容协议,subagent 长对话偶发 stream error(400/网络/超时)落在 tool_use 已 emit 之后,即触发。

> **注**:上面是初判。C1(error 路径补 synthetic)据此实施,是真实缺陷修复(保留),但**实证非主因**。

### 最终根因(诊断订正,2026-06-29,DB+日志三方印证)

实际触发主因是 **loop detection hint 的 Text block 破坏了 OpenAI 的 tool_calls 后续顺序**,不是数量 orphan:

- `chat_loop.rs:2299` 把 hint `insert(0, ...)` 到 `result_blocks` 最前 → user 消息变成 `[Text(hint), ToolResult×N]`。
- wire fan-out(`wire.rs:chat_message_to_wire_messages`)按 block 顺序展开 → `assistant(tool_calls)` 后紧跟 `user(text=hint)`,再才是 `role:tool`。
- OpenAI 硬约束:`assistant(tool_calls)` 后**必须紧跟** `role:tool`;插了 `role:user` → 400 "insufficient tool messages **following** tool_calls"。

DB 诊断排除链(均经 `subagent_runs.transcript_json` 实证):
- **执行/emit**:transcript 19 tool_call / 19 tool_result 配平、顺序正常 → 非因。
- **compact**:`context_window=1M`(GLM-5.2),trigger 80 万,run 实际几万 token → 未触发 → 非因。
- **error 路径**:turn 1-7 全配平,turn 8 才 400 → 非主因(C1 仍修,防边界)。
- 每次 400 前**必然**有 `loop detected (HardLoop { read_file, count:3 })` warn(researcher 连续 read_file 撞 HardLoop)→ hint 触发的强相关。

修复(见 Requirements/Decision):loop_hint `insert(0)` → `push`(末尾);新增 wire 层**顺序**扫描 `orphan_tool_call_order`(`orphan_tool_use_ids` 只查数量,补顺序盲区)。已用户实跑验证 400 消除。

## Assumptions (temporary)

- 主 chat 与 subagent 共用 `run_chat_loop`,故 error 路径 orphan 对两者都存在;只是 subagent turn 多+并发,更易触发。修复应覆盖两者。
- 复用现成 `build_synthetic_tool_result_message`(`helpers.rs:79`,cancel 路径同款)补 error 路径,语义一致("tool 未执行"占位 result)。

## Open Questions

- (无 —— Q1 已决:选 B)

## Decision (ADR-lite)

**Context**: error 路径 push 含 tool_use 的 assistant 却不补 synthetic tool_result,只有 cancel 路径补;orphan 一旦产生会持续到 MAX_TURNS。未来可能还有新的"tool_use 已 emit 却早退"路径。

**Decision**: 选 B —— (1) error 路径补 synthetic tool_result(对齐 cancel);(2) 在 `chat_request_to_wire`(`wire.rs:226`)入口加 orphan 扫描,差集非空则 `tracing::error!` 打印 request_id + 缺失 tool_use_id + 位置,作为防御层 + 回归守卫;(3) 上限 3→10。**不**做 C 的三路径合并(改动面过大,扫描层已能兜住未来新路径)。

**Consequences**: 修复对称、低成本;扫描层让任何新 orphan 路径在发请求前即刻暴露(grep `tracing::error` 即可定位),不必每次重新推理。代价:wire 入口多一次 O(n) 扫描(消息数小,可忽略)。

## Requirements (evolving)

- error 路径(`had_error && !tool_calls.is_empty()`)push assistant 后,补 synthetic tool_result(复用 `build_synthetic_tool_result_message`),消除 orphan。
- `chat_request_to_wire`(`wire.rs:226`)入口加 orphan 扫描:对每条 assistant 收集 tool_use_id,对后续 user 收集 tool_result id,差集非空 → `tracing::error!`(request_id + 缺失 id + 位置);纯诊断,不改变 wire 输出。
- `DEFAULT_DELEGATION_MAX_CONCURRENT_CHILDREN` 3 → 10。

## Acceptance Criteria (evolving)

- [ ] error 路径含 tool_use 时,messages 中 assistant(tool_use) 与 user(tool_result) 数量配平(无 orphan)。
- [ ] 新增单测:mock provider 在 tool_use emit 后抛 stream error → 断言下轮请求消息序列通过 orphan 配平校验(扫描无 diff)。
- [ ] wire orphan 扫描单测:构造故意 orphan 的 messages → 断言扫描命中并打印缺失 id;配平的 messages → 扫描静默。
- [ ] OpenAI 并发 3+ subagent 长对话场景不再出现 400 "insufficient tool messages"。
- [ ] `DEFAULT_DELEGATION_MAX_CONCURRENT_CHILDREN == 10`,且 env 覆盖仍生效。

## Definition of Done

- `cd app/src-tauri && PKG_CONFIG_PATH=... cargo test` 绿(含新单测)。
- 前端 `vue-tsc --noEmit` 不受影响(纯后端改动)。
- 若加 wire 扫描:扫描自身有单测。
- DEBT/ROADMAP 视情更新。

## Out of Scope (explicit)

- 不改 wire fan-out / strip 逻辑(已正确)。
- 不改 OpenAI 流式累积器(已正确)。
- 不改 L3a / loop detection 行为(已正确)。
- 不重构 cancel/error/max_turns 三条早退路径的合并(除非选 Q1-C)。

## Technical Notes

- 根因位点:`app/src-tauri/src/agent/chat_loop.rs:1377-1484`(error vs cancel 路径不对称)。
- 复用:`app/src-tauri/src/agent/helpers.rs:79` `build_synthetic_tool_result_message`。
- 上限:`app/src-tauri/src/agent/chat_loop.rs:2636` `DEFAULT_DELEGATION_MAX_CONCURRENT_CHILDREN`。
- 并发路径:`chat_loop.rs:1853` `DispatchBatch::Concurrent`(L3b PR2,per-worker worktree)。
