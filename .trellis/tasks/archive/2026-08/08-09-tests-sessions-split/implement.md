# Implement: sessions_tests.rs 目录化拆分

> 执行清单。需求见 `prd.md`,技术设计见 `design.md`。纯搬迁铁律适用全程(R2)。
> 同构先例:`.trellis/tasks/archive/2026-08/08-09-tests-memories-split/`(已落地)。

## 前置(基线快照,做一次)

```bash
cd app/src-tauri
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
cargo test --lib --no-run                                          # 确认能编译
cargo test --lib "db::sessions_tests::" 2>&1 | tail -3             # 应 35 passed
```

基线测数:全量 1662;sessions_tests 模块 = **35**(PRD 写"33"是笔误,实测 35,见 design §0)。

## 簇 → 源行号速查(提取依据,见 design §2)

| 文件 | 源行号(含头) | 测数 | 关键边界 |
|---|---|---|---|
| `session_crud.rs` | L52–345 | 8 | 从首个 `#[tokio::test]`(L52)到 touch_session 之前(L346);**不含 make_pool@L49–51**(进 hub) |
| `fields_worktree.rs` | L346–647 | 10 | touch/update_cwd/workflow×2/plugin×2/worktree×3;止于 system_event 属性行(L648)之前一行 |
| `system_events.rs` | L648–726 | 2 | 2 测;止于 part-2 banner(L727–736)之前;下一测属性行在 L737 |
| `model_usage.rs` | L737–937 | 5 | model_id×3 / usage / list;止于 F5 latency 注释块(L938–942)之前;下一测属性行在 L943 |
| `latency_message.rs` | L943–1493 | 10 | latency×6 / find_message_id / record_tool_duration×3;末尾簇,止于文件末 |

> ✅ **无交错**:5 簇在源中**严格连续**排列(同 memories_tests 先例),故每簇用连续区间
> `sed -n '<start>,<end>p'` 一次性提取即可,无需逐测试函数抽取。
>
> 注:簇间分隔注释块(part-2 banner @L727–736、F5 latency 注释 @L938–942)归其**后**的簇
> (随该簇提取区间起点带入),无测试语义,不影响正确性。

## 执行步骤

### Step 1:建目录 + hub 骨架(主迁移 commit 内,原子)

1. `mkdir -p app/src-tauri/src/db/sessions_tests`
2. 写 `sessions_tests/mod.rs`(hub),内容见 design §3:
   ```rust
   #![cfg(test)]

   // 6 兄弟 re-export(关键正确性点,见 design §3)
   #[allow(unused_imports)]
   use super::{migrations, models, projects, providers, sessions, types};

   mod session_crud;
   mod fields_worktree;
   mod system_events;
   mod model_usage;
   mod latency_message;

   pub(super) async fn test_pool() -> sqlx::SqlitePool {
       // ← 原 L38–47 body 平移
   }

   pub(super) async fn make_pool() -> sqlx::SqlitePool {
       test_pool().await
   }
   ```
   - `test_pool` body:`sed -n '38,47p'` 提取原文件 + 改签名为全路径 `sqlx::SqlitePool`。
   - `make_pool` body:`sed -n '49,51p'` 提取(原签名 `SqlitePool` → 全路径)。
   - helper 用全路径 `sqlx::SqlitePool`(hub 不单独 `use sqlx::SqlitePool`,避免 hub 报 unused)。

### Step 2:逐簇提取(每簇一个文件,均加 `#![cfg(test)]` + 实际 use)

因 5 簇在源中严格连续(见上 ✅),每簇用连续区间 `sed -n '<start>,<end>p'` 提取:

```bash
SRC=app/src-tauri/src/db/sessions_tests.rs
DST=app/src-tauri/src/db/sessions_tests
sed -n '52,345p'  $SRC > $DST/session_crud.rs       # 8 测
sed -n '346,647p' $SRC > $DST/fields_worktree.rs    # 10 测
sed -n '648,726p' $SRC > $DST/system_events.rs      # 2 测
sed -n '737,937p' $SRC > $DST/model_usage.rs        # 5 测
sed -n '943,1493p'$SRC > $DST/latency_message.rs    # 10 测
```

每个簇文件**前置** `#![cfg(test)]` + 该簇用到的 `use`:
- `#![cfg(test)]`(文件级,跟随原单文件惯例 + tests_agent_loop / memories_tests 先例)
- `use super::{sessions::{...}, ...}`(该簇实际符号,见 design §4 表;先放完整块,clippy 后删 unused)
- `use super::test_pool;` 或 `use super::make_pool;`(凡用 helper 的簇):
  - session_crud / fields_worktree / system_events → `use super::test_pool;`
  - model_usage / latency_message → `use super::make_pool;`
- `use crate::llm::types::{...}`(该簇实际符号:ContentBlock / MessageContent / Role / TokenUsage)
- `use crate::projects::DEFAULT_PROJECT_ID;`(5 簇全用)
- `use uuid::Uuid;`(5 簇全用,create_session 调用点)
- model_usage.rs 额外:`use super::{models::create_model, providers::create_provider};`(从 db 兄弟)
- **无簇需要 `use sqlx::SqlitePool`**(sqlx 用全路径 `sqlx::query(...)`,helper 经 hub 全路径签名)

> **`#![cfg(test)]` 必加**:原单文件 L1 是文件级 `#![cfg(test)]`,提取出的区间不含它,
> 每个簇文件必须自行声明,否则非 test build 会编译测试代码。
>
> **6 兄弟 re-export 已在 hub**:簇文件直接 `use super::sessions::{...}` 即可命中
> `db::sessions`(经 hub re-export),无需 `use super::super::...`。

### Step 3:删源(主迁移 commit 内,原子)

- `rm app/src-tauri/src/db/sessions_tests.rs`
- **`db/mod.rs` 不动**(L72 `pub mod sessions_tests;` 原样)

### Step 4:编译收敛(主迁移 commit 内)

```bash
cargo test --lib --no-run 2>&1 | grep -E "error|warning: unused" | head -40
```

- `cannot find ... in this scope` / `unresolved import` → 该簇漏 use 或 hub re-export 漏某个兄弟
  (检查 hub `use super::{migrations, models, projects, providers, sessions, types};` 是否齐全)
- `unused import` → 逐簇删 use(允许:clippy 驱动的死 import 清理,非逻辑改动)
- `private function` → helper 可见性:test_pool/make_pool 必须 `pub(super)`
- `cannot find type SqlitePool` → hub helper 签名漏全路径 `sqlx::SqlitePool`

### Step 5:测试收敛(主迁移 commit 内)

```bash
cargo test --lib "db::sessions_tests::" 2>&1 | tail -5    # 应 35 passed
PKG_CONFIG_PATH="..." cargo test --lib 2>&1 | tail -3      # 全量 1662 基线
```

35 缺一不可(漏搬 → 该测消失 → 计数 <35 → 回查 Step 2 提取区间)。

### Step 6:clippy + fmt(主迁移 commit 内)

```bash
cargo clippy --lib --tests 2>&1 | grep -E "warning|error" | head
cargo fmt --check
```

零警告零 diff。若有 `unused import` 按 Step 4 收敛。

### Step 7:行数核验(AC1)

```bash
wc -l app/src-tauri/src/db/sessions_tests/*.rs
```

最大 latency_message.rs ~551 < 1200(远达标)。

### Step 8:主迁移 commit

```bash
git add app/src-tauri/src/db/sessions_tests/ \
        app/src-tauri/src/db/sessions_tests.rs    # 删除
git commit -m "refactor: sessions_tests 子模块化(sessions_tests.rs → hub + 5 簇文件)"
```

### Step 9:文档 sweep commit(AC6 —— 实测 0 行号引用,本步预期为空)

```bash
# 核验无 sessions_tests.rs:LINE 行号引用(应 0 输出)
grep -rn "sessions_tests\.rs:[0-9]" --include="*.md" --include="*.rs" . | grep -v "/archive/"
```

若有输出 → 改符号引用(本任务预判 0);若无,跳过本 commit。

### Step 10:收官表同步(收尾 commit)

更新 `.trellis/spec/backend/directory-structure.md` 的"收官状态表":
将 `db/sessions_tests.rs 1493 | 08-09-tests-sessions-split | ⏳ 规划中` 一行的
现状列改为 `✅ sessions_tests/ 目录(hub ~40 + 5 簇);35 测原样,最大 latency_message.rs ~551`。

同时检查收官表"已知遗留"段:4 个测试任务完成进度(agent_loop ✅ / memories ✅ / sessions ← 本次 /
subagent ⏳)。sessions 完成后改为"3/4 完成,剩 subagent"。若该段措辞需调,一并更新。

```bash
git add .trellis/spec/backend/directory-structure.md
git commit -m "docs(spec): sessions_tests 拆分收官 — directory-structure 收官表同步"
```

## 回滚点

- `git revert <Step 8 commit>` → 回到单文件 1493 行状态
- Step 9 若做了 sweep,独立 `git revert <Step 9 commit>`
- Step 10 收官表同步独立 `git revert <Step 10 commit>`

## 风险清单

| 风险 | 触发 | 缓解 |
|---|---|---|
| **6 兄弟 re-export 漏写/漏某个**(本任务关键扩展点) | 簇文件 `use super::<sibling>::{...}` 报 cannot find | hub 必须有 `#[allow(unused_imports)] use super::{migrations, models, projects, providers, sessions, types};`(design §3)。memories 只 re-export 1 个,本任务扩到 6 个,易漏 |
| 漏搬一个测试 | Step 5 计数 <35 | 计数硬闸,逐簇过滤定位 |
| 簇文件漏 `#![cfg(test)]` | 非 test build 编译测试代码 | 每簇文件头必加(Step 2 模板) |
| helper 可见性不够 | 编译报 private | test_pool/make_pool 必须 pub(super) |
| hub helper 签名漏全路径 SqlitePool | `cannot find type SqlitePool` | hub 用 `sqlx::SqlitePool` 全路径,不 `use sqlx::SqlitePool` |
| unused import 残留 | clippy warning | 依赖编译器逐簇收敛(design §4) |
| db/mod.rs 误改 | AC3 失败 | `git diff app/src-tauri/src/db/mod.rs` 空是硬闸 |
