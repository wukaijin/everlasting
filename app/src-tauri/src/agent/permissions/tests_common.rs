//! Shared test fixtures for the `permissions` test suite (split
//! out of `mod.rs` on 2026-06-23). The 7 domain test files
//! (`tests_types` / `tests_store` / `tests_payload` / `tests_mode`
//! / `tests_audit` / `tests_check` / `tests_ask`) reach these
//! via `use super::tests_common::*`.

#![cfg(test)]

use std::sync::Mutex as StdMutex;

use crate::db::Mode;
use crate::state::{ChatEventPayload, ToolCallPayload, ToolResultPayload};

use super::store::{new_permission_store, PermissionStore};
use super::types::PermissionContext;
use super::PermissionAskPayload;

/// Local `test_pool` — same shape as `db::tests::test_pool`
/// (in-memory SQLite + migrations) but declared here so the
/// permissions test module is self-contained (the `db::tests`
/// module's `test_pool` is private).
pub(super) async fn worker_test_pool() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    crate::db::migrations::run_migrations(&pool).await.unwrap();
    // Insert a parent session row so the audit FK is satisfied.
    // `sessions` table is the audit table's parent — without
    // this insert, the audit INSERT in `record_audit` fails
    // silently (record_audit swallows errors per its docstring).
    sqlx::query(
        r#"
        INSERT INTO sessions (id, title, created_at, updated_at, model, metadata,
        current_cwd, worktree_path, worktree_state, mode)
        VALUES ('parent-sess', 'Test', datetime('now'), datetime('now'),
        '', NULL, '/repo', NULL, 'none', 'edit')
        "#,
    )
    .execute(&pool)
    .await
    .expect("insert parent session for test");
    pool
}

/// Minimal sink capturing `emit_permission_ask` payloads for
/// assertions. The other 3 trait methods are not exercised by
/// `ask_path` — they get no-op impls.
#[derive(Default)]
pub(super) struct CaptureAskSink {
    pub(super) asks: StdMutex<Vec<PermissionAskPayload>>,
}
impl crate::state::ChatEventSink for CaptureAskSink {
    fn emit_chat_event(&self, _p: &ChatEventPayload) {}
    fn emit_tool_call(&self, _p: &ToolCallPayload) {}
    fn emit_tool_result(&self, _p: &ToolResultPayload) {}
    fn emit_permission_ask(&self, p: PermissionAskPayload) {
        self.asks.lock().unwrap().push(p);
    }
}

/// Build a worker `PermissionContext` pointing at a fresh test
/// DB. The test owns the DB / store so it can introspect the
/// audit table + the pending-asks map.
pub(super) async fn worker_ctx_with_db() -> (
    sqlx::SqlitePool,
    PermissionStore,
    std::sync::Arc<CaptureAskSink>,
    PermissionContext,
    tokio_util::sync::CancellationToken,
) {
    let pool = worker_test_pool().await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(CaptureAskSink::default());
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: Mode::Edit,
        cwd: std::path::PathBuf::from("/repo"),
        is_worker: true,
        worker_run_id: Some("worker-run-1".to_string()),
        // 2026-06-26 (task 06-26-subagent-per-run-grant): None by
        // default for tests that don't exercise the run-grant cache
        // (the existing worker-ask tests cover the collapse-to-Allow /
        // collapse-to-Deny paths). Tests that DO exercise the cache
        // construct their own PermissionContext with Some(Arc<...>).
        run_grants: None,
        worktree_path: std::path::PathBuf::from("/repo"),
    };
    (pool, store, sink, ctx, tokio_util::sync::CancellationToken::new())
}
