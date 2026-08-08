//! SQLite pool init (WAL / busy_timeout / foreign_keys pragmas).
//!
//! Split out of `db/migrations.rs` (2026-08-08 batch3).

use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

/// Open (or create) the SQLite file at `db_path` and return a connection
/// pool. `db_path` is typically `<app_data_dir>/everlasting.db`. Creates
/// the parent directory if missing.
///
/// **PRAGMA configuration (P2.4 D8, 2026-07-22)**:
/// - `journal_mode = WAL` — allows concurrent readers while a writer
///   holds the lock, eliminating `SQLITE_BUSY` in the dual-process
///   scenario (Tauri Thin client + browser hitting the same daemon DB,
///   or multiple HTTP handlers writing concurrently). WAL is the
///   canonical SQLite mode for multi-connection access.
/// - `busy_timeout = 5000ms` — a writer-blocked connection waits up to
///   5s for the lock before returning `SQLITE_BUSY`, masking transient
///   lock contention under load. 5s is generous enough for any
///   sub-second agent-loop write, tight enough to surface a genuine
///   deadlock as an error.
/// - `foreign_keys = ON` — so the `messages` → `sessions` CASCADE
///   actually fires.
///
/// These are set via `SqliteConnectOptions` (per-connection, applied
/// on every pool acquire) rather than a one-shot `PRAGMA` execute, so
/// the settings hold for every connection sqlx opens (the pool grows
/// connections lazily).
pub async fn init_pool(db_path: &Path) -> Result<SqlitePool, sqlx::Error> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            sqlx::Error::Configuration(
                format!("failed to create db parent dir {}: {}", parent.display(), e).into(),
            )
        })?;
    }

    tracing::info!(db_path = %db_path.display(), "opening sqlite pool (WAL + busy_timeout=5s)");

    // P2.4 D8: per-connection pragmas via SqliteConnectOptions. The
    // `mode=rwc` query (read/write/create) is the legacy behavior;
    // `pragma` settings are layered on via the builder.
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let options = SqliteConnectOptions::from_str(&url)?
        // WAL: allows N readers + 1 writer concurrently (vs the
        // default `delete` journal which serializes all access).
        .pragma("journal_mode", "WAL")
        // busy_timeout: wait 5s on lock contention before SQLITE_BUSY.
        .pragma("busy_timeout", "5000")
        // foreign_keys: enforced per-connection (CASCADE on
        // messages → sessions).
        .pragma("foreign_keys", "ON");

    let pool = SqlitePoolOptions::new()
        // Test-on-create so a bad connection surfaces immediately
        // rather than on first query (the pragma init doubles as
        // the connect-time validation).
        .test_before_acquire(false)
        .connect_with(options)
        .await?;

    Ok(pool)
}
