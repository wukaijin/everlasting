#![cfg(test)]

// Re-export the 6 `db::` siblings that the pre-split single file imported via
// `use super::{migrations, models, projects, providers, sessions, types}`. After
// splitting, a cluster file's `super` points at this hub (`sessions_tests`), so
// `use super::sessions::{...}` would otherwise resolve to
// `sessions_tests::sessions` (which doesn't exist). Re-importing each sibling at
// the hub makes `super::<sibling>` resolve through the hub to the real
// `db::<sibling>` — call sites unchanged (pure relocation).
#[allow(unused_imports)]
use super::{migrations, models, projects, providers, sessions, types};

mod fields_worktree;
mod latency_message;
mod model_usage;
mod session_crud;
mod system_events;

/// In-memory pool with migrations + FK pragma. Mirrors the `test_pool` in every
/// other `db/*_tests.rs` file (project convention: no shared `common` module;
/// each domain copies the helper so test files stay independent).
pub(super) async fn test_pool() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    // Mirror what `init_pool` does.
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    migrations::run_migrations(&pool).await.unwrap();
    pool
}

pub(super) async fn make_pool() -> sqlx::SqlitePool {
    test_pool().await // alias for readability inside this section
}
