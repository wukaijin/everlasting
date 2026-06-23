#![cfg(test)]

//! Providers-domain integration tests (split from `db/tests.rs` on 2026-06-23).
//!
//! Coverage:
//! - Provider CRUD (create / list / update / delete + cascade to models)
//! - Model CRUD (create / list with provider join / update / delete)
//! - app_config seed (default providers + models + `default_model_id`)
//! - config set/get round-trip
//! - `delete_provider` cascade does not touch unrelated models

use sqlx::{Row, SqlitePool};

use super::{
    config::{get_config_value, seed_default_providers_and_models, set_config_value},
    migrations::run_migrations,
    models::{create_model, delete_model, list_models, update_model},
    providers::{create_provider, delete_provider, list_providers, update_provider},
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

// ---------------------------------------------------------------------------
// PR1 of multi-model task: providers / models / app_config tests
//
// Each CRUD function gets a happy path + a forced-error / edge-case
// test. The "create_session" / "sessions.model_id" interactions are
// covered separately in the seed test.
// ---------------------------------------------------------------------------

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
 Some("sk-ghost"),
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
// RULE-D-001 (2026-06-24): api_key 加密存储测试
//
// 验证 AC: DB 不存明文 / list_providers 解密往返 / 明文迁移幂等.
// 加解密原语 (roundtrip / AAD / tamper) 单测在 `crypto::tests`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_key_is_encrypted_not_plaintext_in_db() {
 let pool = make_pool().await;
 let p = create_provider(
 &pool,
 "anthropic",
 "Enc test",
 "https://a.example.com",
 "sk-secret-123",
 )
 .await
 .unwrap();
 // Legacy `api_key` 列必须为空 (新行从未写明文); `api_key_enc`
 // 必须非空且不含明文.
 let row: (String, String) =
 sqlx::query_as("SELECT api_key, api_key_enc FROM providers WHERE id = ?")
 .bind(&p.id)
 .fetch_one(&pool)
 .await
 .unwrap();
 assert_eq!(row.0, "", "legacy api_key column must be empty");
 assert!(!row.1.is_empty(), "api_key_enc must hold ciphertext");
 assert!(
 !row.1.contains("sk-secret-123"),
 "ciphertext must not leak plaintext"
 );
 // list_providers 解密往返.
 let listed = list_providers(&pool).await.unwrap();
 let got = listed.iter().find(|r| r.id == p.id).expect("provider listed");
 assert_eq!(got.api_key, "sk-secret-123");
 assert!(got.has_key);
}

#[tokio::test]
async fn plaintext_api_key_migration_is_idempotent() {
 let pool = make_pool().await;
 // 模拟旧版明文行: api_key 明文, key_migrated_at NULL, api_key_enc 空.
 sqlx::query(
 "INSERT INTO providers \
 (id, protocol, display_name, base_url, api_key, api_key_enc, created_at, updated_at) \
 VALUES ('migrate-1', 'anthropic', 'M', 'https://x.example.com', 'sk-plain', '', \
 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
 )
 .execute(&pool)
 .await
 .unwrap();
 // 迁移: run_migrations 幂等 (列已存在, 迁移函数处理明文行).
 run_migrations(&pool).await.unwrap();
 let row: (String, String, Option<String>) = sqlx::query_as(
 "SELECT api_key, api_key_enc, key_migrated_at FROM providers WHERE id = 'migrate-1'",
 )
 .fetch_one(&pool)
 .await
 .unwrap();
 assert_eq!(row.0, "", "plaintext migrated away");
 assert!(!row.1.is_empty(), "ciphertext written");
 assert!(row.2.is_some(), "sentinel set");
 let first_enc = row.1;
 // 第二次迁移: 幂等, 明文列仍空, 密文不变 (无重复加密).
 run_migrations(&pool).await.unwrap();
 let row2: (String, String) =
 sqlx::query_as("SELECT api_key, api_key_enc FROM providers WHERE id = 'migrate-1'")
 .fetch_one(&pool)
 .await
 .unwrap();
 assert_eq!(row2.0, "", "still empty after 2nd migration");
 assert_eq!(row2.1, first_enc, "ciphertext unchanged (idempotent)");
}
