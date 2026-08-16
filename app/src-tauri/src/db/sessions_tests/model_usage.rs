#![cfg(test)]

use uuid::Uuid;

use crate::llm::types::TokenUsage;
use crate::projects::DEFAULT_PROJECT_ID;

use super::make_pool;
use super::models::create_model;
use super::providers::create_provider;
use super::sessions::{
    create_session, list_sessions, load_session, update_last_turn_usage, update_session_model_id,
};

// ============================================================================
// === Sessions part 2: PR4 model_id + A4 token + F5 latency + persist_turn ===
// ============================================================================

// ---------------------------------------------------------------------------
// PR4 of multi-model task: update_session_model_id + load_session
// model_id field tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_session_model_id_sets_and_clears() {
    let pool = make_pool().await;
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
    // New session: model_id is NULL (falls back to global default).
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert!(loaded.session.model_id.is_none());

    // Set to a specific model.
    let p = create_provider(
        &pool,
        "anthropic",
        "Test (model_id)",
        "https://api.anthropic.com",
        "sk-test",
    )
    .await
    .unwrap();
    let m = create_model(
        &pool,
        &p.id,
        "test-model",
        "Test Model",
        None,
        None,
        false,
        false,
        100_000,
    )
    .await
    .unwrap();
    update_session_model_id(&pool, &s.id, &m.id).await.unwrap();
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(loaded.session.model_id.as_deref(), Some(m.id.as_str()));

    // Clear by passing empty string.
    update_session_model_id(&pool, &s.id, "").await.unwrap();
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert!(loaded.session.model_id.is_none());
}

#[tokio::test]
async fn update_session_model_id_on_missing_session_is_noop() {
    let pool = make_pool().await;
    // Should not error — the UPDATE simply matches0 rows.
    update_session_model_id(&pool, "nonexistent-session-id", "some-model-id")
        .await
        .unwrap();
}

#[tokio::test]
async fn load_session_includes_model_id() {
    let pool = make_pool().await;
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
    // Directly set model_id in the DB to verify the SELECT picks it up.
    let p = create_provider(
        &pool,
        "anthropic",
        "Test (model_id select)",
        "https://api.anthropic.com",
        "sk-test",
    )
    .await
    .unwrap();
    let m = create_model(
        &pool,
        &p.id,
        "select-test-model",
        "Select Test Model",
        None,
        None,
        false,
        false,
        100_000,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE sessions SET model_id = ? WHERE id = ?")
        .bind(&m.id)
        .bind(&s.id)
        .execute(&pool)
        .await
        .unwrap();
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(loaded.session.model_id.as_deref(), Some(m.id.as_str()));
}

// ---------------------------------------------------------------------------
// A4: per-session token usage accumulation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_last_turn_usage_overwrites_not_accumulates() {
    // 2026-06-26 snapshot fix: `update_last_turn_usage` OVERWRITES
    // the `last_*` columns (vs the legacy `add_token_usage` which
    // accumulated into `*_total`). Two consecutive calls should
    // leave the row with the SECOND call's values, not the sum.
    let pool = make_pool().await;
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
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    // Pre-snapshot: all five `last_*` columns are NULL.
    assert!(loaded.session.last_context_input_tokens.is_none());
    assert!(loaded.session.last_input_tokens.is_none());
    assert!(loaded.session.last_output_tokens.is_none());
    assert!(loaded.session.last_cache_creation.is_none());
    assert!(loaded.session.last_cache_read.is_none());

    let u1 = TokenUsage {
        input_tokens: 100,
        output_tokens: 30,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 50,
        context_input_tokens: 150,
    };
    let u2 = TokenUsage {
        input_tokens: 200,
        output_tokens: 40,
        cache_creation_input_tokens: 25,
        cache_read_input_tokens: 75,
        context_input_tokens: 300,
    };
    update_last_turn_usage(&pool, &s.id, &u1).await.unwrap();
    update_last_turn_usage(&pool, &s.id, &u2).await.unwrap();

    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    let s = &loaded.session;
    // Snapshot semantics: the row carries the SECOND call's values.
    assert_eq!(s.last_context_input_tokens, Some(300));
    assert_eq!(s.last_input_tokens, Some(200));
    assert_eq!(s.last_output_tokens, Some(40));
    assert_eq!(s.last_cache_creation, Some(25));
    assert_eq!(s.last_cache_read, Some(75));
}

#[tokio::test]
async fn list_sessions_includes_last_turn_columns() {
    // The 2026-06-26 snapshot columns are in the SessionSummary
    // shape too, so the SessionList (sidebar) can read them
    // without a per-session IPC round-trip. Verify the SELECT
    // carries them through.
    let pool = make_pool().await;
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
    let u = TokenUsage {
        input_tokens: 500,
        output_tokens: 100,
        cache_creation_input_tokens: 50,
        cache_read_input_tokens: 200,
        context_input_tokens: 750,
    };
    update_last_turn_usage(&pool, &s.id, &u).await.unwrap();

    let list = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
    let found = list.iter().find(|x| x.id == s.id).expect("session in list");
    assert_eq!(found.last_context_input_tokens, Some(750));
    assert_eq!(found.last_input_tokens, Some(500));
    assert_eq!(found.last_output_tokens, Some(100));
    assert_eq!(found.last_cache_creation, Some(50));
    assert_eq!(found.last_cache_read, Some(200));
}

// ---------------------------------------------------------------------------
// F5: LLM latency tracking
// ---------------------------------------------------------------------------
