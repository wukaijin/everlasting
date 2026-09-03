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
// db/backup.rs(F3 磁盘治理 2026-09-03 起 prune 为预算自适应)
pub const KEEP_BACKUPS: usize = 7;            // 份数硬顶(原固定份数语义并入上限)
pub const BACKUP_BUDGET_BYTES: u64 = 200MiB;  // env EVERLASTING_BACKUP_BUDGET_MB 覆盖
pub const MIN_KEEP_BACKUPS: usize = 2;        // 超预算也至少保留的恢复点份数
pub struct PruneOutcome { pub removed: Vec<PathBuf>, pub reclaimed_bytes: u64 }
pub fn backup_dir(data_dir: &Path) -> PathBuf;   // data_dir/backups/
pub async fn backup_database(db: &SqlitePool, dir: &Path) -> std::io::Result<PathBuf>;
pub fn prune_backups(dir: &Path, keep: usize) -> std::io::Result<PruneOutcome>;

// daemon/server.rs — daemon 启动即备份一次,之后每 24h;失败仅 warn 等下一周期
pub fn spawn_backup_task(state: &Arc<AppState>, data_dir: &Path);
```

**Contracts**:

- 备份文件名 `everlasting-YYYYMMDD-HHMMSS.db`(本地时间),同秒已存在
  取 `-2`/`-3` 后缀(上限 999 防死循环)。**文件名字典序 = 时间序**,
  prune 靠这个删最旧 —— 改名格式必须保持可排序。
- **保留策略(F3 磁盘治理 2026-09-03 起,`prune_backups` 预算自适应)**:
  从新到旧保留,累计字节数超 `BACKUP_BUDGET_BYTES`(200 MiB,env
  `EVERLASTING_BACKUP_BUDGET_MB` 覆盖)即停;**恰好等于预算不停**
  (严格 `>` 才停);至少 `MIN_KEEP_BACKUPS`(2)份(超预算也保留),
  至多 `KEEP_BACKUPS`(7)份。小备份场景预算不触发,份数语义与旧行为
  兼容。回收摘要(`PruneOutcome`)供 daemon 备份 task 日志与磁盘
  governor / `run_disk_cleanup` IPC 双消费,契约细节见
  [disk-governance](disk-governance.md)。
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
`backup_dir_with_single_quote_in_path`;F3 预算自适应(2026-09-03):
`prune_keeps_all_when_within_budget` / `prune_drops_oldest_beyond_budget_
but_keeps_two` / `prune_never_drops_below_two_even_over_budget` /
`prune_stops_when_next_file_exceeds_budget`(预算边界:恰好等于不停)/
`backup_budget_env_resolution`。

### Scenario: messages.status 检查点列 + 启动恢复(RULE-PERSIST-001 闭合,2026-08-24)

**Scope/Trigger**: 触碰 turn 持久化路径(messages 写点)、给 messages
加列、或想新增"崩溃残留状态"类生命周期列的改动。

**Signatures**:

```rust
// db/sessions/messages.rs
pub async fn upsert_in_progress_turn(pool, session_id, seq, blocks: &[ContentBlock], speaker) -> Result<()>
pub async fn finalize_turn_persist(pool, ..同 persist_turn 签名..) -> Result<()>  // ON CONFLICT DO UPDATE + status=NULL
pub async fn delete_in_progress_turn(pool, session_id, seq) -> Result<u64>         // WHERE status='in_progress' 守卫
pub async fn recover_interrupted_messages(pool) -> Result<RecoveryReport>          // { interrupted, deleted, orphan_repaired }
```

**Contracts**:

- `messages.status` 取值域:`NULL`=终态(全部存量行)/`'in_progress'`=流式
  检查点 /`'interrupted'`=崩溃恢复过。**不加 CHECK**(存量表加 CHECK 要表
  重建,收益低;写入点保证取值)。部分索引 `idx_messages_status ... WHERE
  status IS NOT NULL`,启动扫描零成本。
- 检查点 upsert 的 `DO UPDATE` 列含 `text` → `messages_fts_update`
  (AFTER UPDATE OF text)随发,≤1次/s/流式 session,docsize 与索引实测
  lockstep —— 无需额外守卫。
- **`persist_turn` 本体保持裸 INSERT**:UNIQUE(session_id,seq) 冲突是 seq
  漂移 bug 的告警信号(RULE-A-003);upsert 只属于 assistant 落库点(它
  知道自己前面有检查点行)。
- 恢复 pass 在 `state.rs` 启动序列 `reap_orphaned_runs` 之后、backup task
  之前(`load_inner` 内)——首拍 VACUUM INTO 捕获的是恢复后状态,且 HTTP
  handler 尚未 accept,无并发观察者。
- db 层引用 `crate::agent::helpers::INTERRUPTED_MARKER`:db→agent 常量
  级依赖已有先例(memories crud / subagent_runs 的运行时调用),可接受;
  新增时优先参数传入而非扩大该方向。

**Validation & Error Matrix**:

| 条件 | 行为 |
|------|------|
| in_progress 行内容为空(含损坏 JSON) | DELETE |
| in_progress 行有内容 | 追加 INTERRUPTED_MARKER 块(`\n\n` 前缀,独立 Text 块)+ status='interrupted' |
| session 尾行 = assistant 且 has_tool_calls=1(孤儿 tool_use) | 按 seq+1 裸 INSERT 合成 is_error tool_result user 行(每 tool_use id 一块) |
| 恢复中 DB 错误 | best-effort:逐行独立写,失败 log,下次启动重跑(幂等) |
| 干净 DB | no-op,计数 0 |

**Wrong vs Correct**:

```sql
-- Wrong:给全表 persist 都换 upsert —— 真 seq bug 被静默覆盖
INSERT INTO messages (...) VALUES (...) ON CONFLICT(session_id, seq) DO UPDATE ...;

-- Correct:只有 assistant 终态点(前面有检查点行)用 upsert,其余裸 INSERT 保告警语义
```

**Tests**:`db/messages_checkpoint_tests.rs`(file-backed)12 例 +
`agent/tests_agent_loop/turn_checkpoint.rs` 4 例(含 AC4:孤儿修复后第二
请求 provider 实收配对 tool_result)。

### Scenario: 审计事件 keyset 分页读(RULE-PERM-001,2026-08-30)

**Scope/Trigger**:给 append-only、按新→旧消费的表加分页读(`session_audit_events`
→ `db::permissions.rs::list_audit_events_page` + 双 transport 命令
`list_session_audit_events_page`)。核心决策与陷阱对同形态表
(逐条追加、秒级 ts、游标消费)直接可复用。

**决策 — keyset `(ts, id)` 游标,禁 OFFSET**:审计表在弹窗开着时会持续追加
(每 tool call 1–2 行),OFFSET 分页「取页后前移插入」整页位移(重复+跳行);
`ts` 是 `datetime('now')` 秒级精度,同秒多行是常态(单轮多 tool),游标必须带
id 段:`(ts < ? OR (ts = ? AND id < ?))`,且 SQL 显式 `ORDER BY ts DESC, id DESC`
(索引只盖 ts 段;id tie-break 在过滤后子集排序,页级量可接受),前端不许重排。

**Trap 1 — 部分游标**。`before_ts` 不带 `before_id` 会静默跳过游标自身那一秒
的剩余行。三层拒:`db` 层 `sqlx::Error::InvalidArgument` → command `_inner`
`InvalidRequest`(daemon 侧 400 非 500)→ 前端恒双发。

**Trap 2 — SQLite `LIMIT` 负数 = 无限**。裸 cap(`min(limit, 500)`)挡不住
`limit = -3` 全量拉表。必须 clamp 到 `1..=MAX`(此处 `DEFAULT 100 / MAX 500`)。

**Trap 3 — `json_extract` 对畸形 JSON 直接 raise**。服务端化「仅 critical」
(`payload_json.$.critical = 1`)时,历史行的 NULL/畸形 payload 会把整个查询
打炸。守卫:`json_valid(payload_json) AND json_extract(...) = 1`——三处都要
(页谓词、matched COUNT、`COUNT(*) FILTER` 的 total_critical),共享同一
filter SQL 片段防漂移。

**计数随页返回**(一次调用喂列表 + 计数 chip):`matched`(当前过滤命中,
游标不参与)、`total_all`(不过滤)、`total_critical`(不受 kind 过滤影响,
`COUNT(*) FILTER (WHERE json_valid AND json_extract...)`,SQLite ≥ 3.30)。
前端 `hasMore = events.length < matched` 派生,不搞 limit+1 探针。

**Wire 形状**:`AuditEventPageRow` camelCase(`events/matched/totalAll/
totalCritical`);旧全量命令 `list_session_audit_events` 零改动(traceStore
按 turnSeq 分组语义上需要全量行——「读端要全集」的场景别顺手改老命令)。

**Tests**:`db/permissions_tests.rs` 8 例(tie-break / 取页后插入新行不重不漏
的 keyset 行为测试 / limit clamp / kind 子集 / critical 四态含畸形 JSON 不
error / 三计数精确 / 空 session 零页 / camelCase wire 锁)+
`daemon/routes/permissions.rs` 路由 oneshot(snake_case body → camelCase 页、
critical 下推、部分游标 400)。
