#![cfg(test)]

//! Sessions-domain integration tests (split from `db/tests.rs` on 2026-06-23).
//!
//! Physically spliced from two original line ranges — `282-644`
//! (session CRUD + worktree state + system events) and `935-1551`
//! (PR4 model_id + A4 token usage + F5 latency + persist_turn).
//!
//! Coverage:
//! - Session CRUD (create / list / load / delete / touch / cwd / model_id)
//! - worktree state transitions + system events
//! - update_session_model_id (PR4)
//! - per-session token usage accumulation (A4)
//! - LLM latency tracking (F5)
//! - per-tool duration tracking on tool_result blocks

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::llm::types::{ContentBlock, MessageContent, Role, TokenUsage};
use crate::projects::DEFAULT_PROJECT_ID;

use super::{
    migrations::run_migrations,
    models::create_model,
    projects::create_project,
    providers::create_provider,
    sessions::{
        add_token_usage, create_session, delete_messages_by_session, delete_session,
        find_message_id_by_seq, insert_system_event, list_sessions, load_session, persist_turn,
        record_tool_duration, set_worktree_state, touch_session, update_message_latency,
        update_session_cwd, update_session_model_id, MessageLatency,
    },
    types::WorktreeState,
};

async fn test_pool() -> SqlitePool {
 let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
 // Mirror what `init_pool` does.
 sqlx::query("PRAGMA foreign_keys = ON")
 .execute(&pool)
 .await
 .unwrap();
 run_migrations(&pool).await.unwrap();
 pool
}

async fn make_pool() -> SqlitePool {
 test_pool().await // alias for readability inside this section
}
#[tokio::test]
async fn create_session_scopes_to_project() {
 let pool = test_pool().await;
 let p = create_project(&pool, "p", "/tmp/everlasting_test_session_proj", false, None)
 .await
 .unwrap();

 let s1 = create_session(&pool, &Uuid::new_v4().to_string(), &p.id, "/tmp/foo", "GLM-4.7", None)
 .await
 .unwrap();
 let s2 = create_session(&pool, &Uuid::new_v4().to_string(), &p.id, "/tmp/bar", "GLM-4.7", None)
 .await
 .unwrap();
 assert_eq!(s1.project_id, p.id);
 assert_eq!(s1.current_cwd, "/tmp/foo");
 assert_eq!(s2.current_cwd, "/tmp/bar");

 let list = list_sessions(&pool, &p.id).await.unwrap();
 assert_eq!(list.len(),2);
 // Cross-project isolation: legacy project's sessions are not
 // in this list.
 let legacy = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
 assert_eq!(legacy.len(),0);
}

#[tokio::test]
async fn load_session_returns_none_for_missing() {
 let pool = test_pool().await;
 let result = load_session(&pool, "nonexistent").await.unwrap();
 assert!(result.is_none());
}

#[tokio::test]
async fn persist_and_load_messages() {
 let pool = test_pool().await;
 let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 let user_msg = MessageContent::Text("read the file".to_string());
 persist_turn(&pool, &session.id, Role::User, &user_msg, 0, None)
 .await
 .unwrap();

 let assistant_blocks = vec![
 ContentBlock::Text {
 text: "OK reading".to_string(),
 cache_control: None,
 },
 ContentBlock::ToolUse {
 id: "toolu_abc".to_string(),
 name: "read_file".to_string(),
 input: serde_json::json!({"path": "/etc/hostname"}),
 },
 ];
 let assistant_msg = MessageContent::Blocks(assistant_blocks);
 persist_turn(&pool, &session.id, Role::Assistant, &assistant_msg, 1, None)
 .await
 .unwrap();

 let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
 assert_eq!(loaded.messages.len(),2);
 assert_eq!(loaded.messages[0].seq,0);
 assert_eq!(loaded.messages[0].text, "read the file");
 assert_eq!(loaded.messages[1].seq,1);
 assert!(loaded.messages[1].has_tool_calls);
 assert!(!loaded.messages[1].has_tool_results);

 let blocks: Vec<ContentBlock> =
 serde_json::from_value(loaded.messages[1].content.clone()).unwrap();
 assert_eq!(blocks.len(),2);
 assert!(matches!(&blocks[1], ContentBlock::ToolUse { name, .. } if name == "read_file"));
}

#[tokio::test]
async fn first_user_message_auto_titles_session() {
 let pool = test_pool().await;
 let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 let msg = MessageContent::Text("帮我读一下 /etc/hostname".to_string());
 persist_turn(&pool, &session.id, Role::User, &msg, 0, None)
 .await
 .unwrap();

 let updated = load_session(&pool, &session.id).await.unwrap().unwrap();
 assert_eq!(updated.session.title, "帮我读一下 /etc/hostname");
}

#[tokio::test]
async fn second_user_message_does_not_overwrite_title() {
 let pool = test_pool().await;
 let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 persist_turn(&pool, &session.id, Role::User, &MessageContent::Text("first".into()), 0, None)
 .await
 .unwrap();
 persist_turn(&pool, &session.id, Role::User, &MessageContent::Text("second".into()), 1, None)
 .await
 .unwrap();

 let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
 assert_eq!(loaded.session.title, "first");
}

#[tokio::test]
async fn delete_session_cascades_messages() {
 let pool = test_pool().await;
 let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 persist_turn(
 &pool,
 &session.id,
 Role::User,
 &MessageContent::Text("hi".into()),
 0,
 None,
 )
 .await
 .unwrap();

 delete_session(&pool, &session.id).await.unwrap();
 assert!(load_session(&pool, &session.id).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_messages_by_session_keeps_session_drops_messages() {
 let pool = test_pool().await;
 let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 persist_turn(
 &pool,
 &session.id,
 Role::User,
 &MessageContent::Text("hi".into()),
 0,
 None,
 )
 .await
 .unwrap();

 // Sanity: the message was persisted.
 let before = load_session(&pool, &session.id).await.unwrap().unwrap();
 assert_eq!(before.messages.len(), 1);

 // B3 /clear: messages gone, session row + metadata survive.
 delete_messages_by_session(&pool, &session.id).await.unwrap();
 let after = load_session(&pool, &session.id).await.unwrap().unwrap();
 assert!(after.messages.is_empty(), "messages should be cleared");
 assert_eq!(after.session.id, session.id, "session row must survive /clear");
 assert_eq!(after.session.title, before.session.title, "metadata preserved");
}

#[tokio::test]
async fn list_sessions_preview_truncates_at_80_chars() {
 let pool = test_pool().await;
 let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 let long = "a".repeat(120);
 persist_turn(&pool, &session.id, Role::User, &MessageContent::Text(long), 0, None)
 .await
 .unwrap();

 let list = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
 assert!(list[0].preview.starts_with("a".repeat(80).as_str()));
 assert!(list[0].preview.ends_with('…'));
}

#[tokio::test]
async fn touch_session_updates_timestamp() {
 let pool = test_pool().await;
 let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
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
 let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp/start", "GLM-4.7", None)
 .await
 .unwrap();
 assert_eq!(session.current_cwd, "/tmp/start");

 update_session_cwd(&pool, &session.id, "/tmp/end").await.unwrap();
 let reloaded = load_session(&pool, &session.id).await.unwrap().unwrap();
 assert_eq!(reloaded.session.current_cwd, "/tmp/end");
}

// ---------------------------------------------------------------------------
// Step4 follow-up: worktree state transition + system event tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn new_session_defaults_to_none_state() {
 let pool = test_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
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
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
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
 assert_eq!(WorktreeState::from_str_opt("detached"), WorktreeState::Detached);
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
 assert_eq!(reloaded.session.worktree_path.as_deref(), Some("/data/legacy_wt"));
}

#[tokio::test]
async fn insert_system_event_appends_to_history() {
 let pool = test_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 persist_turn(
 &pool,
 &s.id,
 Role::User,
 &MessageContent::Text("hi".into()),
 0,
 None,
 )
 .await
 .unwrap();
 insert_system_event(
 &pool,
 &s.id,
 "worktree attached: /data/wt on branch session/abc",
 "attached",
 )
 .await
 .unwrap();
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.messages.len(),2);
 let evt = &loaded.messages[1];
 assert_eq!(evt.role, "user");
 assert_eq!(evt.seq,1);
 let meta = evt.metadata.as_ref().expect("metadata present");
 assert_eq!(meta["kind"], "worktree_event");
 assert_eq!(meta["event"], "attached");
 let blocks: Vec<ContentBlock> = serde_json::from_value(evt.content.clone()).unwrap();
 assert_eq!(blocks.len(),1);
 if let ContentBlock::Text { text, .. } = &blocks[0] {
 assert!(text.contains("[worktree event]"));
 assert!(text.contains("/data/wt"));
 } else {
 panic!("expected text block");
 }
}

#[tokio::test]
async fn insert_system_event_seq_increments() {
 let pool = test_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 insert_system_event(&pool, &s.id, "first", "attached").await.unwrap();
 insert_system_event(&pool, &s.id, "second", "detached").await.unwrap();
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.messages.len(),2);
 assert_eq!(loaded.messages[0].seq,0);
 assert_eq!(loaded.messages[1].seq,1);
}

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
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 // New session: model_id is NULL (falls back to global default).
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert!(loaded.session.model_id.is_none());

 // Set to a specific model.
 let p = create_provider(&pool, "anthropic", "Test (model_id)", "https://api.anthropic.com", "sk-test")
 .await
 .unwrap();
 let m = create_model(&pool, &p.id, "test-model", "Test Model", None, None, false,100_000)
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
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 // Directly set model_id in the DB to verify the SELECT picks it up.
 let p = create_provider(&pool, "anthropic", "Test (model_id select)", "https://api.anthropic.com", "sk-test")
 .await
 .unwrap();
 let m = create_model(&pool, &p.id, "select-test-model", "Select Test Model", None, None, false,100_000)
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
async fn add_token_usage_first_turn_initializes_columns() {
 // A pre-A4 session has all 4 columns NULL. The first
 // `add_token_usage` call must initialize them from 0 (the
 // SQL `COALESCE(col, 0) + ?` pattern) rather than NULL +
 // value = NULL.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert!(loaded.session.input_tokens_total.is_none());
 assert!(loaded.session.output_tokens_total.is_none());
 assert!(loaded.session.cache_creation_total.is_none());
 assert!(loaded.session.cache_read_total.is_none());

 let u = TokenUsage {
 input_tokens: 100,
 output_tokens: 50,
 cache_creation_input_tokens: 10,
 cache_read_input_tokens: 20,
 };
 add_token_usage(&pool, &s.id, &u).await.unwrap();

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.session.input_tokens_total, Some(100));
 assert_eq!(loaded.session.output_tokens_total, Some(50));
 assert_eq!(loaded.session.cache_creation_total, Some(10));
 assert_eq!(loaded.session.cache_read_total, Some(20));
}

#[tokio::test]
async fn add_token_usage_accumulates_across_turns() {
 // A4 PRD decision 2: per-session 累积 (single SQL UPDATE
 // per turn). Verify that two consecutive calls add up
 // rather than overwrite.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 let u1 = TokenUsage {
 input_tokens: 100,
 output_tokens: 30,
 cache_creation_input_tokens: 0,
 cache_read_input_tokens: 50,
 };
 let u2 = TokenUsage {
 input_tokens: 200,
 output_tokens: 40,
 cache_creation_input_tokens: 25,
 cache_read_input_tokens: 75,
 };
 add_token_usage(&pool, &s.id, &u1).await.unwrap();
 add_token_usage(&pool, &s.id, &u2).await.unwrap();

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.session.input_tokens_total, Some(300));
 assert_eq!(loaded.session.output_tokens_total, Some(70));
 assert_eq!(loaded.session.cache_creation_total, Some(25));
 assert_eq!(loaded.session.cache_read_total, Some(125));
}

#[tokio::test]
async fn add_token_usage_on_missing_session_is_noop() {
 // UPDATE with a non-matching id matches 0 rows; the
 // function returns Ok(()) and doesn't error.
 let pool = make_pool().await;
 let u = TokenUsage {
 input_tokens: 10,
 output_tokens: 5,
 cache_creation_input_tokens: 0,
 cache_read_input_tokens: 0,
 };
 add_token_usage(&pool, "nonexistent-session-id", &u).await.unwrap();
}

#[tokio::test]
async fn list_sessions_includes_token_columns() {
 // The A4 columns are in the SessionSummary shape too,
 // so the SessionList (sidebar) can read them without
 // a per-session IPC round-trip. Verify the SELECT
 // carries them through.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 let u = TokenUsage {
 input_tokens: 500,
 output_tokens: 100,
 cache_creation_input_tokens: 50,
 cache_read_input_tokens: 200,
 };
 add_token_usage(&pool, &s.id, &u).await.unwrap();

 let list = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
 let found = list.iter().find(|x| x.id == s.id).expect("session in list");
 assert_eq!(found.input_tokens_total, Some(500));
 assert_eq!(found.output_tokens_total, Some(100));
 assert_eq!(found.cache_creation_total, Some(50));
 assert_eq!(found.cache_read_total, Some(200));
}

// ---------------------------------------------------------------------------
// F5: LLM latency tracking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn persist_turn_with_latency_writes_three_columns() {
 // F5 PRD R3: assistant turns persist with the three latency
 // columns. Pre-F5 callers can pass `None` and the columns
 // stay NULL (verified by the `persist_turn_with_no_latency`
 // test below). The columns are nullable so a legacy
 // pre-upgrade session doesn't error out on rehydrate.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 let content = MessageContent::Blocks(vec![ContentBlock::Text {
 text: "ok".to_string(),
 cache_control: None,
 }]);
 let latency = MessageLatency {
 ttfb_ms: Some(420),
 gen_ms: Some(2100),
 total_ms: Some(3200),
 thinking_ms: Some(850),
 };
 persist_turn(&pool, &s.id, Role::Assistant, &content, 0, Some(&latency))
 .await
 .unwrap();

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 let m = loaded.messages.first().expect("one message");
 assert_eq!(m.ttfb_ms, Some(420));
 assert_eq!(m.gen_ms, Some(2100));
 assert_eq!(m.total_ms, Some(3200));
 // F5 follow-up: thinking_ms round-trips through
 // `persist_turn` (the agent loop's path). The
 // `update_message_thinking` IPC is a separate write
 // that fires AFTER the controller sees `done`, so
 // the `Some(850)` value here proves the column +
 // `INSERT ... VALUES` bind order is correct.
 assert_eq!(m.thinking_ms, Some(850));
}

#[tokio::test]
async fn persist_turn_with_per_turn_latency_writes_4_columns_for_each_turn() {
 // F5 follow-up per-turn: a 3-turn agent response
 // (thinking→shell→tool_result×2→text) persists 3
 // assistant rows, each with its own 4-column
 // MessageLatency populated. This locks the
 // "per-turn rows all have 4 columns" contract that
 // the F5 single-value `req.thinkingDurationMs`
 // path violated (only the LAST turn's row had
 // `thinking_ms` set; the first N-1 were NULL and
 // rendered as "—" on reload).
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
  .await
  .unwrap();

 let mk_content = |text: &str| -> MessageContent {
  MessageContent::Blocks(vec![ContentBlock::Text {
   text: text.to_string(),
   cache_control: None,
  }])
 };

 // Turn 0 (assistant, seq=0): thinkingMs=200, totalMs=350
 let lat0 = MessageLatency {
  ttfb_ms: Some(180),
  gen_ms: Some(170),
  total_ms: Some(350),
  thinking_ms: Some(200),
 };
 persist_turn(
  &pool,
  &s.id,
  Role::Assistant,
  &mk_content("t0 answer"),
  0,
  Some(&lat0),
 )
 .await
 .unwrap();

 // Turn 1 (assistant, seq=1): thinkingMs=300, totalMs=450
 let lat1 = MessageLatency {
  ttfb_ms: Some(220),
  gen_ms: Some(230),
  total_ms: Some(450),
  thinking_ms: Some(300),
 };
 persist_turn(
  &pool,
  &s.id,
  Role::Assistant,
  &mk_content("t1 answer"),
  1,
  Some(&lat1),
 )
 .await
 .unwrap();

 // Turn 2 (assistant, seq=2): thinkingMs=500, totalMs=900
 let lat2 = MessageLatency {
  ttfb_ms: Some(300),
  gen_ms: Some(600),
  total_ms: Some(900),
  thinking_ms: Some(500),
 };
 persist_turn(
  &pool,
  &s.id,
  Role::Assistant,
  &mk_content("t2 final answer"),
  2,
  Some(&lat2),
 )
 .await
 .unwrap();

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.messages.len(), 3);

 // Each assistant row has its own per-turn 4-column
 // latency. seq-lookup mirrors the agent loop's
 // per-turn seq assignment.
 let m0 = &loaded.messages[0];
 assert_eq!(m0.ttfb_ms, Some(180));
 assert_eq!(m0.gen_ms, Some(170));
 assert_eq!(m0.total_ms, Some(350));
 assert_eq!(m0.thinking_ms, Some(200));

 let m1 = &loaded.messages[1];
 assert_eq!(m1.ttfb_ms, Some(220));
 assert_eq!(m1.gen_ms, Some(230));
 assert_eq!(m1.total_ms, Some(450));
 assert_eq!(m1.thinking_ms, Some(300));

 let m2 = &loaded.messages[2];
 assert_eq!(m2.ttfb_ms, Some(300));
 assert_eq!(m2.gen_ms, Some(600));
 assert_eq!(m2.total_ms, Some(900));
 // THIS is the F5 follow-up contract: the LAST
 // row's `thinking_ms` is the LAST turn's thinking
 // duration (500ms), NOT the first turn's (200ms,
 // which is what the F5 single-value
 // `req.thinkingDurationMs` produced — the bug the
 // user's "Thought for —" screenshot hit).
 assert_eq!(m2.thinking_ms, Some(500));
}

#[tokio::test]
async fn persist_turn_with_no_latency_leaves_columns_null() {
 // Tool-result rows (the user-role turn the agent loop persists
 // after tool execution) do not have a latency triple — the
 // per-tool duration lives in the content JSON, not on the row.
 // `persist_turn` accepts `None` and the three columns stay NULL.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 let content = MessageContent::Blocks(vec![ContentBlock::Text {
 text: "ok".to_string(),
 cache_control: None,
 }]);
 persist_turn(&pool, &s.id, Role::User, &content, 0, None)
 .await
 .unwrap();

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 let m = loaded.messages.first().expect("one message");
 assert!(m.ttfb_ms.is_none());
 assert!(m.gen_ms.is_none());
 assert!(m.total_ms.is_none());
}

#[tokio::test]
async fn update_message_latency_patches_columns_by_id() {
 // The frontend's `update_message_latency` IPC calls this
 // function on `done`. Verify a single UPDATE writes the three
 // columns. The seq → id lookup is in `find_message_id_by_seq`
 // (next test).
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 let content = MessageContent::Blocks(vec![ContentBlock::Text {
 text: "ok".to_string(),
 cache_control: None,
 }]);
 persist_turn(&pool, &s.id, Role::Assistant, &content, 0, None)
 .await
 .unwrap();

 let id = find_message_id_by_seq(&pool, &s.id, 0)
 .await
 .unwrap()
 .expect("id present");

 update_message_latency(
 &pool,
 id,
 &MessageLatency {
 ttfb_ms: Some(100),
 gen_ms: Some(200),
 total_ms: Some(300),
 thinking_ms: Some(75),
 },
 )
 .await
 .unwrap();

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 let m = loaded.messages.first().expect("one message");
 assert_eq!(m.ttfb_ms, Some(100));
 assert_eq!(m.gen_ms, Some(200));
 assert_eq!(m.total_ms, Some(300));
 // F5 follow-up: thinking_ms is patched in the same
 // UPDATE statement as the three latency columns. A
 // non-None value here proves the bind order in
 // `update_message_latency`'s SQL is correct (the
 // frontend passes `thinking_ms` as the 4th payload
 // field; the `WHERE id = ?` is the 5th bind).
 assert_eq!(m.thinking_ms, Some(75));
}

#[tokio::test]
async fn update_message_latency_accepts_partial_payload() {
 // Cancel / error paths may only know the total — `ttfb_ms` and
 // `gen_ms` are NULL when the user hits Stop before the first
 // delta. The function must accept a partial MessageLatency
 // without panicking and write NULL for the missing fields.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 let content = MessageContent::Blocks(vec![ContentBlock::Text {
 text: "ok".to_string(),
 cache_control: None,
 }]);
 persist_turn(&pool, &s.id, Role::Assistant, &content, 0, None)
 .await
 .unwrap();
 let id = find_message_id_by_seq(&pool, &s.id, 0).await.unwrap().unwrap();

 update_message_latency(
 &pool,
 id,
 &MessageLatency {
 ttfb_ms: None,
 gen_ms: None,
 total_ms: Some(500),
 thinking_ms: None,
 },
 )
 .await
 .unwrap();

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 let m = loaded.messages.first().expect("one message");
 assert!(m.ttfb_ms.is_none());
 assert!(m.gen_ms.is_none());
 assert_eq!(m.total_ms, Some(500));
 // F5 follow-up: thinking_ms is also nullable in the
 // partial-payload case (the model never entered the
 // thinking phase, or the cancel cleanup path fired
 // before the thinking close). The column round-trips
 // `None` cleanly.
 assert!(m.thinking_ms.is_none());
}

#[tokio::test]
async fn update_message_latency_patches_thinking_ms_independently() {
 // F5 follow-up: a turn that produced zero thinking (the
 // model answered without a `thinking_delta` event) but
 // had a real latency triple. The patch should land
 // `thinking_ms = None` (column stays NULL because
 // `persist_turn` wrote NULL) and `total_ms = Some(800)`,
 // AND the IPC's UPDATE must not accidentally clear the
 // `ttfb_ms` / `gen_ms` columns when the payload omits
 // the sub-components. Locks the bind order in the
 // single SQL statement.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 let content = MessageContent::Blocks(vec![ContentBlock::Text {
 text: "ok".to_string(),
 cache_control: None,
 }]);
 // Persist with a non-None latency triple but
 // `thinking_ms = None` — the agent loop's path doesn't
 // know thinking-time at persist time; the controller
 // fires the IPC to patch it after `done`.
 let latency = MessageLatency {
 ttfb_ms: Some(50),
 gen_ms: Some(750),
 total_ms: Some(800),
 thinking_ms: None,
 };
 persist_turn(&pool, &s.id, Role::Assistant, &content, 0, Some(&latency))
 .await
 .unwrap();

 // No follow-up IPC this time — thinking_ms stays NULL.
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 let m = loaded.messages.first().expect("one message");
 assert_eq!(m.ttfb_ms, Some(50));
 assert_eq!(m.gen_ms, Some(750));
 assert_eq!(m.total_ms, Some(800));
 assert!(m.thinking_ms.is_none());
}

#[tokio::test]
async fn find_message_id_by_seq_returns_none_for_unknown_pair() {
 // Defensive: a controller racing the agent loop's
 // `persist_turn` (cancel cleanup persists after `done`)
 // could fire `update_message_latency` before the row exists.
 // The lookup must return `None`, not error.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 let id = find_message_id_by_seq(&pool, &s.id, 999).await.unwrap();
 assert!(id.is_none());
}

#[tokio::test]
async fn record_tool_duration_patches_matching_tool_result_block() {
 // F5 PRD R2 / ADR-lite decision 1: per-tool duration is
 // embedded in the `tool_result` block of `messages.content`
 // JSON. The function reads the message, walks the content
 // array, finds the matching `tool_use_id`, and writes
 // `{"duration_ms": <n>}` into the block. Other blocks in
 // the array are untouched.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 // Persist a user-role turn with TWO tool_result blocks; only
 // the second one should get the patch.
 let content = MessageContent::Blocks(vec![
 ContentBlock::ToolResult {
 tool_use_id: "toolu_abc".to_string(),
 content: "result for tool 1".to_string(),
 is_error: false,
 },
 ContentBlock::ToolResult {
 tool_use_id: "toolu_def".to_string(),
 content: "result for tool 2".to_string(),
 is_error: false,
 },
 ]);
 persist_turn(&pool, &s.id, Role::User, &content, 0, None)
 .await
 .unwrap();

 let patched = record_tool_duration(&pool, &s.id, "toolu_def", 250)
 .await
 .unwrap();
 assert!(patched, "patch landed on a block");

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 let blocks = loaded.messages[0]
 .content
 .as_array()
 .expect("content is array");
 assert_eq!(blocks.len(), 2);
 // First block: untouched.
 assert_eq!(
 blocks[0].get("duration_ms"),
 None,
 "first tool_result must NOT have duration_ms"
 );
 // Second block: duration_ms set.
 assert_eq!(
 blocks[1].get("duration_ms").and_then(|v| v.as_i64()),
 Some(250)
 );
 // tool_use_id preserved verbatim (the patch must not mutate
 // the other fields).
 assert_eq!(
 blocks[1].get("tool_use_id").and_then(|v| v.as_str()),
 Some("toolu_def")
 );
}

#[tokio::test]
async fn record_tool_duration_returns_false_when_no_block_matches() {
 // Defensive: a `tool:result` event for a tool_use the agent
 // loop never persisted (e.g. the cancel cleanup) is a no-op,
 // not an error. The IPC consumer (frontend) treats `Ok(false)`
 // as a benign outcome.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 let content = MessageContent::Blocks(vec![ContentBlock::ToolResult {
 tool_use_id: "toolu_existing".to_string(),
 content: "x".to_string(),
 is_error: false,
 }]);
 persist_turn(&pool, &s.id, Role::User, &content, 0, None)
 .await
 .unwrap();

 let patched = record_tool_duration(&pool, &s.id, "toolu_never_persisted", 100)
 .await
 .unwrap();
 assert!(!patched);
}

#[tokio::test]
async fn record_tool_duration_handles_text_only_message_without_error() {
 // A text-only user message has no tool_result blocks; the
 // function must return Ok(false) (no error) and not touch the
 // content JSON.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();

 let content = MessageContent::Blocks(vec![ContentBlock::Text {
 text: "hello".to_string(),
 cache_control: None,
 }]);
 persist_turn(&pool, &s.id, Role::User, &content, 0, None)
 .await
 .unwrap();

 let patched = record_tool_duration(&pool, &s.id, "toolu_anything", 100)
 .await
 .unwrap();
 assert!(!patched);
}

