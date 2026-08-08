# A类单体重构:subagent dispatch 拆分

## Goal

把 `app/src-tauri/src/agent/subagent/dispatch.rs`(1472 行)中 `run_subagent` 的 1295 行单体函数按阶段提取为 per-stage 函数,再按 Rust 2018 module 模式子模块化(hub + `dispatch/` 子目录),使 `run_subagent` 主体变成清晰的阶段调用序列。行为零变化,为后续 `anthropic.rs` / `sink.rs` / `chat_loop.rs`(4298 行单体)的拆分沉淀 A 类方法论。

## Background / 已确认事实

- `dispatch.rs` 是 `subagent/` 目录的叶子文件(`pub(crate) mod dispatch;`),内含:
  - `run_subagent`(L100–1396,~1297 行)— 25 参数,返回 `(String, bool, bool, Option<i32>)`(content, is_error, cancel_parent, exit_code)
  - `check_workflow_role_gate`(L1412–1472,61 行)— 已是独立纯函数(W1 角色门控)
- `run_subagent` 内部有 ~24 个清晰阶段,每个阶段有独立注释块,边界明确(评审核验通过;阶段 B 实际跨不连续两段,已按评审拆分为 B1/B2 两个独立阶段函数,见 design §1/§2):
  - A 解析+校验(L219–319):参数解析、cache lookup(wf/legacy 分支)、unknown-name hint、空 task 校验、role gate
  - B1 决策(L343–402):isolation 决策(force_readonly > dispatch input > parallel+writable > frontmatter)、dispatch_model 候选解析
  - C worktree+guard+toolset(L411–527):worker_run_id/branch、`create_worker_worktree`(失败 fail dispatch)、project_main_override、worker ReadGuard 重置、`filter_tools_for_subagent`+readonly
  - D messages(L537–591):`task_with_env_hint`、resume_from 分支(`build_resume_messages`/`build_worker_messages`)、delegation template 注入
  - B2 model 解析(L619–697):`resolve_final_model`+dispatch_model overlay、`resolve_worker_provider`、worker_display backfill
  - E run 注册+sink(L720–849):worker_rid/token、cancellations 注册、`insert_run_with_id`、`set_worktree_path`、`SubagentBufferSink::new_with_event_sink`
  - F drive(L878–1045):`Box::pin(run_chat_loop(...))` 24 位置参数 + per-run grant cache
  - G 收集+持久化(L1058–1218):status picker(5 分支)、transcript/messages 截断、`update_run_finished`、`emit_subagent_finished`
  - H 收尾+格式化(L1229–1395):cancel_parent、partial_actions、worktree probe/auto-commit/destroy、`format_dispatch_result_with_model` + loop-terminated/changes/resume-fallback 三个 trailing append
- 前序拆分历史:批1(08-07)已拆 `prep.rs`(`build_resume_messages`/`task_with_env_hint`/`worker_is_writable`);批3(08-08)已拆 mod.rs 内联 cluster(definition/prompt/registry/tools_filter)。dispatch.rs 剩余部分为 A 类(非纯搬迁,需先提取函数)。
- 调用方 3 处,全部走全路径 `crate::agent::subagent::dispatch::run_subagent`(`chat_loop.rs:1313/3603/4151`),拆分后 hub re-export 保持路径不变,零调用方改动。
- 测试:`agent/subagent/tests_dispatch.rs`(#![cfg(test)] 文件级门控,~630 行)通过 `use super::dispatch::*;` 引用符号;含 `check_workflow_role_gate` 11 处、`run_subagent` 5 处引用 + resolve_isolation truth table + probe_worker_changes 测试。测试迁移后仍须经 hub 解析。
- 文档引用:`.trellis/spec/backend/tool-contract/04-dispatch-subagent.md`、`pattern-worker-worktree-override.md`、`subagent-runs-schema.md`、`docs/CONTEXT.md`、`docs/WORKFLOW-INTEGRATION*`、`docs/IMPLEMENTATION/decisions-*.md` 等多处引用 `dispatch.rs` / `run_subagent`(行号级引用需改符号引用)。
- 基线:`cargo test --lib` = 1657 测试全绿;`cargo fmt` + `clippy --lib --tests` 零警告。

## Requirements

- R1 `run_subagent` 主体重构为阶段调用序列(提取 per-stage 函数),每阶段一个提取 commit、独立回滚;禁止"搬了只是挪 blob"——提取即函数化,带明确输入输出契约。
- R2 按 Rust 2018 module 模式拆分:`dispatch.rs` 保留为 hub(`pub use` + `#[allow(unused_imports)]` 全量 re-export,对照 `tools/merge_worker.rs` 惯例),拆分文件进 `dispatch/` 子目录。
- R3 行为零变化:返回 tuple 形状、早期 error 返回路径、日志、DB 写入顺序、事件 emit 顺序均不变。
- R4 测试组织:新提取阶段的单元测试(若加)随代码就近放同文件 `#[cfg(test)] mod tests`;`tests_dispatch.rs` 保持单文件不拆分,通过 hub re-export 解析符号(文件级 `#![cfg(test)]` 门控保持)。
- R5 被测私有项升 `pub(crate)`,禁止为可测性改公开 API。
- R6 文档引用 sweep:旧路径/行号引用改符号引用;`.trellis/tasks/archive/` 历史快照不改。
- R7 收尾 `cargo fmt` + `clippy --lib --tests` + `fmt --check` 零警告;`cargo test --lib` 全绿(1657 基线 + 新增)。

## Acceptance Criteria

- [ ] AC1 `run_subagent` 主体 ≤ 250 行(不含 use 语句、不含 `// ===` 阶段注释 marker 块、含函数体;测量:`awk '/^pub.*fn run_subagent/,/^}$/' dispatch.rs | wc -l`):只剩阶段调用序列 + 极少数胶水代码,每个阶段一个具名函数调用。
- [ ] AC2 `dispatch.rs` 为 hub:子模块声明 + `pub use` re-export + 文档注释;re-export 后 `crate::agent::subagent::dispatch::run_subagent` 等既有路径解析不变。
- [ ] AC3 每个提取/拆分 commit 单独可回滚(独立 commit,不混入其他变更)。
- [ ] AC4 `cargo test --lib` 全绿(1657 基线无减少);`cargo fmt --check` + `clippy --lib --tests` 零警告。
- [ ] AC5 无公开 API 变更;所有 `pub` 新增项均为 `pub(crate)` 或模块内可见。
- [ ] AC6 文档/注释中的 `dispatch.rs:LINE` 行号引用已改符号引用;非 archive 文档无残留旧行号。
- [ ] AC7 `EarlyReturn` 早返路径不变量:`parse_dispatch` / `prepare_worker` 的早返 `is_error` 必为 `true`(角色门控拒绝、空 task、worktree 创建失败等所有现有点均为此值;`cancel_parent`/`exit_code` 沿正常路径初值)。
- [ ] AC8 `collect_outcome` 内部 persist 顺序固化:status picker → transcript/messages 截断 → `update_run_finished` → `emit_subagent_finished`,顺序与现状一致。

## Out of Scope

- 不改 `run_subagent` 25 参数签名、不改返回 tuple 语义(签名/形状债务另立任务,本任务只拆函数)。
- 不做 `anthropic.rs` / `sink.rs` / `chat_loop.rs`(后续专项)。
- 前端 `stores/chat.ts` / `ChatPanel.vue`(已评审为可选项,本任务不做)。
- 不新增 feature / 不修 bug / 不改行为。

## 已决决策(2026-08-08 用户评审)

- D1 中间状态组织:阶段输出 struct(parse/plan/prepare/register/collect 各输出 struct,drive/finalize 消费),非纯参数传递、非单一 ctx struct。
- D2 验收行数:AC1 以"run_subagent 主体 ≤250 行"为准;dispatch.rs hub 总行数不作硬性约束。
- D3 执行节奏:先提取(dispatch.rs 内部,每阶段独立 commit + 中间 cargo test)→ 再拆分(文件级移动单 commit)→ 文档 sweep(见 implement.md)。
