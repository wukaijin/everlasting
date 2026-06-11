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
 add_token_usage, create_session, delete_session, find_message_id_by_seq,
 insert_system_event, list_sessions, load_session, persist_turn,
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
 };
 persist_turn(&pool, &s.id, Role::Assistant, &content, 0, Some(&latency))
 .await
 .unwrap();

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 let m = loaded.messages.first().expect("one message");
 assert_eq!(m.ttfb_ms, Some(420));
 assert_eq!(m.gen_ms, Some(2100));
 assert_eq!(m.total_ms, Some(3200));
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
 },
 )
 .await
 .unwrap();

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 let m = loaded.messages.first().expect("one message");
 assert_eq!(m.ttfb_ms, Some(100));
 assert_eq!(m.gen_ms, Some(200));
 assert_eq!(m.total_ms, Some(300));
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
 },
 )
 .await
 .unwrap();

 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 let m = loaded.messages.first().expect("one message");
 assert!(m.ttfb_ms.is_none());
 assert!(m.gen_ms.is_none());
 assert_eq!(m.total_ms, Some(500));
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

