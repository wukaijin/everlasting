# PRD — RULE-ARGS-001 参数对象重构（agent loop 主链）

> Source debt: `.trellis/reviews/DEBT.md` §RULE-ARGS-001（P2, Agent Loop）
> Task: `.trellis/tasks/08-27-rule-args-001-param-object`

## Goal

把 agent loop 主链的超长线性参数签名收敛为少量领域对象，终结"每次 feature 往既有签名尾部追加一截"的债务生产模式，压缩漏穿参数类事故面（RULE-A-014 即此类事故的直接代价）。

## Background（实证锚点，2026-08-27 盘点）

- `run_chat_loop` 当前 **38 参**（`app/src-tauri/src/agent/chat_loop.rs:319`；DEBT/journal 的 18/25 均为过期数字）、`drive_turn` **49 参**（`chat_loop/drive.rs:166`）、`dispatch_tool_calls` **33 参**（`chat_loop/tools.rs:47`）、`prepare_loop_state` 19 参（`chat_loop/init.rs:121`）；`finalize_turn` 11 参（tools.rs:1779）。
- 全库 `#[allow(clippy::too_many_arguments)]` 实测 **46 处**（DEBT 记 43）；一期范围四文件合计 9 处。
- 生产调用 `run_chat_loop` 仅 4 处（chat.rs 直发 / queue driver / 群聊两条 / worker 嵌套递归）；双传输（Tauri 命令 + daemon HTTP）已收敛单一入口 `chat_inner`。
- 测试侧 **70 处**位参调用点（全按序传参，位置敏感——这正是"尾部追加一行改"红利与长期成本所在）。
- 详版证据：[research/run-chat-loop-signature-inventory.md](./research/run-chat-loop-signature-inventory.md)。

## Requirements

- **R1 引入 `ChatLoopDeps`**：承载 AppState 派生的长生命周期套件（db / cancellations / session_active_request / read_guard / caches / permission_asks / token / registries / stores 等，≈24 参的归宿），命名比照库内既有 `QueueDriverDeps` 先例。
- **R2 引入角色域对象**：`is_worker / worker_run_id / run_grants / overrides / forced_dispatch / skip_*` 等约 14 参聚为独立 suite（命名沿用 `XxxCtx` 惯例）；group chat 三件套可单独小簇。
- **R3 每-turn 可变状态维持既有 by-value 管道**：LoopInit 出参 → 循环体 mutation → DriveTurnOutcome 回传的现状不动，仅把 `drive_turn` 的恒定输入改为引用 Deps/request 对象；不把每-turn 状态塞进 Deps。
- **R4 显著瘦身后的公开签名**：四大主函数改造后显式参数 ≤ 8（clippy 默认阈值以下），并删除对应 `too_many_arguments` 豁免。
- **R5 语义保真（硬约束）**：
  - RULE-A-014 的 `is_worker` 显式传播路径与其回归测试 `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`（15s 退出）必须原样存活；
  - RULE-A-015（skip_persist 门不罩终态 Done emit）、RULE-A-016（worker 隔离）、entry invariant（尾条 message 新鲜未落盘）、messages[0] cache_control 断点顺序、P3/P4 seam 挂载点全部不变；
  - 行为零变化：纯重构，不改任何行为断言。
- **R6 无分叉**：生产与测试共用同一新签名；测试侧统一走 `tests_common` 构造 helper；严禁 v1/v2 共存（spec 防分叉条款）。顺带消除 `chat_inner` 与 `emit_max_turns_terminal` 两处 08-26 新增豁免。
- **R7 文档销账**：`signature-run-chat-loop.md`（23 参时代快照 + "不要拆 struct"旧警告）同任务内更新；完成后 DEBT.md 删除 RULE-ARGS-001 条目。

## Acceptance Criteria

1. `cargo clippy -p everlasting --lib -- -D warnings`（WSL 带 PKG_CONFIG_PATH）通过，且 `chat_loop.rs / drive.rs / tools.rs / init.rs` 四文件内 `too_many_arguments` allow 归零（含 `ask_turn_limit_softcap` 13 参、`attempt_summary_compaction` 13 参、`emit_max_turns_terminal` 8 参收编）。
2. `cargo test -p everlasting --lib` 全绿（基线 ~1689 tests，多线程），重点：RULE-A-014 回归测试、tests_message_queue 的 `QueueDriverDeps` 构造、F1 队列续轮、群聊双调用点。
3. `grep -rn "run_chat_loop_v2\|#\[allow(clippy::too_many_arguments)\]" app/src-tauri/src/agent/chat_loop*` 结果符合预期（前者 0，后者归零）。
4. spec 更新落地：`signature-run-chat-loop.md` 演进表与警告段重写为 parameter-object 形态。
5. remote crate 与前端无接口变化（`cargo test -p everlasting-remote` + app 构建不受影响）。

## Out of Scope

- 其余 ~37 处非 chat_loop 家族的 allow（providers CRUD / trace.rs / budget.rs / db 层 / daemon tunnel 等）——后续独立任务分批消化。
- `subagent/dispatch/*` 六处 allow（worker 镜像链）除非一期方案让它们免费受益，不做主动迁移。
- `run_group_chat_loop`（23 参）本身的签名收缩。
- 任何行为优化 / 重命名附带改动 / git blame 保护之外的注释整理。

## Open Questions

- ~~一期范围是否包含 `subagent/dispatch/*` 镜像链？~~ **已决（2026-08-27）**：按 design.md 分期默认执行——一期仅四文件 9 处（审查请求未获回复，采用推荐口径 A，提交门可再否决）；dispatch 镜像链二期。
