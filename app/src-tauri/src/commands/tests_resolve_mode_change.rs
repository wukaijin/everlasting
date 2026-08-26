//! Phase E3 (07-07-request-mode-change-tool) — `resolve_mode_change`
//! IPC handler behavior tests.
//!
//! These tests verify the `resolve_mode_change` Tauri command (the
//! LLM-driven `request_mode_change` tool's IPC resolver — frontend
//! invokes on `允许` / `拒绝` of the `<RequestModeChangeCard>`).
//!
//! ## Why we test the internal function, not the IPC handler
//!
//! `resolve_mode_change` is a `#[tauri::command]` taking
//! `State<'_, Arc<AppState>>`. We can't easily call it without
//! `tauri::test::mock_app`, which this project doesn't use (per
//! existing precedent: `permission_response` and other IPCs have
//! only manual `tauri dev` smoke tests, not unit tests). The clean
//! approach (mirroring how `set_session_mode` was refactored into
//! `set_session_mode_internal`) is to extract the core logic into
//! a `pub(crate)` pure function and have the IPC wrapper be a thin
//! shell. That's exactly what `resolve_mode_change_internal` is —
//! we test it directly here.
//!
//! ## Coverage
//!
//! | Test | Verifies |
//! |---|---|
//! | `resolve_mode_change_internal_allow_plan_updates_mode_and_writes_allowed_audit` | AC4 partial: `allow=true` + target="plan" → DB mode flips to plan + `mode_change_allowed` audit + `mode_changed` audit + returns SessionRow |
//! | `resolve_mode_change_internal_allow_yolo_non_root_updates_mode` | AC8' partial: `allow=true` + target="yolo" + non-root → DB mode flips to yolo + `mode_change_allowed` + `yolo_entered` audits + returns SessionRow |
//! | `resolve_mode_change_internal_allow_yolo_root_denies` | AC8': when `is_running_as_root()` is true, `allow=true` + target="yolo" → DB mode NOT updated + `mode_change_denied{reason: "yolo_root_guard"}` audit + returns Err("Cannot enable Yolo as root") |
//! | `resolve_mode_change_internal_deny_writes_denied_audit_and_returns_session_row` | AC partial: `allow=false` → no DB mode change + `mode_change_denied{reason: "user denied"}` audit + returns SessionRow with mode unchanged |
//! | `resolve_mode_change_internal_unknown_session_returns_invalid_request` | `get_session` not found → Err with category=InvalidRequest |
//! | `resolve_mode_change_internal_allow_plan_unregisters_pending` | Helper sanity check: `allow=true` after a `register` clears the pending entry (the agent loop's `tokio::select!` arm fires) |
//! | `resolve_mode_change_internal_deny_unregisters_pending` | Helper sanity check: `allow=false` after a `register` clears the pending entry |
//!
//! The Yolo root guard case is gated on `is_running_as_root()` —
//! when running as non-root (the typical CI / dev case), the test
//! is skipped with `eprintln!` (cargo test captures stderr).

#![cfg(test)]

use crate::agent::permissions::AuditKind;
use crate::agent::question_store::{ModeChangePayload, PendingInteraction, QuestionStore};
use crate::commands::permissions::is_running_as_root;
use crate::commands::question::resolve_mode_change_internal;
use crate::db;
use crate::db::test_support::test_pool;
use crate::error::ErrorCategory;
use sqlx::SqlitePool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a project + session in the DB so audit / mode writes
/// have a valid FK target. Returns `(project_id, session_id)`.
async fn seed_session(pool: &SqlitePool) -> (String, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let project_path = dir.path().to_path_buf();
    let project_id = format!("proj-{}", Uuid::new_v4());
    db::create_project(
        pool,
        &project_id,
        project_path.to_str().unwrap(),
        false,
        None,
    )
    .await
    .expect("create_project");
    let session_id = Uuid::new_v4().to_string();
    db::create_session(
        pool,
        &session_id,
        &project_id,
        project_path.to_str().unwrap(),
        "mock-model",
        None,
        None,
        None,
    )
    .await
    .expect("create_session");
    (project_id, session_id)
}

/// Pre-register a pending mode change in `store` for `session_id`.
/// Mirrors the `tools/request_mode_change.rs::execute_blocking`
/// flow's register step so the test starts with a "user has just
/// been shown the card" state. Returns the registered payload's
/// `target_mode` so callers can assert against it.
async fn register_pending_mode_change(
    store: &QuestionStore,
    session_id: &str,
    tool_use_id: &str,
    target_mode: &str,
) {
    let payload = ModeChangePayload {
        session_id: session_id.to_string(),
        tool_use_id: tool_use_id.to_string(),
        target_mode: target_mode.to_string(),
        current_mode: Some("edit".to_string()),
        reason: Some("need to write code".to_string()),
        ts: 1_700_000_000_000,
    };
    store
        .register(
            session_id,
            tool_use_id,
            PendingInteraction::ModeChange(payload),
        )
        .await
        .expect("register pending mode change ok");
}

/// Count `session_audit_events` rows matching `(kind, session_id)`
/// — same shape as the assertion in
/// `tests_request_mode_change.rs::agent_loop_request_mode_change_happy_path_records_audit`.
async fn audit_count(pool: &SqlitePool, kind: &str, session_id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_audit_events WHERE kind = ? AND session_id = ?",
    )
    .bind(kind)
    .bind(session_id)
    .fetch_one(pool)
    .await
    .expect("audit count query")
}

// ---------------------------------------------------------------------------
// Test 1 — allow=true + target="plan" (AC4 partial)
//
// Happy path: the user clicked 允许 on a card asking to switch
// from edit to plan. The IPC must:
//   - update `sessions.mode` from edit → plan
//   - write `mode_change_allowed` audit row
//   - write `mode_changed` audit row (via set_session_mode_internal)
//   - resolve the QuestionStore entry so the agent loop unblocks
//   - return the freshly-loaded SessionRow with mode=plan
// ---------------------------------------------------------------------------
#[tokio::test]
async fn resolve_mode_change_internal_allow_plan_updates_mode_and_writes_allowed_audit() {
    let pool = test_pool().await;
    let (_project_id, session_id) = seed_session(&pool).await;
    // Session starts as Edit (create_session default).
    let store = QuestionStore::new();
    register_pending_mode_change(&store, &session_id, "tu_mc", "plan").await;

    let row = resolve_mode_change_internal(&pool, &store, &session_id, "plan", true)
        .await
        .expect("resolve ok");

    assert_eq!(
        row.mode.as_str(),
        "plan",
        "session row.mode flipped to plan"
    );

    // DB side: reload + assert.
    let loaded = db::load_session(&pool, &session_id)
        .await
        .expect("load_session")
        .expect("session present")
        .session;
    assert_eq!(
        loaded.mode.as_str(),
        "plan",
        "DB sessions.mode persisted as plan"
    );

    // Audit side: at least 1 mode_change_allowed row + at least
    // 1 mode_changed row (the latter from
    // set_session_mode_internal).
    let allowed_count =
        audit_count(&pool, AuditKind::ModeChangeAllowed.as_str(), &session_id).await;
    assert_eq!(allowed_count, 1, "exactly 1 mode_change_allowed audit row");

    let mode_changed_count = audit_count(&pool, AuditKind::ModeChanged.as_str(), &session_id).await;
    assert_eq!(
        mode_changed_count, 1,
        "exactly 1 mode_changed audit row (from set_session_mode_internal)"
    );

    // QuestionStore: the pending entry is cleared by resolve so
    // the agent loop's `tokio::select!` arm fires.
    assert!(
        store.get_payload(&session_id).await.is_none(),
        "QuestionStore entry cleared after resolve"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — allow=true + target="yolo" + non-root (AC8' partial)
//
// Same as test 1 but for the Yolo target mode. Non-root CI / dev
// case must succeed; the yolo_entered transition audit is written
// on top of mode_changed (per `set_session_mode_internal`'s
// transition match). Skipped if running as root (covered by
// test 3 instead).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn resolve_mode_change_internal_allow_yolo_non_root_updates_mode() {
    if is_running_as_root() {
        eprintln!(
            "SKIP: resolve_mode_change_internal_allow_yolo_non_root_updates_mode \
             — running as root; see resolve_mode_change_internal_allow_yolo_root_denies"
        );
        return;
    }

    let pool = test_pool().await;
    let (_project_id, session_id) = seed_session(&pool).await;
    let store = QuestionStore::new();
    register_pending_mode_change(&store, &session_id, "tu_mc", "yolo").await;

    let row = resolve_mode_change_internal(&pool, &store, &session_id, "yolo", true)
        .await
        .expect("non-root yolo allow should succeed");

    assert_eq!(row.mode.as_str(), "yolo");

    // yolo_entered audit is written on the prev!=Yolo, new==Yolo
    // transition (set_session_mode_internal).
    let yolo_entered_count = audit_count(&pool, AuditKind::YoloEntered.as_str(), &session_id).await;
    assert_eq!(
        yolo_entered_count, 1,
        "exactly 1 yolo_entered audit row on edit→yolo transition"
    );

    // mode_change_allowed row written by the IPC handler.
    let allowed_count =
        audit_count(&pool, AuditKind::ModeChangeAllowed.as_str(), &session_id).await;
    assert_eq!(allowed_count, 1);
}

// ---------------------------------------------------------------------------
// Test 3 — allow=true + target="yolo" + root (AC8')
//
// When running as root, the Yolo root guard inside
// `set_session_mode_internal` blocks the mode change. The IPC
// must:
//   - NOT update DB mode
//   - write `mode_change_denied{reason: "yolo_root_guard"}` audit
//   - resolve the QuestionStore entry as Cancelled (so the
//     agent loop sees `cancelled_by_user: true`)
//   - return Err("Cannot enable Yolo as root") with
//     category=InvalidRequest
//
// This test is gated on `is_running_as_root()` — skipped when not
// running as root (the typical CI case).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn resolve_mode_change_internal_allow_yolo_root_denies() {
    if !is_running_as_root() {
        eprintln!(
            "SKIP: resolve_mode_change_internal_allow_yolo_root_denies \
             — not running as root (re-run as root to exercise this case)"
        );
        return;
    }

    let pool = test_pool().await;
    let (_project_id, session_id) = seed_session(&pool).await;
    let store = QuestionStore::new();
    register_pending_mode_change(&store, &session_id, "tu_mc", "yolo").await;

    let err = resolve_mode_change_internal(&pool, &store, &session_id, "yolo", true)
        .await
        .expect_err("root yolo allow must fail");
    assert_eq!(err.category, ErrorCategory::InvalidRequest);
    assert_eq!(err.message, "Cannot enable Yolo as root");

    // DB mode unchanged.
    let loaded = db::load_session(&pool, &session_id)
        .await
        .expect("load")
        .expect("present")
        .session;
    assert_eq!(
        loaded.mode.as_str(),
        "edit",
        "DB sessions.mode stayed at edit (root guard blocked)"
    );

    // mode_change_denied audit with reason="yolo_root_guard".
    let denied_count = audit_count(&pool, AuditKind::ModeChangeDenied.as_str(), &session_id).await;
    assert_eq!(
        denied_count, 1,
        "exactly 1 mode_change_denied audit row on root guard"
    );

    // Store cleaned (Cancelled resolve).
    assert!(
        store.get_payload(&session_id).await.is_none(),
        "QuestionStore entry cleared by Cancelled resolve"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — allow=false → deny path (AC partial)
//
// User clicked 拒绝. The IPC must:
//   - NOT update DB mode
//   - write `mode_change_denied{reason: "user denied"}` audit
//   - resolve the QuestionStore entry as Cancelled
//   - return SessionRow with mode unchanged
//
// `target_mode` doesn't matter on the deny path (the DB write
// is skipped entirely) — we use "plan" for symmetry with test 1.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn resolve_mode_change_internal_deny_writes_denied_audit_and_returns_session_row() {
    let pool = test_pool().await;
    let (_project_id, session_id) = seed_session(&pool).await;
    let store = QuestionStore::new();
    register_pending_mode_change(&store, &session_id, "tu_mc", "plan").await;

    let row = resolve_mode_change_internal(&pool, &store, &session_id, "plan", false)
        .await
        .expect("deny returns Ok(SessionRow)");

    // Mode unchanged (deny path skips DB write).
    assert_eq!(
        row.mode.as_str(),
        "edit",
        "session row.mode stayed at edit on deny path"
    );

    // DB side: confirm persistence.
    let loaded = db::load_session(&pool, &session_id)
        .await
        .expect("load")
        .expect("present")
        .session;
    assert_eq!(loaded.mode.as_str(), "edit");

    // mode_change_denied audit (1 row).
    let denied_count = audit_count(&pool, AuditKind::ModeChangeDenied.as_str(), &session_id).await;
    assert_eq!(
        denied_count, 1,
        "exactly 1 mode_change_denied audit row on user deny"
    );

    // NO mode_change_allowed on the deny path.
    let allowed_count =
        audit_count(&pool, AuditKind::ModeChangeAllowed.as_str(), &session_id).await;
    assert_eq!(
        allowed_count, 0,
        "no mode_change_allowed audit row on deny path"
    );

    // NO mode_changed on the deny path (DB write was skipped).
    let mode_changed_count = audit_count(&pool, AuditKind::ModeChanged.as_str(), &session_id).await;
    assert_eq!(
        mode_changed_count, 0,
        "no mode_changed audit row on deny path"
    );

    // Store cleared by Cancelled resolve.
    assert!(store.get_payload(&session_id).await.is_none());
}

// ---------------------------------------------------------------------------
// Test 5 — unknown session_id → InvalidRequest
//
// A session_id that was never created (or was deleted) hits
// `set_session_mode_internal`'s "session not found" branch,
// which returns `Err(InvalidRequest)`. The IPC handler must
// surface this error.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn resolve_mode_change_internal_unknown_session_returns_invalid_request() {
    let pool = test_pool().await;
    let store = QuestionStore::new();
    // No seed_session — the session doesn't exist.

    let err = resolve_mode_change_internal(&pool, &store, "nonexistent-session-id", "plan", true)
        .await
        .expect_err("unknown session must error");

    assert_eq!(err.category, ErrorCategory::InvalidRequest);
    // The message format from `set_session_mode_internal` is
    // "set_session_mode_internal: session '...' not found".
    assert!(
        err.message.contains("not found"),
        "error message mentions 'not found': {}",
        err.message
    );
}
