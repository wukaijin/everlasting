# C2+ 循环检测主动干预

## Goal

在 C2（2026-06-24 落地的软提示循环检测）与 `MAX_TURNS=200` 硬兜底之间，补一层**主动干预**：当软提示连续命中 N 次仍无法让 LLM 自我纠正时，harness 主动通过 `ask_user_question` 询问用户「是否终止本次 agent loop」，堵住 MAX_TURNS=200 烧 token 的敞口。

**用户价值**：agent 死循环时（典型：反复 `read_file` 同路径 / 反复 `edit_file` 同一失败 `old_string` / 反复近似 `shell`），当前唯一兜底是烧满 200 turn。C2+ 让用户在「软提示连续失效」时拿到**显式终止入口**，而不是被动等 MAX_TURNS 或手动点 Stop。

## Background（代码确认事实）

- **`loop_detection.rs` 纯函数已稳定**（31 单测，`detect → LoopVerdict { None | HardLoop | SoftLoop }`，`hint_text()` 产 hint）。C2+ **不改** `loop_detection.rs`，只在 caller 侧加计数 + 询问。
- **detect 接入点** = `chat_loop.rs:1968-1984`，在 `for turn in 1..=turn_limit` 内、所有 tool 执行完之后、`result_blocks` 构造之前，**每轮一次**。
- **`loop_window: VecDeque<ToolCall>` 是 per-`run_chat_loop`-local**，声明在 turn 循环之外（`chat_loop.rs:1106-1112` 注释：「Declared OUTSIDE the turn loop so it accumulates across turns」）→ 跨 turn 累积，且 **B6 worker subagent 复用 `run_chat_loop` → 自动继承循环检测**。
- **`MAX_TURNS=200` 不变**，仍是硬兜底；C2+ 是中间主动层。
- **cancel/break 机制**：`tokio_util::sync::CancellationToken`，退出走 `emit Done(stop_reason) + return`（`chat_loop.rs:1099-1103` / `1955`）。
- **`QuestionStore`（`question_store.rs`）复用路径**：用 `session_id` 作 key，单 pending gate（`AlreadyPending`）；复用 `emit_tool_question` event + `resolve_tool_question` command（前端 submit/跳过 → `resolve`）+ `get_pending_question`（session-switch 恢复）+ 前端 `<AskUserQuestionCard>`。**caller 是 chat_loop 顶层**（harness-driven），非 LLM tool 路径；伪 `tool_use_id = "loop_intervention_<turn>"`（前端 card 不依赖它做匹配）。
- **`AuditKind`（`audit.rs:34`）是 enum + `as_str()` 落 TEXT 列**，加变体**不需 DB migration**；有 `record_tool_executed_audit` / `record_message_resend_audit` 现成 record 模式。

## Requirements

### R1 — 跨 turn 累计计数器

新增 per-`run_chat_loop`-local `loop_hit_count: u32`（与 `loop_window` 同生命周期，跨 turn 累积，worker 继承）。每轮 detect 后：
- `verdict != None` → `loop_hit_count += 1`
- `verdict == None` → `loop_hit_count = 0`（连续性重置）

### R2 — 触发主动询问（N=3，Hard/Soft 共用）

当 `loop_hit_count >= 3`：
1. 构造固定文案 `ToolQuestionPayload`（harness 生成，文案见下）
2. `QuestionStore::register(session_id, "loop_intervention_<turn>", payload)` + `emit_tool_question`
3. chat_loop 顶层 `tokio::select!{ token.cancelled(), oneshot }` 等待
4. 用户响应分支（见 R3）

**文案（定稿）**：
- `header`: `"循环检测干预"`
- `question`: `"检测到 agent 似乎在循环重复相同操作（已连续 3 次触发循环检测，注入的软提示未能让模型纠正）。是否终止本次 agent loop？"`
- `options`:
  - `终止 loop` — description: `"停止本次 agent loop，保留已生成的内容"`
  - `继续` — description: `"清零计数器继续，给模型再次自我纠正的机会"`

**并发安全**：`QuestionStore` 单 pending gate 已保证「同 session 同时只有一个询问」；若 LLM 同时发起 `ask_user_question` tool 导致 `register` 返 `AlreadyPending` → C2+ 降级为本轮跳过（下轮再试），不阻塞 loop。

### R3 — 用户响应处理

- **「终止 loop」**：走 cancel 同款 break 路径，`emit Done(stop_reason="loop_terminated")` + `QuestionStore::remove(session_id)`。
- **「继续」**：`loop_hit_count = 0`（清零）+ 向 result message 注入一条**增强 hint**（告诉 LLM「用户已确认你在循环，请改变策略」）+ 继续 loop。
- **session cancel**（用户点 Stop）：select! cancel arm → 同「终止」break，但 `stop_reason="cancelled"`（区分用户主动 Stop 与 loop 干预终止）。

### R4 — AuditKind::LoopIntervention 落表

新增 `AuditKind::LoopIntervention` enum 变体 + `as_str()`（无 DB migration，`kind` 列已是 TEXT），同款 `record_loop_intervention_audit` 函数（仿 `record_message_resend_audit`），前端 `<AuditLogModal>` 加 kind 分发。payload `{ hit_count, verdict_kind: "hard"|"soft", action: "asked"|"terminated"|"continued" }`。便于 E2 trace viewer 复用 + 审计完整性（17→18 类）。

### R5 — worker/subagent 行为（直接 break）

worker 复用 `run_chat_loop` → 继承计数 + 触发。触发时 **worker 直接 break**（不弹 banner、不打扰用户），在 `dispatch_result` 里告知父 agent「worker 因循环被终止」，让父 agent 决策是否重试 / 换路径。理由：worker 有独立 token 预算 + 更短 max_turns，烧钱风险本就小；避免占父 session `QuestionStore` slot + 避免打扰用户。worker 的 break 走 `emit Done(stop_reason="loop_terminated")`，**不**写 AuditKind（worker 无独立审计 surface，worker run 自有 transcript 记录）。

## Acceptance Criteria

- [ ] AC1：连续 3 轮 detect 命中（Hard 或 Soft）后，chat_loop 阻塞在 `QuestionStore` 询问，前端 `<AskUserQuestionCard>` 渲染 R2 文案
- [ ] AC2：用户选「终止 loop」→ loop break，`Done.stop_reason = "loop_terminated"`，`QuestionStore` entry 清除
- [ ] AC3：用户选「继续」→ `loop_hit_count` 清零，loop 继续，下一轮 result 含增强 hint
- [ ] AC4：detect `None` 一轮后 `loop_hit_count` 归零（连续性重置）
- [ ] AC5：session cancel（Stop）在询问期间 → 走 cancel break（`stop_reason="cancelled"`），无悬挂 pending
- [ ] AC6：`QuestionStore::register` 返 `AlreadyPending`（LLM 并发 ask）→ C2+ 本轮跳过，不阻塞 loop
- [ ] AC7：`loop_detection.rs` 零改动（纯函数层不变）
- [ ] AC8：worker subagent 触发 → 直接 break，`dispatch_result` 含「循环被终止」提示，不写 AuditKind
- [ ] AC9：`AuditKind::LoopIntervention` 落表，payload 正确；前端 `<AuditLogModal>` 能渲染
- [ ] AC10：`cargo test --lib` 全绿（含 N 次累计 / 「继续」清零 / 「终止」break / cancel race / AlreadyPending 降级 / worker break 测试）

## Out of Scope

- 改 `loop_detection.rs` 的 detect 算法 / 阈值（已稳定，C2+ 只加 caller 侧逻辑）
- 跨 session 持久化 `loop_hit_count`（loop 是当次行为，session 重启不保留）
- 调整 `MAX_TURNS=200`
- E2 trace viewer 的实际实现（C2+ 只在 AuditKind 层为它铺路）

## Decisions Log

| 决策 | 结论 | 理由 |
|---|---|---|
| N 阈值 | 3 | Hard 零假阳 → ~5 连 byte-identical 才问；Soft 同 3 轮。单一计数器最简 |
| Hard/Soft 区别对待 | 不区别 | 共用 `loop_hit_count`；区别对待增状态机复杂度无收益 |
| 「继续」后计数器 | 清零 | 简单可预测；用户「继续」= 信任模型再试一轮 |
| stop_reason（终止） | `"loop_terminated"` | 区分用户主动 Stop（`"cancelled"`）与 loop 干预 |
| AuditKind | 加 `LoopIntervention` | 无 migration；审计完整 + E2 铺路；C2+ 是用户可见行为应落审计 |
| 文案风格 | 平衡（header + 完整说明 + 选项 desc） | 首次看到干预的用户能理解原因 |
| worker 触发处理 | 直接 break + 告知父 | worker 烧钱风险小（独立预算 + 短 max_turns）；不打扰用户；避免占父 QuestionStore slot |

## Notes

- 复杂任务，三件套：`prd.md` + `design.md` + `implement.md`
- spec 影响：`backend/agent-loop-architecture.md`（C2 章节扩展 C2+）+ `backend/tool-contract.md`（AuditKind 新增）+ 前端 audit 渲染分发
