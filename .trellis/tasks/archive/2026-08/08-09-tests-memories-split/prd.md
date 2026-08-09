# 测试文件拆分:memories_tests.rs(2241 行)

## Goal

把 `app/src-tauri/src/db/memories_tests.rs`(2241 行)按被测功能簇拆分为目录化测试模块(`db/memories_tests/` hub + 簇文件),改善可导航性。延续拆分专项惯例:纯搬迁铁律——复制 + 删源 + module 接线,**禁止顺手改逻辑**。

> 批次衔接:批 2(`08-08-large-file-splitting-batch2`)PRD 把"memories_tests.rs 本身不拆"划出**当批**范围(仅批次区分,非永久排除);本任务落地批 1 总纲 Out of Scope 的"既有独立测试文件的拆分"一行。被测模块 `db/memories.rs` 已在该批拆完(hub + 子模块),本任务只动测试文件。

## Background / 已确认事实

- 声明处:`db/mod.rs:60` `pub mod memories_tests;`(文件级 `#![cfg(test)]`)。**目录化后此声明零改动**(Rust 自动解析到 `memories_tests/mod.rs`)。
- 头注释说明:本文件是 autonomous_memories 域集成测试(`test_pool` helper 为复制而来,镜像既有 6 个 `db/*_tests.rs` 域文件)。
- 实测结构:40 个测试函数 + 3 个 helper(`test_pool`@L35 / `make_pool`@L45 / `reseat_created_at`@L1710)。
- 功能簇(行号为实测起点,切分以函数边界/控制流为准):
  | 簇 | 行号区间 | 测试数 | 代表函数 |
  |---|---|---|---|
  | migration / schema check | L126–329 | 2 | `am_migration_is_idempotent` / `am_db_check_rejects_invalid_enum_and_oversize` |
  | insert_memory(roundtrip / scope / 校验 / 敏感 / 泛化) | L330–567 | 9 | `insert_memory_happy_path_roundtrip` / `insert_memory_rejects_*` / `insert_memory_generalizes_home_path` |
  | list / delete / search FTS / count | L568–1213 | 7 | `list_memories_filters_by_scope_correctly` / `delete_memory_removes_row_and_fts_index` / `search_memories_fts_*` / `count_memories_for_session_counts_across_statuses` |
  | pitfalls 查询 | L1214–1415 | 3 | `find_pitfalls_by_trigger_tool_name_exact_match` / `find_pitfalls_path_globs_semantics` / `find_pitfalls_command_pattern_substring_filter` |
  | hit_count / status 迁移 / FTS 触发器 / query plan | L1416–1709 | 4 | `bump_hit_count_increments_and_stamps_last_used` / `update_status_legal_and_illegal_transitions` / `fts_triggers_sync_on_insert_update_delete` / `explain_query_plan_uses_index` |
  | validate_memory_text 单测 | L1932–1987 | 5 | `validate_memory_text_happy_path_generalizes_home` / `validate_memory_text_rejects_*` |
  | update_memory | L1988–2177 | 7 | `update_memory_roundtrip` / `update_memory_rejects_*` / `update_memory_sets_edited_by_user` / `update_memory_not_found` |
  | build_recall_text | L2178–2241 | 3 | `build_recall_text_with_rows_returns_rows` 等 |
- 基线:`cargo test --lib` = 1662 全绿;`clippy --lib --tests` + `fmt --check` 零警告。拆分不增删测试,终验 N 不变。

## Requirements

- R1 拆分形态:**目录化**(首选)——建 `db/memories_tests/` 目录,`mod.rs` 作 hub(`mod` 声明 + 共享 helper 提取),各簇独立文件;`db/mod.rs:60` 声明零改动。备选:平级多文件(需改 db/mod.rs 加声明)——design 阶段拍板,以声明零改动为优先。
- R2 纯搬迁铁律:复制 + 删源 + module 接线;禁止改逻辑 / 重命名 / 合并 / 拆分测试函数。
- R3 共享 helper(`test_pool` / `make_pool` / `reseat_created_at`)提取到 hub,`pub(crate)` 可见性;簇间复用经 `use super::*`。注意被测符号路径 `crate::db::memories::X`(memories.rs 拆分后经 hub re-export)保持稳定。
- R4 每簇独立 commit、独立回滚(回滚点 `git revert <commit>`);每 commit 后跑该簇过滤 + 全量 `cargo test --lib`。
- R5 测试函数数量与名称零变化(40 个测试原样保留)。
- R6 非 archive 文档/代码注释若引用 `memories_tests.rs:LINE` 行号,随拆改符号引用。

## Acceptance Criteria

- [ ] AC1 各子文件(含 hub)< 1200 行
- [ ] AC2 `cargo test --lib` 全绿(1662 基线无减少,40 个测试原样)
- [ ] AC3 `db/mod.rs` 声明零改动(目录化方案);`cargo check --lib` 调用方零影响
- [ ] AC4 `clippy --lib --tests` + `fmt --check` 零警告
- [ ] AC5 每个拆分 commit 独立可回滚;diff 只含代码位移 + module 接线,无逻辑改动
- [ ] AC6 非 archive 无残留 `memories_tests.rs:LINE` 行号引用

## Out of Scope

- 不改被测业务代码(`db/memories.rs` 及其子模块)
- 不新增 / 不重命名 / 不合并测试(含 helper)
- 不改变测试语义、执行顺序与并行度
- 其余 3 个测试大文件各自独立任务:`08-09-tests-agent-loop-split` / `08-09-tests-subagent-split` / `08-09-tests-sessions-split`
