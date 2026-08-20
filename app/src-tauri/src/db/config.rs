//! `app_config` key/value helpers + the one-time seed for the
//! default `providers` / `models` / `default_model_id`.
//!
//! The `app_config` table is a small KV store for global settings;
//! the only key written today is `default_model_id`, but the table
//! is generic so future global settings don't need a new migration.
//!
//! `seed_default_providers_and_models` is idempotent: when at least
//! one provider row exists, it returns without inserting anything
//! (preserves any user edits). On first run it inserts:
//!
//! - `Anthropic官方` provider, base URL `https://api.anthropic.com`,
//! empty api_key
//! - `OpenAI官方` provider, base URL `https://api.openai.com/v1`,
//! empty api_key
//! - `claude-sonnet-4-5` model bound to Anthropic, `supports_thinking=true`,
//! `context_window=200_000`
//! - `claude-opus-4-7` model bound to Anthropic, `supports_thinking=true`,
//! `context_window=200_000`
//! - `gpt-4o` model bound to OpenAI, `supports_thinking=false`,
//! `context_window=128_000`
//! - `gpt-4.1` model bound to OpenAI, `supports_thinking=false`,
//! `context_window=1_000_000`
//! - `default_model_id` -> `claude-sonnet-4-5`
//!
//! After the catalog is in place, backfills `sessions.model_id` for
//! any row still NULL or empty (legacy sessions from the pre-PR1
//! era) with the default model id.

use chrono::Utc;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

/// Read a value from `app_config` by key. Returns `None` if the key
/// is not present. Kept as a generic key/value getter so future
/// global settings don't need a new IPC.
pub async fn get_config_value(pool: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT value FROM app_config WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    row.map(|r| r.try_get("value")).transpose()
}

/// Write a value to `app_config`, inserting or replacing the row.
pub async fn set_config_value(
    pool: &SqlitePool,
    key: &str,
    value: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
 INSERT INTO app_config (key, value) VALUES (?, ?)
 ON CONFLICT(key) DO UPDATE SET value = excluded.value
 "#,
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// 08-20-turn-usage-event-quota-view WP2: 删除一个 config 键(quota
/// 额度"清除"语义)。表名 `app_config` 的 schema 知识留在本层 ——
/// command 层裸写 DELETE 曾写错表名(质检 Step 5 实证)。
pub async fn delete_config_value(pool: &SqlitePool, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM app_config WHERE key = ?")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/// Seed the default `providers` + `models` + `default_model_id` if
/// the `providers` table is empty. Idempotent: when at least one
/// provider already exists, the function is a no-op (preserves any
/// user edits). When run, it inserts the catalog described in the
/// module-level docstring above.
///
/// After the catalog is in place, backfills `sessions.model_id` for
/// any row still NULL or empty (legacy sessions from the pre-PR1
/// era) with the default model id.
pub async fn seed_default_providers_and_models(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let count: i64 = sqlx::query("SELECT COUNT(*) FROM providers")
        .fetch_one(pool)
        .await?
        .try_get(0)?;
    if count > 0 {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();

    // --- providers ---
    let anthropic_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
 INSERT INTO providers
 (id, protocol, display_name, base_url, api_key, created_at, updated_at)
 VALUES (?, 'anthropic', 'Anthropic官方', 'https://api.anthropic.com', '', ?, ?)
 "#,
    )
    .bind(&anthropic_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    let openai_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
 INSERT INTO providers
 (id, protocol, display_name, base_url, api_key, created_at, updated_at)
 VALUES (?, 'openai', 'OpenAI官方', 'https://api.openai.com/v1', '', ?, ?)
 "#,
    )
    .bind(&openai_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    // --- models ---
    // `supports_images` per real vendor capability (B1, 2026-08-16):
    // Claude Sonnet/Opus and gpt-4o accept image input; gpt-4.1 is
    // text-only. Users override per row in Settings.
    let sonnet_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
 INSERT INTO models
 (id, provider_id, model_name, display_name, max_tokens, thinking_effort,
 supports_thinking, supports_images, context_window, created_at, updated_at)
 VALUES (?, ?, 'claude-sonnet-4-5', 'Claude Sonnet4.5',
 NULL, NULL,1,1,200000, ?, ?)
 "#,
    )
    .bind(&sonnet_id)
    .bind(&anthropic_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    let opus_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
 INSERT INTO models
 (id, provider_id, model_name, display_name, max_tokens, thinking_effort,
 supports_thinking, supports_images, context_window, created_at, updated_at)
 VALUES (?, ?, 'claude-opus-4-7', 'Claude Opus4.7',
 NULL, NULL,1,1,200000, ?, ?)
 "#,
    )
    .bind(&opus_id)
    .bind(&anthropic_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    let gpt4o_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
 INSERT INTO models
 (id, provider_id, model_name, display_name, max_tokens, thinking_effort,
 supports_thinking, supports_images, context_window, created_at, updated_at)
 VALUES (?, ?, 'gpt-4o', 'GPT-4o',
 NULL, NULL,0,1,128000, ?, ?)
 "#,
    )
    .bind(&gpt4o_id)
    .bind(&openai_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    let gpt41_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
 INSERT INTO models
 (id, provider_id, model_name, display_name, max_tokens, thinking_effort,
 supports_thinking, supports_images, context_window, created_at, updated_at)
 VALUES (?, ?, 'gpt-4.1', 'GPT-4.1',
 NULL, NULL,0,0,1000000, ?, ?)
 "#,
    )
    .bind(&gpt41_id)
    .bind(&openai_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    // --- default model ---
    set_config_value(pool, "default_model_id", &sonnet_id).await?;

    // --- backfill sessions.model_id with the default ---
    sqlx::query("UPDATE sessions SET model_id = ? WHERE model_id IS NULL OR model_id = ''")
        .bind(&sonnet_id)
        .execute(pool)
        .await?;

    Ok(())
}
