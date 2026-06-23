# 拆分 db/tests.rs — 按 SQL 域拆 6 个测试文件

## Goal

`db/tests.rs` 3242 行 95 个集成测试,扁平 `#![cfg(test)]` 结构(单一 use 块 17-44),按 SQL 域混合。按域拆成 6 个测试文件,降低单文件阅读负担,测试可并行编译,git blame 噪音减少。零行为变更(纯物理搬运 + 必要 use 收敛)。

## What I already know

* `tests.rs` 结构(`#![cfg(test)]` 文件级 gate,行 7):
  - **use 块**(17-44):`sqlx::{Row, SqlitePool}` / `uuid::Uuid` / `crate::llm::types::{ContentBlock, MessageContent, Role}` / `projects::DEFAULT_PROJECT_ID` / `super::{config::{get_config_value, seed_default_providers_and_models, set_config_value}, migrations::run_migrations, models::{create_model, delete_model, list_models, update_model}, projects::{create_project, get_project, hide_project, list_hidden_projects, list_projects, list_projects_with_stale_git_probe, unhide_project, update_project_git_metadata, update_project_name, update_project_path}, providers::{create_provider, delete_provider, list_providers, update_provider}, sessions::{add_token_usage, create_session, delete_messages_by_session, delete_session, edit_user_message, find_message_id_by_seq, insert_system_event, list_sessions, load_session, persist_turn, record_tool_duration, set_worktree_state, touch_session, update_message_latency, update_session_cwd, update_session_model_id, MessageLatency}, permissions::{grant_tool_permission, has_tool_permission, list_audit_events, record_audit_event, update_session_mode}, subagent_runs::{add_token_usage_streaming, get_run, insert_run, list_runs_by_session, list_runs_summary_by_session, update_run_finished, SubagentStatusDb}, types::WorktreeState}`
  - **helpers**:`test_pool()`(行 48-57)所有 95 测试共享,`make_pool()`(行 645-647)仅 providers 域用 → common.rs
* **section 划分**(95 测试,按行号 + section marker):

| 域 | 行范围 | 测试数 | 内容 |
|---|---|---|---|
| projects | 1-281 | 10 | migrations(2)+ project CRUD(8,含 git_branch/reprobe) |
| sessions | 282-644 | 19 | session CRUD(13) + worktree state(3) + system event(2) + insert_system_event_seq |
| providers | 645-911 | 14 | provider CRUD(3) + model CRUD(3) + config seed(4) + set/get_config + delete_cascade_does_not_touch_unrelated_models(实际跨 912) |
| sessions(续) | 912-1551 | 21 | model_id(3) + A4 token(4) + F5 latency(8) + persist_turn 系列(6) |
| permissions | 1552-1940 | 13 | A2+B7(13):permission grant/cascade + audit + mode(3) + audit round-trip(4) + audit wire-shape + mode_backfill |
| messages | 1941-2365 | 10 | D3 edit_user_message(8) + resend_message_audit(2) |
| subagent_runs | 2366-3242 | 18 | B6 subagent_runs PR2(11) + B6 redesign PR1(7) |

**注**:sessions 域物理上跨 2 段(282-644 + 912-1551),messages 域独立成段(1941-2365);sessions_tests.rs 拼接 2 段,messages_tests.rs 装 D3+resend 完整一段。

## Requirements

### 必含(6 个新文件,无 common 文件)

* **projects_tests.rs** — 10 测试(项目 + migrations),行 1-281
* **sessions_tests.rs** — 40 测试(session CRUD + worktree + system event + model_id + token + latency),物理上拼 2 段(行 282-644 + 912-1551)
* **providers_tests.rs** — 15 测试(provider + model + config seed + 1 delete_provider_cascade),行 645-940
* **permissions_tests.rs** — 13 测试(grant + audit + mode + audit round-trip),行 1552-1940
* **messages_tests.rs** — 10 测试(D3 edit_user_message + resend_message_audit),行 1941-2365
* **subagent_runs_tests.rs** — 18 测试(B6 + B6 redesign),行 2366-3242

### 不含 common 文件

**与 agent/tests.rs 拆分不同点**:db/tests.rs 只有 1 个 `test_pool()` helper + 1 个 `make_pool()` helper(providers 专用),且 `test_pool` 跨全部 6 文件用。两种处理:
1. 每个测试文件独立 copy 一份 `test_pool`(零依赖,简单粗暴)
2. 建 `tests_common.rs` 共享 `test_pool`(`pub(crate)`)

**MVP 决定**:选 1 — 每个文件独立 copy `test_pool`(共 6 份,~10 行重复),理由:(a) helper 只 8 行,copy 维护成本低于引一个 `pub(crate)` 模块 + mod.rs 额外声明;(b) 与 agent/tests.rs 拆分对齐(那边是 5+1 大 helper 才值得 common);(c) DB 测试迁移逻辑相对稳定,改了能 6 处一起改。

### 不变

* 95 个测试的 body 零改动(纯搬运)
* `test_pool()` helper 逻辑零改动(只是 6 份复制)
* 测试函数名 / `#[tokio::test]` 属性不变

### 关键改动

* **visibility**:无 — 跨文件不需要共享 helper,use 路径仍走 `super::super::{config, migrations, ...}`(因为新文件平铺在 `db/` 下,不是子目录)
* **mod.rs**:`pub mod tests;`(行 66)→ 加 6 个 `pub mod tests_<name>;`(跟随 agent/tests.rs 拆分惯例,不加 `#[cfg(test)]`,依赖文件级 `#![cfg(test)]`)
* **use 收敛**:每个新文件只 use 自己用到的(`test_pool` 引用 `SqlitePool` + `run_migrations`;其它按域收敛)
* **物理拼接**:sessions_tests.rs 内容顺序拼接 2 段(282-644 + 912-1551);messages_tests.rs 装完整一段(1941-2365);提取时用 `sed -n 'start,end p'` 按段提取

## Acceptance Criteria

* [ ] 6 个新文件存在,各自 `#![cfg(test)]` 文件头
* [ ] tests.rs 删除
* [ ] mod.rs 声明 6 个 `pub mod tests_<name>;`(无 `pub mod tests;`)
* [ ] 95 个测试全部保留(cargo test --lib 测试数不变)
* [ ] `PKG_CONFIG_PATH=... cargo test --lib` 全绿(测试数与拆分前一致,0 failed)
* [ ] 0 warning(无 unused import)
* [ ] sessions_tests.rs ~1400 行 / subagent_runs_tests.rs ~900 行 / messages_tests.rs ~430 行 / permissions_tests.rs ~500 行 / providers_tests.rs ~400 行 / projects_tests.rs ~300 行

## Definition of Done

* 测试按 SQL 域物理隔离,零行为变化
* `cargo test --lib` 全绿,0 warning
* commit message: `refactor(db): split tests.rs into 6 SQL-domain files`

## Technical Approach

* **零行为变更**:纯文件物理搬运 + use 路径调整(`super::*` 仍可,因为新文件在 `db/` 同级;不需要 `super::super::*` —— 实际需要确认)
* **use 收敛策略**:每个文件 use 自己用到的子集(避免 unused import 报错)
* **提取命令**:`sed -n '282,644p' tests.rs > sessions_part1.txt` 等,各文件 prepend `#![cfg(test)]` + 收敛 use 块
* **test_pool 复制**:6 份相同实现(8 行),无 `pub(crate)` 修饰

## Out of Scope

* 改任何测试 body / 断言 / 逻辑
* 拆 `db/` 主模块(那是另一任务,8-PR2 已拆过)
* 建 common.rs helper 文件
* 改 test_pool 实现
* 重新设计 test 命名/分组

## Decision (ADR-lite)

**Context**:db/tests.rs 3242 行 95 测试混合 7 个 SQL 域。原 task.json 拆 5 文件,sessions_tests 拼 3 段(~2000 行/50 测试)仍偏大。

**Decision**(2026-06-23 用户确认):
- ✅ **6 文件方案** — D3 edit_user_message + resend 独立成 messages_tests.rs(纯 messages 域,audit 副作用忽略);sessions_tests.rs 降至 2 段拼接(~1400 行/40 测试),文件体量最平衡。
- ✅ **无 common 文件** — 6 份复制 test_pool(8 行/份),零额外 `pub(crate)` 修饰,降低封装放宽面。
- ✅ **D3 edit_user_message 归 messages** — 它写 `messages` 表 + 附带 audit row,主表是 messages;audit 测试场景由 permissions_tests.rs 覆盖。

**Consequences**:
- ✅ 6 文件均 < 1500 行,最大 sessions_tests.rs ~1400 行
- ✅ messages 域物理隔离,后续 D3 follow-up 易扩展
- ⚠️ test_pool 复制 6 份(48 行总),改 helper 需 6 处同步(但 helper 极稳定)

## Open Questions

*(已全部解决)*

## Technical Notes

* 当前文件:`app/src-tauri/src/db/tests.rs`(3242 行,`#![cfg(test)]` 扁平)
* 编译/测试:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`
* 参考先例:`.trellis/tasks/archive/2026-06/06-23-06-23-split-agent-tests/prd.md`(agent/tests.rs 拆分样板)
* 参考实现:`app/src-tauri/src/agent/{tests_cancellation,tests_envelope,tests_prompts,tests_agent_loop,tests_subagent,tests_common}.rs` 6 文件
* Trellis 优先级:P2

## Research References

* [`agent/tests.rs 拆分 PRD`](../../archive/2026-06/06-23-06-23-split-agent-tests/prd.md) — 平铺文件 `tests_<name>.rs` + 文件级 `#![cfg(test)]` 模式样板
* [`agent/mod.rs`](../../../app/src-tauri/src/agent/mod.rs) — 6 个 `pub mod tests_*;` 声明参考
* [`db/mod.rs`](../../../app/src-tauri/src/db/mod.rs) — 当前 `pub mod tests;` 单一声明,需替换为 6 个 `pub mod tests_<name>;`