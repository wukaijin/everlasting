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

mod compaction_summary;
mod fields_worktree;
mod latency_message;
mod model_usage;
mod session_crud;
mod system_events;

/// In-memory pool with migrations + FK pragma — shared fixture from
/// `db::test_support` (RULE-TESTPOOL-001 dedup). Re-imported here (and
/// not `use`d ad hoc per cluster file) so cluster tests keep their
/// `use super::test_pool;` imports unchanged from the pre-dedup layout.
pub(super) use crate::db::test_support::test_pool;

pub(super) async fn make_pool() -> sqlx::SqlitePool {
    test_pool().await // alias for readability inside this section
}
