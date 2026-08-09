# Design: sessions_tests.rs 目录化拆分

> 本文是技术设计。需求与验收见 `prd.md`。执行清单见 `implement.md`。
> 沿用同构先例 `.trellis/tasks/archive/2026-08/08-09-tests-memories-split/`(已落地)。

## 0. PRD 订正(实测后)

| 项 | PRD 原值 | 实测 | 处置 |
|---|---|---|---|
| 测试函数数 | "33 个" | **35 个** | 以实测为准;`task.json` 描述本就是"35",PRD 表错。终验 N=35 |
| helper 数 | 2 个(`test_pool`/`make_pool`) | **2 个**(无 cluster-private helper) | R3 一致,无需补 |
| 簇数 | PRD 表 5 簇 | **5 簇**(见 §2 权威表) | 一致 |
| 基线测数 | 1662 全绿 | ✅ 实测 1662 passed(35 + 1627 filtered) | 一致 |
| 头注释 | "33 个测试函数 + 2 helper" | 实测 35 测 + 2 helper | 文件头注释 L3–15 是描述性的,不随拆改 |

> 同类订正在 memories-split(PRD"40"实测 49)、agent-loop-split(PRD"32"实测 41)都出现过,本任务同构。
> PRD §Background 表中"33 个测试函数"是笔误,以本 design §0 / §2 为权威。

## 1. 形态:目录化(R1 首选方案)

```
db/
├── mod.rs                         # L72 `pub mod sessions_tests;` 零改动(Rust 自动解析到目录)
└── sessions_tests/                # 新目录(hub + 5 簇文件)
    ├── mod.rs                     # hub:#![cfg(test)] + 5 mod 声明 + test_pool/make_pool + 6 兄弟 re-export
    ├── session_crud.rs            # session CRUD(scopes/标题/级联删除/list 截断):8 测
    ├── fields_worktree.rs         # 字段/worktree 状态(touch/cwd/workflow/plugin/worktree):10 测
    ├── system_events.rs           # system event 追加 + seq 自增:2 测
    ├── model_usage.rs             # model_id / token usage / list 列:5 测
    └── latency_message.rs         # latency / message 定位 / tool duration:10 测
```

`sessions_tests.rs`(1493 行)整体删除,内容按簇平移到 5 个簇文件。`db/mod.rs:72`
`pub mod sessions_tests;` 一字不改 —— Rust 2018 module 路径解析器对 `sessions_tests/`
目录与 `sessions_tests.rs` 文件等价对待,调用方语义不变。

### 为什么不用平级多文件(备选)

平级方案要把 `db/mod.rs:72` 的 `pub mod sessions_tests;` 改成 6 行声明,违反 R1
"声明零改动"且改动调用方模块路径。目录化零改动更优。**与 `tests_agent_loop` /
`memories_tests` 先例一致**。

## 2. 簇映射(5 文件 / 35 测 + 2 helper)—— 权威表

> 行号是 `sessions_tests.rs` 的实测起止(含 `#[tokio::test]` 行)。
> **本文件测试在源中按簇连续排列,无交错**(同 memories_tests 先例),
> 故每簇可用连续区间 `sed` 提取,无需逐测试函数抽取。

| 文件 | 测数 | 源行号(含头) | 行数 | 簇内测试 |
|---|---|---|---|---|
| `session_crud.rs` | 8 | L52–345 | ~294 | create_session_scopes_to_project / load_session_returns_none_for_missing / persist_and_load_messages / first_user_message_auto_titles_session / second_user_message_does_not_overwrite_title / delete_session_cascades_messages / delete_messages_by_session_keeps_session_drops_messages / list_sessions_preview_truncates_at_80_chars |
| `fields_worktree.rs` | 10 | L346–647 | ~302 | touch_session_updates_timestamp / update_session_cwd_persists / set_session_workflow_enabled_round_trip / set_session_workflow_enabled_on_missing_session_is_noop / set_session_plugin_name_round_trip / set_session_plugin_name_on_missing_session_is_noop / new_session_defaults_to_none_state / worktree_state_setter_round_trip / worktree_state_unknown_string_defaults_to_none / worktree_state_backfill_legacy_active |
| `system_events.rs` | 2 | L648–726 | ~79 | insert_system_event_appends_to_history / insert_system_event_seq_increments |
| `model_usage.rs` | 5 | L737–937 | ~201 | update_session_model_id_sets_and_clears / update_session_model_id_on_missing_session_is_noop / load_session_includes_model_id / update_last_turn_usage_overwrites_not_accumulates / list_sessions_includes_last_turn_columns |
| `latency_message.rs` | 10 | L943–1493 | ~551 | persist_turn_with_latency_writes_three_columns / persist_turn_with_per_turn_latency_writes_4_columns_for_each_turn / persist_turn_with_no_latency_leaves_columns_null / update_message_latency_patches_columns_by_id / update_message_latency_accepts_partial_payload / update_message_latency_patches_thinking_ms_independently / find_message_id_by_seq_returns_none_for_unknown_pair / record_tool_duration_patches_matching_tool_result_block / record_tool_duration_returns_false_when_no_block_matches / record_tool_duration_handles_text_only_message_without_error |
| **hub mod.rs** | — | — | ~40 | `test_pool`+`make_pool`(`pub(super)`) + 6 兄弟 re-export |

**测数核验**:8+10+2+5+10 = **35** ✓
**最大文件** latency_message.rs ~551 < 1200 ✓(AC1 大幅达标,最大簇也不到目标一半)
**helper 归属**:`test_pool`@L38 / `make_pool`@L49 跨 5 簇复用 → 提 hub;**无 cluster-private helper**(与 memories_tests 的 `input`/`reseat_created_at` 不同)。

### 区间边界细节(为什么是这些行号)

- `session_crud.rs` L52 起:含 `make_pool` 收尾(L49–51)后紧跟的首个 `#[tokio::test]`(L52)。
  `make_pool`@L49 是 hub helper,不随本簇提取(进 hub)。止于 L345(`list_sessions_preview_truncates_at_80_chars` 末),下一测 `touch_session_updates_timestamp` 的 `#[tokio::test]` 在 L346。
- `fields_worktree.rs` L346–647:L346 是 `touch_session_updates_timestamp` 属性行;止于 L647(`worktree_state_backfill_legacy_active` 末),下一测 `insert_system_event_appends_to_history` 在 L649(L648 是属性行)。**L648 属 system_events 簇**,故 fields_worktree 提取区间是 L346–647(不含 L648)。
- `system_events.rs` L648–726:L648 是 `insert_system_event_appends_to_history` 属性行;止于 L726(`insert_system_event_seq_increments` 末)。L727–736 是分隔注释块(`// ===...part 2...`)。下一测属性行在 L737。
- `model_usage.rs` L737–937:L737 是 `update_session_model_id_sets_and_clears` 属性行;止于 L937(`list_sessions_includes_last_turn_columns` 末)。L938–942 是 `// F5 latency` 分隔注释。下一测属性行在 L943。
- `latency_message.rs` L943–1493:L943 是 `persist_turn_with_latency_writes_three_columns` 属性行;止于文件末 L1493。

> 注:每簇提取区间从该簇**首个属性行**到下一簇**首个属性行前一行**,中间的分隔注释块
> (如 L727–736 的 part-2 banner)归其后的簇(model_usage 的 banner 进 model_usage 提取区间,
> 因为 L737 起点已含)。**分隔注释无测试语义**,落在哪个簇不影响正确性,本表按"起点含属性行"
> 约定归属。执行时 `sed -n '<start>,<end>p'` 严格按下表区间。

## 3. hub(mod.rs)内容

```rust
#![cfg(test)]

// Re-export the 6 `db::` siblings that the pre-split single file imported via
// `use super::{migrations, models, projects, providers, sessions, types}`. After
// splitting, a cluster file's `super` points at the hub (`sessions_tests`), so
// `use super::sessions::{...}` would resolve to `sessions_tests::sessions` (nope).
// Re-importing each sibling at the hub makes `super::<sibling>` resolve through
// the hub to the real `db::<sibling>` — **call sites unchanged** (pure relocation).
#[allow(unused_imports)]
use super::{migrations, models, projects, providers, sessions, types};

mod session_crud;
mod fields_worktree;
mod system_events;
mod model_usage;
mod latency_message;

/// 跨簇共享:簇测试通过 test_pool()/make_pool() 建池。仅模块树内可见。
pub(super) async fn test_pool() -> sqlx::SqlitePool {
    // …原 L38–47 body 平移…
}

pub(super) async fn make_pool() -> sqlx::SqlitePool {
    test_pool().await // alias for readability
}
```

### `use super::{...};` re-export 技巧(关键正确性点 —— 本任务与 memories 的差异)

原单文件头(L23–36)是一组**多兄弟** `use super::{ migrations::run_migrations,
models::create_model, projects::create_project, providers::create_provider,
sessions::{...}, types::WorktreeState }`。原文件的 `super` 指向 `db`(文件级模块的父)。
拆分后簇文件的 `super` 指向 hub(`sessions_tests`)。若不做任何处理,簇文件里任何
`use super::sessions::{...}` / `use super::types::WorktreeState` 都会解析到
`sessions_tests::sessions` / `sessions_tests::types`(不存在)→ 编译错。

**解决**(沿用 `memories_tests/mod.rs` 先例,只是兄弟数从 1 扩到 6):hub 加

```rust
#[allow(unused_imports)]
use super::{migrations, models, projects, providers, sessions, types};
```

hub 的 `super` 是 `db`,这条把 6 个兄弟模块引入 hub 作用域;簇文件 `use super::<sibling>::{...}`
经 hub 的 re-export 命中 `db::<sibling>`,**调用点零改动**。这是纯搬迁允许的 module 接线(R2)。

> hub 自身不直接用这 6 个兄弟的符号(只做 re-export),故 `#[allow(unused_imports)]` 必需,
> 否则 clippy 报 unused。`db/mod.rs` 已对这 6 个模块用 `pub mod`(实测 L62/64/67/69/71/78),
> 故 hub 用 `use super::<sibling>` 即可见其公开项。

### 可见性决策(R3)

- `test_pool` / `make_pool`:5 簇测试**全部**用(`test_pool` 前 20 测 + `make_pool` 后 15 测,
  实测 35 处调用点)→ 提 hub,`pub(super)`(限定模块树,不放宽到 crate)。
- **无 cluster-private helper**:本文件除 test_pool/make_pool 外无其它非测试函数
  (create_session 等是被测函数,经 `use super::sessions::create_session` 引入,非本地 helper)。
  故 R3 偏离字面 `pub(crate)` 取"真跨簇才进 hub",与 memories 先例一致。

## 4. 每簇 import 策略

每个簇文件自含 `#![cfg(test)]` + 该簇**实际用到**的 `use`。不集中到 hub re-export
(避免 hub 膨胀 + 违反"unused-import 精确收敛"惯例)。原文件单一 use 块(L17–36)按簇拆。

**簇文件头标准模板**:
```rust
#![cfg(test)]

use super::{ /* 该簇用到的 hub 符号:test_pool / make_pool */ };
use super::{ /* 该簇用到的 db 兄弟:sessions / types / models / providers / ... */ }::...;
use crate::llm::types::{ /* 该簇用到的 crate 级符号 */ };
// 其余 use
```

**收敛办法**:每簇先放原文件的完整 use 块(L17–36 全量),跑 `cargo test --lib <簇过滤>`
+ `cargo clippy --lib --tests`,按 `unused import` 警告逐簇删到零警告。这是 split-agent-tests /
memories_tests 先例的"依赖编译器精确收敛"法。

### 各簇实测用到的符号(初步,以 clippy 终验为准)

| 簇 | hub helper | `super::sessions::` 符号 | 其它兄弟 | crate 级 + 外部 |
|---|---|---|---|---|
| `session_crud.rs` | `test_pool` | `create_session`, `delete_session`, `delete_messages_by_session`, `list_sessions`, `load_session`, `persist_turn` | — | `crate::llm::types::{ContentBlock, MessageContent, Role}`, `crate::projects::DEFAULT_PROJECT_ID`, `uuid::Uuid`, `sqlx` (inline query 无) |
| `fields_worktree.rs` | `test_pool` | `create_session`(间接,经 `load_session`), `load_session`, `list_sessions`, `set_session_plugin_name`, `set_session_workflow_enabled`, `set_worktree_state`, `touch_session`, `update_session_cwd` | `super::types::WorktreeState` | `crate::llm::types::{...}`, `crate::projects::DEFAULT_PROJECT_ID`, `uuid::Uuid`, `sqlx` |
| `system_events.rs` | `test_pool` | `create_session`(间接), `insert_system_event`, `load_session`, `persist_turn` | — | `crate::llm::types::{ContentBlock, MessageContent, Role}`, `crate::projects::DEFAULT_PROJECT_ID`, `uuid::Uuid` |
| `model_usage.rs` | `make_pool` | `create_session`, `list_sessions`, `load_session`, `update_last_turn_usage`, `update_session_model_id` | `super::models::create_model`, `super::providers::create_provider` | `crate::llm::types::TokenUsage`, `crate::projects::DEFAULT_PROJECT_ID`, `uuid::Uuid`, `sqlx` |
| `latency_message.rs` | `make_pool` | `create_session`, `find_message_id_by_seq`, `load_session`, `persist_turn`, `record_tool_duration`, `update_message_latency`, `MessageLatency` | — | `crate::llm::types::{ContentBlock, MessageContent, Role}`, `crate::projects::DEFAULT_PROJECT_ID`, `uuid::Uuid` |

> 上表是人工预估,执行时**以 clippy `unused import`/`cannot find` 报错为准**逐簇收敛,不主动判断。
> 注意 `DEFAULT_PROJECT_ID` 来自 `crate::projects`(非 db 兄弟),原文件 L21 已是全路径 `use crate::projects::DEFAULT_PROJECT_ID;`。

### `SqlitePool` 使用注记

`use sqlx::SqlitePool`(原 L17)只被 2 个 hub helper 用(test_pool@L38/39、make_pool@L49)。
拆分后:
- hub mod.rs:`SqlitePool` 进 test_pool/make_pool 签名 → hub 用全路径 `sqlx::SqlitePool`
  (不在 hub 单独 `use sqlx::SqlitePool`,避免 hub 报 unused —— 与 memories 先例一致)。
- 簇文件:测试体里用 `sqlx::query(...)`(全路径,无需 `use sqlx::SqlitePool`),
  经 `make_pool().await` 拿 pool 实例 → **无簇需要 `use sqlx::SqlitePool`**。
  (fields_worktree.rs / model_usage.rs 内有 `sqlx::query(...)` 内联,但那是 `sqlx::` 路径,不需 use。)

## 5. 纯搬迁铁律边界(R2)

| 允许 | 禁止 |
|---|---|
| 文件物理搬迁(`sed -n 'start,end p'` 提取簇 body) | 改测试函数 body / 断言 / 顺序 |
| helper `pub(super)` 可见性调整(R3) | 重命名测试函数或 helper |
| hub 加 `use super::{6 兄弟};` re-export(module 接线) | 合并 / 拆分测试函数 |
| 每簇删 `unused import`(clippy 驱动) | 改 `#[tokio::test]` 属性 |
| hub helper 用全路径 `sqlx::SqlitePool` | 改执行顺序 / 并行度 |
| mod 声明接线(`mod xxx;`) | 删/增测试或 helper |

## 6. AC6 文档 sweep

实测 `grep -rn "sessions_tests\.rs:[0-9]" --include="*.md" --include="*.rs" . | grep -v archive`:

- **行号引用(`:LINE`)0 处** —— AC6 天然满足,无需改。
- **符号路径引用**:目录化后模块路径 `sessions_tests::...` 不变(声明零改动),即便有 `::fn`
  符号引用也保留不改。

本任务无行号引用需 sweep(与 memories 先例一致)。

## 7. 风险与回滚

- **风险 1:簇边界切错(漏搬/多搬一行)** → `cargo test --lib` 立即报缺测或编译错,
  编译器是硬闸。逐簇过滤跑测可定位。**本文件 5 簇严格连续,边界清晰,风险低**。
- **风险 2:hub re-export 漏某个兄弟** → 簇文件里该兄弟的 `use super::<sibling>::{...}`
  报 `cannot find`。hub 必须把 6 个兄弟全列(migrations/models/projects/providers/sessions/types)。
  **这是本任务相对 memories 的关键扩展点**(memories 只 re-export 1 个兄弟)。
- **风险 3:helper 可见性不够** → 编译报 private。test_pool/make_pool 必须 `pub(super)`。
- **风险 4:hub helper 签名漏 `sqlx::SqlitePool` 全路径** → 报 `cannot find type SqlitePool`。
  hub 用全路径(§3),不要 `use sqlx::SqlitePool`(否则 hub 报 unused)。
- **回滚**:`git revert <主迁移 commit>`(回退到单文件 1493 行状态)。
- **编译中间态**:目录化模式下单 commit 内 `sessions_tests.rs` 删除 + 目录建立必须原子完成
  (否则源文件与 `mod` 声明冲突)。故不逐簇 commit,主迁移单 commit(见 implement.md)。

## 8. 验证矩阵(终验)

| 检查 | 命令 | 期望 |
|---|---|---|
| 调用方零影响(AC3) | `git diff app/src-tauri/src/db/mod.rs` | 空 diff(L72 一字未改) |
| 全量绿(AC2) | `PKG_CONFIG_PATH=... cargo test --lib` | 1662 基线,35 个 sessions_tests 测全过 |
| 行数合规(AC1) | 各簇 + hub `< 1200`(`wc -l`) | latency_message ~551,均达标 |
| clippy(AC4) | `cargo clippy --lib --tests` | 0 warning |
| fmt(AC4) | `cargo fmt --check` | 0 diff |
| 测数守恒(AC2/R5) | `cargo test --lib "db::sessions_tests::"` | 跑前 35 / 跑后 35 |
| AC6 文档 sweep | `grep -rn "sessions_tests\.rs:[0-9]" ... \| grep -v archive` | 0 输出 |
