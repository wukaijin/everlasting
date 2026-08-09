# Implement: memories_tests.rs 目录化拆分

> 执行清单。需求见 `prd.md`,技术设计见 `design.md`。纯搬迁铁律适用全程(R2)。

## 前置(基线快照,做一次)

```bash
cd app/src-tauri
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
cargo test --lib --no-run                                          # 确认能编译
cargo test --lib "db::memories_tests::" 2>&1 | tail -3             # 应 49 passed
```

基线测数:全量 1662;memories_tests 模块 = **49**(PRD 写"40"是笔误,实测 49,见 design §0)。

## 簇 → 源行号速查(提取依据,见 design §2)

| 文件 | 源行号(含头) | 测数 | 关键边界 |
|---|---|---|---|
| fts5_migration.rs | L62–328 | 4 | 从首个 `#[tokio::test]` 到 insert_memory 簇之前 |
| insert_memory.rs | L304–567 | 9 | **含 `input` helper(L304–328)** + 9 测;止于 list_memories 之前 |
| list_delete_search.rs | L568–1213 | 8 | list/delete/search×5/count;止于 find_pitfalls 之前 |
| find_pitfalls.rs | L1214–1415 | 3 | 3 测;止于 bump_hit_count 之前 |
| hitcount_status.rs | L1416–1708 | 4 | bump/status/fts_triggers/query_plan;**不含** reseat_created_at(L1710) |
| p5_promote.rs | L1709–1931 | 6 | **含 `reseat_created_at` helper(L1710–1721)** + 6 测;止于 validate 之前 |
| validate_memory_text.rs | L1932–1987 | 5 | 5 个 `#[test]`(非 tokio);止于 insert_still_safe 之前 |
| update_memory.rs | L1988–2177 | 7 | 含 insert_memory_still_safe(L1987)+ update_memory×6;止于 build_recall 之前 |
| build_recall_text.rs | L2178–2241 | 3 | 末尾 3 测(用 crate::agent::memory_recall) |

> ✅ **无交错**:与 `tests_agent_loop` 的 mock/error_path 交错不同,本文件 9 簇在源中**严格连续**
> 排列,故每簇用连续区间 `sed -n '<start>,<end>p'` 一次性提取即可,无需逐测试函数抽取。
>
> 注:insert_memory.rs 提取起点 L304(含 input helper),其首个测试属性行在 L329——
> 即 helper(L304–328)位于该簇首测之前,连续区间 L304–567 自然涵盖。

## 执行步骤

### Step 1:建目录 + hub 骨架(主迁移 commit 内,原子)

1. `mkdir -p app/src-tauri/src/db/memories_tests`
2. 写 `memories_tests/mod.rs`(hub),内容见 design §3:
   ```rust
   #![cfg(test)]

   #[allow(unused_imports)]
   use super::memories;   // re-export trick:让簇文件 use super::memories::{...} 不改

   mod fts5_migration;
   mod insert_memory;
   mod list_delete_search;
   mod find_pitfalls;
   mod hitcount_status;
   mod p5_promote;
   mod validate_memory_text;
   mod update_memory;
   mod build_recall_text;

   pub(super) async fn test_pool() -> sqlx::SqlitePool {
       // ← 原 L35–44 body 平移
   }

   pub(super) async fn make_pool() -> sqlx::SqlitePool {
       test_pool().await
   }
   ```
   - `test_pool` body:`sed -n '35,44p'` 提取原文件。
   - helper 用全路径 `sqlx::SqlitePool`(hub 不单独 `use sqlx::SqlitePool`,避免 hub 报 unused)。

### Step 2:逐簇提取(每簇一个文件,均加 `#![cfg(test)]` + 实际 use)

因 9 簇在源中严格连续(见上 ✅),每簇用连续区间 `sed -n '<start>,<end>p'` 提取:

```bash
SRC=app/src-tauri/src/db/memories_tests.rs
DST=app/src-tauri/src/db/memories_tests
sed -n '62,328p'   $SRC > $DST/fts5_migration.rs       # 4 测
sed -n '304,567p'  $SRC > $DST/insert_memory.rs        # 9 测 + input helper
sed -n '568,1213p' $SRC > $DST/list_delete_search.rs   # 8 测
sed -n '1214,1415p'$SRC > $DST/find_pitfalls.rs        # 3 测
sed -n '1416,1708p'$SRC > $DST/hitcount_status.rs      # 4 测
sed -n '1709,1931p'$SRC > $DST/p5_promote.rs           # 6 测 + reseat_created_at helper
sed -n '1932,1987p'$SRC > $DST/validate_memory_text.rs # 5 测
sed -n '1988,2177p'$SRC > $DST/update_memory.rs        # 7 测
sed -n '2178,2241p'$SRC > $DST/build_recall_text.rs    # 3 测
```

每个簇文件**前置** `#![cfg(test)]` + 该簇用到的 `use`:
- `#![cfg(test)]`(文件级,跟随原单文件惯例 + tests_agent_loop 先例)
- `use super::memories::{...}`(该簇实际符号,见 design §4 表;先放完整块,clippy 后删 unused)
- `use super::{test_pool, make_pool};` 或 `use super::make_pool;`(凡用 make_pool 的簇)
- `use sqlx::SqlitePool;`(仅 p5_promote.rs,因 reseat_created_at 签名用;其余簇不需)
- build_recall_text.rs:用 `use crate::agent::memory_recall::build_recall_text_with_rows;`(原测内 inline use 可保留或上提,以 clippy 为准)

> **`#![cfg(test)]` 必加**:原单文件 L1 是文件级 `#![cfg(test)]`,提取出的区间不含它,
> 每个簇文件必须自行声明,否则非 test build 会编译测试代码。

### Step 3:删源(主迁移 commit 内,原子)

- `rm app/src-tauri/src/db/memories_tests.rs`
- **`db/mod.rs` 不动**(L60 `pub mod memories_tests;` 原样)

### Step 4:编译收敛(主迁移 commit 内)

```bash
cargo test --lib --no-run 2>&1 | grep -E "error|warning: unused" | head -40
```

- `cannot find ... in this scope` / `unresolved import` → 该簇漏 use 或 re-export 漏写
  (检查 hub `use super::memories;` 是否就位)
- `unused import` → 逐簇删 use(允许:clippy 驱动的死 import 清理,非逻辑改动)
- `private function` → helper 可见性:input/reseat_created_at 留簇内私有即可(簇内调用不跨文件);
  test_pool/make_pool 必须 `pub(super)`

### Step 5:测试收敛(主迁移 commit 内)

```bash
cargo test --lib "db::memories_tests::" 2>&1 | tail -5    # 应 49 passed
PKG_CONFIG_PATH="..." cargo test --lib 2>&1 | tail -3      # 全量 1662 基线
```

49 缺一不可(漏搬 → 该测消失 → 计数 <49 → 回查 Step 2 提取区间)。

### Step 6:clippy + fmt(主迁移 commit 内)

```bash
cargo clippy --lib --tests 2>&1 | grep -E "warning|error" | head
cargo fmt --check
```
零警告零 diff。若有 `unused import` 按 Step 4 收敛。

### Step 7:行数核验(AC1)

```bash
wc -l app/src-tauri/src/db/memories_tests/*.rs
```
最大 list_delete_search.rs ~646 < 1200(远达标)。

### Step 8:主迁移 commit

```bash
git add app/src-tauri/src/db/memories_tests/ \
        app/src-tauri/src/db/memories_tests.rs    # 删除
git commit -m "refactor: memories_tests 子模块化(memories_tests.rs → hub + 9 簇文件)"
```

### Step 9:文档 sweep commit(AC6 —— 实测 0 行号引用,本步预期为空)

```bash
# 核验无 memories_tests.rs:LINE 行号引用(应 0 输出)
grep -rn "memories_tests\.rs:[0-9]" --include="*.md" --include="*.rs" . | grep -v "/archive/"
```
若有输出 → 改符号引用(本任务预判 0);若无,跳过本 commit。

### Step 10:收官表同步(收尾 commit)

更新 `.trellis/spec/backend/directory-structure.md` 的"收官状态表":
将 `db/memories_tests.rs 2241 | 08-09-tests-memories-split | ⏳ 规划中` 一行的
现状列改为 `✅ memories_tests/ 目录(hub ~30 + 9 簇);49 测原样,最大 list_delete_search.rs ~646`。

```bash
git add .trellis/spec/backend/directory-structure.md
git commit -m "docs(spec): memories_tests 拆分收官 — directory-structure 收官表同步"
```

## 回滚点

- `git revert <Step 8 commit>` → 回到单文件 2241 行状态
- Step 9 若做了 sweep,独立 `git revert <Step 9 commit>`
- Step 10 收官表同步独立 `git revert <Step 10 commit>`

## 风险清单

| 风险 | 触发 | 缓解 |
|---|---|---|
| re-export 漏写 | 簇文件 `use super::memories::{...}` 全报 cannot find | hub 必须有 `#[allow(unused_imports)] use super::memories;`(design §3) |
| 漏搬一个测试 | Step 5 计数 <49 | 计数硬闸,逐簇过滤定位 |
| 簇文件漏 `#![cfg(test)]` | 非 test build 编译测试代码 | 每簇文件头必加(Step 2 模板) |
| helper 可见性不够 | 编译报 private | test_pool/make_pool 必须 pub(super);input/reseat_created_at 留簇内私有 |
| unused import 残留 | clippy warning | 依赖编译器逐簇收敛(design §4) |
| db/mod.rs 误改 | AC3 失败 | `git diff app/src-tauri/src/db/mod.rs` 空是硬闸 |
