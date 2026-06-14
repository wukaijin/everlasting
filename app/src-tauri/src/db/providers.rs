//! Provider CRUD.
//!
//! Each row in the `providers` table is one LLM endpoint the user
//! has registered (e.g. "Anthropic官方" + "第三方Anthropic-compat" both with
//! `protocol=anthropic`). Multiple rows may share the same
//! `protocol`; the `display_name` is what disambiguates them in
//! the UI. The enum dispatch lives in
//! `app/src-tauri/src/llm/provider/` (PR2 wiring).

use chrono::Utc;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use super::types::ProviderRow;

/// Insert a new provider. Returns the inserted row with server-set
/// fields (`id`, `created_at`, `updated_at`) populated. `protocol`
/// is taken as a `String` (not `ProviderProtocol`) so an unknown
/// future value from a newer DB can still be stored without
/// crashing the writer; the read path will fall back to
/// `ProviderProtocol::from_str_opt` for lenient parsing.
pub async fn create_provider(
 pool: &SqlitePool,
 protocol: &str,
 display_name: &str,
 base_url: &str,
 api_key: &str,
) -> Result<ProviderRow, sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let id = Uuid::new_v4().to_string();
 sqlx::query(
 r#"
 INSERT INTO providers
 (id, protocol, display_name, base_url, api_key, created_at, updated_at)
 VALUES (?, ?, ?, ?, ?, ?, ?)
 "#,
 )
 .bind(&id)
 .bind(protocol)
 .bind(display_name)
 .bind(base_url)
 .bind(api_key)
 .bind(&now)
 .bind(&now)
 .execute(pool)
 .await?;
 Ok(ProviderRow {
 id,
 protocol: protocol.to_string(),
 display_name: display_name.to_string(),
 base_url: base_url.to_string(),
 api_key: api_key.to_string(),
 created_at: now.clone(),
 updated_at: now,
 })
}

/// List all providers, newest updated first.
pub async fn list_providers(pool: &SqlitePool) -> Result<Vec<ProviderRow>, sqlx::Error> {
 let rows = sqlx::query(
 r#"
 SELECT id, protocol, display_name, base_url, api_key, created_at, updated_at
 FROM providers
 ORDER BY updated_at DESC
 "#,
 )
 .fetch_all(pool)
 .await?;
 rows.into_iter()
 .map(|r| {
 Ok(ProviderRow {
 id: r.try_get("id")?,
 protocol: r.try_get("protocol")?,
 display_name: r.try_get("display_name")?,
 base_url: r.try_get("base_url")?,
 api_key: r.try_get("api_key")?,
 created_at: r.try_get("created_at")?,
 updated_at: r.try_get("updated_at")?,
 })
 })
 .collect()
}

/// Get a single provider by `id`. Returns `None` when the row
/// doesn't exist. Used by the `test_model` IPC to look up the
/// parent provider given a model id.
pub async fn get_provider(
 pool: &SqlitePool,
 id: &str,
) -> Result<Option<ProviderRow>, sqlx::Error> {
 let row = sqlx::query(
 r#"
 SELECT id, protocol, display_name, base_url, api_key, created_at, updated_at
 FROM providers
 WHERE id = ?
 "#,
 )
 .bind(id)
 .fetch_optional(pool)
 .await?;
 match row {
 None => Ok(None),
 Some(r) => Ok(Some(ProviderRow {
 id: r.try_get("id")?,
 protocol: r.try_get("protocol")?,
 display_name: r.try_get("display_name")?,
 base_url: r.try_get("base_url")?,
 api_key: r.try_get("api_key")?,
 created_at: r.try_get("created_at")?,
 updated_at: r.try_get("updated_at")?,
 })),
 }
}

/// Patch a provider by `id`. Returns `None` if the row doesn't
/// exist; otherwise returns the updated row. `updated_at` is bumped
/// to the current time on every successful update.
pub async fn update_provider(
 pool: &SqlitePool,
 id: &str,
 protocol: &str,
 display_name: &str,
 base_url: &str,
 api_key: &str,
) -> Result<Option<ProviderRow>, sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let res = sqlx::query(
 r#"
 UPDATE providers
 SET protocol = ?, display_name = ?, base_url = ?,
 api_key = ?, updated_at = ?
 WHERE id = ?
 "#,
 )
 .bind(protocol)
 .bind(display_name)
 .bind(base_url)
 .bind(api_key)
 .bind(&now)
 .bind(id)
 .execute(pool)
 .await?;
 if res.rows_affected() == 0 {
 return Ok(None);
 }
 Ok(Some(ProviderRow {
 id: id.to_string(),
 protocol: protocol.to_string(),
 display_name: display_name.to_string(),
 base_url: base_url.to_string(),
 api_key: api_key.to_string(),
 created_at: String::new(), // not reloaded; callers that need it should re-fetch
 updated_at: now,
 }))
}

/// Delete a provider by `id`. Cascades to its models (FK is
/// `ON DELETE CASCADE`). Returns whether a row was actually
/// removed.
pub async fn delete_provider(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
 let res = sqlx::query("DELETE FROM providers WHERE id = ?")
 .bind(id)
 .execute(pool)
 .await?;
 Ok(res.rows_affected() >0)
}
