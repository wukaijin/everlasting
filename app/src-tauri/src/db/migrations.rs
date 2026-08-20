//! Database pool init + idempotent schema migrations.
//!
//! Split 2026-08-08 batch3: this hub re-exports the surface of the
//! `migrations/` directory module. [`init_pool`] (pool pragmas) lives
//! in [`pool`]; the idempotent [`run_migrations`] sequence lives in
//! [`schema`] with its one-shot data migrations in
//! [`schema_helpers`] and the column-probe helpers in [`columns`].
//!
//! Callers reach the two entry points via `db::init_pool` /
//! `db::run_migrations` (the `db/mod.rs` `pub use migrations::*`
//! re-export keeps both the bare and the `crate::db::migrations::*`
//! paths working unchanged).

pub mod columns;
pub mod pool;
pub mod schema;
pub mod schema_helpers;

#[allow(unused_imports)]
pub use pool::init_pool;
#[allow(unused_imports)]
pub use schema::run_migrations;

// The column-probe + one-shot helpers are `pub(crate)` and consumed by
// `schema::run_migrations` via explicit `super::` imports; re-export
// them here so the historical `pub use migrations::*` in `db/mod.rs`
// keeps resolving them for any test-fixture / doc references.
#[allow(unused_imports)]
pub(crate) use columns::{
    add_autonomous_memories_column_if_missing, add_messages_column_if_missing,
    add_project_column_if_missing, add_provider_column_if_missing,
    add_session_audit_events_column_if_missing, add_session_column_if_missing,
    add_subagent_runs_column_if_missing,
};
#[allow(unused_imports)]
pub(crate) use schema_helpers::{
    migrate_provider_api_keys_to_encrypted, rebuild_turn_trace_with_run_id,
    widen_subagent_runs_status_check_for_incomplete,
};
