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

/// In-memory pool with migrations + FK pragma — shared fixture from
/// `db::test_support` (RULE-TESTPOOL-001 dedup). Re-imported here (and
/// not `use`d ad hoc per cluster file) so cluster tests keep their
/// `use super::test_pool;` imports unchanged from the pre-dedup layout.
pub(super) use crate::db::test_support::test_pool;

pub(super) async fn make_pool() -> sqlx::SqlitePool {
    test_pool().await // alias for readability inside this section
}
