#![cfg(test)]

use sqlx::{Row, SqlitePool};

use crate::db::migrations::*;

/// Helper: open a pool against a tempfile path, return (pool, path)
/// so the test can assert pragmas then let both drop.
async fn fresh_pool() -> (SqlitePool, tempfile::TempPath) {
    let file = tempfile::NamedTempFile::new().expect("create tempfile");
    let (file, path) = file.into_parts();
    let pool = init_pool(&path).await.expect("init_pool");
    // Keep the TempPath alive (drops the file on test end); the
    // NamedTempFile's file handle is discarded — sqlite opens its
    // own handle via the path.
    drop(file);
    (pool, path)
}

/// P2.4 D8: `init_pool` must set `journal_mode = WAL` so concurrent
/// readers don't block a writer (eliminates SQLITE_BUSY in the
/// dual-process daemon scenario). The pragma is set per-connection
/// via `SqliteConnectOptions`, so we verify it on an acquired
/// connection.
#[tokio::test]
async fn init_pool_sets_wal_journal_mode() {
    let (pool, _path) = fresh_pool().await;
    let row = sqlx::query("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .expect("query journal_mode");
    let mode: String = row.try_get::<String, _>(0).expect("get journal_mode");
    // SQLite returns "wal" (lowercase) for WAL mode.
    assert_eq!(
        mode.to_lowercase(),
        "wal",
        "init_pool must set journal_mode=WAL (got {mode})"
    );
}

/// P2.4 D8: `init_pool` must set `busy_timeout` so transient lock
/// contention waits instead of immediately returning SQLITE_BUSY.
/// 5000ms is the configured value.
#[tokio::test]
async fn init_pool_sets_busy_timeout() {
    let (pool, _path) = fresh_pool().await;
    let row = sqlx::query("PRAGMA busy_timeout")
        .fetch_one(&pool)
        .await
        .expect("query busy_timeout");
    let timeout: i64 = row.try_get::<i64, _>(0).expect("get busy_timeout");
    assert_eq!(
        timeout, 5000,
        "init_pool must set busy_timeout=5000ms (got {timeout})"
    );
}

/// P2.4 D8: foreign_keys ON is preserved (the pre-P2.4 behavior).
#[tokio::test]
async fn init_pool_sets_foreign_keys_on() {
    let (pool, _path) = fresh_pool().await;
    let row = sqlx::query("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .expect("query foreign_keys");
    let fk: i64 = row.try_get::<i64, _>(0).expect("get foreign_keys");
    assert_eq!(fk, 1, "init_pool must set foreign_keys=ON");
}

/// P2.4 D8: concurrent reads while a write is in-flight must NOT
/// return SQLITE_BUSY under WAL. Two pools on the same file
/// simulates the dual-process scenario (daemon + a second reader).
///
/// Uses `pool.begin()` (NOT raw `BEGIN`) so the transaction is bound
/// to a single pooled connection — raw `BEGIN`/`COMMIT` via
/// `.execute(&pool)` would run on different connections and the txn
/// state wouldn't carry. This is the core dual-process guarantee: a
/// browser reader hitting the daemon's DB while the agent loop is
/// mid-write must not see a busy error.
#[tokio::test]
async fn concurrent_read_during_write_under_wal() {
    let file = tempfile::NamedTempFile::new().expect("create tempfile");
    let (_file, path) = file.into_parts();
    // Two independent pools on the same file (daemon + reader).
    let writer = init_pool(&path).await.expect("writer pool");
    let reader = init_pool(&path).await.expect("reader pool");
    // Set up a table + one row so the reader has something to read.
    sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY)")
        .execute(&writer)
        .await
        .expect("create table");
    sqlx::query("INSERT INTO t (id) VALUES (1)")
        .execute(&writer)
        .await
        .expect("insert");

    // Begin a write transaction bound to ONE writer connection
    // (holds the reserved lock). Held open across the read below.
    let mut txn = writer.begin().await.expect("begin write txn");
    sqlx::query("INSERT INTO t (id) VALUES (2)")
        .execute(&mut *txn)
        .await
        .expect("insert in txn");

    // Concurrent read from the OTHER pool — must NOT error (no
    // SQLITE_BUSY). Under WAL the reader sees the last committed
    // snapshot regardless of the in-flight write.
    let read_result = sqlx::query("SELECT COUNT(*) FROM t")
        .fetch_one(&reader)
        .await;
    assert!(
        read_result.is_ok(),
        "concurrent read under WAL must not return SQLITE_BUSY; got: {:?}",
        read_result.err()
    );
    txn.commit().await.expect("commit");
}
