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

/// How many snapshot files [`prune_backups`] keeps (D5: 15MB × 7 ≈ 105MB
/// — not worth a config knob; the daemon passes this every cycle).
pub const KEEP_BACKUPS: usize = 7;

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

/// Enforce retention on `dir`: keep the newest `keep` files matching
/// `everlasting-*.db` (zero-padded timestamp names → lexicographic ==
/// chronological), delete the rest, and return the deleted paths. Files
/// not matching the pattern are never touched.
pub fn prune_backups(dir: &Path, keep: usize) -> io::Result<Vec<PathBuf>> {
    let mut backups: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("everlasting-") && n.ends_with(".db"))
        })
        .collect();
    backups.sort();

    let excess = backups.len().saturating_sub(keep);
    let mut removed = Vec::with_capacity(excess);
    for path in backups.into_iter().take(excess) {
        std::fs::remove_file(&path)?;
        removed.push(path);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::{backup_database, backup_dir, prune_backups};

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
    /// (lexicographic == chronological for zero-padded names).
    #[test]
    fn prune_keeps_newest_n() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (1..=9)
            .map(|i| format!("everlasting-20260101-00000{i}.db"))
            .collect();
        for name in &names {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }

        let removed = prune_backups(dir.path(), 7).expect("prune");
        let removed_names: Vec<String> = removed
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(removed.len(), 2, "9 backups / keep 7 → drop exactly 2");
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

        let removed = prune_backups(dir.path(), 7).expect("prune");
        assert_eq!(removed.len(), 2, "only the 2 oldest backups go");
        for name in foreign {
            assert!(
                dir.path().join(name).exists(),
                "prune must not touch non-backup file {name}"
            );
        }
    }
}
