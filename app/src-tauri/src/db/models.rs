//! Model CRUD.
//!
//! Each row in the `models` table binds a `model_name` to one
//! `ProviderRow` via `provider_id` (FK with `ON DELETE CASCADE`),
//! carrying per-row capability hints (`supports_thinking`,
//! `context_window`) and optional overrides for the global env
//! defaults (`max_tokens`, `thinking_effort`). The read path
//! (`list_models`) joins with `providers` to denormalize the
//! `display_name` + `protocol` into [`ModelWithProvider`] so the
//! frontend's model picker can render in one IPC roundtrip.

use chrono::Utc;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use super::types::{ModelRow, ModelWithProvider};

/// Insert a new model. Returns the inserted row.
pub async fn create_model(
 pool: &SqlitePool,
 provider_id: &str,
 model_name: &str,
 display_name: &str,
 max_tokens: Option<u32>,
 thinking_effort: Option<&str>,
 supports_thinking: bool,
 context_window: u32,
) -> Result<ModelRow, sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let id = Uuid::new_v4().to_string();
 sqlx::query(
 r#"
 INSERT INTO models
 (id, provider_id, model_name, display_name, max_tokens, thinking_effort,
 supports_thinking, context_window, created_at, updated_at)
 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
 "#,
 )
 .bind(&id)
 .bind(provider_id)
 .bind(model_name)
 .bind(display_name)
 .bind(max_tokens)
 .bind(thinking_effort)
 .bind(supports_thinking as i32)
 .bind(context_window)
 .bind(&now)
 .bind(&now)
 .execute(pool)
 .await?;
 Ok(ModelRow {
 id,
 provider_id: provider_id.to_string(),
 model_name: model_name.to_string(),
 display_name: display_name.to_string(),
 max_tokens,
 thinking_effort: thinking_effort.map(str::to_string),
 supports_thinking,
 context_window,
 created_at: now.clone(),
 updated_at: now,
 })
}

/// List all models joined with their parent provider's
/// `display_name` + `protocol` for UI rendering. Newest updated
/// first; within a model, sort is by `display_name` ascending.
pub async fn list_models(pool: &SqlitePool) -> Result<Vec<ModelWithProvider>, sqlx::Error> {
 let rows = sqlx::query(
 r#"
 SELECT m.id, m.provider_id, m.model_name, m.display_name,
 m.max_tokens, m.thinking_effort, m.supports_thinking,
 m.context_window, m.created_at, m.updated_at,
 p.display_name AS provider_display_name,
 p.protocol AS provider_protocol
 FROM models m
 JOIN providers p ON p.id = m.provider_id
 ORDER BY m.updated_at DESC, m.display_name ASC
 "#,
 )
 .fetch_all(pool)
 .await?;
 rows.into_iter()
 .map(|r| {
 let supports_thinking_i: i32 = r.try_get("supports_thinking")?;
 Ok(ModelWithProvider {
 model: ModelRow {
 id: r.try_get("id")?,
 provider_id: r.try_get("provider_id")?,
 model_name: r.try_get("model_name")?,
 display_name: r.try_get("display_name")?,
 max_tokens: r.try_get("max_tokens")?,
 thinking_effort: r.try_get("thinking_effort")?,
 supports_thinking: supports_thinking_i != 0,
 context_window: r.try_get("context_window")?,
 created_at: r.try_get("created_at")?,
 updated_at: r.try_get("updated_at")?,
 },
 provider_display_name: r.try_get("provider_display_name")?,
 provider_protocol: r.try_get("provider_protocol")?,
 })
 })
 .collect()
}

/// Get a single model row by `id` (no provider join). Returns
/// `None` when the row doesn't exist. Used by the `test_model`
/// IPC, which then looks up the parent provider separately.
pub async fn get_model(
 pool: &SqlitePool,
 id: &str,
) -> Result<Option<ModelRow>, sqlx::Error> {
 let row = sqlx::query(
 r#"
 SELECT id, provider_id, model_name, display_name,
 max_tokens, thinking_effort, supports_thinking,
 context_window, created_at, updated_at
 FROM models
 WHERE id = ?
 "#,
 )
 .bind(id)
 .fetch_optional(pool)
 .await?;
 match row {
 None => Ok(None),
 Some(r) => {
 let supports_thinking_i: i32 = r.try_get("supports_thinking")?;
 Ok(Some(ModelRow {
 id: r.try_get("id")?,
 provider_id: r.try_get("provider_id")?,
 model_name: r.try_get("model_name")?,
 display_name: r.try_get("display_name")?,
 max_tokens: r.try_get("max_tokens")?,
 thinking_effort: r.try_get("thinking_effort")?,
 supports_thinking: supports_thinking_i != 0,
 context_window: r.try_get("context_window")?,
 created_at: r.try_get("created_at")?,
 updated_at: r.try_get("updated_at")?,
 }))
 }
 }
}

/// Patch a model by `id`. Returns `None` if the row doesn't exist.
pub async fn update_model(
 pool: &SqlitePool,
 id: &str,
 provider_id: &str,
 model_name: &str,
 display_name: &str,
 max_tokens: Option<u32>,
 thinking_effort: Option<&str>,
 supports_thinking: bool,
 context_window: u32,
) -> Result<Option<ModelRow>, sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let res = sqlx::query(
 r#"
 UPDATE models
 SET provider_id = ?, model_name = ?, display_name = ?,
 max_tokens = ?, thinking_effort = ?,
 supports_thinking = ?, context_window = ?, updated_at = ?
 WHERE id = ?
 "#,
 )
 .bind(provider_id)
 .bind(model_name)
 .bind(display_name)
 .bind(max_tokens)
 .bind(thinking_effort)
 .bind(supports_thinking as i32)
 .bind(context_window)
 .bind(&now)
 .bind(id)
 .execute(pool)
 .await?;
 if res.rows_affected() == 0 {
 return Ok(None);
 }
 Ok(Some(ModelRow {
 id: id.to_string(),
 provider_id: provider_id.to_string(),
 model_name: model_name.to_string(),
 display_name: display_name.to_string(),
 max_tokens,
 thinking_effort: thinking_effort.map(str::to_string),
 supports_thinking,
 context_window,
 created_at: String::new(),
 updated_at: now,
 }))
}

/// Delete a model by `id`. Returns whether a row was actually
/// removed. Sessions that referenced this model keep the dangling
/// `model_id` (it's a soft FK with no `ON DELETE` clause); the
/// reader path is responsible for the fallback (PR2 wires the
/// resolve-default fallback in the agent loop).
pub async fn delete_model(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
 let res = sqlx::query("DELETE FROM models WHERE id = ?")
 .bind(id)
 .execute(pool)
 .await?;
 Ok(res.rows_affected() >0)
}
