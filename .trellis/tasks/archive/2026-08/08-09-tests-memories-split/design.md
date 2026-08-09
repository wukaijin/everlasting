# Design: memories_tests.rs 目录化拆分

> 本文是技术设计。需求与验收见 `prd.md`。执行清单见 `implement.md`。

## 0. PRD 订正(实测后)

| 项 | PRD 原值 | 实测 | 处置 |
|---|---|---|---|
| 测试函数数 | "40 个" | **49 个** | 以实测为准;`task.json` 描述本就是"49",PRD 表错。终验 N=49 |
| helper 数 | 3 个(`test_pool`/`make_pool`/`reseat_created_at`) | **4 个**(多 `input`@L304) | R3 helper 清单补入 `input` |
| 簇边界 | PRD 表 8 簇,行号区间粗略 | **9 簇**(见 §2 权威表) | 重新拍板,以本 design §2 为准 |
| 基线测数 | 1662 全绿 | ✅ 实测 1662 passed | 一致 |

> 同类订正在 `08-09-tests-agent-loop-split` 里出现过(PRD"32 测"实测 41),与本任务同构。

## 1. 形态:目录化(R1 首选方案)

```
db/
├── mod.rs                         # L60 `pub mod memories_tests;` 零改动(Rust 自动解析到目录)
└── memories_tests/                # 新目录(hub + 9 簇文件)
    ├── mod.rs                     # hub:#![cfg(test)] + 9 mod 声明 + test_pool/make_pool
    ├── fts5_migration.rs          # FTS5/migration/schema-check/pragma:4 测
    ├── insert_memory.rs           # insert 写入安全网:9 测 + input(私有 helper)
    ├── list_delete_search.rs      # list/delete/FTS search/count:8 测
    ├── find_pitfalls.rs           # find_pitfalls_by_trigger 查询:3 测
    ├── hitcount_status.rs         # hit_count/status 迁移/FTS 触发器/query plan:4 测
    ├── p5_promote.rs              # promote_if_eligible 状态机:6 测 + reseat_created_at(私有)
    ├── validate_memory_text.rs    # validate_memory_text 单测:5 测
    ├── update_memory.rs           # update_memory + insert_still_safe:7 测
    └── build_recall_text.rs       # build_recall_text_with_rows:3 测
```

`memories_tests.rs`(2241 行)整体删除,内容按簇平移到 9 个簇文件。`db/mod.rs:60`
`pub mod memories_tests;` 一字不改 —— Rust 2018 module 路径解析器对 `memories_tests/`
目录与 `memories_tests.rs` 文件等价对待,调用方语义不变。

### 为什么不用平级多文件(备选)

平级方案要把 `db/mod.rs:60` 的 `pub mod memories_tests;` 改成 10 行声明,违反 R1
"声明零改动"且改动调用方模块路径。目录化零改动更优。**与 `tests_agent_loop` 先例一致**。

## 2. 簇映射(9 文件 / 49 测 + 4 helper)—— 权威表

> 行号是 `memories_tests.rs` 的实测起止(含 `#[test]`/`#[tokio::test]` 行及簇内 helper)。
> **本文件测试在源中按簇连续排列,无交错**(与 `tests_agent_loop` 的 mock/error_path 交错不同),
> 故每簇可用连续区间 `sed` 提取,无需逐测试函数抽取。

| 文件 | 测数 | 源行号(含头) | 行数 | 含 helper | 簇内测试 |
|---|---|---|---|---|---|
| `fts5_migration.rs` | 4 | L62–328 | ~267 | — | fts5_trigram_tokenizer / am_migration_is_idempotent / am_db_check_rejects_invalid_enum_and_oversize / am_pragma_status_recorded_for_open_q4 |
| `insert_memory.rs` | 9 | L304–567 | ~264 | `input`(私有,L304) | insert_memory_happy_path_roundtrip / _user_scope_rejected / _project_scope_rejected / _rejects_empty / _rejects_oversize / _rejects_sensitive_content / _rejects_sensitive_path / _rejects_temporary_paths / _generalizes_home_path |
| `list_delete_search.rs` | 8 | L568–1213 | ~646 | — | list_memories_filters_by_scope / delete_memory_removes_row_and_fts / search_memories_fts_bm25 / _escapes_special / _scope_project_id / _project_isolation / _status_filter_candidate / count_memories_for_session |
| `find_pitfalls.rs` | 3 | L1214–1415 | ~202 | — | find_pitfalls_by_trigger_tool_name / _path_globs / _command_pattern |
| `hitcount_status.rs` | 4 | L1416–1708 | ~293 | — | bump_hit_count / update_status_legal_illegal / fts_triggers_sync / explain_query_plan_uses_index |
| `p5_promote.rs` | 6 | L1709–1931 | ~223 | `reseat_created_at`(私有,L1710) | p5_promote_candidate_to_active / _active_to_verified / _skips_demoted / _unknown_id_noop / _verified_stays / _candidate_below_threshold |
| `validate_memory_text.rs` | 5 | L1932–1987 | ~56 | — | validate_memory_text_happy_path / _rejects_empty / _rejects_oversize / _rejects_sensitive / _rejects_sensitive_path |
| `update_memory.rs` | 7 | L1988–2177 | ~190 | — | insert_memory_still_safe_after_helper_extract / update_memory_roundtrip / _rejects_oversize / _rejects_sensitive / _rejects_sensitive_path / _sets_edited_by_user / _not_found |
| `build_recall_text.rs` | 3 | L2178–2241 | ~64 | — | build_recall_text_with_rows_returns_rows / _empty_query / _no_match |
| **hub mod.rs** | — | — | ~30 | `test_pool`+`make_pool`(`pub(super)`) | — |

**测数核验**:4+9+8+3+4+6+5+7+3 = **49** ✓
**最大文件** list_delete_search.rs ~646 < 1200 ✓(AC1 大幅达标,最大簇也不到目标一半)

### `insert_memory.rs` 边界说明

`input` helper(L304–328)在第一个 insert 测(`insert_memory_happy_path_roundtrip`@L329)
之前定义,且**只被 insert 簇 9 测使用**(实测 15 处调用全在 L332–534 区间内)。
故随 insert 簇迁,作簇内私有 helper,不进 hub。提取区间含 helper 即 L304–567。

### `update_memory.rs` 含 `insert_memory_still_safe_after_helper_extract`

L1987 `insert_memory_still_safe_after_helper_extract` 是 insert 域的回归守护测,但它在源文件中
紧邻 validate_memory_text 簇之后、update_memory 簇之前。语义上它验证 insert 仍安全,可归
insert 或 update 任一簇。**按源文件连续性原则并入 update_memory.rs**(避免与 insert 簇在
源中非连续:insert 簇止于 L567,中间隔着 6 个簇)。这保持"连续区间提取"的不变量。

## 3. hub(mod.rs)内容

```rust
#![cfg(test)]

// Re-export so cluster files can keep their `use super::memories::{...}`
// imports unchanged from the pre-split single-file layout (pure relocation).
// `super::memories` here resolves to `db::memories` (the hub's parent is `db`).
#[allow(unused_imports)]
use super::memories;

mod fts5_migration;
mod insert_memory;
mod list_delete_search;
mod find_pitfalls;
mod hitcount_status;
mod p5_promote;
mod validate_memory_text;
mod update_memory;
mod build_recall_text;

/// 跨簇共享:每个簇测试都通过 make_pool() 建池。仅模块树内可见。
pub(super) async fn test_pool() -> sqlx::SqlitePool {
    // …原 L35–44 body 平移…
}

pub(super) async fn make_pool() -> sqlx::SqlitePool {
    test_pool().await // alias for readability
}
```

### `use super::memories;` re-export 技巧(关键正确性点)

原单文件 L20–28 `use super::memories::{...}` 中,`super` 指向 `db`(文件级模块的父)。
拆分后簇文件的 `super` 指向 hub(`memories_tests`)。若不做任何处理,簇文件里
`use super::memories::{...}` 会解析到 `memories_tests::memories`(不存在)→ 编译错。

**解决**(沿用 `tests_agent_loop/mod.rs` 先例):hub 加 `#[allow(unused_imports)] use super::memories;`。
hub 的 `super` 是 `db`,故这条把 `db::memories` 引入 hub 作用域;簇文件 `use super::memories::{...}`
经 hub 的 re-export 命中 `db::memories`,**调用点零改动**。这是纯搬迁允许的 module 接线(R2)。

> hub 自身不直接用 `memories` 的符号(只做 re-export),故 `#[allow(unused_imports)]` 必需,
> 否则 clippy 报 unused。与 `tests_agent_loop` 先例的 `use super::tests_common;` 同构。

### 可见性决策(R3 修正)

- `test_pool` / `make_pool`:9 簇测试**全部**用 `make_pool().await`(实测 46 处调用点)
  → 提 hub,`pub(super)`(限定模块树,不放宽到 crate)。
- `input`:15 处调用**全在 insert 簇内**(L332–534)→ 留 insert_memory.rs 私有。
- `reseat_created_at`:2 处调用**全在 p5 簇内**(L1783/1806)→ 留 p5_promote.rs 私有。

R3"簇间复用"前提只对 test_pool/make_pool 成立,故偏离字面 `pub(crate)` 取"真跨簇才进 hub"。

## 4. 每簇 import 策略

每个簇文件自含 `#![cfg(test)]` + 该簇**实际用到**的 `use`。不集中到 hub re-export
(避免 hub 膨胀 + 违反"unused-import 精确收敛"惯例)。原文件单一 use 块(L20–28)按簇拆。

**簇文件头标准模板**:
```rust
#![cfg(test)]

use super::memories::{ /* 该簇实际用到的符号 */ };
// 其余 use(super::make_pool 等)
```

**收敛办法**:每簇先放完整 use 块,跑 `cargo test --lib <簇过滤>` + `cargo clippy --lib --tests`,
按 `unused import` 警告逐簇删到零警告。这是 split-agent-tests / tests_agent_loop 先例的
"依赖编译器精确收敛"法。

### 各簇实测用到的 `super::memories::` 符号(初步,以 clippy 终验为准)

| 簇 | 用到的符号 |
|---|---|
| fts5_migration | (无直接用 memories 符号,纯 pragma/migration 探测) |
| insert_memory | `insert_memory`, `MemoryInput`, `MemoryScope`, `MemoryKind`, `MemoryStatus`, `MemoryInsertError` |
| list_delete_search | `list_memories`, `delete_memory`, `search_memories_fts`, `count_memories_for_session`, `get_memory_by_id`, `test_helpers::insert_raw`, `MemoryScope`, `MemoryKind`, `MemoryStatus`, `RecallStatusFilter` |
| find_pitfalls | `find_pitfalls_by_trigger`, `test_helpers::insert_raw`, `MemoryScope`, `MemoryKind`, `MemoryStatus` |
| hitcount_status | `bump_hit_count`, `update_status`, `get_memory_by_id`, `test_helpers::insert_raw`, `MemoryScope`, `MemoryKind`, `MemoryStatus`, `StatusTransitionError` |
| p5_promote | `promote_if_eligible`, `get_memory_by_id`, `test_helpers::insert_raw`, `MemoryScope`, `MemoryKind`, `MemoryStatus`, `ACTIVE_TO_VERIFIED_AGE_DAYS`, `ACTIVE_TO_VERIFIED_AT`, `CANDIDATE_TO_ACTIVE_AT` |
| validate_memory_text | `validate_memory_text`, `MemoryScope`, `MemoryKind`, `MemoryStatus` |
| update_memory | `update_memory`, `get_memory_by_id`, `test_helpers::insert_raw`, `MemoryUpdateError`, `MemoryScope`, `MemoryKind`, `MemoryStatus` |
| build_recall_text | (用 `crate::agent::memory_recall::build_recall_text_with_rows`,不依赖 super::memories) |

> 上表是人工预估,执行时**以 clippy `unused import`/`cannot find` 报错为准**逐簇收敛,不主动判断。

### `SqlitePool` 使用注记

`use sqlx::SqlitePool`(原 L20)只被 3 个 helper 用(test_pool@L35/36、make_pool@L45、
reseat_created_at@L1710 签名)。拆分后:
- hub mod.rs:`SqlitePool` 进 test_pool/make_pool 签名 → hub 需 `use sqlx::SqlitePool`
  (或写全路径 `sqlx::SqlitePool`,见 §3 代码用全路径避免 hub 单独 use)。
- p5_promote.rs:`reseat_created_at(pool: &SqlitePool, ...)` 签名用 → 该簇需 `use sqlx::SqlitePool`。
- 其余簇:不直接用 `SqlitePool` 类型(只通过 `make_pool().await` 拿实例)→ 不需此 use。

## 5. 纯搬迁铁律边界(R2)

允许 | 禁止
---|---
文件物理搬迁(`sed -n 'start,end p'` 提取簇 body,含簇内 helper) | 改测试函数 body / 断言 / 顺序
helper `pub(super)`/私有 可见性调整(R3) | 重命名测试函数或 helper
hub 加 `use super::memories;` re-export(module 接线) | 合并 / 拆分测试函数
每簇删 `unused import`(clippy 驱动) | 改 `#[tokio::test]`/`#[test]` 属性
hub helper 用全路径 `sqlx::SqlitePool`(避免 hub 单独 use) | 改执行顺序 / 并行度
mod 声明接线(`mod xxx;`) | 删/增测试或 helper

## 6. AC6 文档 sweep

实测 `grep -rn "memories_tests\.rs:[0-9]" --include="*.md" --include="*.rs" . | grep -v archive`:

- **行号引用(`:LINE`)0 处** —— AC6 天然满足,无需改。
- **符号路径引用**(目录化后语义不变):无 `:LINE` 引用,R6 措辞已精确为"行号引用"。
  目录化后模块路径 `memories_tests::...` 不变,即便有 `::fn` 符号引用也保留不改。

本任务无行号引用需 sweep。

## 7. 风险与回滚

- **风险 1:簇边界切错(漏搬/多搬一行)** → `cargo test --lib` 立即报缺测或编译错,
  编译器是硬闸。逐簇过滤跑测可定位。
- **风险 2:`use super::memories;` re-export 漏写** → 簇文件 `use super::memories::{...}`
  全部编译错(`cannot find`)。hub 必须有这条(见 §3)。
- **风险 3:helper 可见性不够** → 编译报 private。test_pool/make_pool 必须 `pub(super)`;
  input/reseat_created_at 留簇内私有。
- **回滚**:`git revert <主迁移 commit>`(回退到单文件 2241 行状态)。
- **编译中间态**:目录化模式下单 commit 内 `memories_tests.rs` 删除 + 目录建立必须原子完成
  (否则源文件与 `mod` 声明冲突)。故不逐簇 commit,主迁移单 commit(见 implement.md)。

## 8. 验证矩阵(终验)

| 检查 | 命令 | 期望 |
|---|---|---|
| 调用方零影响(AC3) | `git diff app/src-tauri/src/db/mod.rs` | 空 diff(L60 一字未改) |
| 全量绿(AC2) | `PKG_CONFIG_PATH=... cargo test --lib` | 1662 基线,49 个 memories_tests 测全过 |
| 行数合规(AC1) | 各簇 + hub `< 1200`(`wc -l`) | list_delete_search ~646,均达标 |
| clippy(AC4) | `cargo clippy --lib --tests` | 0 warning |
| fmt(AC4) | `cargo fmt --check` | 0 diff |
| 测数守恒(AC2/R5) | `cargo test --lib "db::memories_tests::"` | 跑前 49 / 跑后 49 |
| AC6 文档 sweep | `grep -rn "memories_tests\.rs:[0-9]" ... \| grep -v archive` | 0 输出 |
