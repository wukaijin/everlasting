# Design — 大文件拆分批 3:12 处 B 类纯搬迁

> PRD: `prd.md`。本设计覆盖 12 个源码文件的簇拆分 / 测试迁出 / 文档引用同步。无文档拆分(本批纯源码)。

## 0. 拆分总原则(延续批 1/批 2)

- **纯搬迁**:内容原样复制 → 删源 → 新 module 接线;禁止顺手改逻辑(R3)。
- **9 文件 = 纯测试搬迁**(迁出内嵌 tests 即 <1200);**3 文件需结构拆**(migrations/sessions/check,见 §1/§8/§9 详注)。
- **验证节奏**:每个文件拆分后跑对应模块测试(`cargo test --lib "<filter>"`);每 3-4 个文件做一轮 clippy/fmt 中检;最终全量终验。
- **回滚**:每个文件拆分独立 commit,`git revert <commit>` 即回滚。
- **调用点不变**:经 hub re-export 保持既有 `use crate::<module>::X` 调用点零改动。
- **Rust 2018 模式**:`foo.rs` hub + `foo/` 子目录共存(hub 用 `pub use` + `#[allow(unused_imports)]` 全量 re-export,对照 `wire/mod.rs`、`git/worktree.rs` 批 1/批 2 惯例)。子模块 `#[cfg(test)]` 测试迁 `tests_*.rs` 同级文件,加 `#![cfg(test)]` 文件级门控。
- **链接更新范围**:只改活跃文档(`docs/`、`.trellis/spec/`、`AGENTS.md`);`.trellis/tasks/archive/` 是历史记录不改。
- **执行顺序**(按风险递增,纯测试搬迁先做热身,3 处结构拆放后):
  1. `llm/types.rs`(纯类型目录,最干净,验证 llm/mod.rs re-export 模式)
  2. `agent/workflow/task.rs`(纯同步 IO,零共享状态)
  3. `agent/workflow/inject.rs`(验证 inject.rs 物理顺序错位处理)
  4. `agent/permissions/shell_trust.rs`(纯函数,验证 check.rs 的 `super::shell_trust::` 路径)
  5. `agent/subagent/mod.rs`(已是 hub,搬内联类型/函数到子文件)
  6. `tools/merge_worker.rs`(含 `static LOCKS`,验证锁序保持)
  7. `tools/request_task_state_transition.rs`(2 caller 最干净)
  8. `tools/shell.rs`(验证 `apply_safe_env` 跨模块 pub(crate) 复用)
  9. `tools/web_fetch.rs`(验证 `WebFetchError` 被 error.rs 消费)
  10. `db/sessions.rs`(**结构拆 #1**:session CRUD vs message 持久化双簇)
  11. `agent/permissions/check.rs`(**结构拆 #2**:check 路径 vs pitfall 路径双簇)
  12. `db/migrations.rs`(**结构拆 #3**:pool init vs schema 迁移;run_migrations 单体只搬不切)

## 1. `llm/types.rs`(1229 → hub <120 + 子模块) — ① 最先

- 现状:8 簇纯类型目录,弱内耦合(唯一环 `ChatEvent::Error` → `LlmErrorCategory`)。内嵌测试 578-1229(651 行)。
- 目标(`llm/types.rs` hub + `llm/types/` 子目录):
  - `llm/types.rs`(hub)— module 声明 + re-export 全部 pub 类型(**R1.3 关键**:`llm/mod.rs` 现 `pub use types::{ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Role, ToolDef}`,hub 必须 `pub use` 全量 re-export 保持 ~10 个 Pattern-2 调用方零改动)
  - `llm/types/message.rs` — `Role`/`CacheControl`/`ContentBlock`/`is_false`/`MessageContent`+ impl + 2 Serde impl(cluster B+C 紧耦合,不可分)
  - `llm/types/chat.rs` — `ChatMessage`/`ToolDef`+ impl(cluster D)
  - `llm/types/request.rs` — `ThinkingConfig`/`ChatRequest`(cluster E)
  - `llm/types/usage.rs` — `TokenUsage`(cluster F,standalone)
  - `llm/types/event.rs` — `ChatEvent`/`RecallHit`/`LlmErrorCategory`(cluster G+H,因 ChatEvent::Error 引用 LlmErrorCategory 必须同文件)
- 测试:内嵌 651 行 → `llm/tests_types.rs`(`llm/mod.rs` 加 `#[cfg(test)] mod tests_types;`)。
- 验证:`cargo test --lib "llm::types"` + `cargo test --lib "tests_types"` + `cargo test --lib "tests"` 检查 Pattern-2 调用。
- 预期 hub <120 行。

## 2. `agent/workflow/task.rs`(1527 → hub ~300 + 子模块) — ②

- 现状:8 簇纯同步文件 IO,零共享状态(Mutex/Atomic/static 均无)。内嵌测试 794-1527(733 行)。已有父 mod.rs re-export 13 符号。
- 目标(`agent/workflow/task.rs` hub + `agent/workflow/task/` 子目录):
  - `agent/workflow/task.rs`(hub)— module 声明 + `pub use` re-export 13 符号(对照现 mod.rs `pub use task::{archive_task_init, create_task_init, read_task, task_dir, task_json_path, task_prd_path, validate_slug, write_task, TaskError, TaskItem, TaskJson, TaskResult, TaskStatus, PROJ_NS_TASKS_ARCHIVE_DIR}`)
  - `agent/workflow/task/types.rs` — `TaskStatus`+3 impl / `TaskItem` / `TaskJson` / 2 serde helper / `TaskError` / `TaskResult`(cluster B+C)
  - `agent/workflow/task/paths.rs` — `validate_slug` / `task_dir` / `task_json_path` / `task_prd_path` / `PROJ_NS_TASKS_DIR` / `MAX_SLUG_LEN`(cluster D+E)
  - `agent/workflow/task/io.rs` — `read_task` / `write_task` / `create_task_init`(cluster F)
  - `agent/workflow/task/archive.rs` — `PROJ_NS_TASKS_ARCHIVE_DIR` / `archive_task_init` / `git_add_path` / `git_commit`(cluster G)
- **2 个 `::task::` 直路径调用方**(`commands/question.rs:53`、`tools/request_task_state_transition.rs:91`)——hub 保持 `task` 模块名则零改动。
- 测试:内嵌 733 行 → `agent/workflow/tests_task.rs`(`agent/workflow/mod.rs` 声明)。fixture helpers 随迁。
- 验证:`cargo test --lib "workflow::task"` + `cargo test --lib "tests_task"`。
- 预期 hub ~300 行(re-export + 类型定义可留 hub 或全搬,按内聚定)。

## 3. `agent/workflow/inject.rs`(1222 → hub ~700 + 子模块) — ③

- 现状:6 簇,零共享状态。**物理顺序错位**:`breadcrumb_body`(629)/`build_breadcrumb_block`(617)物理在 delegation 簇后、测试前,但逻辑属 breadcrumb 簇。内嵌测试 708-1222(514 行)。
- 目标:测试迁出后剩 708 行 <1200,**可选不再结构拆**(但为内聚建议拆)。
  - 最小方案(只迁测试):`inject.rs` 留全部代码 708 行,内嵌测试 → `agent/workflow/tests_inject.rs`。
  - 进阶方案(内聚拆,可选):hub + `inject/breadcrumb.rs`(cluster D:append_workflow_breadcrumb/build_breadcrumb_block/breadcrumb_body)+ `inject/delegation.rs`(cluster E:compute_delegation_template/resolve_relevant_specs/append_delegation_template)+ `inject/ctx.rs`(cluster B+C:WorkflowCtx/build_workflow_ctx/resolve_current_task)。
- **建议最小方案**(降风险:物理顺序错位增加搬迁出错概率,纯测试搬迁已 <1200)。
- **3 种访问路径并存**(flat `crate::agent::workflow::<sym>` / 直 `::inject::<sym>` / re-export)——拆分必须保持 `inject` 模块名(最小方案天然保持)。
- 测试:内嵌 514 行 → `agent/workflow/tests_inject.rs`。
- 验证:`cargo test --lib "workflow::inject"` + `cargo test --lib "tests_inject"`。

## 4. `agent/permissions/shell_trust.rs`(1458 → 790,纯测试搬迁) — ④

- 现状:5 簇纯函数(无 struct 共享状态、无 static)。4 个分类表 const + 分类引擎紧耦合(`classify_prefix`→`split_top_level`→`classify_single`→...)。内嵌测试 790-1458(668 行,引用私有 `first_token`/`split_top_level`/`has_command_substitution`/`detect_write_redirect`)。
- 目标:测试迁出后 790 行 <1200。**不再结构拆**(引擎簇不可分,表 const 是引擎私有数据)。
  - `shell_trust.rs` 留全部代码 790 行,内嵌测试 → `agent/permissions/tests_shell_trust.rs`(`agent/permissions/mod.rs` 声明)。
- **`check.rs` 通过 `super::shell_trust::` 访问**(`has_structural_metachar`/`first_token_for_allow_always`/`classify_prefix`/`ShellTrust`)——shell_trust.rs 留原位模块名,check.rs 零改动。
- 测试:内嵌 668 行 → `agent/permissions/tests_shell_trust.rs`;被测私有 fn(`first_token`/`split_top_level`/`has_command_substitution`/`detect_write_redirect`/`classify_single`/`classify_git_subcommand`)升 `pub(crate)`(R1.2)。`max_of` 已 `pub(crate)` 无需动。
- 验证:`cargo test --lib "shell_trust"` + `cargo test --lib "tests_shell_trust"`。

## 5. `agent/subagent/mod.rs`(1389 → hub ~350 + 子文件) — ⑤

- 现状:已是 Rust 2018 hub(13 个兄弟子文件)。8 簇,1 个 function-local `OnceLock`(在 `builtin_subagents` 内,随函数搬)。内嵌测试 928-1389(461 行)。另有 2 个外部测试文件 `tests_dispatch.rs`/`tests_loader.rs`(不动)。
- 目标:测试迁出后 928 行 <1200,**可选不再结构拆**。但为内聚建议把大簇搬出到子文件(hub 已是目录模块,模式天然):
  - `subagent/mod.rs`(hub)— 留 module 声明 + re-export + 小簇(ForcedDispatch/ModelBrief/SubagentStatus)+ DISPATCH_TOOL_NAME const
  - `subagent/definition.rs` — `definition`/`definition_with_cache`(cluster C,~270 行)
  - `subagent/registry.rs` — `SubagentDef`/`builtin_subagents`(含 OnceLock)/`lookup_subagent`(cluster D,~170 行)
  - `subagent/prompt.rs` — `assemble_subagent_prompt`/`build_worker_messages`(cluster E)
  - `subagent/tools_filter.rs` — `STRUCTURALLY_DISABLED`/`filter_tools_for_subagent`/`READONLY_TOOL_ALLOWLIST`/`filter_tools_readonly`(cluster F)
- **直路径调用方**(`crate::agent::subagent::DISPATCH_TOOL_NAME`/`definition_with_cache`/`ForcedDispatch`/`ModelBrief`)——搬出到子文件后 hub 必须 `pub use` re-export 保持。
- 测试:内嵌 461 行 → `subagent/tests_mod.rs`(`subagent/mod.rs` 声明,与现 `tests_dispatch`/`tests_loader` 同级)。
- 验证:`cargo test --lib "subagent::tests_mod"` + `cargo test --lib "tests_dispatch"`(确保 re-export 未破坏)。

## 6. `tools/merge_worker.rs`(1251 → hub ~870 + 子模块) — ⑥

- 现状:8 簇,**含 `static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>>` (line 304,in `merge_lock_for`)**。内嵌测试 914-1251(337 行)。5 个 pub fn(widest API surface)。
- 目标(`tools/merge_worker.rs` hub + `tools/merge_worker/` 子目录):
  - `tools/merge_worker.rs`(hub)— module 声明 + `pub use` re-export 6 pub 符号(`definition`/`execute`/`ensure_parent_worktree_attached`/`do_merge_blocking`/`merge_session_into_main`/`finalize_merge`)
  - `tools/merge_worker/execute.rs` — `definition`/`execute`(cluster B+C)
  - `tools/merge_worker/merge.rs` — `merge_lock_for`(**含 static LOCKS**)+ `do_merge_blocking`/`merge_session_into_main`/`is_ancestor`/`collect_conflict_paths`(cluster D+F,**锁与合并体必须同文件**)
  - `tools/merge_worker/finalize.rs` — `ensure_parent_worktree_attached`/`finalize_merge`(cluster E+G)
- **R1.5 跨簇**:`finalize_merge` 调 `destroy_worker`(git)和 db;`execute` 调 `do_merge_blocking`/`finalize_merge`/`ensure_parent_worktree_attached`——经 hub re-export 或 `use super::...` 引入。
- 测试:内嵌 337 行 → `tools/tests_merge_worker.rs`(`tools/mod.rs` 声明)。
- 验证:`cargo test --lib "merge_worker"` + `cargo test --lib "tests_merge_worker"`;确认 `static LOCKS` 仍是 process-global(搬 merge.rs 后路径变但语义不变)。

## 7. `tools/request_task_state_transition.rs`(1506 → 735,纯测试搬迁) — ⑦

- 现状:6 簇,零共享状态。2 个 caller(`mod.rs::builtin_tools` definition、`chat_loop.rs:4073` execute_blocking)。内嵌测试 736-1506(771 行)。`execute_blocking` 390 行单体(不切)。
- 目标:测试迁出后 735 行 <1200。**不再结构拆**(单体 execute_blocking 不可分,其余簇小)。
  - `request_task_state_transition.rs` 留全部代码 735 行,内嵌测试 → `tools/tests_request_task_state_transition.rs`。
- 测试:内嵌 771 行 → `tools/tests_request_task_state_transition.rs`;被测 `validate`/`ValidationError` 已 `pub(crate)`。
- 验证:`cargo test --lib "request_task_state_transition"` + `cargo test --lib "tests_request_task_state_transition"`。

## 8. `tools/shell.rs`(1472 → 587,纯测试搬迁) — ⑧

- 现状:6 簇,零模块级共享状态。`apply_safe_env`(`pub(crate)`)被 `background_shell/in_memory.rs:196` 复用——**load-bearing pub(crate)**。内嵌测试 588-1472(885 行)。
- 目标:测试迁出后 587 行 <1200。**不再结构拆**(为降风险,纯测试搬迁足够)。
  - `shell.rs` 留全部代码 587 行,内嵌测试 → `tools/tests_shell.rs`。
- 测试:内嵌 885 行 → `tools/tests_shell.rs`;被测私有 fn(`kill_and_collect`/`format_output`/`spill_to_disk`/`head_tail_preview`/`truncate_output`)升 `pub(crate)`。
- **R1.2 关键**:`apply_safe_env` 保持 `pub(crate)` 不变(已是对外可见性,不动)。
- 验证:`cargo test --lib "tools::shell"` + `cargo test --lib "tests_shell"`;确认 `background_shell::in_memory` 编译通过。

## 9. `tools/web_fetch.rs`(1371 → 854,纯测试搬迁) — ⑨

- 现状:9 簇,零共享状态。`WebFetchError`(pub)被 `error.rs:22` 消费(`impl AppError`/`From`)——**load-bearing pub**。内嵌测试 855-1371(517 行)。
- 目标:测试迁出后 854 行 <1200。**不再结构拆**(SSRF + fetch 紧耦合,纯测试搬迁足够)。
  - `web_fetch.rs` 留全部代码 854 行,内嵌测试 → `tools/tests_web_fetch.rs`。
- 测试:内嵌 517 行 → `tools/tests_web_fetch.rs`;被测私有 fn(`is_blocked`/`v4_in_cidr`/`v6_in_cidr`/`classify_reqwest_error`/`convert_body`/`pretty_json`/`html_to_markdown`/`html_to_text`/`truncate_output`/`resolve_public`/`resolve_and_check_sync`/`build_redirect_policy`/`execute_with`/`fetch_and_process`)升 `pub(crate)`;`Format` enum + impl 升 `pub(crate)`。
- **R1.2 关键**:`WebFetchError` 保持 `pub` 不变;`execute_for_test` 保持 `#[cfg(test)] pub`。
- 验证:`cargo test --lib "web_fetch"` + `cargo test --lib "tests_web_fetch"`;确认 `error.rs` 编译通过。

## 10. `db/sessions.rs`(1297 → 结构拆 #1) — ⑩

- 现状:6 簇,零内嵌测试(全在外部 `sessions_tests.rs` 1493 行)。所有项 `pub`(无 pub(crate))。`MessageLatency` 是唯一 pub struct。
- **结构拆必要性**:无内嵌测试可迁,1297 行必须切簇。自然 seam:session 表(cluster B+C+D,L17-734,末 fn `insert_system_event` 689-753)vs message 表(cluster E+F,L754-1297,首 fn `persist_turn`)。
- 目标(`db/sessions.rs` hub + `db/sessions/` 子目录):
  - `db/sessions.rs`(hub)— module 声明 + `pub use` 全量 re-export(保持 `pub use sessions::*` in `db/mod.rs` + 全路径 `crate::db::sessions::X`)
  - `db/sessions/session_crud.rs` — cluster B+C+D:`create_session`/`list_sessions`/`load_session`/`delete_session`/`delete_messages_by_session`/`touch_session`/`update_session_cwd` + 7 个单列 setter(`update_session_model_id`/`update_last_turn_usage`/`set_worktree_state`/`rename_session`/`set_session_color`/`set_session_workflow_enabled`/`set_session_plugin_name`)+ `insert_system_event`(~734 行)
  - `db/sessions/messages.rs` — cluster E+F:`persist_turn`/`MessageLatency`/`update_message_latency`/`find_message_id_by_seq`/`update_message_metadata`/`record_tool_duration`/`edit_user_message`(~563 行)
- **R1.5 跨簇**:`edit_user_message`(messages.rs)依赖 `find_message_id_by_seq`(同 messages.rs,无跨簇);session_crud 与 messages 无相互依赖。**无跨簇 helper 问题**(探查确认)。
- 测试:`sessions_tests.rs`(1493 行)**不动**,hub re-export 保证 `crate::db::sessions::X` 路径稳定。
- 验证:`cargo test --lib "db::sessions"` + `cargo test --lib "sessions_tests"`(141 load_session / 112 create_session 等高频引用必须保持)。

## 11. `agent/permissions/check.rs`(1365 → 结构拆 #2) — ⑪

- 现状:5 簇,零共享状态,零内嵌测试(全在外部 `tests_check.rs`)。`check` 函数 517 行单体(不切)。父 mod.rs re-export 5 符号(`check`/`recall_pitfall`/`recall_pitfall_with_hits`/`recall_pitfall_footnote`/`PitfallRecall`)。
- **结构拆必要性**:无内嵌测试,1365 行必须切。自然 seam:check 路径(cluster B+C,1-857)vs pitfall recall 路径(cluster D+E,859-1365)。
- 目标(`agent/permissions/check.rs` hub + `agent/permissions/check/` 子目录):
  - `agent/permissions/check.rs`(hub)— module 声明 + re-export(实测 `tests_check.rs:9-12` 从 `check::` 导入 **8 符号**,hub 必须全部可达):
    - `pub use` 5 pub 符号:`check`/`recall_pitfall`/`recall_pitfall_with_hits`/`recall_pitfall_footnote`/`PitfallRecall`(对照现 mod.rs re-export)
    - `pub(crate) use permission::{classify_tool, extract_path_arg, sqlite_glob_match, ToolKind, match_value_for_allow_always}` — 5 个 Tier4 helper(tests_check.rs + ask.rs 消费)
  - `agent/permissions/check/permission.rs` — cluster B+C:`check`(L53-570,518 行单体)+ Tier4 helpers(均已是 pub(crate),无需升可见性)
  - `agent/permissions/check/pitfall.rs` — cluster D+E(L924 起):`recall_pitfall_footnote`/`PitfallRecall`+impl/`PITFALL_SOFT_BLOCK_ENABLED`/`recall_pitfall`/`recall_pitfall_with_hits`/`recall_pitfall_inner`/`recall_pitfall_inner_with_rows`/`build_footnote_body`/`format_soft_block_hint`/`is_full_match`/`extract_probe_args`
- **R1.5 跨簇**:`pitfall.rs` 的 `recall_pitfall_footnote`/`recall_pitfall_inner_with_rows` 调 `extract_probe_args`(同 pitfall.rs);`extract_probe_args` 调 `classify_tool`/`extract_path_arg`(permission.rs)——**pitfall.rs 需 `use super::permission::{classify_tool, extract_path_arg}`(已是 pub(crate),无需升)**。这是唯一跨簇依赖。
- **`ask.rs:13`** `use super::check::match_value_for_allow_always`——hub `pub(crate) use permission::match_value_for_allow_always` 转发保持。
- 测试:`tests_check.rs` **不动**——它导入的 8 符号全经 hub re-export 可达。
- 验证:`cargo test --lib "permissions::check"` + `cargo test --lib "tests_check"`;确认 `ask.rs` 编译。

## 12. `db/migrations.rs`(1569 → 结构拆 #3) — ⑫ 最后(最高风险)

- 现状:7 簇,零共享状态。`run_migrations` **1011 行单体**(L81-1091,本批只搬不切)。6 个 `add_*_column_if_missing` `pub(crate)` helper(实际零外部代码调用方,仅文档引用)。内嵌测试 1454-1569(116 行,仅 init_pool pragma 测试)。
- **结构拆必要性**:测试迁出后 1453 行仍 >1200。`run_migrations` 单体(L81-1091,1011 行)只搬不切,但可把 pool init + 测试分出。
- **行数预算(P1-2 兜底,实测)**:`run_migrations` 1011 行 + 头部 ≈ 1035 行,**单独成 `schema.rs` 即 <1200 有余量**;若同夹 `migrate_provider_api_keys_to_encrypted`(75)+ `widen_subagent_runs_status_check`(106)+ `home_dir_or_dot`(11)= ~1185 行,贴 AC1 上限无余量。故后者单列 `schema_helpers.rs`。
- 目标(`db/migrations.rs` hub + `db/migrations/` 子目录):
  - `db/migrations.rs`(hub)— module 声明 + `pub use` re-export(`init_pool`/`run_migrations`)+ 6 个 `add_*_column_if_missing`(保持 pub(crate),因 mod.rs `pub use migrations::*`)。
  - `db/migrations/schema.rs` — **仅** `run_migrations`(1011 行单体,~1035 行含头部)
  - `db/migrations/schema_helpers.rs` — `migrate_provider_api_keys_to_encrypted` + `widen_subagent_runs_status_check_for_incomplete` + `home_dir_or_dot`(run_migrations 的私有 helper)
  - `db/migrations/columns.rs` — 6 个 `add_*_column_if_missing`(cluster D,pub(crate))
  - `db/migrations/pool.rs` — `init_pool`(cluster B)
- **R1.5 跨簇**:`run_migrations`(schema.rs)调 6 个 `add_*_column_if_missing`(columns.rs)+ `migrate_provider_api_keys_to_encrypted`/`home_dir_or_dot`(schema_helpers.rs)+ `super::config::seed_default_providers_and_models`——schema.rs 需 `use super::columns::*; use super::schema_helpers::*;`。
- 测试:内嵌 116 行 → `db/migrations_tests.rs`(**P2-1:db/ 目录沿用 `*_tests.rs` 后缀惯例**,对照 memories_tests/sessions_tests 等 8 个既有文件;`db/mod.rs` 声明);被测 `init_pool` 已 pub。
- **`super::config` 路径**:`run_migrations` 搬到 `migrations/schema.rs` 后,`super::config` 仍指 `db::config`(因 schema.rs 的 super 是 migrations,migrations 的 super 是 db)——需改为 `crate::db::config::seed_default_providers_and_models` 或 `super::super::config::`。**实现时编译验证**。
- 验证:`cargo test --lib "db::migrations"` + `cargo test --lib "migrations_tests"`;`state.rs:280` 的 `crate::db::run_migrations` 调用保持。

## 13. 文档引用同步(R2)

- 每次代码拆分落地后 sweep:`grep -rn "<旧路径/被搬符号>" docs/ .trellis/spec/ AGENTS.md`(排除 archive)。
- 已识别 ~20 个引用文档(探查):`docs/ROADMAP.md`、`docs/TECH.md`、`docs/DESIGN.md`、`docs/CONTEXT.md`、`docs/A2-SHELL-CLASSIFICATION.md`、`docs/INTERLEAVED-THINKING-DESIGN.md`、`docs/WORKFLOW-INTEGRATION-REVIEW*.md`、`.trellis/spec/backend/{workflow-plugin-builtin,daemon-server,subagent-runs-schema,latency-tracking}.md`、`.trellis/spec/backend/tool-contract/{01,02,04,05,06,07}-*.md`、`.trellis/spec/frontend/popover-pattern.md`、`docs/_reviews/FINDINGS-b5-cache-wire-validation.md`。
- 关注点:行号引用改符号引用(行号必然漂移);路径引用若仅指模块(如 `tools/shell.rs`)则不变(模块名保持)。参照批 1 AC3 教训。
- 每 3-4 个文件拆分后做一轮文档 sweep(避免最后堆积);最终全量 grep 复验。

## 14. 兼容性与风险

- **零运行时变化**:全部编译期搬迁;module 路径经 re-export 保持。
- **风险点**:
  - `llm/mod.rs` 7 符号 re-export(§1)——遗漏任一符号,~10 个 Pattern-2 调用方编译失败。缓解:hub `pub use` 显式列全,`cargo check` 全量验证。
  - `merge_worker.rs` static LOCKS(§6)——`merge_lock_for` 与合并体必须同文件,否则锁失效。缓解:同落 `merge.rs`。
  - `migrations.rs` `super::config` 路径(§12)——搬到 schema.rs 后 super 层级变。缓解:改全路径 `crate::db::config::`。
  - `check.rs` pitfall→permission 跨簇(§11)——`extract_probe_args` 调 `classify_tool`/`extract_path_arg`。缓解:升 `pub(crate)` + `use super::permission::`。
  - 3 处结构拆的 hub re-export 完整性——漏 re-export 任一 pub 符号则外部 tests_*.rs 编译失败。缓解:hub 用 `pub use submodule::*` glob 或显式列全。
- **不涉及**:前端、单体函数重构、A 类 4 文件(已 Out of Scope)。
- **顺序**:纯测试搬迁 9 文件热身(types→task→inject→shell_trust→subagent→merge_worker→request_task→shell→web_fetch)→ 3 结构拆(sessions→check→migrations,风险递增)。
