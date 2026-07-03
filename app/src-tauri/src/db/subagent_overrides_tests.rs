#![cfg(test)]

//! subagent_model_overrides-domain integration tests (2026-07-03,
//! task 07-03-subagent-per-agent-model-ui).
//!
//! Coverage:
//! - get / set / clear / list CRUD
//! - set is UPSERT (second call overwrites)
//! - clear on unknown row is Ok(())
//! - list order is stable (ORDER BY agent_name)

use sqlx::SqlitePool;

use super::{
    migrations::run_migrations,
    subagent_overrides::{
        clear_subagent_model_override, get_subagent_model_override,
        list_subagent_model_overrides, set_subagent_model_override,
    },
};

async fn make_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    pool
}

// ---------------------------------------------------------------------------
// B6+ C (2026-07-03): subagent_model_overrides CRUD tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subagent_model_overrides_get_missing_returns_none() {
    let pool = make_pool().await;
    let got = get_subagent_model_override(&pool, "researcher")
        .await
        .unwrap();
    assert!(got.is_none(), "unknown name → None (not Err)");
}

#[tokio::test]
async fn subagent_model_overrides_set_then_get_round_trips() {
    let pool = make_pool().await;
    set_subagent_model_override(&pool, "researcher", "model-uuid-a")
        .await
        .unwrap();
    let got = get_subagent_model_override(&pool, "researcher")
        .await
        .unwrap();
    assert_eq!(got.as_deref(), Some("model-uuid-a"));
}

#[tokio::test]
async fn subagent_model_overrides_set_is_upsert() {
    // A second set on the same agent_name overwrites the prior
    // model_id (no constraint violation, no duplicate row).
    let pool = make_pool().await;
    set_subagent_model_override(&pool, "researcher", "model-uuid-a")
        .await
        .unwrap();
    set_subagent_model_override(&pool, "researcher", "model-uuid-b")
        .await
        .unwrap();
    let got = get_subagent_model_override(&pool, "researcher")
        .await
        .unwrap();
    assert_eq!(got.as_deref(), Some("model-uuid-b"));
    // And the list still shows ONE row (not two — UPSERT).
    let all = list_subagent_model_overrides(&pool).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, "researcher");
    assert_eq!(all[0].1, "model-uuid-b");
}

#[tokio::test]
async fn subagent_model_overrides_clear_unknown_is_noop() {
    // Clearing a name that has no row → Ok(()) (defensive delete-
    // everywhere semantics; mirrors clear_session_permissions).
    let pool = make_pool().await;
    clear_subagent_model_override(&pool, "does-not-exist")
        .await
        .unwrap();
    // The list is still empty.
    let all = list_subagent_model_overrides(&pool).await.unwrap();
    assert!(all.is_empty());
}

#[tokio::test]
async fn subagent_model_overrides_clear_removes_row() {
    let pool = make_pool().await;
    set_subagent_model_override(&pool, "researcher", "model-uuid-a")
        .await
        .unwrap();
    set_subagent_model_override(&pool, "general-purpose", "model-uuid-b")
        .await
        .unwrap();
    clear_subagent_model_override(&pool, "researcher")
        .await
        .unwrap();
    assert!(get_subagent_model_override(&pool, "researcher")
        .await
        .unwrap()
        .is_none());
    // The other row is untouched.
    assert_eq!(
        get_subagent_model_override(&pool, "general-purpose")
            .await
            .unwrap()
            .as_deref(),
        Some("model-uuid-b")
    );
}

#[tokio::test]
async fn subagent_model_overrides_list_is_sorted_and_complete() {
    let pool = make_pool().await;
    // Insert in non-alphabetical order to verify ORDER BY agent_name.
    set_subagent_model_override(&pool, "zeta", "m-z").await.unwrap();
    set_subagent_model_override(&pool, "alpha", "m-a").await.unwrap();
    set_subagent_model_override(&pool, "mid", "m-m").await.unwrap();
    let all = list_subagent_model_overrides(&pool).await.unwrap();
    assert_eq!(
        all,
        vec![
            ("alpha".to_string(), "m-a".to_string()),
            ("mid".to_string(), "m-m".to_string()),
            ("zeta".to_string(), "m-z".to_string()),
        ]
    );
}
