# A类单体重构:subagent dispatch 拆分 — Design

## 1. 目标形态

`run_subagent`(1295 行)→ 8 个 per-stage 函数 + 主体调用序列(~100 行)。中间状态以**阶段输出 struct** 传递(用户拍板,PRD Open Question 1 已决)。

> **评审 P1-1 修订(2026-08-08)**:阶段 B 跨不连续两段(L343–402 + L619–697),原 design 合并进一个 `plan_worker` 需穿越中间 ~200 行 C/D 代码,行为漂移风险高。**调整为两个连续段函数**:`plan_worker` 只含 B1(L343–402),`resolve_worker` 承接 B2(L619–697)。每个阶段函数严格对应代码中的连续段,提取不改变执行顺序。

```
run_subagent (hub, 25 参数签名不变)
  ├─ parse_dispatch     → Result<ParsedDispatch, EarlyReturn>     // 阶段 A (L219–319)
  ├─ plan_worker        → WorkerPlan                              // 阶段 B1 (L343–402)
  ├─ prepare_worker     → Result<WorkerPrep, EarlyReturn>         // 阶段 C+D (L411–591)
  ├─ resolve_worker     → WorkerModel                             // 阶段 B2 (L619–697)
  ├─ register_run       → RegisteredRun                           // 阶段 E (L720–849)
  ├─ drive_worker       → ()                                      // 阶段 F (L878–1045)
  ├─ collect_outcome    → WorkerOutcome                           // 阶段 G (L1058–1218,含 persist 副作用)
  └─ finalize_dispatch  → (String, bool, bool, Option<i32>)       // 阶段 H (L1229–1395)
```

## 2. 阶段边界与输出 struct(数据流契约)

单向数据流,无回环;每阶段消费"上一阶段输出 struct + 所需外部借用参数"。

| struct | 字段 | 来源阶段 | 消费方 |
|---|---|---|---|
| `ParsedDispatch` | `subagent_name: String`、`task: String`、`loaded: LoadedSubagent`(含 `def`)、`project_id: String`、`project_path: String`、`tool_use_id_owned: String` | A | B1、C/D、B2、E |
| `WorkerPlan` | `isolated: bool`、`dispatch_model: Option<String>` | B1 | C/D(prepare 消费 `isolated`)、B2(resolve 消费 `dispatch_model`) |
| `WorkerModel` | `worker_provider: Arc<dyn Provider>`、`worker_ctx: u32`、`worker_display: Option<String>` | B2 | F(drive)、G/H(display 供 finalize) |
| `WorkerPrep` | `worker_run_id: String`、`worker_branch: String`、`worker_worktree_opt: Option<PathBuf>`、`project_main_override: Option<PathBuf>`、`worker_read_guard: ReadGuard`、`worker_tool_defs: Vec<ToolDef>`、`worker_messages: Vec<ChatMessage>`、`resume_fallback_note: Option<String>` | C+D | E、F、H |
| `RegisteredRun` | `worker_rid: String`、`worker_token: CancellationToken`、`worker_run_id_opt: Option<String>`、`worker_sink: Arc<SubagentBufferSink>`、`event_run_id: String` | E | F、G、H |
| `WorkerOutcome` | `worker_text: String`、`worker_loop_terminated: bool`、`status: SubagentStatus` | G | H |

`type EarlyReturn = (String, bool, bool, Option<i32>);` — 早期 error 返回与 run_subagent 返回 tuple 同形(A 阶段 unknown-name / 空 task / role gate 拒绝,C/D 阶段 worktree 创建失败)。

> **不变量(评审 P2-2 固化)**:`EarlyReturn` 早返路径必须满足 `is_error == true`(现有 L319 角色门控、L280 unknown-name、L286 空 task、L451 worktree 创建失败等所有早返点均为此值;`cancel_parent` 与 `exit_code` 沿用正常路径初值)。`parse_dispatch` / `prepare_worker` 实现时禁止构造"友好早返"(`is_error: false`)——否则 `chat_loop.rs` 三处调用点对 `is_error` 的判断改变行为路径,违反 R3。对应 AC7。

### 阶段函数签名(提取时以实际借用为准,此为形态)

- `parse_dispatch(db, parent_session_id, current_ctx, subagent_cache, workflow_ctx, input, tool_use_id) -> Result<ParsedDispatch, EarlyReturn>`
- `plan_worker(db, input, parsed, force_readonly, parallel) -> WorkerPlan`(isolation 决策 + dispatch_model 候选解析;**连续段 L343–402**,不穿越 C/D)
- `prepare_worker(db, parent_session_id, app_data_dir, current_ctx, parsed, plan, memory_cache, input) -> Result<WorkerPrep, EarlyReturn>`(worktree 创建 + guard 重置 + toolset + messages + resume 分支 + delegation template;**连续段 L411–591**)
- `resolve_worker(db, provider, context_window, catalog, parent_session_id, parsed, plan) -> WorkerModel`(`resolve_final_model` + dispatch_model overlay + `resolve_worker_provider` + worker_display backfill;**连续段 L619–697**)
- `register_run(cancellations, db, parent_rid, tool_use_id: &str, project_id: &str, prep: &WorkerPrep, parent_token, worker_event_sink, parent_session_id) -> RegisteredRun`(评审 P2-1:签名收窄——`parsed` 仅消费 `tool_use_id_owned` / `project_id` 两字段,直接传 `&str`;`plan` 在 register 阶段零消费,不传入)
- `drive_worker(model: &WorkerModel, prep: &WorkerPrep, reg: &RegisteredRun, + 外部借用: db, cancellations, session_active_request, memory_cache, skill_cache, permission_asks, background_shells, app_data_dir, parent_session_id, parent_question_store)` — 输入 = 3 个阶段输出 struct + 外部借用,内部构造 per-run grant cache + `Box::pin(run_chat_loop(...))`
- `collect_outcome(db, parent_session_id, reg) -> WorkerOutcome`(status picker + transcript/messages 截断 + `update_run_finished` + `emit_subagent_finished`;**内部严格按原代码顺序,AC8**)
- `finalize_dispatch(db, parent_session_id, parent_token, parsed, plan, prep, reg, model, outcome) -> (String, bool, bool, Option<i32>)`(cancel_parent + partial_actions + worktree probe/auto-commit/destroy + format + 3 个 trailing append)

**已知债务(显式接受)**:`drive_worker` 参数约 14 个(3 struct + 11 外部借用)。这是 `run_chat_loop` 24 位置参数调用点的直接映射,命名化后已优于散落位置参数;外部借用参数不打包(尊重"阶段输出 struct"决策,不引入第二个 ctx 类 struct)。本任务不为 `drive_worker` 进一步结构化;若后续 A 类任务中该函数仍 > 250 行,另立任务。

## 3. 文件布局(Rust 2018 module 模式)

参照 `tools/merge_worker.rs`(hub + `merge_worker/{execute,merge,finalize}.rs`)惯例:

```
agent/subagent/dispatch.rs          # hub:run_subagent 主体 + check_workflow_role_gate + pub use re-export
agent/subagent/dispatch/parse.rs    # ParsedDispatch + parse_dispatch (A)
agent/subagent/dispatch/plan.rs     # WorkerPlan + plan_worker (B1)
agent/subagent/dispatch/prepare.rs  # WorkerPrep + prepare_worker (C+D)
agent/subagent/dispatch/model.rs    # WorkerModel + resolve_worker (B2)
agent/subagent/dispatch/register.rs # RegisteredRun + register_run (E)
agent/subagent/dispatch/drive.rs    # drive_worker (F)
agent/subagent/dispatch/finalize.rs # WorkerOutcome + collect_outcome + finalize_dispatch (G+H)
```

> 命名注:`dispatch/model.rs` 与 `subagent/resolve.rs`(既有,含 `resolve_final_model` 等纯函数)不冲突——路径不同,且 model.rs 聚焦 worker model 解析流程而非 resolve 纯函数。

- hub 声明 `pub(crate) mod parse; ...` + `#[allow(unused_imports)] pub(crate) use parse::*;`(全量 re-export,对照 merge_worker.rs / wire 惯例),保证 `crate::agent::subagent::dispatch::run_subagent` 与 `tests_dispatch.rs` 的 `use super::dispatch::*` 零改动解析。
- `check_workflow_role_gate` 保留在 hub(已是独立纯函数,被 tests_dispatch.rs 直接引用 11 处;挪动无收益)。
- struct 字段可见性:`pub(crate)` 字段(跨子模块消费);`def` 经 `LoadedSubagent` 引用时注意生命周期(字段化后需 owned 或 Arc,见 §5 风险)。

## 4. 兼容与迁移

- **调用方**:3 处(`chat_loop.rs:1313/3603/4151`)走 `crate::agent::subagent::dispatch::run_subagent` — hub 保留定义,零改动。
- **测试**:`tests_dispatch.rs` 保留,`use super::dispatch::*` 经 hub re-export 解析;run_subagent 签名不变,现有 1657 测试全绿即行为零变化的验证锚点。
- **文档**:spec/docs 中 `dispatch.rs:LINE` 行号引用改符号引用;路径引用(`agent/subagent/dispatch.rs`)保留(文件仍存在);历史决策文档(`docs/IMPLEMENTATION/decisions-*.md`、`.trellis/tasks/archive/`)不动。
- **DB / 事件顺序**:阶段重排禁止;每个阶段函数内部保持原代码顺序,提取只改"代码所在函数",不改"执行顺序"。

## 5. 风险与回滚

| 风险 | 缓解 |
|---|---|
| 借用/生命周期冲突(struct 字段化导致 borrow checker 报错)| 提取顺序按依赖排序(parse → plan → prepare → resolve → register → drive → collect → finalize),每步 `cargo check` 即时报;`LoadedSubagent` 若需跨阶段消费,将 `def` 移出或 clone(字段为 owned) |
| 行为漂移(顺序/日志/DB) | 每提取一个阶段:独立 commit + `cargo test --lib` 全绿;`tracing` 日志原样保留;早期返回路径逐一核对;每个阶段函数对应**连续代码段**(P1-1 修订后无跨段合并) |
| 阶段 B1/B2 之间变量被改写 | 提取前 `git blame` 核对 L343–402 / L619–697 两段引用的变量(`dispatch_model` / `isolated` / `def` / `project_id`)在 L403–618 区间无改写(已核对:区间仅新增 `worker_run_id` 等新局部,不改写 B1 输出) |
| 提取后文件过大 | 每阶段提取立即独立 commit,单 commit 可控(~150-250 行 diff);全部提取完才做文件级拆分(最后一个 commit,纯移动) |
| 测试路径断 | hub 全量 re-export 保证 `dispatch::*` 解析;拆分 commit 前先 `cargo check` 验证 |

**回滚**:每个提取 commit 独立(AC3);`git revert <commit>` 即回退单阶段,不影响其他阶段。

## 6. 明确不做

- 不改 25 参数签名 / 返回 tuple 形状(PRD Out of Scope)。
- 不引入 ctx struct / env struct(用户选定阶段输出 struct 方案)。
- 不新增阶段级单元测试(现有 run_subagent 整体测试 + role gate 11 处 + isolation truth table 已覆盖行为;提取不改变逻辑)。若提取中发现某纯函数恰好可独立测且成本极低,作为可选加分项,不承诺。
