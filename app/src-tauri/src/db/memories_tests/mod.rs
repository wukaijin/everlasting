#![cfg(test)]

// Re-export so cluster files can keep their `use super::memories::{...}`
// imports unchanged from the pre-split single-file layout (pure relocation).
// `super::memories` here resolves to `db::memories` (the hub's parent is `db`).
#[allow(unused_imports)]
use super::memories;

mod build_recall_text;
mod find_pitfalls;
mod fts5_migration;
mod hitcount_status;
mod insert_memory;
mod list_delete_search;
mod p5_promote;
mod update_memory;
mod validate_memory_text;

/// In-memory pool with migrations + FK pragma. Mirrors the
/// `test_pool` in every other `db/*_tests.rs` file (project
/// convention: no shared `common` module; each domain copies the
/// helper so test files stay independent).
pub(super) async fn test_pool() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    crate::db::migrations::run_migrations(&pool).await.unwrap();
    pool
}

pub(super) async fn make_pool() -> sqlx::SqlitePool {
    test_pool().await // alias for readability inside this section
}
