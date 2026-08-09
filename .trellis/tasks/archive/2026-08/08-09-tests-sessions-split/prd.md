# 测试文件拆分:sessions_tests.rs(1493 行)

## Goal

把 `app/src-tauri/src/db/sessions_tests.rs`(1493 行)按被测功能簇拆分为目录化测试模块(`db/sessions_tests/` hub + 簇文件),改善可导航性。延续拆分专项惯例:纯搬迁铁律——复制 + 删源 + module 接线,**禁止顺手改逻辑**。

> 批次衔接:批 3(`08-08-batch3-large-file-splitting`)对 `db/sessions.rs` 拆分时把"sessions_tests.rs(1493 行)不动"划出**当批**范围(仅批次区分,非永久排除);本任务落地批 1 总纲 Out of Scope 的"既有独立测试文件的拆分"一行。

## Background / 已确认事实

- 声明处:`db/mod.rs:72` `pub mod sessions_tests;`(文件级 `#![cfg(test)]`)。**目录化后此声明零改动**(Rust 自动解析到 `sessions_tests/mod.rs`)。
- 头注释说明:本文件 2026-06-23 从 `db/tests.rs` **物理拼接**两段而来(`282-644` session CRUD + worktree state + system events;`935-1551` 消息/latency 段)——天然对应两大簇。
- 实测结构:**35 个测试函数**(PRD 表头与 task.json 描述原写"33"是笔误,以实测 35 + design §0 为准)+ 2 个 hub helper(`test_pool`@L38 / `make_pool`@L49),无 cluster-private helper。被测符号经 `crate::db::sessions::X`(sessions.rs 拆分后 hub re-export)路径稳定。
- 功能簇(行号为实测起点,切分以函数边界/控制流为准):
  | 簇 | 行号区间 | 测试数 | 代表函数 |
  |---|---|---|---|
  | session CRUD(scopes / 标题 / 级联删除) | L53–346 | 7 | `create_session_scopes_to_project` / `first_user_message_auto_titles_session` / `delete_session_cascades_messages` / `delete_messages_by_session_keeps_session_drops_messages` |
  | 字段 / worktree 状态(cwd / workflow / plugin / model 无关项) | L347–649 | 10 | `touch_session_updates_timestamp` / `update_session_cwd_persists` / `set_session_workflow_enabled_*` / `worktree_state_*` |
  | system events | L650–737 | 2 | `insert_system_event_appends_to_history` / `insert_system_event_seq_increments` |
  | model_id / usage / list | L738–943 | 5 | `update_session_model_id_sets_and_clears` / `load_session_includes_model_id` / `update_last_turn_usage_overwrites_not_accumulates` / `list_sessions_includes_last_turn_columns` |
  | latency / message 定位 / tool duration | L944–1493 | 9 | `persist_turn_with_latency_writes_three_columns` / `update_message_latency_*` / `find_message_id_by_seq_returns_none_for_unknown_pair` / `record_tool_duration_*` |
- 基线:`cargo test --lib` = 1662 全绿;`clippy --lib --tests` + `fmt --check` 零警告。拆分不增删测试,终验 N 不变。

## Requirements

- R1 拆分形态:**目录化**(首选)——建 `db/sessions_tests/` 目录,`mod.rs` 作 hub(`mod` 声明 + 共享 helper 提取),各簇独立文件;`db/mod.rs:72` 声明零改动。备选:平级多文件(需改 db/mod.rs 加声明)——design 阶段拍板,以声明零改动为优先。
- R2 纯搬迁铁律:复制 + 删源 + module 接线;禁止改逻辑 / 重命名 / 合并 / 拆分测试函数。
- R3 共享 helper(`test_pool` / `make_pool`)提取到 hub,`pub(crate)` 可见性;簇间复用经 `use super::*`。
- R4 每簇独立 commit、独立回滚(回滚点 `git revert <commit>`);每 commit 后跑该簇过滤 + 全量 `cargo test --lib`。高频引用(`load_session` 141 / `create_session` 112 等跨簇复用)拆前 grep 核对归属。
- R5 测试函数数量与名称零变化(35 个测试原样保留;`cargo test --lib "db::sessions_tests::"` 跑前 35 / 跑后 35)。
- R6 非 archive 文档/代码注释若引用 `sessions_tests.rs:LINE` 行号,随拆改符号引用。

## Acceptance Criteria

- [ ] AC1 各子文件(含 hub)< 1200 行
- [ ] AC2 `cargo test --lib` 全绿(1662 基线无减少,35 个测试原样)
- [ ] AC3 `db/mod.rs` 声明零改动(目录化方案);`cargo check --lib` 调用方零影响
- [ ] AC4 `clippy --lib --tests` + `fmt --check` 零警告
- [ ] AC5 每个拆分 commit 独立可回滚;diff 只含代码位移 + module 接线,无逻辑改动
- [ ] AC6 非 archive 无残留 `sessions_tests.rs:LINE` 行号引用

## Out of Scope

- 不改被测业务代码(`db/sessions.rs` 及其子模块)
- 不新增 / 不重命名 / 不合并测试(含 helper)
- 不改变测试语义、执行顺序与并行度
- 其余 3 个测试大文件各自独立任务:`08-09-tests-agent-loop-split` / `08-09-tests-subagent-split` / `08-09-tests-memories-split`
