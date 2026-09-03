//! RULE-DB-001 (P1, 2026-08-24 task `08-24-p1-db-backup-log-rotation`) —
//! full-store snapshot backups via SQLite `VACUUM INTO`.
//!
//! All durable data (sessions / messages / autonomous memories / audit /
//! token usage) lives in a single `everlasting.db`; before this module a
//! bad disk, an accidental delete, or a migration gone wrong meant total
//! data loss. The daemon now snapshots the store into
//! `<data_dir>/backups/everlasting-YYYYMMDD-HHMMSS.db` (startup + every
//! 24h — orchestration lives in `bin/everlasting-daemon.rs`, the GUI
//! Full mode deliberately stays backup-free).
//!
//! Key decisions (see the task's design.md):
//! - **`VACUUM INTO` over page-copy / `sqlite3 .backup`**: usable through
//!   plain sqlx, online-safe under WAL concurrent writes (runs as a read
//!   transaction), and produces a compacted copy. Cannot run inside a
//!   transaction, so we execute directly on the pool — never wrap this
//!   call in `begin()`.
//! - **Backup dir follows `data_dir`** (not `~/.local/state`): the sidecar
//!   passes the GUI `app_data_dir`, keeping backups on the same root as
//!   the DB itself — the `--data-dir` path-consistency invariant holds
//!   unchanged.
//! - **Timestamp filenames, no locking**: lexicographic order equals
//!   chronological order (zero-padded `%Y%m%d-%H%M%S`), which is what
//!   [`prune_backups`] relies on. Same-second collisions only happen in
//!   tests and get a `-2`/`-3`… suffix.
//!
//! All failures are the caller's to swallow: the daemon backup loop only
//! `warn!`s and waits for the next cycle (the backup is an insurance
//! layer and must never become an availability risk).

use std::io;
use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

/// How many snapshot files [`prune_backups`] keeps at MOST (D5 origin:
/// 15MB × 7 ≈ 105MB). F3 磁盘治理(2026-09-03, task
/// `09-03-f3-disk-governance`)把固定份数降级为**上限**:实际保留份数
/// 由 [`BACKUP_BUDGET_BYTES`] 预算自适应决定(见 [`prune_backups`]);
/// 本常量仍是份数硬顶,语义并入预算判定。
pub const KEEP_BACKUPS: usize = 7;

/// 备份总量目标预算(F3 design §2.4):从新到旧保留,累计字节数超本
/// 预算即停。单份 DB 涨大后保留份数自适应下降(2026-09-03 摸底:26M×7
/// = 182M 仍在预算内,行为不变)。
pub const BACKUP_BUDGET_BYTES: u64 = 200 * 1024 * 1024;

/// [`BACKUP_BUDGET_BYTES`] 的 env 覆盖(单位 **MB**;沿
/// `resolve_cleanup_period_days` 先例:0 / 垃圾值视为未设)。
pub const BACKUP_BUDGET_ENV: &str = "EVERLASTING_BACKUP_BUDGET_MB";

/// 预算超限时仍至少保留的份数(哪怕单份就超预算,也保住最近两份
/// 恢复点——AC5「始终保留最近 2 份」)。
pub const MIN_KEEP_BACKUPS: usize = 2;

/// [`prune_backups`] 的结果:被删文件列表 + 回收字节数(F3:磁盘治理
/// 节拍日志与 PR3 IPC 回收摘要消费)。
#[derive(Debug, Default)]
pub struct PruneOutcome {
    pub removed: Vec<PathBuf>,
    pub reclaimed_bytes: u64,
}

/// 预算解析纯函数核心(`budget_from_env_str` 的单测锚点,避免并行
/// 测试下 set_var 竞态)。`Some(正整数 MB)` → 换算字节;缺失 / 0 /
/// 解析失败 → [`BACKUP_BUDGET_BYTES`] 缺省。
fn budget_from_env_str(v: Option<&str>) -> u64 {
    v.and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|mb| *mb > 0)
        .map(|mb| mb * 1024 * 1024)
        .unwrap_or(BACKUP_BUDGET_BYTES)
}

/// 运行期预算解析(env 覆盖 → 缺省常量)。
pub fn resolve_backup_budget_bytes() -> u64 {
    budget_from_env_str(std::env::var(BACKUP_BUDGET_ENV).ok().as_deref())
}

/// Backup directory for a data dir: `<data_dir>/backups`. Sits next to
/// `everlasting.db` (same filesystem as the store itself).
pub fn backup_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

/// Snapshot `db` into `dir` via `VACUUM INTO` and return the new file's
/// path. Creates `dir` if missing. Target filename is
/// `everlasting-{local now as %Y%m%d-%H%M%S}.db`; when that name already
/// exists (same-second collision, AC2) a `-2`/`-3`… suffix is tried —
/// `VACUUM INTO` refuses to overwrite an existing file, so the probe
/// loop also guarantees we never clobber a fresh backup.
pub async fn backup_database(db: &SqlitePool, dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut path = dir.join(format!("everlasting-{stamp}.db"));
    let mut suffix = 2u32;
    while path.exists() {
        // Bound the retry so a pathological dir (thousands of same-second
        // files) surfaces as an error instead of spinning forever.
        if suffix > 999 {
            return Err(io::Error::other(format!(
                "backup dir {} exhausted same-second filename suffixes",
                dir.display()
            )));
        }
        path = dir.join(format!("everlasting-{stamp}-{suffix}.db"));
        suffix += 1;
    }

    // `VACUUM INTO` cannot run inside a transaction — execute directly
    // on the pool (sqlx never wraps a bare statement in one). The path is
    // built from our own timestamp, never user input, but still escape
    // single quotes defensively: a `'` in the dir path would otherwise
    // break out of the SQL string literal. `raw_sql` (not `query`)
    // because VACUUM is a utility statement, not row-returning DML.
    // Quirk (empirical, 2026-08-24): on an in-memory (`:memory:`) pool
    // sqlx silently drops VACUUM INTO — every executor form reports
    // success and no file appears (plain sqlite3 handles it fine).
    // Production always calls this with the daemon's file-backed pool,
    // where all forms work; the tests below mirror that with `init_pool`.
    let target = path.to_string_lossy().replace('\'', "''");
    sqlx::raw_sql(&format!("VACUUM INTO '{target}'"))
        .execute(db)
        .await
        .map_err(|e| io::Error::other(format!("VACUUM INTO {} failed: {e}", path.display())))?;
    Ok(path)
}

/// Enforce retention on `dir` — F3 磁盘治理(2026-09-03)起为**大小预算
/// 自适应**:在 `everlasting-*.db`(零填充时间戳名,字典序 == 时间序)
/// 中**从新到旧**保留,累计字节数超过 [`BACKUP_BUDGET_BYTES`] 即停;
/// 但**至少保留 [`MIN_KEEP_BACKUPS`] 份**(哪怕超预算),**至多保留
/// `keep` 份**(原固定份数语义并入为上限,现有调用方传
/// [`KEEP_BACKUPS`] 行为兼容:小备份场景预算不触发,份数语义不变)。
/// 预算可通过 [`BACKUP_BUDGET_ENV`] 覆盖。超出保留窗口的最旧份被删,
/// 结果携带被删列表 + 回收字节数。不匹配模式的文件永不触碰。
pub fn prune_backups(dir: &Path, keep: usize) -> io::Result<PruneOutcome> {
    let mut backups: Vec<(PathBuf, u64)> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("everlasting-") && n.ends_with(".db"))
        })
        // 表观大小即可(spawn 场景不存在;若未来有硬链/稀疏备份,按
        // du 口径估计本就是回收摘要语义)。
        .map(|p| {
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            (p, size)
        })
        .collect();
    backups.sort();

    // 从新到旧(倒序)决定保留份数:预算内继续留,超了停;前
    // MIN_KEEP_BACKUPS 份无条件留(与调用方上限取小,防 keep<2 的
    // 调用被 min-keep 反超)。停下的那一刻,更旧的全部进入删除集。
    let budget = resolve_backup_budget_bytes();
    let min_keep = MIN_KEEP_BACKUPS.min(keep);
    let mut keep_count = 0usize;
    let mut cumulative: u64 = 0;
    for (_, size) in backups.iter().rev() {
        if keep_count >= keep {
            break;
        }
        if keep_count >= min_keep && cumulative.saturating_add(*size) > budget {
            break;
        }
        cumulative = cumulative.saturating_add(*size);
        keep_count += 1;
    }

    let excess = backups.len().saturating_sub(keep_count);
    let mut outcome = PruneOutcome {
        removed: Vec::with_capacity(excess),
        reclaimed_bytes: 0,
    };
    for (path, size) in backups.into_iter().take(excess) {
        std::fs::remove_file(&path)?;
        outcome.reclaimed_bytes += size;
        outcome.removed.push(path);
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::{
        backup_database, backup_dir, budget_from_env_str, prune_backups, BACKUP_BUDGET_BYTES,
        KEEP_BACKUPS,
    };

    use std::path::Path;

    use sqlx::SqlitePool;

    /// File-backed pool with the production config (`init_pool`: WAL +
    /// busy_timeout + FK pragmas) + full migrations, rooted at
    /// `<data_dir>/everlasting.db` just like the daemon.
    ///
    /// Deliberately NOT the in-memory `test_pool` convention here:
    /// sqlx silently drops `VACUUM INTO` on a `:memory:` pool (see the
    /// comment in [`backup_database`]), while production always backs
    /// up the file-backed daemon pool — so the tests must mirror the
    /// file-backed shape for the snapshot path to be exercised at all.
    async fn file_backed_pool(data_dir: &Path) -> SqlitePool {
        let pool = crate::db::migrations::init_pool(&data_dir.join("everlasting.db"))
            .await
            .unwrap();
        crate::db::migrations::run_migrations(&pool).await.unwrap();
        pool
    }

    async fn count_sessions(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// AC1: the snapshot opens with an independent pool and carries the
    /// same rows as the source DB.
    #[tokio::test]
    async fn backup_creates_valid_copy() {
        let data_dir = tempfile::tempdir().unwrap();
        let pool = file_backed_pool(data_dir.path()).await;
        for i in 1..=3 {
            sqlx::query(
                "INSERT INTO sessions (id, title, created_at, updated_at, model) \
                 VALUES (?, 'backup probe', '2026-08-24T00:00:00Z', \
                 '2026-08-24T00:00:00Z', 'GLM-4.7')",
            )
            .bind(format!("backup-{i}"))
            .execute(&pool)
            .await
            .unwrap();
        }

        let dir = backup_dir(data_dir.path());
        let path = backup_database(&pool, &dir).await.expect("backup");
        assert!(path.is_file(), "snapshot missing: {}", path.display());
        assert!(
            std::fs::metadata(&path).unwrap().len() > 0,
            "snapshot must be non-empty"
        );

        // Open the copy with a SEPARATE pool (not a handle on the source)
        // and compare row counts — this is the "restorable copy" contract.
        let copy = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=ro", path.display()))
            .await
            .expect("open snapshot copy");
        let source_rows = count_sessions(&pool).await;
        let copy_rows = count_sessions(&copy).await;
        assert_eq!(copy_rows, source_rows);
        assert_eq!(copy_rows, 3, "all 3 probe sessions must survive");
    }

    /// AC2: same-second collisions produce distinct sibling files — the
    /// retry suffix kicks in and neither backup is overwritten. The
    /// collision branch is forced deterministically by pre-creating the
    /// exact filename `backup_database` would pick right now.
    #[tokio::test]
    async fn backup_same_second_collision() {
        let data_dir = tempfile::tempdir().unwrap();
        let pool = file_backed_pool(data_dir.path()).await;

        let dir = backup_dir(data_dir.path());
        let first = backup_database(&pool, &dir).await.expect("first backup");
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let blocked = dir.join(format!("everlasting-{stamp}.db"));
        if !blocked.exists() {
            std::fs::write(&blocked, b"").unwrap();
        }
        let second = backup_database(&pool, &dir).await.expect("second backup");

        assert_ne!(
            first, second,
            "second backup must not reuse the first one's path"
        );
        assert!(first.is_file() && second.is_file());
        assert!(std::fs::metadata(&first).unwrap().len() > 0);
        assert!(std::fs::metadata(&second).unwrap().len() > 0);
    }

    /// AC3: 9 pre-made backups with keep=7 → exactly the 2 oldest deleted
    /// (lexicographic == chronological for zero-padded names). F3 后这些
    /// 小文件远低于预算,份数上限语义不变。
    #[test]
    fn prune_keeps_newest_n() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (1..=9)
            .map(|i| format!("everlasting-20260101-00000{i}.db"))
            .collect();
        for name in &names {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }

        let outcome = prune_backups(dir.path(), 7).expect("prune");
        let removed_names: Vec<String> = outcome
            .removed
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            outcome.removed.len(),
            2,
            "9 backups / keep 7 → drop exactly 2"
        );
        assert_eq!(
            removed_names,
            vec![names[0].clone(), names[1].clone()],
            "must drop the OLDEST two"
        );
        // Survivors: names[2..=8], oldest kept + newest kept verified.
        assert!(dir.path().join(&names[2]).exists());
        assert!(dir.path().join(&names[8]).exists());
        let remaining = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(remaining, 7, "exactly 7 files must remain");
    }

    /// AC4's failure premise: when the backups path is occupied by a
    /// regular file (or otherwise uncreatable), the error is RETURNED —
    /// never a panic. The daemon's backup loop turns this into a
    /// non-fatal `warn!` (R1: backups must never block the daemon).
    #[tokio::test]
    async fn backup_uncreatable_dir_returns_err() {
        let data_dir = tempfile::tempdir().unwrap();
        let pool = file_backed_pool(data_dir.path()).await;
        let dir = backup_dir(data_dir.path());
        std::fs::write(&dir, b"not a dir").unwrap();

        let err = backup_database(&pool, &dir)
            .await
            .expect_err("create_dir_all over a regular file must fail");
        assert!(
            !err.to_string().is_empty(),
            "error must carry a message for the warn! log"
        );
    }

    /// Path safety: Linux filenames may contain `'`, which would break
    /// out of the SQL string literal without the escape. The snapshot
    /// must land at the quoted path itself (and be a real backup file,
    /// not an empty artifact of a mangled statement).
    #[tokio::test]
    async fn backup_dir_with_single_quote_in_path() {
        let data_dir = tempfile::tempdir().unwrap();
        let pool = file_backed_pool(data_dir.path()).await;
        let dir = data_dir.path().join("bac'kups");

        let path = backup_database(&pool, &dir).await.expect("backup");
        assert!(path.is_file(), "snapshot missing: {}", path.display());
        assert!(
            std::fs::metadata(&path).unwrap().len() > 0,
            "snapshot must be non-empty"
        );
    }

    /// The `everlasting-*.db` glob must not swallow unrelated files —
    /// prune only ever deletes files it produced.
    #[test]
    fn prune_ignores_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=9 {
            std::fs::write(
                dir.path().join(format!("everlasting-20260202-00000{i}.db")),
                b"x",
            )
            .unwrap();
        }
        let foreign = [
            "notes.txt",
            "everlasting-readme.md",
            "everlasting.db",
            "other.db",
            "everlasting-stray.db.bak",
        ];
        for name in foreign {
            std::fs::write(dir.path().join(name), b"keep me").unwrap();
        }

        let outcome = prune_backups(dir.path(), 7).expect("prune");
        assert_eq!(outcome.removed.len(), 2, "only the 2 oldest backups go");
        for name in foreign {
            assert!(
                dir.path().join(name).exists(),
                "prune must not touch non-backup file {name}"
            );
        }
    }

    // ---- F3 预算自适应(2026-09-03, task `09-03-f3-disk-governance`)----

    /// 造一份**稀疏**备份文件:`set_len` 只给表观长度不落盘(120MB
    /// 级测试预算文件的零磁盘成本做法;`metadata().len()` 报表观值,
    /// prune 按表观大小判定,与真实 VACUUM INTO 产物同口径)。
    fn write_sparse_backup(dir: &Path, name: &str, len: u64) {
        let f = std::fs::File::create(dir.join(name)).unwrap();
        f.set_len(len).unwrap();
    }

    /// AC5 预算内:5 份小备份 ≤ 7 份上限且总量远低于 200MiB 预算 →
    /// 全留,一份不删。
    #[test]
    fn prune_keeps_all_when_within_budget() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=5 {
            std::fs::write(
                dir.path().join(format!("everlasting-20260303-00000{i}.db")),
                b"x",
            )
            .unwrap();
        }

        let outcome = prune_backups(dir.path(), KEEP_BACKUPS).expect("prune");
        assert!(outcome.removed.is_empty(), "within budget → nothing pruned");
        assert_eq!(outcome.reclaimed_bytes, 0);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            5,
            "all 5 backups must remain"
        );
    }

    /// AC5 超预算:4 份 120MB 备份、预算 200MB → 新到旧累计 120、240
    /// 超预算,但最少保留 2 份 → 恰好保 2 删 2(最旧两份),回收 240MB。
    #[test]
    fn prune_drops_oldest_beyond_budget_but_keeps_two() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (1..=4)
            .map(|i| format!("everlasting-20260404-00000{i}.db"))
            .collect();
        for name in &names {
            write_sparse_backup(dir.path(), name, 120 * 1024 * 1024);
        }

        let outcome = prune_backups(dir.path(), KEEP_BACKUPS).expect("prune");
        assert_eq!(
            outcome.removed.len(),
            2,
            "budget 200MB vs 4×120MB → keep 2, drop 2"
        );
        assert_eq!(
            outcome.reclaimed_bytes,
            240 * 1024 * 1024,
            "reclaimed bytes = the two dropped files' sizes"
        );
        assert!(!dir.path().join(&names[0]).exists());
        assert!(!dir.path().join(&names[1]).exists());
        assert!(
            dir.path().join(&names[2]).exists(),
            "2nd newest kept (min-keep)"
        );
        assert!(dir.path().join(&names[3]).exists(), "newest kept");
    }

    /// AC5 下界:只剩 2 份且总量超预算 → 一份不删(min-keep 优先于
    /// 预算;只剩 1 份同理,由同一条 `keep_count < min_keep` 分支覆盖)。
    #[test]
    fn prune_never_drops_below_two_even_over_budget() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (1..=2)
            .map(|i| format!("everlasting-20260505-00000{i}.db"))
            .collect();
        for name in &names {
            write_sparse_backup(dir.path(), name, 300 * 1024 * 1024);
        }

        let outcome = prune_backups(dir.path(), KEEP_BACKUPS).expect("prune");
        assert!(
            outcome.removed.is_empty(),
            "2 backups (600MB > 200MB budget) must both stay"
        );
        assert!(dir.path().join(&names[0]).exists());
        assert!(dir.path().join(&names[1]).exists());
    }

    /// 预算边界:预算刚好容纳前两份(100+100 ≤ 200MB),第三份会超
    /// (300MB)→ 停,保 2 删 1(与 min-keep 无关的纯预算停点)。
    #[test]
    fn prune_stops_when_next_file_exceeds_budget() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (1..=3)
            .map(|i| format!("everlasting-20260606-00000{i}.db"))
            .collect();
        write_sparse_backup(dir.path(), &names[0], 100 * 1024 * 1024);
        write_sparse_backup(dir.path(), &names[1], 100 * 1024 * 1024);
        write_sparse_backup(dir.path(), &names[2], 100 * 1024 * 1024);

        // 累计:100 ≤ 200 留;100+100 = 200 ≤ 200 留(预算是「超出即停」,
        // 恰好等于不算超);100+100+100 = 300 > 200 停 → 保 2 删 1。
        let outcome = prune_backups(dir.path(), KEEP_BACKUPS).expect("prune");
        assert_eq!(outcome.removed.len(), 1, "exactly the oldest goes");
        assert!(!dir.path().join(&names[0]).exists());
        assert!(dir.path().join(&names[1]).exists());
        assert!(dir.path().join(&names[2]).exists());
    }

    /// env 覆盖解析(纯函数核心,不碰真 env):正整数 MB → 字节;缺失 /
    /// 0 / 垃圾值 → 缺省 200MiB。
    #[test]
    fn backup_budget_env_resolution() {
        assert_eq!(budget_from_env_str(Some("300")), 300 * 1024 * 1024);
        assert_eq!(budget_from_env_str(Some(" 8 ")), 8 * 1024 * 1024);
        assert_eq!(budget_from_env_str(None), BACKUP_BUDGET_BYTES);
        assert_eq!(budget_from_env_str(Some("0")), BACKUP_BUDGET_BYTES);
        assert_eq!(budget_from_env_str(Some("")), BACKUP_BUDGET_BYTES);
        assert_eq!(budget_from_env_str(Some("abc")), BACKUP_BUDGET_BYTES);
        assert_eq!(budget_from_env_str(Some("-5")), BACKUP_BUDGET_BYTES);
    }
}
