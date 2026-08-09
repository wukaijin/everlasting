#![cfg(test)]

use uuid::Uuid;

use crate::projects::DEFAULT_PROJECT_ID;

use super::sessions::{
    create_session, list_sessions, load_session, set_session_plugin_name,
    set_session_workflow_enabled, set_worktree_state, touch_session, update_session_cwd,
};
use super::test_pool;
use super::types::WorktreeState;

#[tokio::test]
async fn touch_session_updates_timestamp() {
    let pool = test_pool().await;
    let session = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let original = session.updated_at.clone();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    touch_session(&pool, &session.id).await.unwrap();
    let reloaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_ne!(reloaded.session.updated_at, original);
}

#[tokio::test]
async fn update_session_cwd_persists() {
    let pool = test_pool().await;
    let session = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp/start",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(session.current_cwd, "/tmp/start");

    update_session_cwd(&pool, &session.id, "/tmp/end")
        .await
        .unwrap();
    let reloaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(reloaded.session.current_cwd, "/tmp/end");
}

// ---------------------------------------------------------------------------
// W1 (Workflow integration, Step 0.2 — 2026-07-08):
// `set_session_workflow_enabled` round-trip — locks the
// per-session opt-in column flip + load_session rehydrate.
// Symmetric to `set_worktree_state` + `update_session_cwd`:
// a plain setter that mirrors `SessionRow.workflow_enabled`
// across the IPC boundary.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_session_workflow_enabled_round_trip() {
    let pool = test_pool().await;
    let session = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // New session: workflow_enabled defaults to false (per the
    // `INTEGER NOT NULL DEFAULT 0` migration probe).
    let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert!(
        !loaded.session.workflow_enabled,
        "new session defaults to workflow_enabled=false"
    );

    // Flip ON.
    set_session_workflow_enabled(&pool, &session.id, true)
        .await
        .unwrap();
    let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert!(loaded.session.workflow_enabled);
    // `list_sessions` rehydrate must also surface the new value
    // — the sidebar / per-session IPC consumer reads from
    // `SessionSummary.workflow_enabled`.
    let listed = list_sessions(&pool, DEFAULT_PROJECT_ID)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == session.id)
        .expect("session in list");
    assert!(listed.workflow_enabled);

    // Flip back OFF.
    set_session_workflow_enabled(&pool, &session.id, false)
        .await
        .unwrap();
    let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert!(!loaded.session.workflow_enabled);
}

#[tokio::test]
async fn set_session_workflow_enabled_on_missing_session_is_noop() {
    // Symmetric to `update_session_model_id_on_missing_session_is_noop`:
    // an unknown `session_id` is a silent no-op so a stale
    // frontend (caching a deleted session's id) doesn't error.
    let pool = test_pool().await;
    set_session_workflow_enabled(&pool, "nonexistent-session-id", true)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// W1 (Workflow integration, Step 2.2 — 2026-07-08):
// `set_session_plugin_name` round-trip — locks the
// per-session plugin_name flip + load_session rehydrate +
// `list_sessions` rehydrate (the `PluginSelect.vue`
// popover reads from `SessionSummary.plugin_name`).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_session_plugin_name_round_trip() {
    let pool = test_pool().await;
    let session = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // New session: plugin_name defaults to "dev" (per the
    // migration `DEFAULT 'dev'` probe).
    let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(
        loaded.session.plugin_name, "dev",
        "new session defaults to plugin_name=\"dev\""
    );

    // Switch to a (hypothetical) review plugin.
    set_session_plugin_name(&pool, &session.id, "review")
        .await
        .unwrap();
    let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(loaded.session.plugin_name, "review");
    let listed = list_sessions(&pool, DEFAULT_PROJECT_ID)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == session.id)
        .expect("session in list");
    assert_eq!(
        listed.plugin_name, "review",
        "list_sessions rehydrate must surface the new plugin_name"
    );

    // Switch back to dev.
    set_session_plugin_name(&pool, &session.id, "dev")
        .await
        .unwrap();
    let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(loaded.session.plugin_name, "dev");
}

#[tokio::test]
async fn set_session_plugin_name_on_missing_session_is_noop() {
    // Symmetric to the workflow_enabled equivalent:
    // an unknown session_id is a silent no-op.
    let pool = test_pool().await;
    set_session_plugin_name(&pool, "nonexistent-session-id", "dev")
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// Step4 follow-up: worktree state transition + system event tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn new_session_defaults_to_none_state() {
    let pool = test_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.worktree_state, WorktreeState::None);
    assert!(s.worktree_path.is_none());
    assert!(s.last_worktree_path.is_none());

    let reloaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(reloaded.session.worktree_state, WorktreeState::None);
    assert!(reloaded.session.worktree_path.is_none());
}

#[tokio::test]
async fn worktree_state_setter_round_trip() {
    let pool = test_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // Attach.
    set_worktree_state(&pool, &s.id, WorktreeState::Active, Some("/data/wt"), None)
        .await
        .unwrap();
    let r = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(r.session.worktree_state, WorktreeState::Active);
    assert_eq!(r.session.worktree_path.as_deref(), Some("/data/wt"));
    // Detach: clear worktree_path, preserve via last_worktree_path.
    set_worktree_state(
        &pool,
        &s.id,
        WorktreeState::Detached,
        None,
        Some("/data/wt"),
    )
    .await
    .unwrap();
    let r = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(r.session.worktree_state, WorktreeState::Detached);
    assert!(r.session.worktree_path.is_none());
    assert_eq!(r.session.last_worktree_path.as_deref(), Some("/data/wt"));
    // Delete: both clear.
    set_worktree_state(&pool, &s.id, WorktreeState::None, None, None)
        .await
        .unwrap();
    let r = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(r.session.worktree_state, WorktreeState::None);
    assert!(r.session.worktree_path.is_none());
    assert!(r.session.last_worktree_path.is_none());
}

#[tokio::test]
async fn worktree_state_unknown_string_defaults_to_none() {
    // Defensive: a future schema migration may add a new state;
    // older binaries must not crash reading unknown values.
    assert_eq!(WorktreeState::from_str_opt(""), WorktreeState::None);
    assert_eq!(WorktreeState::from_str_opt("nope"), WorktreeState::None);
    assert_eq!(WorktreeState::from_str_opt("active"), WorktreeState::Active);
    assert_eq!(
        WorktreeState::from_str_opt("detached"),
        WorktreeState::Detached
    );
}

#[tokio::test]
async fn worktree_state_backfill_legacy_active() {
    // Simulate a row that existed before the follow-up migration:
    // worktree_path set, worktree_state '' (the column exists
    // with DEFAULT 'none' but the row was inserted before the
    // backfill ran).
    let pool = test_pool().await;
    let sid = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
 INSERT INTO sessions
 (id, title, created_at, updated_at, model, project_id, current_cwd,
 worktree_path, worktree_state)
 VALUES (?, 'legacy', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z',
 'GLM-4.7', ?, '/tmp', '/data/legacy_wt', '')
 "#,
    )
    .bind(&sid)
    .bind(DEFAULT_PROJECT_ID)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
 "UPDATE sessions SET worktree_state = 'active' WHERE worktree_path IS NOT NULL AND (worktree_state IS NULL OR worktree_state = '')"
 )
 .execute(&pool)
 .await
 .unwrap();
    let reloaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(reloaded.session.worktree_state, WorktreeState::Active);
    assert_eq!(
        reloaded.session.worktree_path.as_deref(),
        Some("/data/legacy_wt")
    );
}
