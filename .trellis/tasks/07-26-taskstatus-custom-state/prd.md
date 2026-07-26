# TaskStatus accommodate custom plugin state (C0)

> 父任务：`07-26-workflow-review-plugin`（review epic）
> 依赖：无（架构级前置，最先做）。C3/C2 都依赖本任务。
> 触发：C2/C3 评审期间发现 review 的 4 state 存不进 task.json.status。方案分析见 `../research/taskstatus-custom-state.md`。

## Goal

让 `TaskStatus` 能容纳 plugin 自定义的 state 值（如 review 的 intake/reviewing/revising/reported），而非只支持 dev 的硬编码 4 值。这是 review plugin 状态机能跑起来的硬前置——当前 `from_str_opt` 把任何非已知值 fallback 到 Planning，review 的 state 写进去读回来全塌。

**采用方案 X**（enum + Custom(String)，见 research/taskstatus-custom-state.md §2 推荐）。

## Background（调研事实，见 research/taskstatus-custom-state.md）

- `TaskStatus` enum（`task.rs:122`）只有 `Planning/InProgress/Done/Completed`，`from_str_opt` 未知值 → Planning。
- dev 的 state（planning/in_progress/done）**就是 task.json.status**（task.rs:87-94 注释）——不是两个系统，是同一个。
- role gate（`dispatch.rs:1630`）用 `status.as_str()` 查 `roles_by_state[state]`——靠字符串查 map，Custom(s).as_str()=s 能直接命中。
- set_task_state 钩子（`state.rs:266`）按 `(from, to)` 穷举 match（InProgress→Done 触发 spec_distillation）——review transition 不该触发 dev 钩子。
- transition 校验（`request_task_state_transition.rs`）现校验「是已知 TaskStatus」——应改成「`can_transition(workflow_def, from, to)`」（def.rs:306 现有函数）。

## Requirements

### R1. TaskStatus enum 加 Custom(String)

- 加 `Custom(String)` 变体。
- `from_str_opt`：已知值（planning/in_progress/implement/check/done/completed）优先 match，未知值 → `Custom(s)`（不再 fallback Planning）。
- `as_str`：`Custom(s) → s`。
- 序列化：Custom(s) 写成字符串原值（serde rename_all snake_case 不影响 Custom，需自定义 Serialize 或 as_str 转换）。
- 反序列化：旧 task.json 的 "planning" 等仍读回已知变体（兼容）。

### R2. set_task_state 钩子加兜底

- `match (from, to)`（state.rs:266）加 `_ => {}` 兜底分支——Custom 状态的 transition 不触发 dev 钩子（spec_distillation 等）。
- audit 所有 TaskStatus match 点，确保 `_` 分支语义正确（不静默吞 dev 合法 transition）。

### R3. transition 校验改为 plugin def 驱动

- `request_task_state_transition.rs` 的 from/to 合法性校验：从「是已知 TaskStatus」改成「`can_transition(workflow_def, from, to)`」。
- 即 transition 合法性由 plugin 的 workflow.json 定义决定，而非全局 enum。

### R4. dev 零回归

- dev 的 Planning/InProgress/Done/Completed 行为完全不变（Custom 只对非 dev 状态触发）。
- 现有 set_task_state 钩子（spec_distillation/preflight）对 dev transition 仍正常触发。

## Acceptance Criteria

- [ ] `Custom("reviewing")` 写进 task.json.status，读回仍是 `Custom("reviewing")`（round-trip）。
- [ ] role gate 用 Custom state 查 `roles_by_state` 命中（如 `roles_by_state["reviewing"]` 返回 `["reviewer"]`）。
- [ ] set_task_state 的 Custom transition（如 intake→reviewing）不触发 dev 钩子（spec_distillation）。
- [ ] transition 校验用 `can_transition`：review 的 intake→reviewing 合法（workflow.json 定义了），planning→done 非法（无此 transition）。
- [ ] **回归**：dev 的 planning→in_progress→done 全流程正常，spec_distillation 仍触发；旧 task.json（"planning" 等字符串）读回正确变体。
- [ ] `cargo test --lib` + `cargo clippy --lib --tests` 通过。

## Out of Scope

- review 自己的钩子（如 reported 时的自动归档）——若需要，在 C3 落地时按 review 专属分支加。
- task.json schema 其他字段的改动。

## Open Questions（design.md 需解决）

1. Custom(String) 的 serde 序列化：用 `#[serde(untagged)]` + 自定义 Serialize，还是用 as_str 转字符串后存？需确保旧数据兼容 + 新 Custom 值 round-trip。
2. set_task_state 钩子的 `_ => {}` 兜底是否要改成更显式（如 `_ => tracing::debug!("non-dev transition, no hook")`）以便观测。
3. transition 校验改成 can_transition 后，非 workflow session（无 workflow_def）的 transition 怎么校验（fallback 到旧 enum 校验？还是拒绝？）。

## Notes

- 本任务是 review epic 的最硬前置（C3/C2 都依赖），优先级高于 C1。
- 方案分析详见 `../research/taskstatus-custom-state.md`（含 X/Y/Z 三方案对比，采纳 X）。
- 与 C1（resume）无依赖，可并行。
