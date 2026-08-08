# Implement — 大文件拆分批 3:12 处 B 类纯搬迁

> 执行计划。每步独立 commit、独立回滚。验证命令:后端
> `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib "<filter>"`
> (不要 `--test-threads=1`)。

## Phase 0:前置

- [ ] 0.1 建分支 `refactor/file-splitting-batch3`,工作树干净
- [ ] 0.2 base 是 main 最新(`git log --oneline -1 main` 应为 `2eb901a` archive 或更新)
- [ ] 0.3 跑一次基线 `cargo test --lib` 确认全绿,记录末尾 `test result: ok. N passed` 的 **N 值到 task notes**(终验对照 N 不变,比"全绿"更硬;延续批 1 建议)

## Phase 1:llm/types.rs 拆分(① 最先,验证 re-export 模式)

- [ ] 1.1 建 `llm/types/` 目录,按 design §1 切出 `message.rs`(Role/CacheControl/ContentBlock/is_false/MessageContent+impl+2 Serde)/ `chat.rs`(ChatMessage/ToolDef+impl)/ `request.rs`(ThinkingConfig/ChatRequest)/ `usage.rs`(TokenUsage)/ `event.rs`(ChatEvent/RecallHit/LlmErrorCategory)
- [ ] 1.2 `llm/types.rs` 改为 hub:`mod` 声明 + `pub use` 全量 re-export(**R1.3**:必须含 ChatEvent/ChatMessage/ContentBlock/LlmErrorCategory/MessageContent/Role/ToolDef 7 符号,保持 `llm/mod.rs` 的 `pub use types::{...}` 零改动)
- [ ] 1.3 被测私有 fn 升 `pub(crate)`
- [ ] 1.4 内嵌测试(651 行)→ `llm/tests_types.rs`(`llm/mod.rs` 加 `#[cfg(test)] mod tests_types;`),文件级 `#![cfg(test)]`,import 改 `use crate::llm::types::*`
- [ ] 1.5 验证:`cargo test --lib "llm::types"` + `cargo test --lib "tests_types"` 全绿
- [ ] 1.6 **R1.3 专项**:`cargo check` 确认 ~10 个 Pattern-2 调用方(state.rs/context.rs/helpers.rs/chat.rs/auto_reflect.rs/provider.rs/subagent/mod.rs 等 `use crate::llm::{...}`)零改动
- [ ] 1.7 行数核对:`wc -l llm/types.rs llm/types/*.rs` — hub <1200,各子模块 <1200
- [ ] 1.8 commit:`refactor(llm): types 拆 message/chat/request/usage/event + 测试迁出`

**回滚点**:`git revert <1.8>`。

## Phase 2:agent/workflow/task.rs 拆分(②)

- [ ] 2.1 建 `agent/workflow/task/` 目录,按 design §2 切出 `types.rs`(TaskStatus+3impl/TaskItem/TaskJson/2serde helper/TaskError/TaskResult)/ `paths.rs`(validate_slug/task_dir/task_json_path/task_prd_path/PROJ_NS_TASKS_DIR/MAX_SLUG_LEN)/ `io.rs`(read_task/write_task/create_task_init)/ `archive.rs`(PROJ_NS_TASKS_ARCHIVE_DIR/archive_task_init/git_add_path/git_commit)
- [ ] 2.2 `agent/workflow/task.rs` 改为 hub:`mod` 声明 + `pub use` re-export 13 符号(对照现 mod.rs)
- [ ] 2.3 被测私有 fn(`git_add_path`/`git_commit`/`default_workflow_plugin`/`is_default_plugin`)升 `pub(crate)`
- [ ] 2.4 内嵌测试(733 行)→ `agent/workflow/tests_task.rs`(`agent/workflow/mod.rs` 声明)
- [ ] 2.5 验证:`cargo test --lib "workflow::task"` + `cargo test --lib "tests_task"` 全绿
- [ ] 2.6 确认 `commands/question.rs:53`、`tools/request_task_state_transition.rs:91` 的 `::task::` 直路径零改动
- [ ] 2.7 行数核对;commit:`refactor(workflow): task 拆 types/paths/io/archive + 测试迁出`

**回滚点**:`git revert <2.7>`。

## Phase 3:agent/workflow/inject.rs 拆分(③ 最小方案)

- [ ] 3.1 内嵌测试(514 行)→ `agent/workflow/tests_inject.rs`(`agent/workflow/mod.rs` 声明)
- [ ] 3.2 `inject.rs` 留全部代码(708 行 <1200,最小方案不结构拆);被测私有 fn(`build_breadcrumb_block`/`resolve_relevant_specs`)升 `pub(crate)`
- [ ] 3.3 验证:`cargo test --lib "workflow::inject"` + `cargo test --lib "tests_inject"` 全绿
- [ ] 3.4 确认 3 种访问路径(flat/`::inject::`/re-export)零改动
- [ ] 3.5 行数核对:inject.rs <1200;commit:`refactor(workflow): inject 测试迁出`

**回滚点**:`git revert <3.5>`。

## Phase 4:agent/permissions/shell_trust.rs 拆分(④ 纯测试搬迁)

- [ ] 4.1 内嵌测试(668 行)→ `agent/permissions/tests_shell_trust.rs`(`agent/permissions/mod.rs` 声明)
- [ ] 4.2 `shell_trust.rs` 留全部代码(790 行 <1200);被测私有 fn(`first_token`/`split_top_level`/`has_command_substitution`/`detect_write_redirect`/`classify_single`/`classify_git_subcommand`/`first_token`/`max_of` 中的私有项)升 `pub(crate)`
- [ ] 4.3 验证:`cargo test --lib "shell_trust"` + `cargo test --lib "tests_shell_trust"` 全绿
- [ ] 4.4 确认 `check.rs` 的 `super::shell_trust::` 路径零改动
- [ ] 4.5 commit:`refactor(permissions): shell_trust 测试迁出`

**回滚点**:`git revert <4.5>`。

## Phase 5:agent/subagent/mod.rs 拆分(⑤ 已是 hub,搬内联簇)

- [ ] 5.1 按 design §5 切出到子文件:`definition.rs`(definition/definition_with_cache)/ `registry.rs`(SubagentDef/builtin_subagents 含 OnceLock/lookup_subagent)/ `prompt.rs`(assemble_subagent_prompt/build_worker_messages)/ `tools_filter.rs`(STRUCTURALLY_DISABLED/filter_tools_for_subagent/READONLY_TOOL_ALLOWLIST/filter_tools_readonly)
- [ ] 5.2 `subagent/mod.rs` 留 module 声明 + re-export + 小簇(ForcedDispatch/ModelBrief/SubagentStatus+impl/DISPATCH_TOOL_NAME);hub `pub use` re-export 搬出的 pub 符号。**P2-2**:`mod.rs:104` 的 `#[cfg(test)] pub(crate) use event_sink::{arm_test_collector, clear_test_collector};`(消费者 sink.rs cfg(test))保留在 hub 不动,与内嵌测试无关
- [ ] 5.3 内嵌测试(461 行)→ `subagent/tests_mod.rs`(`subagent/mod.rs` 声明,与 tests_dispatch/tests_loader 同级)
- [ ] 5.4 验证:`cargo test --lib "subagent::tests_mod"` + `cargo test --lib "tests_dispatch"` 全绿
- [ ] 5.5 确认直路径调用方(DISPATCH_TOOL_NAME/definition_with_cache/ForcedDispatch/ModelBrief/SubagentDef/builtin_subagents)经 hub re-export 零改动
- [ ] 5.6 commit:`refactor(subagent): mod 拆 definition/registry/prompt/tools_filter + 测试迁出`

**回滚点**:`git revert <5.6>`。

## Phase 6:tools/merge_worker.rs 拆分(⑥ 含 static LOCKS)

- [ ] 6.1 建 `tools/merge_worker/` 目录,按 design §6 切出 `execute.rs`(definition/execute)/ `merge.rs`(**merge_lock_for 含 static LOCKS** + do_merge_blocking/merge_session_into_main/is_ancestor/collect_conflict_paths)/ `finalize.rs`(ensure_parent_worktree_attached/finalize_merge)
- [ ] 6.2 `tools/merge_worker.rs` 改为 hub:`mod` 声明 + `pub use` re-export 6 pub 符号
- [ ] 6.3 **R1.5**:`execute.rs` 调 `do_merge_blocking`/`finalize_merge`/`ensure_parent_worktree_attached`——经 hub re-export 或 `use super::{...}`
- [ ] 6.4 内嵌测试(337 行)→ `tools/tests_merge_worker.rs`(`tools/mod.rs` 声明)
- [ ] 6.5 验证:`cargo test --lib "merge_worker"` + `cargo test --lib "tests_merge_worker"` 全绿
- [ ] 6.6 确认 `commands/subagent_runs.rs`/`commands/worktree.rs` 调用方零改动
- [ ] 6.7 commit:`refactor(tools): merge_worker 拆 execute/merge/finalize + 测试迁出`

**回滚点**:`git revert <6.7>`。

## Phase 7:tools/request_task_state_transition.rs 拆分(⑦ 纯测试搬迁)

- [ ] 7.1 内嵌测试(771 行)→ `tools/tests_request_task_state_transition.rs`
- [ ] 7.2 留全部代码(735 行 <1200)
- [ ] 7.3 验证:`cargo test --lib "request_task_state_transition"` + `cargo test --lib "tests_request_task_state_transition"`
- [ ] 7.4 commit:`refactor(tools): request_task_state_transition 测试迁出`

**回滚点**:`git revert <7.4>`。

## Phase 8:tools/shell.rs 拆分(⑧ 纯测试搬迁)

- [ ] 8.1 内嵌测试(885 行)→ `tools/tests_shell.rs`
- [ ] 8.2 留全部代码(587 行 <1200);被测私有 fn(kill_and_collect/format_output/spill_to_disk/head_tail_preview/truncate_output)升 `pub(crate)`
- [ ] 8.3 **R1.2**:`apply_safe_env` 保持 `pub(crate)` 不变
- [ ] 8.4 验证:`cargo test --lib "tools::shell"` + `cargo test --lib "tests_shell"`;确认 `background_shell::in_memory` 编译
- [ ] 8.5 commit:`refactor(tools): shell 测试迁出`

**回滚点**:`git revert <8.5>`。

## Phase 9:tools/web_fetch.rs 拆分(⑨ 纯测试搬迁)

- [ ] 9.1 内嵌测试(517 行)→ `tools/tests_web_fetch.rs`
- [ ] 9.2 留全部代码(854 行 <1200);被测私有 fn 升 `pub(crate)`;Format enum+impl 升 `pub(crate)`
- [ ] 9.3 **R1.2**:`WebFetchError` 保持 `pub`;`execute_for_test` 保持 `#[cfg(test)] pub`
- [ ] 9.4 验证:`cargo test --lib "web_fetch"` + `cargo test --lib "tests_web_fetch"`;确认 `error.rs` 编译
- [ ] 9.5 commit:`refactor(tools): web_fetch 测试迁出`

**回滚点**:`git revert <9.5>`。

## Phase 10:db/sessions.rs 拆分(⑩ 结构拆 #1)

- [ ] 10.1 建 `db/sessions/` 目录,按 design §10 切出 `session_crud.rs`(cluster B+C+D:create/list/load/delete/delete_messages/touch/update_cwd + 7 setter + insert_system_event)/ `messages.rs`(cluster E+F:persist_turn/MessageLatency/update_message_latency/find_message_id_by_seq/update_message_metadata/record_tool_duration/edit_user_message)
- [ ] 10.2 `db/sessions.rs` 改为 hub:`mod` 声明 + `pub use {session_crud::*, messages::*}` 全量 re-export(保持 `db/mod.rs` `pub use sessions::*` + 全路径 `crate::db::sessions::X`)
- [ ] 10.3 验证 `sessions_tests.rs`(1493 行)**不动**:hub re-export 保证路径
- [ ] 10.4 验证:`cargo test --lib "db::sessions"` + `cargo test --lib "sessions_tests"` 全绿
- [ ] 10.5 确认高频调用方(load_session 141/create_session 112/persist_turn 82 等)零改动
- [ ] 10.6 行数核对:hub <1200,session_crud <1200,messages <1200
- [ ] 10.7 commit:`refactor(db): sessions 拆 session_crud/messages`

**回滚点**:`git revert <10.7>`。

## Phase 11:agent/permissions/check.rs 拆分(⑪ 结构拆 #2)

- [ ] 11.1 建 `agent/permissions/check/` 目录,按 design §11 切出 `permission.rs`(cluster B+C:check 518 行单体 L53-570 + Tier4 helpers)/ `pitfall.rs`(cluster D+E,L924 起:recall_pitfall* 系列 + PitfallRecall + helpers)
- [ ] 11.2 `agent/permissions/check.rs` 改为 hub:`mod` 声明 + re-export。**P1-1(实测 tests_check.rs:9-12 导入 8 符号)**:`pub use` 5 pub 符号(check/recall_pitfall/recall_pitfall_with_hits/recall_pitfall_footnote/PitfallRecall)+ `pub(crate) use permission::{classify_tool, extract_path_arg, sqlite_glob_match, ToolKind, match_value_for_allow_always}`(Tier4 helper,tests_check.rs + ask.rs 消费,均已是 pub(crate) 无需升)
- [ ] 11.3 **R1.5**:`pitfall.rs` 的 `extract_probe_args` 调 `classify_tool`/`extract_path_arg`(permission.rs)——已是 pub(crate),加 `use super::permission::{classify_tool, extract_path_arg}`
- [ ] 11.4 验证 `tests_check.rs` **不动**:hub re-export 保证路径
- [ ] 11.5 验证:`cargo test --lib "permissions::check"` + `cargo test --lib "tests_check"` 全绿
- [ ] 11.6 确认 `ask.rs:13` 的 `super::check::match_value_for_allow_always` 编译
- [ ] 11.7 行数核对;commit:`refactor(permissions): check 拆 permission/pitfall`

**回滚点**:`git revert <11.7>`。

## Phase 12:db/migrations.rs 拆分(⑫ 结构拆 #3,最高风险,最后)

- [ ] 12.1 建 `db/migrations/` 目录,按 design §12 切出 `schema.rs`(**仅 run_migrations 单体 L81-1091,~1035 行,P1-2 兜底**)/ `schema_helpers.rs`(`migrate_provider_api_keys_to_encrypted`/`widen_subagent_runs_status_check_for_incomplete`/`home_dir_or_dot`)/ `columns.rs`(6 个 add_*_column_if_missing)/ `pool.rs`(init_pool)
- [ ] 12.2 `db/migrations.rs` 改为 hub:`mod` 声明 + `pub use` re-export(init_pool/run_migrations)+ `pub(crate) use columns::*`(保持 mod.rs `pub use migrations::*`)
- [ ] 12.3 **R1.5**:`schema.rs` 的 `run_migrations` 调 columns 的 6 helper + schema_helpers 的 3 helper + `crate::db::config::seed_default_providers_and_models`(原 `super::config`,层级变后改全路径)——`use super::columns::*; use super::schema_helpers::*;` 编译验证
- [ ] 12.4 内嵌测试(116 行)→ `db/migrations_tests.rs`(**P2-1:db/ 目录沿用 `*_tests.rs` 后缀**,对照 memories_tests 等 8 个既有文件;`db/mod.rs` 声明)
- [ ] 12.5 验证:`cargo test --lib "db::migrations"` + `cargo test --lib "migrations_tests"` 全绿(注:`sessions_tests.rs:24` 也导入 `migrations::run_migrations`,本步一并覆盖)
- [ ] 12.6 确认 `state.rs:277/280` 的 `crate::db::init_pool`/`crate::db::run_migrations` 调用零改动
- [ ] 12.7 **P1-2 行数预算核对(开工前先算)**:hub <1200 / schema ~1035(<1200 有余量) / schema_helpers ~190 / columns ~250 / pool ~60。若 schema.rs 实际超 1200 则回退把 `home_dir_or_dot` 留 hub
- [ ] 12.8 commit:`refactor(db): migrations 拆 schema/schema_helpers/columns/pool + 测试迁出`

**回滚点**:`git revert <12.8>`。

## Phase 13:文档引用同步(R2)+ 终验

- [ ] 13.1 每 3-4 个文件拆分后做一轮 sweep(Phase 3/6/9/12 后):`grep -rn "<旧路径/被搬符号>" docs/ .trellis/spec/ AGENTS.md`(排除 archive),更新失效路径/行号 → 符号引用
- [ ] 13.2 最终全量 sweep 复验:确认无活跃文档引用旧路径/旧行号
- [ ] 13.3 全量终验:`PKG_CONFIG_PATH="..." cargo test --lib`(全绿,~1657)
- [ ] 13.4 `cargo clippy --lib`(零警告)+ `cargo fmt --check`(零差异)
- [ ] 13.5 AC 逐项核对(AC1-6)
- [ ] 13.6 commit(若有文档改动):`docs: 同步批3 拆分后引用`
- [ ] 13.7 squash merge 回 main(确认分支名 `refactor/file-splitting-batch3`)
- [ ] 13.8 `task.py archive 08-08-batch3-large-file-splitting`
- [ ] 13.9 复验 `cargo test --lib` 全绿

## 验证命令速查

```bash
# 单模块过滤测试(避免全量)
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib "llm::types"

# 全量终验
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib

# clippy + fmt
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo clippy --lib && cargo fmt --check

# 行数核对(示例)
wc -l app/src-tauri/src/llm/types.rs app/src-tauri/src/llm/types/*.rs
wc -l app/src-tauri/src/db/sessions.rs app/src-tauri/src/db/sessions/*.rs
```

## 新增测试声明提示(P3-2)

以下父 mod.rs **无既有 `#[cfg(test)] mod tests_*;` 先例**,本批迁出测试时为新增声明(非沿用):
- `tools/mod.rs` — 4 个新增(`tests_request_task_state_transition`/`tests_shell`/`tests_web_fetch`/`tests_merge_worker`);L69 是 `test_default_pool` 辅助函数非测试模块
- `llm/mod.rs` — 1 个新增(`tests_types`);provider 测试声明在 `provider/mod.rs` 不在此

以下父 mod.rs **有先例**,沿用即可:
- `agent/permissions/mod.rs`(tests_check/tests_ask 等已存在)
- `agent/workflow/mod.rs`、`agent/subagent/mod.rs`(tests_dispatch/tests_loader 已存在)
- `db/mod.rs`(8 个 `*_tests.rs` 已存在,本批加 `migrations_tests.rs` 沿用 `*_tests.rs` 后缀)

## 风险热图

| Phase | 文件 | 风险 | 关键约束 |
|---|---|:---:|---|
| 1 | llm/types | 中 | 7 符号 re-export,~10 Pattern-2 调用方 |
| 2 | workflow/task | 低 | 零共享状态,13 符号 re-export |
| 3 | workflow/inject | 低 | 最小方案只迁测试 |
| 4 | shell_trust | 低 | 纯测试搬迁 |
| 5 | subagent/mod | 中 | 已是 hub,搬内联簇 + 4 直路径 |
| 6 | merge_worker | **高** | `static LOCKS`,锁与合并体同文件 |
| 7 | request_task | 低 | 纯测试搬迁 |
| 8 | shell | 低 | 纯测试搬迁,apply_safe_env pub(crate) |
| 9 | web_fetch | 低 | 纯测试搬迁,WebFetchError pub |
| 10 | db/sessions | 中 | 结构拆 #1,无跨簇依赖 |
| 11 | permissions/check | **高** | 结构拆 #2,pitfall→permission 跨簇 |
| 12 | db/migrations | **高** | 结构拆 #3,super::config 路径 + 1011 行单体 |
