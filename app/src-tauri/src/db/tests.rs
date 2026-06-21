//! Integration tests for the [`db`](super) module.
//!
//! These tests live in their own submodule (post-PR2 of the audit
//! task) so the per-domain `db/*.rs` files stay focused on the
//! happy-path code. Tests still share `super::*` (the `db` module)
//! for the public API.
//!
//! Coverage:
//! - migrations idempotency + auto-default project seed
//! - Project CRUD (create / list / list_hidden / get / update_path /
//! update_name / hide / unhide / list_stale_git_probe / git_metadata)
//! - Session CRUD (create / list / load / delete / touch / cwd /
//! model_id) + worktree state transitions + system events
//! - Provider / Model CRUD + the `app_config` seed

#![cfg(test)]

use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::llm::types::{ContentBlock, MessageContent, Role};
use crate::projects::DEFAULT_PROJECT_ID;

use super::{
 config::{get_config_value, seed_default_providers_and_models, set_config_value},
 migrations::run_migrations,
 models::{create_model, delete_model, list_models, update_model},
 projects::{
 create_project, get_project, hide_project, list_hidden_projects, list_projects,
 list_projects_with_stale_git_probe, unhide_project, update_project_git_metadata,
 update_project_name, update_project_path,
 },
 providers::{create_provider, delete_provider, list_providers, update_provider},
 sessions::{
 add_token_usage, create_session, delete_messages_by_session, delete_session, edit_user_message,
 find_message_id_by_seq, insert_system_event, list_sessions, load_session, persist_turn,
 record_tool_duration, set_worktree_state, touch_session, update_message_latency,
 update_session_cwd, update_session_model_id, MessageLatency,
 },
 permissions::{grant_tool_permission, has_tool_permission, list_audit_events, record_audit_event, update_session_mode},
 subagent_runs::{
 add_token_usage_streaming, get_run, insert_run, list_runs_by_session,
 list_runs_summary_by_session, update_run_finished, SubagentStatusDb,
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

#[tokio::test]
async fn migrations_are_idempotent() {
 let pool = test_pool().await;
 // Running twice should not error.
 run_migrations(&pool).await.unwrap();
}

#[tokio::test]
async fn default_project_is_seeded() {
 let pool = test_pool().await;
 let projects = list_projects(&pool, true).await.unwrap();
 let backstop = projects
 .iter()
 .find(|p| p.id == DEFAULT_PROJECT_ID)
 .expect("default project should be seeded");
 assert!(backstop.is_legacy);
 assert_eq!(backstop.name, "Legacy / 未分类");
 assert!(!backstop.hidden);
}

#[tokio::test]
async fn create_and_list_project() {
 let pool = test_pool().await;
 let dir = std::env::temp_dir().join("everlasting_test_create_proj");
 let _ = std::fs::create_dir_all(&dir);
 let path_str = dir.to_str().unwrap();

 let p1 = create_project(&pool, "a", path_str, false, None).await.unwrap();
 // Duplicate path → unique violation → Err.
 let dup = create_project(&pool, "b", path_str, false, None).await;
 assert!(dup.is_err(), "duplicate path should fail");

 let p2 = create_project(&pool, "c", "/tmp/everlasting_test_other", true, None)
 .await
 .unwrap();
 let list = list_projects(&pool, false).await.unwrap();
 let ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
 assert!(ids.contains(&p1.id.as_str()));
 assert!(ids.contains(&p2.id.as_str()));
 assert!(ids.contains(&DEFAULT_PROJECT_ID));

 let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn hide_and_unhide_project() {
 let pool = test_pool().await;
 let p = create_project(&pool, "x", "/tmp/everlasting_test_hide", false, None)
 .await
 .unwrap();

 hide_project(&pool, &p.id).await.unwrap();
 let visible = list_projects(&pool, false).await.unwrap();
 assert!(!visible.iter().any(|q| q.id == p.id));
 let hidden = list_hidden_projects(&pool).await.unwrap();
 assert!(hidden.iter().any(|q| q.id == p.id));

 unhide_project(&pool, &p.id).await.unwrap();
 let visible = list_projects(&pool, false).await.unwrap();
 assert!(visible.iter().any(|q| q.id == p.id));
}

#[tokio::test]
async fn update_project_name_works() {
 let pool = test_pool().await;
 let p = create_project(&pool, "old", "/tmp/everlasting_test_rename", false, None)
 .await
 .unwrap();
 let p2 = update_project_name(&pool, &p.id, "new").await.unwrap();
 assert_eq!(p2.name, "new");
 // updated_at should have advanced.
 assert_ne!(p2.updated_at, p.updated_at);
}

#[tokio::test]
async fn update_project_path_reprobes_git_flag() {
 let pool = test_pool().await;
 let p = create_project(&pool, "p", "/tmp/everlasting_test_repath", false, None)
 .await
 .unwrap();
 assert!(!p.is_git_repo);
 assert!(p.git_branch.is_none());

 let p2 =
 update_project_path(&pool, &p.id, "/tmp/everlasting_test_repath2", true, None)
 .await
 .unwrap();
 assert!(p2.is_git_repo);
 assert_eq!(p2.path, "/tmp/everlasting_test_repath2");
}

#[tokio::test]
async fn list_projects_with_stale_git_probe_filters_correctly() {
 // Pre-PR2 row: should appear.
 let pool = test_pool().await;
 let p_stale =
 create_project(&pool, "stale", "/tmp/everlasting_test_stale", false, None)
 .await
 .unwrap();
 assert!(!p_stale.is_git_repo);

 // Already-probed row: should NOT appear.
 let p_fresh = create_project(
 &pool,
 "fresh",
 "/tmp/everlasting_test_fresh",
 true,
 Some("main".to_string()),
 )
 .await
 .unwrap();
 assert!(p_fresh.is_git_repo);

 // Hidden stale row: should NOT appear (we skip hidden
 // projects — see the function's docstring).
 let p_hidden =
 create_project(&pool, "hidden", "/tmp/everlasting_test_hidden", false, None)
 .await
 .unwrap();
 hide_project(&pool, &p_hidden.id).await.unwrap();

 let stale = list_projects_with_stale_git_probe(&pool).await.unwrap();
 let ids: Vec<&str> = stale.iter().map(|p| p.id.as_str()).collect();
 assert!(ids.contains(&p_stale.id.as_str()));
 assert!(!ids.contains(&p_fresh.id.as_str()));
 assert!(!ids.contains(&p_hidden.id.as_str()));
}

#[tokio::test]
async fn update_project_git_metadata_round_trip() {
 let pool = test_pool().await;
 // Start from a non-git row, write git metadata, reload, verify.
 let p = create_project(&pool, "p", "/tmp/everlasting_test_metaupd", false, None)
 .await
 .unwrap();
 assert!(!p.is_git_repo);

 update_project_git_metadata(&pool, &p.id, true, Some("feature/x"))
 .await
 .unwrap();
 let reloaded = get_project(&pool, &p.id).await.unwrap().unwrap();
 assert!(reloaded.is_git_repo);
 assert_eq!(reloaded.git_branch.as_deref(), Some("feature/x"));

 // Setting `git_branch = None` (e.g. for a non-git repo) is
 // distinct from "empty string".
 update_project_git_metadata(&pool, &p.id, false, None).await.unwrap();
 let reloaded = get_project(&pool, &p.id).await.unwrap().unwrap();
 assert!(!reloaded.is_git_repo);
 assert!(reloaded.git_branch.is_none());
}

#[tokio::test]
async fn create_project_persists_git_branch() {
 let pool = test_pool().await;
 // Branch string survives a round-trip through the DB; the
 // detached-HEAD literal "HEAD" is also accepted.
 let p = create_project(
 &pool,
 "branchy",
 "/tmp/everlasting_test_branch",
 true,
 Some("feature/pr2".to_string()),
 )
 .await
 .unwrap();
 assert_eq!(p.git_branch.as_deref(), Some("feature/pr2"));

 let reloaded = get_project(&pool, &p.id).await.unwrap().unwrap();
 assert_eq!(reloaded.git_branch.as_deref(), Some("feature/pr2"));

 let detached = create_project(
 &pool,
 "detached",
 "/tmp/everlasting_test_detached",
 true,
 Some("HEAD".to_string()),
 )
 .await
 .unwrap();
 assert_eq!(detached.git_branch.as_deref(), Some("HEAD"));
}

#[tokio::test]
async fn update_project_path_reprobes_git_branch() {
 let pool = test_pool().await;
 let p = create_project(
 &pool,
 "rebranch",
 "/tmp/everlasting_test_rebranch",
 true,
 Some("main".to_string()),
 )
 .await
 .unwrap();
 assert_eq!(p.git_branch.as_deref(), Some("main"));

 // Re-probe with a different branch.
 let p2 = update_project_path(
 &pool,
 &p.id,
 "/tmp/everlasting_test_rebranch2",
 true,
 Some("develop".to_string()),
 )
 .await
 .unwrap();
 assert_eq!(p2.git_branch.as_deref(), Some("develop"));

 // Re-probe to a non-git path → branch cleared.
 let p3 = update_project_path(
 &pool,
 &p.id,
 "/tmp/everlasting_test_rebranch3",
 false,
 None,
 )
 .await
 .unwrap();
 assert!(!p3.is_git_repo);
 assert!(p3.git_branch.is_none());
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

// ---------------------------------------------------------------------------
// PR1 of multi-model task: providers / models / app_config tests
//
// Each CRUD function gets a happy path + a forced-error / edge-case
// test. The "create_session" / "sessions.model_id" interactions are
// covered separately in the seed test.
// ---------------------------------------------------------------------------

async fn make_pool() -> SqlitePool {
 test_pool().await // alias for readability inside this section
}

#[tokio::test]
async fn create_provider_then_list_returns_it() {
 let pool = make_pool().await;
 // `test_pool` already ran `run_migrations`, which seeded
 //2 providers. Add one more and assert it appears in the list
 // (without asserting total count, since the seed counts
 // aren't the test's concern).
 let before = list_providers(&pool).await.unwrap().len();
 let p = create_provider(&pool, "anthropic", "Test provider", "https://api.anthropic.com", "sk-test")
 .await
 .unwrap();
 assert_eq!(p.protocol, "anthropic");
 assert_eq!(p.display_name, "Test provider");
 assert!(!p.id.is_empty());
 let list = list_providers(&pool).await.unwrap();
 assert_eq!(list.len(), before +1);
 assert!(list.iter().any(|row| row.id == p.id));
}

#[tokio::test]
async fn update_provider_on_missing_id_returns_none() {
 let pool = make_pool().await;
 let res = update_provider(
 &pool,
 "00000000-0000-0000-0000-000000000000",
 "openai",
 "ghost",
 "https://example.com",
 "sk-ghost",
 )
 .await
 .unwrap();
 assert!(res.is_none());
}

#[tokio::test]
async fn delete_provider_cascades_to_models() {
 let pool = make_pool().await;
 let p = create_provider(&pool, "openai", "OpenAI官方 (test)", "https://api.openai.com/v1", "")
 .await
 .unwrap();
 let m = create_model(
 &pool,
 &p.id,
 "gpt-4o-test",
 "GPT-4o (test)",
 None,
 None,
 false,
128_000,
 )
 .await
 .unwrap();
 assert!(list_models(&pool).await.unwrap().iter().any(|mwp| mwp.model.id == m.id));
 assert!(delete_provider(&pool, &p.id).await.unwrap());
 // Cascade FK should have removed the model.
 assert!(!list_models(&pool).await.unwrap().iter().any(|mwp| mwp.model.id == m.id));
 assert!(!delete_model(&pool, &m.id).await.unwrap());
}

#[tokio::test]
async fn create_model_then_list_joins_provider_fields() {
 let pool = make_pool().await;
 let p = create_provider(&pool, "anthropic", "Anthropic官方 (test)", "https://api.anthropic.com", "")
 .await
 .unwrap();
 let m = create_model(
 &pool,
 &p.id,
 "claude-sonnet-4-5-test",
 "Claude Sonnet4.5 (test)",
 Some(8192),
 Some("high"),
 true,
200_000,
 )
 .await
 .unwrap();
 let list = list_models(&pool).await.unwrap();
 let mwp = list
 .iter()
 .find(|x| x.model.id == m.id)
 .expect("test model in list");
 assert_eq!(mwp.model.model_name, "claude-sonnet-4-5-test");
 assert_eq!(mwp.model.max_tokens, Some(8192));
 assert_eq!(mwp.model.thinking_effort.as_deref(), Some("high"));
 assert!(mwp.model.supports_thinking);
 assert_eq!(mwp.model.context_window,200_000);
 assert_eq!(mwp.provider_display_name, "Anthropic官方 (test)");
 assert_eq!(mwp.provider_protocol, "anthropic");
}

#[tokio::test]
async fn update_model_on_missing_id_returns_none() {
 let pool = make_pool().await;
 let res = update_model(
 &pool,
 "00000000-0000-0000-0000-000000000000",
 "p",
 "gpt-4o",
 "GPT-4o",
 None,
 None,
 false,
128_000,
 )
 .await
 .unwrap();
 assert!(res.is_none());
}

#[tokio::test]
async fn delete_model_on_missing_id_returns_false() {
 let pool = make_pool().await;
 let res = delete_model(&pool, "00000000-0000-0000-0000-000000000000")
 .await
 .unwrap();
 assert!(!res);
}

#[tokio::test]
async fn default_model_is_set_by_seed() {
 // The seed function runs as part of run_migrations; we
 // assert the contract that `default_model_id` is set AND
 // points at a real model row.
 let pool = make_pool().await;
 let id = get_config_value(&pool, "default_model_id").await.unwrap();
 let id = id.expect("default_model_id set by seed");
 let list = list_models(&pool).await.unwrap();
 assert!(list.iter().any(|mwp| mwp.model.id == id));
}

#[tokio::test]
async fn set_then_get_config_value_round_trips() {
 let pool = make_pool().await;
 // `default_model_id` is already set by the seed; we use
 // a custom key to avoid clobbering.
 set_config_value(&pool, "test_key", "abc-123").await.unwrap();
 let res = get_config_value(&pool, "test_key").await.unwrap();
 assert_eq!(res.as_deref(), Some("abc-123"));
 // Overwrite.
 set_config_value(&pool, "test_key", "xyz-789").await.unwrap();
 let res = get_config_value(&pool, "test_key").await.unwrap();
 assert_eq!(res.as_deref(), Some("xyz-789"));
}

#[tokio::test]
async fn seed_is_idempotent_and_inserts_defaults() {
 let pool = make_pool().await;
 // First call is a no-op because run_migrations already invoked
 // the seed; call again to prove idempotency (no duplicate
 // rows).
 let before_p = list_providers(&pool).await.unwrap().len();
 let before_m = list_models(&pool).await.unwrap().len();
 seed_default_providers_and_models(&pool).await.unwrap();
 assert_eq!(list_providers(&pool).await.unwrap().len(), before_p);
 assert_eq!(list_models(&pool).await.unwrap().len(), before_m);
}

#[tokio::test]
async fn seed_backfills_sessions_model_id() {
 // Build a fresh DB that mirrors a pre-PR1 state: only the
 // pre-PR1 tables exist (projects / sessions / messages),
 // no providers/models/app_config yet. Insert a legacy
 // sessions row with `model_id IS NULL`, then call
 // `seed_default_providers_and_models` and assert the
 // backfill query sets `model_id` on the legacy row.
 let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
 sqlx::query("PRAGMA foreign_keys = ON")
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 r#"
 CREATE TABLE projects (
 id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
 is_git_repo INTEGER NOT NULL DEFAULT 0, is_legacy INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 hidden INTEGER NOT NULL DEFAULT 0, metadata TEXT
 )
 "#,
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 r#"
 CREATE TABLE sessions (
 id TEXT PRIMARY KEY, title TEXT NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 model TEXT NOT NULL, metadata TEXT,
 project_id TEXT NOT NULL DEFAULT '__default__',
 current_cwd TEXT NOT NULL DEFAULT '',
 worktree_path TEXT,
 worktree_state TEXT NOT NULL DEFAULT 'none',
 last_worktree_path TEXT,
 model_id TEXT
 )
 "#,
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 r#"
 CREATE TABLE providers (
 id TEXT PRIMARY KEY, protocol TEXT NOT NULL,
 display_name TEXT NOT NULL, base_url TEXT NOT NULL,
 api_key TEXT NOT NULL DEFAULT '',
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL
 )
 "#,
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 r#"
 CREATE TABLE models (
 id TEXT PRIMARY KEY, provider_id TEXT NOT NULL,
 model_name TEXT NOT NULL, display_name TEXT NOT NULL,
 max_tokens INTEGER, thinking_effort TEXT,
 supports_thinking INTEGER NOT NULL DEFAULT 0,
 context_window INTEGER NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL
 )
 "#,
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 r#"
 CREATE TABLE app_config (
 key TEXT PRIMARY KEY, value TEXT NOT NULL
 )
 "#,
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 "INSERT INTO sessions (id, title, created_at, updated_at, model) \
 VALUES ('s1', 't', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'claude-sonnet-4-5')"
 )
 .execute(&pool)
 .await
 .unwrap();
 // Now call the seed directly; it inserts providers/models
 // + sets default_model_id, then backfills sessions.model_id.
 seed_default_providers_and_models(&pool).await.unwrap();
 let row: String = sqlx::query("SELECT model_id FROM sessions WHERE id = 's1'")
 .fetch_one(&pool)
 .await
 .unwrap()
 .try_get("model_id")
 .unwrap();
 assert!(!row.is_empty(), "model_id should be backfilled");
 // The default model id should match the backfilled value.
 let default_id = get_config_value(&pool, "default_model_id").await.unwrap();
 assert_eq!(row, default_id.expect("default set"));
}

#[tokio::test]
async fn delete_provider_cascade_does_not_touch_unrelated_models() {
 let pool = make_pool().await;
 let p1 = create_provider(&pool, "anthropic", "P1 (cascade test)", "https://a.example.com", "")
 .await
 .unwrap();
 let p2 = create_provider(&pool, "openai", "P2 (cascade test)", "https://b.example.com", "")
 .await
 .unwrap();
 let m1 = create_model(&pool, &p1.id, "m1-cascade-test", "M1", None, None, false,100_000)
 .await
 .unwrap();
 let m2 = create_model(&pool, &p2.id, "m2-cascade-test", "M2", None, None, false,100_000)
 .await
 .unwrap();
 let list = list_models(&pool).await.unwrap();
 assert!(list.iter().any(|mwp| mwp.model.id == m1.id));
 assert!(list.iter().any(|mwp| mwp.model.id == m2.id));
 delete_provider(&pool, &p1.id).await.unwrap();
 let remaining = list_models(&pool).await.unwrap();
 assert!(!remaining.iter().any(|mwp| mwp.model.id == m1.id));
 assert!(remaining.iter().any(|mwp| mwp.model.id == m2.id));
}

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

use crate::llm::types::TokenUsage;

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

// ---------------------------------------------------------------------------
// A2 + B7 (2026-06-13): permission DB CRUD tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_session_mode_persists_and_round_trips() {
 // The migration backfill sets mode='edit' on legacy rows; the
 // `set_session_mode` IPC call must flip it to any of the 3
 // valid modes and survive a re-load.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 // Default after create_session = 'edit'.
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.session.mode, crate::db::Mode::Edit);

 update_session_mode(&pool, &s.id, crate::db::Mode::Plan).await.unwrap();
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.session.mode, crate::db::Mode::Plan);

 update_session_mode(&pool, &s.id, crate::db::Mode::Yolo).await.unwrap();
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.session.mode, crate::db::Mode::Yolo);
}

#[tokio::test]
async fn update_session_mode_on_missing_session_is_noop() {
 let pool = make_pool().await;
 // UPDATE with a non-matching id matches 0 rows; no error.
 update_session_mode(&pool, "nonexistent-session-id", crate::db::Mode::Plan)
 .await
 .unwrap();
}

#[tokio::test]
async fn list_sessions_includes_mode_field() {
 // The mode field on SessionSummary must round-trip through
 // the SELECT path so the sidebar / mode badge reads it
 // without a per-session IPC.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 update_session_mode(&pool, &s.id, crate::db::Mode::Yolo).await.unwrap();

 let list = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
 let found = list.iter().find(|x| x.id == s.id).expect("session in list");
 assert_eq!(found.mode, crate::db::Mode::Yolo);
}

#[tokio::test]
async fn grant_tool_permission_round_trip_and_has_check() {
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 // Fresh session: no permissions yet.
 assert!(!has_tool_permission(&pool, &s.id, "shell").await.unwrap());
 assert!(!has_tool_permission(&pool, &s.id, "write_file").await.unwrap());

 grant_tool_permission(&pool, &s.id, "shell", "tool", None)
 .await
 .unwrap();
 assert!(has_tool_permission(&pool, &s.id, "shell").await.unwrap());
 // Different tool: still no permission.
 assert!(!has_tool_permission(&pool, &s.id, "write_file").await.unwrap());

 // Re-granting the same tool is a no-op (UPSERT semantics —
 // the `granted_at` is updated, but the row count stays 1).
 grant_tool_permission(&pool, &s.id, "shell", "tool", None)
 .await
 .unwrap();
 assert!(has_tool_permission(&pool, &s.id, "shell").await.unwrap());
}

#[tokio::test]
async fn grant_tool_permission_cascades_on_session_delete() {
 // ON DELETE CASCADE: deleting the session must clean up its
 // permission rows. PRAGMA foreign_keys = ON is set in
 // test_pool — without it the cascade silently no-ops.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 grant_tool_permission(&pool, &s.id, "shell", "tool", None)
 .await
 .unwrap();
 assert!(has_tool_permission(&pool, &s.id, "shell").await.unwrap());

 delete_session(&pool, &s.id).await.unwrap();
 // Session row is gone — the (sid, tool_name) lookup must
 // return false (the permission row was CASCADE-deleted).
 assert!(!has_tool_permission(&pool, &s.id, "shell").await.unwrap());
}

#[tokio::test]
async fn record_audit_event_inserts_and_cascades_on_delete() {
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 record_audit_event(
 &pool,
 &s.id,
 "tool_allowed",
 Some(r#"{"tool_name":"shell","reason":null}"#),
 )
 .await
 .unwrap();
 record_audit_event(&pool, &s.id, "mode_changed", Some(r#"{"new_mode":"yolo"}"#))
 .await
 .unwrap();
 // Verify the rows are present by SELECTing directly.
 let count: i64 = sqlx::query_scalar(
 "SELECT COUNT(*) FROM session_audit_events WHERE session_id = ?",
 )
 .bind(&s.id)
 .fetch_one(&pool)
 .await
 .unwrap();
 assert_eq!(count, 2);

 // Cascade on session delete.
 delete_session(&pool, &s.id).await.unwrap();
 let count: i64 = sqlx::query_scalar(
 "SELECT COUNT(*) FROM session_audit_events WHERE session_id = ?",
 )
 .bind(&s.id)
 .fetch_one(&pool)
 .await
 .unwrap();
 assert_eq!(count, 0);
}

/// C4 PR1 (2026-06-14): the new `tool_executed` audit kind writes
/// through `record_audit_event` with the C4 payload shape
/// (`tool_name` / `tool_input` / `duration_ms` / `exit_code`) and
/// round-trips through `list_audit_events` so the audit-log UI
/// can read it back. The `kind` is a plain string on the wire (the
/// DB column is TEXT) so the new variant requires no migration;
/// this test locks the round-trip + the payload parse path the
/// frontend will rely on.
///
/// **Order-independence note**: `session_audit_events.ts` is
/// `datetime('now')` (1-second resolution). Two inserts inside the
/// same wall-clock second share the same `ts`, so
/// `ORDER BY ts DESC` is non-deterministic for ties. The test
/// therefore finds each row by its `tool_name` instead of
/// assuming `rows[0]` is the shell row.
#[tokio::test]
async fn tool_executed_audit_round_trips_via_list_audit_events() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();

    // Case 1: shell tool with a real exit code.
    let payload_shell = serde_json::json!({
        "tool_name": "shell",
        "tool_input": {"command": "cargo build"},
        "duration_ms": 1234_u64,
        "exit_code": 0_i32,
    })
    .to_string();
    record_audit_event(&pool, &s.id, "tool_executed", Some(&payload_shell))
        .await
        .unwrap();

    // Case 2: read_file tool with no exit code (Option::None on
    // the agent-loop side serializes as JSON null).
    let payload_read = serde_json::json!({
        "tool_name": "read_file",
        "tool_input": {"path": "/tmp/foo.rs"},
        "duration_ms": 12_u64,
        "exit_code": serde_json::Value::Null,
    })
    .to_string();
    record_audit_event(&pool, &s.id, "tool_executed", Some(&payload_read))
        .await
        .unwrap();

    // Round-trip: list_audit_events returns both rows.
    let rows = list_audit_events(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].kind, "tool_executed");
    assert_eq!(rows[1].kind, "tool_executed");

    // Find each payload by tool_name (the `ts` ties make the row
    // order non-deterministic).
    let mut shell_payload: Option<serde_json::Value> = None;
    let mut read_payload: Option<serde_json::Value> = None;
    for r in &rows {
        let p: serde_json::Value =
            serde_json::from_str(r.payload_json.as_deref().unwrap()).unwrap();
        match p["tool_name"].as_str() {
            Some("shell") => shell_payload = Some(p),
            Some("read_file") => read_payload = Some(p),
            _ => {}
        }
    }

    let p_shell = shell_payload.expect("shell payload must be present");
    assert_eq!(p_shell["duration_ms"], 1234);
    assert_eq!(p_shell["exit_code"], 0);
    assert!(
        !p_shell["exit_code"].is_null(),
        "exit_code must NOT be null for shell"
    );

    let p_read = read_payload.expect("read_file payload must be present");
    assert_eq!(p_read["duration_ms"], 12);
    assert!(
        p_read["exit_code"].is_null(),
        "exit_code must be null for read_file"
    );
}

/// C4 PR1: list_audit_events on an empty session returns an empty
/// Vec (NOT an error). The audit-log UI renders its "暂无审计事件"
/// placeholder against this shape; an error would surface as a
/// toast instead.
#[tokio::test]
async fn list_audit_events_empty_session_returns_empty_vec() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let rows = list_audit_events(&pool, &s.id).await.unwrap();
    assert!(rows.is_empty());
}

/// C4 PR1: list_audit_events tolerates a NULL payload_json. Older
/// code paths (or future ones) may write rows without a payload;
/// the read side must surface them as `payload_json: None` instead
/// of crashing. The audit-log UI's "payload 为 null/malformed 时不
/// 崩" AC leans on this.
#[tokio::test]
async fn list_audit_events_tolerates_null_payload() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    record_audit_event(&pool, &s.id, "tool_executed", None)
        .await
        .unwrap();
    let rows = list_audit_events(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "tool_executed");
    assert!(rows[0].payload_json.is_none());
}

/// C4 PR1 check-phase follow-up (2026-06-14): lock the wire shape
/// of `AuditEventRow` to **camelCase** — matches every other
/// `db::*Row` that crosses the IPC boundary (SessionRow,
/// SessionSummary, ProviderRow, ModelRow, … all carry
/// `#[serde(rename_all = \"camelCase\")]`). The frontend's TS
/// interface reads `sessionId` / `payloadJson`, not snake_case —
/// if a future refactor drops the `rename_all` attribute, the
/// frontend gets `undefined` for every field. This regression test
/// fails in that case.
///
/// Locks spec `.trellis/spec/backend/database-guidelines.md`:
/// "All Serialize structs that cross the IPC boundary have
///  #[serde(rename_all = \"camelCase\")]"
#[tokio::test]
async fn audit_event_row_serializes_to_camel_case_wire_shape() {
    use crate::db::permissions::AuditEventRow;
    let row = AuditEventRow {
        id: 42,
        session_id: "sess-abc".to_string(),
        ts: "2026-06-14T10:00:00Z".to_string(),
        kind: "tool_executed".to_string(),
        payload_json: Some("{\"tool_name\":\"shell\"}".to_string()),
    };
    let v: serde_json::Value = serde_json::to_value(&row).unwrap();
    let obj = v.as_object().expect("row must serialize to JSON object");

    // camelCase keys must be present.
    assert!(
        obj.contains_key("sessionId"),
        "wire shape must use `sessionId` (camelCase), got keys: {:?}",
        obj.keys().collect::<Vec<_>>()
    );
    assert!(
        obj.contains_key("payloadJson"),
        "wire shape must use `payloadJson` (camelCase), got keys: {:?}",
        obj.keys().collect::<Vec<_>>()
    );

    // snake_case keys must NOT be present (would mean `rename_all`
    // was dropped).
    assert!(
        !obj.contains_key("session_id"),
        "wire shape must NOT leak snake_case `session_id`"
    );
    assert!(
        !obj.contains_key("payload_json"),
        "wire shape must NOT leak snake_case `payload_json`"
    );

    // Round-trip the value to confirm the non-renamed fields are intact.
    assert_eq!(obj.get("id").and_then(|v| v.as_i64()), Some(42));
    assert_eq!(obj.get("kind").and_then(|v| v.as_str()), Some("tool_executed"));
}

#[tokio::test]
async fn mode_backfill_legacy_null_to_edit() {
 // Simulate a pre-A2 session with `mode IS NULL` (column was
 // added but the backfill hasn't run yet). Mirrors what a
 // real upgrade path looks like between the ALTER and the
 // UPDATE in `run_migrations`.
 let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
 sqlx::query("PRAGMA foreign_keys = ON")
 .execute(&pool)
 .await
 .unwrap();
 // Minimal pre-A2 schema: sessions row without `mode`.
 sqlx::query(
 r#"
 CREATE TABLE sessions (
 id TEXT PRIMARY KEY, title TEXT NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 model TEXT NOT NULL, project_id TEXT NOT NULL DEFAULT '__default__',
 current_cwd TEXT NOT NULL DEFAULT ''
 )
 "#,
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 r#"
 CREATE TABLE projects (
 id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
 is_git_repo INTEGER NOT NULL DEFAULT 0, is_legacy INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 hidden INTEGER NOT NULL DEFAULT 0, metadata TEXT
 )
 "#,
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 "INSERT INTO projects (id, name, path, is_legacy, created_at, updated_at, hidden) \
 VALUES ('__default__', 'legacy', '/tmp', 1, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 0)",
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 "INSERT INTO sessions (id, title, created_at, updated_at, model) \
 VALUES ('legacy-1', 't', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'GLM-4.7')",
 )
 .execute(&pool)
 .await
 .unwrap();
 // Run the full migration (adds the `mode` column + backfills).
 run_migrations(&pool).await.unwrap();
 // Verify the backfill set mode='edit' on the legacy row.
 let mode: Option<String> = sqlx::query_scalar("SELECT mode FROM sessions WHERE id = 'legacy-1'")
 .fetch_one(&pool)
 .await
 .unwrap();
 assert_eq!(mode.as_deref(), Some("edit"));
}

// =====================================================================
// D3 PR1 (2026-06-17): edit_user_message tests
//
// Coverage:
// - cascade delete of subsequent messages (assistant turn +
// tool_result rows)
// - metadata edits: `edited_at` stamp + `original_content` backup
// - audit row insert with kind='edit_message' + JSON payload
// - no-op fast path when content is unchanged (no audit, no
// metadata bump)
// - idempotent on subsequent edits (original_content preserved,
// edited_at bumped)
// - foreign-key safe: edit + cascade delete run in one tx
// - atomic rollback: a synthetic INSERT failure inside the tx
// reverts the edit (no partial commit)
// - unknown (session_id, seq) pair is a silent no-op (defensive)
// =====================================================================

/// Helper: build a fresh session + persist user + assistant +
/// tool_result rows at seq 0 / 1 / 2. The assistant turn carries
/// a `tool_use` block; the user turn at seq 2 carries a
/// `tool_result` block that references it. Used as the canonical
/// setup for cascade-delete assertions.
async fn setup_session_with_3_turns(pool: &SqlitePool) -> String {
 let s = create_session(
 pool,
 &Uuid::new_v4().to_string(),
 DEFAULT_PROJECT_ID,
 "/tmp",
 "GLM-4.7",
 None,
 )
 .await
 .unwrap();
 // seq 0: user prompt
 persist_turn(
 pool,
 &s.id,
 Role::User,
 &MessageContent::Text("old prompt text".to_string()),
 0,
 None,
 )
 .await
 .unwrap();
 // seq 1: assistant turn with tool_use
 let assistant_content = MessageContent::Blocks(vec![ContentBlock::ToolUse {
 id: "toolu_abc".to_string(),
 name: "read_file".to_string(),
 input: serde_json::json!({"path": "/etc/hostname"}),
 }]);
 persist_turn(pool, &s.id, Role::Assistant, &assistant_content, 1, None)
 .await
 .unwrap();
 // seq 2: user(tool_result) — the assistant's tool result.
 let tool_result_content = MessageContent::Blocks(vec![ContentBlock::ToolResult {
 tool_use_id: "toolu_abc".to_string(),
 content: "host1".to_string(),
 is_error: false,
 }]);
 persist_turn(pool, &s.id, Role::User, &tool_result_content, 2, None)
 .await
 .unwrap();
 s.id
}

#[tokio::test]
async fn edit_user_message_cascade_deletes_subsequent_messages() {
 // The cascade DELETE removes every strictly-later message in
 // the session — the assistant turn (seq 1) + the user(tool_result)
 // turn (seq 2). Only the edited user message (seq 0) survives.
 let pool = make_pool().await;
 let sid = setup_session_with_3_turns(&pool).await;

 let new_content = MessageContent::Text("new prompt text".to_string());
 edit_user_message(&pool, &sid, 0, &new_content)
 .await
 .unwrap();

 let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
 assert_eq!(loaded.messages.len(), 1, "only the edited row survives");
 assert_eq!(loaded.messages[0].seq, 0);
 assert_eq!(loaded.messages[0].role, "user");
 // The new text landed (denormalized `text` column).
 assert_eq!(loaded.messages[0].text, "new prompt text");
}

#[tokio::test]
async fn edit_user_message_writes_edited_at_metadata() {
 // First edit: stamps `edited_at` + `original_content` into the
 // metadata JSON. `edited_at` parses back as a non-null string
 // (RFC3339).
 let pool = make_pool().await;
 let sid = setup_session_with_3_turns(&pool).await;

 let new_content = MessageContent::Text("v2 text".to_string());
 edit_user_message(&pool, &sid, 0, &new_content)
 .await
 .unwrap();

 let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
 let md = loaded.messages[0]
 .metadata
 .as_ref()
 .expect("metadata should be set after first edit");
 let edited_at = md
 .get("edited_at")
 .and_then(|v| v.as_str())
 .expect("edited_at should be set");
 assert!(!edited_at.is_empty(), "edited_at is RFC3339 timestamp");
 // Spot-check format (contains "T" + timezone offset or "Z").
 assert!(edited_at.contains('T'), "edited_at should be RFC3339");
}

#[tokio::test]
async fn edit_user_message_preserves_original_content_on_first_edit() {
 // `original_content` is the JSON-serialized pre-edit value of
 // the row's `content` column. For a plain text message that
 // serializes as a JSON string, `original_content` should be
 // that string verbatim.
 let pool = make_pool().await;
 let sid = setup_session_with_3_turns(&pool).await;

 let new_content = MessageContent::Text("v2".to_string());
 edit_user_message(&pool, &sid, 0, &new_content)
 .await
 .unwrap();

 let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
 let md = loaded.messages[0].metadata.as_ref().unwrap();
 let original = md
 .get("original_content")
 .expect("original_content should be set on first edit");
 // Pre-edit content was "old prompt text" → JSON string form.
 assert_eq!(
 original.as_str(),
 Some("old prompt text"),
 "original_content should be the pre-edit value"
 );
}

#[tokio::test]
async fn edit_user_message_preserves_original_across_subsequent_edits() {
 // Re-editing an already-edited row must NOT overwrite
 // `original_content`. It always points at the pre-ANY-edit
 // value, so a future "undo edit" affordance has a stable
 // restore target.
 let pool = make_pool().await;
 let sid = setup_session_with_3_turns(&pool).await;

 // First edit: original prompt → "v2"
 edit_user_message(
 &pool,
 &sid,
 0,
 &MessageContent::Text("v2".to_string()),
 )
 .await
 .unwrap();
 // Re-insert a new assistant row so the second edit has
 // something to cascade-delete (the first edit wiped
 // seqs 1 + 2).
 persist_turn(
 &pool,
 &sid,
 Role::Assistant,
 &MessageContent::Text("assistant reply 2".to_string()),
 1,
 None,
 )
 .await
 .unwrap();
 // Second edit: "v2" → "v3"
 edit_user_message(
 &pool,
 &sid,
 0,
 &MessageContent::Text("v3".to_string()),
 )
 .await
 .unwrap();

 let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
 let md = loaded.messages[0].metadata.as_ref().unwrap();
 let original = md.get("original_content").unwrap();
 // Still points at "old prompt text" — NOT at "v2".
 assert_eq!(original.as_str(), Some("old prompt text"));
 // And the latest content reflects the second edit.
 assert_eq!(loaded.messages[0].text, "v3");
}

#[tokio::test]
async fn edit_user_message_records_audit_event() {
 // The single-transaction flow appends one row to
 // `session_audit_events` with kind='edit_message' and a JSON
 // payload carrying message_seq + new_text_preview + edited_at.
 let pool = make_pool().await;
 let sid = setup_session_with_3_turns(&pool).await;

 edit_user_message(
 &pool,
 &sid,
 0,
 &MessageContent::Text("edited prompt".to_string()),
 )
 .await
 .unwrap();

 // list_audit_events is session-scoped + sorted ts DESC.
 let events = list_audit_events(&pool, &sid).await.unwrap();
 assert_eq!(events.len(), 1);
 let event = &events[0];
 assert_eq!(event.kind, "edit_message");
 let payload = event
 .payload_json
 .as_ref()
 .expect("payload_json should be set");
 let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
 assert_eq!(parsed["message_seq"], serde_json::json!(0));
 assert_eq!(parsed["new_text_preview"], serde_json::json!("edited prompt"));
 assert!(parsed["edited_at"].is_string());
}

#[tokio::test]
async fn edit_user_message_is_noop_when_content_unchanged() {
 // Save-without-change: the JSON of `new_content` matches the
 // current row's `content` column verbatim. The function must
 // return Ok(()) without writing any state — no audit row, no
 // `edited_at` bump on the metadata.
 let pool = make_pool().await;
 let sid = setup_session_with_3_turns(&pool).await;

 let same_content = MessageContent::Text("old prompt text".to_string());
 edit_user_message(&pool, &sid, 0, &same_content)
 .await
 .unwrap();

 // Tail rows still present (no cascade delete).
 let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
 assert_eq!(loaded.messages.len(), 3, "no cascade on no-op");
 // Metadata is null (no edit happened).
 assert!(loaded.messages[0].metadata.is_none());
 // Audit log empty (no event on no-op).
 let events = list_audit_events(&pool, &sid).await.unwrap();
 assert_eq!(events.len(), 0, "no audit row on no-op");
}

#[tokio::test]
async fn edit_user_message_on_unknown_seq_is_silent_noop() {
 // Defensive: an edit request whose (session_id, seq) pair
 // resolves to nothing (e.g. cancel race wiping the row) is a
 // no-op, not an error. Mirrors the F5 latency IPC contract.
 let pool = make_pool().await;
 let sid = setup_session_with_3_turns(&pool).await;

 edit_user_message(
 &pool,
 &sid,
 999, // unknown seq
 &MessageContent::Text("garbage".to_string()),
 )
 .await
 .unwrap();

 // Original 3-turn history is intact.
 let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
 assert_eq!(loaded.messages.len(), 3);
}

#[tokio::test]
async fn edit_user_message_atomic_rollback_on_db_error() {
 // Synthetic failure path: we register a SQLite trigger that
 // throws on the second INSERT inside the transaction (the
 // audit row insert). The transaction must roll back: the
 // UPDATE on the user row + the cascade DELETE on the tail
 // must NOT commit. End state: the 3-turn history is intact
 // (no edit, no cascade), no audit row.
 //
 // SQLite raises "Abort due to constraint violation" from
 // RAISE(FAIL, ...) inside a trigger — this is the cleanest
 // way to force a deterministic mid-transaction failure
 // without filesystem-level fault injection.
 let pool = make_pool().await;
 let sid = setup_session_with_3_turns(&pool).await;

 // Trigger: when an INSERT into session_audit_events happens,
 // abort the transaction with a clear message. The first INSERT
 // (from the edit's audit row) will hit this trigger and fail.
 sqlx::query(
 r#"
 CREATE TRIGGER fail_edit_audit_insert
 BEFORE INSERT ON session_audit_events
 WHEN NEW.kind = 'edit_message'
 BEGIN
 SELECT RAISE(FAIL, 'synthetic: forced failure for rollback test');
 END
 "#,
 )
 .execute(&pool)
 .await
 .unwrap();

 let result = edit_user_message(
 &pool,
 &sid,
 0,
 &MessageContent::Text("this edit will fail".to_string()),
 )
 .await;
 assert!(
 result.is_err(),
 "edit_user_message must error when audit insert fails"
 );

 // State after the failed transaction:
 // - The user row at seq 0 still has the ORIGINAL content
 // - The assistant turn at seq 1 + the tool_result at seq 2
 // are still present (no cascade)
 // - No audit row was appended
 let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
 assert_eq!(loaded.messages.len(), 3, "all 3 turns intact on rollback");
 assert_eq!(loaded.messages[0].text, "old prompt text");
 let events = list_audit_events(&pool, &sid).await.unwrap();
 assert_eq!(
 events.len(),
 0,
 "no audit row should have committed before the trigger fired"
 );
}

/// D3 PR3 (2026-06-17): the resend audit helper writes a
/// `resend_message` row through `record_audit_event` with the
/// expected payload shape (`message_seq` + `content_text_preview`)
/// and round-trips through `list_audit_events`. Mirrors the
/// `tool_executed_audit_round_trips_via_list_audit_events` test
/// (C4 PR1, 2026-06-14) so the new variant is locked the same way:
/// the audit-log UI can dispatch on the kind string and parse the
/// payload without further coordination.
#[tokio::test]
async fn resend_message_audit_round_trips_via_list_audit_events() {
    use crate::agent::permissions::record_message_resend_audit;

    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();

    // Case 1: typical resend of a short user prompt.
    record_message_resend_audit(
        &pool,
        &s.id,
        3,
        "re-run: explain the cancellation token pattern",
    )
    .await
    .unwrap();

    let events = list_audit_events(&pool, &s.id).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "resend_message");
    let payload: serde_json::Value =
        serde_json::from_str(events[0].payload_json.as_deref().unwrap()).unwrap();
    assert_eq!(payload["message_seq"], 3);
    assert_eq!(
        payload["content_text_preview"],
        "re-run: explain the cancellation token pattern"
    );

    // Case 2: long content is truncated to 80 chars (matches the
    // edit audit preview budget).
    let long = "a".repeat(200);
    record_message_resend_audit(&pool, &s.id, 5, &long).await.unwrap();
    let events = list_audit_events(&pool, &s.id).await.unwrap();
    assert_eq!(events.len(), 2);
    let target = "5";
    let second = events
        .iter()
        .find(|e| {
            e.kind == "resend_message"
                && e.payload_json.as_deref().unwrap().contains(target)
        })
        .unwrap();
    let payload: serde_json::Value =
        serde_json::from_str(second.payload_json.as_deref().unwrap()).unwrap();
    let preview = payload["content_text_preview"].as_str().unwrap();
    assert_eq!(preview.chars().count(), 80, "preview truncated to 80 chars");
    assert!(
        preview.chars().all(|c| c == 'a'),
        "preview is the leading 80 a chars"
    );
}

/// D3 PR3 (2026-06-17): the resend audit helper returns
/// `Result<(), sqlx::Error>` like the other audit helpers; a DB
/// write failure surfaces to the caller (the agent loop
/// log-and-swallow path at the user-message persist site).
/// Asserted by inserting into a deleted session (FK constraint
/// fires; helper returns `Err`).
#[tokio::test]
async fn resend_message_audit_on_deleted_session_returns_error() {
    use crate::agent::permissions::record_message_resend_audit;

    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    delete_session(&pool, &s.id).await.unwrap();
    let result = record_message_resend_audit(&pool, &s.id, 0, "after-delete").await;
    assert!(result.is_err(), "audit insert must fail on missing session");
}

// ---------------------------------------------------------------------------
// B6 PR2: subagent_runs tests
// ---------------------------------------------------------------------------

/// Insert returns a unique id and the row lands in `running`
/// state with `finished_at` NULL, the empty `TokenUsage` JSON
/// default, the empty transcript `[]`, and `transcript_truncated=0`.
#[tokio::test]
async fn subagent_runs_insert_creates_running_row() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-test", "researcher", None)
        .await
        .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.id, id);
    assert_eq!(row.parent_session_id, s.id);
    assert_eq!(row.parent_request_id, "rid-test");
    assert_eq!(row.subagent_name, "researcher");
    assert_eq!(row.status, "running");
    assert!(row.finished_at.is_none(), "running → finished_at=NULL");
    assert_eq!(
        row.transcript_truncated, 0,
        "fresh row → transcript_truncated=0"
    );
    assert_eq!(
        row.transcript_json.as_deref(),
        Some("[]"),
        "fresh row → transcript_json=[]"
    );
    assert!(
        row.token_usage_json.is_some(),
        "fresh row → token_usage_json seeded"
    );
    assert!(row.summary.is_none(), "running → summary=NULL");
    assert!(row.task.is_none(), "task=None at insert → column NULL");
    assert!(row.final_text.is_none(), "running → final_text=NULL");
    assert!(!row.started_at.is_empty());
    assert!(!row.created_at.is_empty());
}

/// `update_run_finished` flips `status` to the terminal value,
/// sets `finished_at`, populates `summary` + `token_usage_json`
/// + `transcript_json`, and sets `transcript_truncated` to the
/// caller's choice.
#[tokio::test]
async fn subagent_runs_update_finished_sets_status_and_fields() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-test", "general-purpose", None)
        .await
        .unwrap();
    let usage = TokenUsage {
        input_tokens: 1234,
        output_tokens: 567,
        cache_creation_input_tokens: 10,
        cache_read_input_tokens: 20,
    };
    let transcript = vec![crate::agent::subagent::TranscriptEntry {
        kind: crate::agent::subagent::TranscriptKind::ChatEvent,
        payload_json: serde_json::json!({"hello": "world"}),
    }];
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Completed,
        "2026-06-20T00:00:00+00:00",
        "found 3 files",
        "found 3 files",
        &usage,
        &transcript,
        false,
    )
    .await
    .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.status, "completed");
    assert_eq!(row.finished_at.as_deref(), Some("2026-06-20T00:00:00+00:00"));
    assert_eq!(row.summary.as_deref(), Some("found 3 files"));
    assert_eq!(
        row.final_text.as_deref(),
        Some("found 3 files"),
        "final_text column reflects the same final assistant text"
    );
    assert_eq!(row.transcript_truncated, 0);
    let parsed_usage: TokenUsage =
        serde_json::from_str(row.token_usage_json.as_deref().unwrap()).unwrap();
    assert_eq!(parsed_usage.input_tokens, 1234);
    assert_eq!(parsed_usage.output_tokens, 567);
    let parsed_transcript: Vec<crate::agent::subagent::TranscriptEntry> =
        serde_json::from_str(row.transcript_json.as_deref().unwrap()).unwrap();
    assert_eq!(parsed_transcript.len(), 1);
}

/// `transcript_truncated=true` is reflected in the column read.
#[tokio::test]
async fn subagent_runs_update_finished_records_truncated_flag() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-test", "researcher", None)
        .await
        .unwrap();
    let empty = vec![];
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Error,
        "2026-06-20T00:00:00+00:00",
        "",
        "",
        &TokenUsage::default(),
        &empty,
        true,
    )
    .await
    .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.status, "error");
    assert_eq!(row.transcript_truncated, 1);
}

/// `ON DELETE CASCADE`: deleting the parent `sessions` row drops
/// every `subagent_runs` row that references it.
#[tokio::test]
async fn subagent_runs_cascade_delete_with_parent_session() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id1 = insert_run(&pool, &s.id, "rid-1", "researcher", None)
        .await
        .unwrap();
    let id2 = insert_run(&pool, &s.id, "rid-2", "general-purpose", None)
        .await
        .unwrap();
    // Sanity: both rows are there.
    assert!(get_run(&pool, &id1).await.unwrap().is_some());
    assert!(get_run(&pool, &id2).await.unwrap().is_some());

    delete_session(&pool, &s.id).await.unwrap();
    // CASCADE: both rows gone.
    assert!(get_run(&pool, &id1).await.unwrap().is_none());
    assert!(get_run(&pool, &id2).await.unwrap().is_none());
}

/// `list_runs_by_session` returns all runs for the parent
/// session, sorted by `started_at DESC` (newest first).
#[tokio::test]
async fn subagent_runs_list_by_session_orders_by_started_desc() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id1 = insert_run(&pool, &s.id, "rid-1", "researcher", None)
        .await
        .unwrap();
    // Tiny sleep so `started_at` advances between inserts.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let id2 = insert_run(&pool, &s.id, "rid-2", "general-purpose", None)
        .await
        .unwrap();
    let rows = list_runs_by_session(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 2);
    // Newest first → id2 (later insert) before id1.
    assert_eq!(rows[0].id, id2);
    assert_eq!(rows[1].id, id1);
}

/// `add_token_usage_streaming` accumulates per-turn usage into
/// the parent session's 4 token columns. Mirrors the
/// `add_token_usage_accumulates_across_turns` test but uses the
/// streaming wrapper (which the PRD §"db module" requires as
/// PR2 public API).
#[tokio::test]
async fn subagent_runs_token_usage_streaming_accumulates_in_parent() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let u1 = TokenUsage {
        input_tokens: 100,
        output_tokens: 30,
        cache_creation_input_tokens: 5,
        cache_read_input_tokens: 50,
    };
    let u2 = TokenUsage {
        input_tokens: 200,
        output_tokens: 40,
        cache_creation_input_tokens: 25,
        cache_read_input_tokens: 75,
    };
    add_token_usage_streaming(&pool, &s.id, &u1).await.unwrap();
    add_token_usage_streaming(&pool, &s.id, &u2).await.unwrap();
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(loaded.session.input_tokens_total, Some(300));
    assert_eq!(loaded.session.output_tokens_total, Some(70));
    assert_eq!(loaded.session.cache_creation_total, Some(30));
    assert_eq!(loaded.session.cache_read_total, Some(125));
}

/// `add_token_usage_streaming` on a missing parent session id is
/// a silent no-op (matches `add_token_usage`'s contract).
#[tokio::test]
async fn subagent_runs_token_usage_streaming_missing_session_is_noop() {
    let pool = make_pool().await;
    let u = TokenUsage {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
    };
    add_token_usage_streaming(&pool, "nonexistent-session-id", &u)
        .await
        .unwrap();
}

/// B6 PR3a (2026-06-20): `list_runs_summary_by_session` returns
/// the projected `SubagentRunSummary` (no transcript column) for
/// the parent session. Verifies:
/// 1. Newest-first ordering (same as `list_runs_by_session`).
/// 2. The typed `SubagentStatusDb::Completed` enum variant is
///    decoded (NOT the raw wire string) — the frontend renders
///    the status badge from the enum without an extra parse.
/// 3. Summary field carries the worker's final_text verbatim.
#[tokio::test]
async fn subagent_runs_list_runs_summary_by_session_projects_typed_enum() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    // Insert + complete 1 run with a populated transcript + summary.
    let id = insert_run(&pool, &s.id, "rid-summary", "researcher", None)
        .await
        .unwrap();
    let usage = TokenUsage {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
    };
    let transcript = vec![crate::agent::subagent::TranscriptEntry {
        kind: crate::agent::subagent::TranscriptKind::ChatEvent,
        payload_json: serde_json::json!({"text": "hello"}),
    }];
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Completed,
        "2026-06-20T00:00:00+00:00",
        "summary text",
        "summary text",
        &usage,
        &transcript,
        false,
    )
    .await
    .unwrap();

    let summaries = list_runs_summary_by_session(&pool, &s.id)
        .await
        .expect("list_runs_summary_by_session");
    assert_eq!(summaries.len(), 1);
    let sum = &summaries[0];
    assert_eq!(sum.id, id);
    assert_eq!(sum.subagent_name, "researcher");
    assert_eq!(
        sum.status,
        SubagentStatusDb::Completed,
        "status must be decoded to the typed enum (not the wire string)"
    );
    assert_eq!(sum.summary.as_deref(), Some("summary text"));
    assert_eq!(
        sum.final_text.as_deref(),
        Some("summary text"),
        "final_text projected into summary"
    );
    assert!(
        sum.task.is_none(),
        "task=None at insert → column NULL, projected as None"
    );
    assert_eq!(sum.token_usage_json.as_deref(), Some(serde_json::to_string(&usage).unwrap().as_str()));
}

/// B6 PR3a (2026-06-20): `list_runs_summary_by_session` returns
/// an empty `Vec` (NOT an error) for a session with no runs.
#[tokio::test]
async fn subagent_runs_list_runs_summary_by_session_empty() {
    let pool = make_pool().await;
    let summaries = list_runs_summary_by_session(&pool, "nonexistent-session-id")
        .await
        .expect("empty list, no error");
    assert!(summaries.is_empty());
}

// ---------------------------------------------------------------------------
// B6 redesign PR1 (2026-06-21): task + final_text columns
// ---------------------------------------------------------------------------

/// `insert_run` with `task = Some(...)` writes the task verbatim
/// into the `task` column. The drawer reads this as the prompt
/// card header — the prompt must land on the row the moment
/// `insert_run` returns so the user can open the drawer mid-
/// worker (before `update_run_finished` fires).
#[tokio::test]
async fn subagent_runs_insert_writes_task_column() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let task_text = "find all files that mention dispatch_subagent";
    let id = insert_run(&pool, &s.id, "rid-task", "researcher", Some(task_text))
        .await
        .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.task.as_deref(), Some(task_text));
    // task is non-NULL but the worker hasn't run yet → summary +
    // final_text are still NULL.
    assert!(row.summary.is_none());
    assert!(row.final_text.is_none());
    assert!(row.finished_at.is_none(), "still running");
}

/// `insert_run` with `task = None` leaves the column NULL. Mirrors
/// the legacy pre-PR1 behavior — pre-existing test callers pass
/// `None` so the migration remains backward-compatible.
#[tokio::test]
async fn subagent_runs_insert_with_none_task_leaves_column_null() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-no-task", "researcher", None)
        .await
        .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert!(row.task.is_none());
}

/// `update_run_finished` with `final_text` writes the column
/// verbatim — the caller is responsible for pre-stripping the
/// `[status: ...]\n` prefix (via
/// `crate::agent::subagent::format_final_text`). This test
/// exercises the storage-layer contract: the column carries
/// whatever string the caller passes.
#[tokio::test]
async fn subagent_runs_update_finished_writes_final_text_column() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-ft", "general-purpose", Some("do the thing"))
        .await
        .unwrap();
    // Caller passes the prefix-stripped final_text (per the
    // run_subagent contract: format_final_text is invoked at the
    // call site, not inside update_run_finished).
    let stripped = "the worker finished and reported X";
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Completed,
        "2026-06-21T12:00:00+00:00",
        stripped, // summary (legacy wire field)
        stripped, // final_text (drawer Reply segment)
        &TokenUsage::default(),
        &[],
        false,
    )
    .await
    .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.summary.as_deref(), Some(stripped));
    assert_eq!(row.final_text.as_deref(), Some(stripped));
    // task (written at insert) is preserved through the update.
    assert_eq!(row.task.as_deref(), Some("do the thing"));
    // final_text is independent of summary at the column level —
    // a future PR could store different shapes per column (e.g.
    // status-prefixed summary for the wire, prefix-stripped
    // final_text for the UI).
}

/// Cancelled run: `final_text` carries the worker's partial text
/// plus the `[已停止]` marker (the format `format_final_text`
/// produces for `Cancelled` + non-empty worker_text). The status
/// column carries `cancelled` independently.
#[tokio::test]
async fn subagent_runs_update_finished_cancelled_status_and_marker() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-cancel", "researcher", None)
        .await
        .unwrap();
    // Mirror what run_subagent sends for a cancelled run with
    // partial worker text.
    let final_text = format!(
        "partial analysis\n\n{}",
        crate::agent::helpers::CANCELLED_MARKER
    );
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Cancelled,
        "2026-06-21T12:00:00+00:00",
        &final_text,
        &final_text,
        &TokenUsage::default(),
        &[],
        false,
    )
    .await
    .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.status, "cancelled");
    assert!(row.finished_at.is_some());
    assert_eq!(row.final_text.as_deref(), Some(final_text.as_str()));
    assert!(row
        .final_text
        .as_deref()
        .unwrap()
        .contains(crate::agent::helpers::CANCELLED_MARKER));
}

/// Error run: `final_text` carries the error message verbatim
/// (the `format_final_text` shape for `Error`). The `status`
/// column carries `error` independently — the drawer renders
/// the status badge from the column.
#[tokio::test]
async fn subagent_runs_update_finished_error_status_and_text() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-error", "general-purpose", None)
        .await
        .unwrap();
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Error,
        "2026-06-21T12:00:00+00:00",
        "LLM stream errored",
        "LLM stream errored",
        &TokenUsage::default(),
        &[],
        false,
    )
    .await
    .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.status, "error");
    assert_eq!(row.final_text.as_deref(), Some("LLM stream errored"));
}

/// `list_runs_by_session` returns rows with `task` + `final_text`
/// populated (no column is dropped on the list path — the
/// projected shape still carries the new fields).
#[tokio::test]
async fn subagent_runs_list_returns_task_and_final_text() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-list", "researcher", Some("prompt here"))
        .await
        .unwrap();
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Completed,
        "2026-06-21T12:00:00+00:00",
        "found 5 files",
        "found 5 files",
        &TokenUsage::default(),
        &[],
        false,
    )
    .await
    .unwrap();
    let rows = list_runs_by_session(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].task.as_deref(), Some("prompt here"));
    assert_eq!(rows[0].final_text.as_deref(), Some("found 5 files"));
}

/// Migration idempotency: re-running the migration on a pre-PR1
/// DB brings it up to date; re-running on a post-PR1 DB is a
/// no-op. This is the regression guard for the
/// `add_subagent_runs_column_if_missing` helper (analogous to
/// the existing `add_session_column_if_missing` smoke tests).
#[tokio::test]
async fn subagent_runs_migration_is_idempotent_on_pr1_columns() {
    let pool = make_pool().await;
    // First run (above via `make_pool`) already added `task` +
    // `final_text`. Re-run the migration — must NOT error.
    crate::db::migrations::run_migrations(&pool)
        .await
        .expect("migration re-run is idempotent");
    // Columns are still there.
    let exists_task: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('subagent_runs') WHERE name = 'task'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    let exists_final: i64 = sqlx::query(
        "SELECT COUNT(*) FROM pragma_table_info('subagent_runs') WHERE name = 'final_text'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .try_get(0)
    .unwrap();
    assert_eq!(exists_task, 1, "task column present");
    assert_eq!(exists_final, 1, "final_text column present");
}
