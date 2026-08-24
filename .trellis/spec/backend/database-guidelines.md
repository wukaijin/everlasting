# Database Guidelines

> Database patterns and conventions for this project.

---

## Overview

<!--
Document your project's database conventions here.

Questions to answer:
- What ORM/query library do you use?
- How are migrations managed?
- What are the naming conventions for tables/columns?
- How do you handle transactions?
-->

(To be filled by the team)

---

## Query Patterns

<!-- How should queries be written? Batch operations? -->

(To be filled by the team)

---

## Migrations

<!-- How to create and run migrations -->

(To be filled by the team)

---

## Naming Conventions

<!-- Table names, column names, index names -->

(To be filled by the team)

---

## Common Mistakes

<!-- Database-related mistakes your team has made -->

### Scenario: messages_fts — external-content FTS5 on a live table (D2, 2026-08-17)

**Scope/Trigger**: adding or changing an FTS5 external-content index on a table
that already has production rows (`messages_fts` over `messages.text`,
`db/migrations/schema.rs`; query layer `db/search.rs`). The traps below were
all verified empirically on 2026-08-17 — two of them silently fail (no error,
wrong result), which is worse than a crash.

**Trap 1 — staleness probes that lie**. After creating the vtable on an
existing DB you must backfill, but both "obvious" probes are wrong:

- ❌ `SELECT COUNT(*) FROM messages_fts` — **reads through to the content
  table**. Returns the base row count even when the index is completely empty
  (stale). Verified: 2 content rows + 0 indexed → `COUNT(*)` = 2.
- ❌ `INSERT INTO fts(fts) VALUES('integrity-check')` — passes on a
  never-indexed external-content table (it only checks index-internal
  consistency, not content coverage).
- ✅ `SELECT COUNT(*) FROM messages_fts_docsize` — the `%_docsize` shadow
  table holds exactly one row per **indexed** document (0 for a fresh index, N
  after backfill; empty-`text` rows count too — tool-result-only messages
  produce them and are common). Compare against `COUNT(*) FROM messages` and
  `rebuild` on divergence. This is the guard `run_migrations` uses — first
  boot after upgrade rebuilds once, subsequent boots skip.

**Trap 2 — unqualified `AFTER UPDATE` triggers**. The memories FTS
(`am_fts_update`) uses plain `AFTER UPDATE` — fine there because that table
has no hot non-text write path. `messages` does: `update_message_latency`,
`update_message_metadata`, tool-duration patches. An unqualified update
trigger does an FTS delete+insert pair on every one of those writes.
`messages_fts_update` is `AFTER UPDATE OF text` — keep that qualifier.

**Trap 3 — external-content delete idiom**. `AFTER DELETE` must insert
`VALUES('delete', old.id, old.text)` into the **vtable itself** (with the
table-name first column), not a normal row. Deleting sessions goes through
explicit `DELETE FROM messages` (plus FK cascade as backstop), so the trigger
covers it; AC test locks "deleted session leaves no hits".

**Query dispatch (db/search.rs contract)**: trigram tokenizer needs ≥3
unicode **chars** — shorter queries (2-char Chinese words like "权限") must
route to a `LIKE '%q%' ESCAPE '\'` fallback. `COUNT(*)` lies (trap 1), so
"does the fallback ever fire?" is only answerable from the dispatch code, not
from index counts.

**Tests**: `db/search_tests.rs` — backfill (drop vtable+triggers, re-migrate,
old rows searchable), delete propagation, `UPDATE OF text` red line
(metadata/latency updates don't churn docsize; text rewrite swaps the index),
LIKE-wildcard literalism, per-kind limit semantics.

### Pattern: SQLite 表约束加宽 = 表重建;哨兵空串;显式列清单(2026-08-20,08-20-worker-turn-trace-persist)

**Scope/Trigger**:给已有表的 `UNIQUE` 表约束加一列(如 `turn_trace` 的
`UNIQUE(session_id, seq)` → `UNIQUE(session_id, run_id, seq)`),或任何
SQLite 无法 `ALTER` 的表级约束变更。先例两则:
`widen_subagent_runs_status_check_for_incomplete`(CHECK 加宽)与
`rebuild_turn_trace_with_run_id`(UNIQUE 加宽,`db/migrations/schema_helpers.rs`)。

- **NULL 不能当哨兵**:新维度列若有"无此值"语义,必须 `NOT NULL
  DEFAULT ''` 之类的空串哨兵。SQLite `UNIQUE` 视 NULL 互异 —— NULL 主行
  的 `(sid, NULL, seq)` 冲突**不触发 upsert 冲突子句**而是插入第二行,
  既有 upsert 语义静默破坏。
- **重建拷贝用显式列清单,不抄 `SELECT *`**:widen 先例列集不变才敢
  `SELECT *`;列集一旦变化(新列插中间),位置拷贝每列错位。清单必须
  覆盖全部遗留列 —— 这也决定了 helper 必须排在相关
  `add_*_column_if_missing` 回填**之后**挂进迁移链。
- **幂等探针 + 残留守卫**:`pragma_table_info` 查目标列短路(重跑
  no-op);重建前 `DROP TABLE IF EXISTS <t>_old` 清残留(崩溃遗留的
  old 表会撞 RENAME)。
- **事务包裹五步**(rename → create → copy → drop → index),崩溃不留
  半重建态。不 toggle `PRAGMA foreign_keys`(单进程迁移 + 无外部表引用
  该表时成立,先例论证可平移;多连接测试池的 pragma 污染比理论上的
  FK 窗口更实际)。

### Pattern: compaction_summary 摘要行(C3,2026-08-18)

- **行形态**:普通 `messages` 行,`role='user'`,`metadata.kind =
  "compaction_summary"`(无 migration,复用 B1 的 metadata 列;FTS insert
  trigger 自动索引,D2 可搜)。
- **两列同值契约**:`content` 与 `text` 列**必须同值写纯摘要正文** ——
  wire 对齐锚在 `text` 列(前端 rehydrate 回发 text 列原文),in-context
  折叠从 `content` 重建;两列分叉会让摘要文本漂移。**不要照抄
  `insert_system_event` 的两列分叉先例**(content 带前缀、text 不带)。
  回填前缀话术(`SUMMARY_CONTEXT_PREFIX`)只加在 in-context 构建时,不落库。
- **metadata 字段**:`cutoff_seq`(被压区末行真实 seq,**load-bearing 水位
  折叠点,禁止 seq-1 近似**)/ `preserve_from_seq`(cutoff+1)/ `tokens_before`/
  `tokens_after` / `trigger` / `model`(协议族 Debug 名)/ `prior_summary_seq`/
  `summary_usage`。
- **seq 分配**:摘要行 insert **吃 loop 的 seq 游标、返回推进值**,绝不
  独立 `MAX(seq)+1`(活跃 loop 内会与 loop 后续 persist 撞
  `(session_id, seq)` 主键;`insert_system_event` 的 MAX+1 只在无活跃
  loop 的 IPC 路径安全)。
- **水位语义**:context = `[最新摘要行] + [seq > cutoff_seq 且 kind ≠
  compaction_summary 的行]`;保留区跨请求存活;旧摘要行被增量合并吸收
  (kind 过滤防重复);D3 cascade 删摘要行 → 倒序找现存最新自愈。

### Scenario: DB 快照备份 — VACUUM INTO(RULE-DB-001 闭合,2026-08-24)

**Scope/Trigger**: 任何触碰备份链路(`db/backup.rs` 的
`backup_database` / `prune_backups`,daemon 的 `spawn_backup_task`)或
想在其他路径调用 `VACUUM INTO` 的改动。

**Signatures**:

```rust
// db/backup.rs
pub const KEEP_BACKUPS: usize = 7;
pub fn backup_dir(data_dir: &Path) -> PathBuf;   // data_dir/backups/
pub async fn backup_database(db: &SqlitePool, dir: &Path) -> std::io::Result<PathBuf>;
pub fn prune_backups(dir: &Path, keep: usize) -> std::io::Result<Vec<PathBuf>>;  // 返回被删列表

// daemon/server.rs — daemon 启动即备份一次,之后每 24h;失败仅 warn 等下一周期
pub fn spawn_backup_task(state: &Arc<AppState>, data_dir: &Path);
```

**Contracts**:

- 备份文件名 `everlasting-YYYYMMDD-HHMMSS.db`(本地时间),同秒已存在
  取 `-2`/`-3` 后缀(上限 999 防死循环)。**文件名字典序 = 时间序**,
  prune 靠这个删最旧 —— 改名格式必须保持可排序。
- `VACUUM INTO` 路径是拼接 SQL:路径中的 `'` 必须 `''` 转义(SQLite
  字符串字面量标准;**不认反斜杠转义**)。
- 备份目录跟随 `--data-dir`(sidecar 传 GUI `app_data_dir` 时与 DB
  同根,P2.1 路径一致性),**不要**挪去 state 目录。

> **Warning(gotcha,实测)**: sqlx 在 **`:memory:` pool 上执行
> `VACUUM INTO` 会静默 no-op** —— 所有 executor 形式都返回 `Ok` 但
> 目标文件不出现(sqlite3 CLI 对内存库同操作正常)。file-backed 池一切
> 正常。**涉及 `VACUUM INTO` 的测试必须用 file-backed 临时目录建池**
> (`db/migrations/pool.rs::init_pool` + tempdir),不要用 in-memory
> test pool 模式,否则测了个寂寞。

**Validation & Error Matrix**:

| 条件 | 行为 |
|------|------|
| backups 目录不存在 | `create_dir_all` 自建 |
| 备份失败(磁盘满/目录只读/VACUUM 报错) | `Err` → `warn!`,**不 panic 不阻塞 daemon**,等下一周期 |
| VACUUM 中途进程被杀 | 残留 0 字节文件;下次同秒后缀避让,最终被 prune 按最旧清掉 |
| WAL writer 并发写 | 安全 —— `VACUUM INTO` 是读事务,WAL 读者不阻塞 writer |

**Tests**(`db/backup.rs` 内联,file-backed 池):
`backup_creates_valid_copy`(副本用 `mode=ro` 独立 pool 打开数行数 ==
源库)/ `backup_same_second_collision` / `prune_keeps_newest_n` /
`prune_ignores_foreign_files` / `backup_uncreatable_dir_returns_err` /
`backup_dir_with_single_quote_in_path`。
