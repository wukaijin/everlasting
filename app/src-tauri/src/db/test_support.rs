//! Shared in-memory test pool (RULE-TESTPOOL-001 dedup).
//!
//! One copy of the common fixture shape every `db/*_tests.rs` file (and
//! the out-of-`db` test hosts) previously hand-copied: connect an
//! in-memory SQLite pool, mirror `init_pool`'s `PRAGMA foreign_keys =
//! ON`, then run the idempotent schema migrations. Hosts that need
//! extra setup keep their local variant next to their call sites.

use sqlx::SqlitePool;

use super::migrations::run_migrations;

/// Build a fresh in-memory pool with foreign keys ON + migrations
/// applied.
pub(crate) async fn test_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    // Mirror what `init_pool` does.
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    pool
}
