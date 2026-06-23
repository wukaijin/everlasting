//! Provider CRUD.
//!
//! Each row in the `providers` table is one LLM endpoint the user
//! has registered (e.g. "Anthropic官方" + "第三方Anthropic-compat" both with
//! `protocol=anthropic`). Multiple rows may share the same
//! `protocol`; the `display_name` is what disambiguates them in
//! the UI. The enum dispatch lives in
//! `app/src-tauri/src/llm/provider/` (PR2 wiring).
//!
//! RULE-D-001 (P1, 2026-06-24): `api_key` 不再明文存储. 写入侧把明文
//! `crypto::encrypt(.., aad = provider id)` 后写入 `api_key_enc`;
//! 读取侧 `crypto::decrypt` 还原明文填进 [`ProviderRow::api_key`]
//! (内部消费, `#[serde(skip)]` 不经 IPC 暴露). 解密失败(机器变化/
//! 损坏)降级为空串 + warn, 不阻断 list/get.

use chrono::Utc;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use super::types::ProviderRow;

/// 把 `crypto` 的 `anyhow::Error` 包成 `sqlx::Error::Configuration`,
/// 让 CRUD 函数走统一的 `Result<_, sqlx::Error>` 返回类型.
fn crypto_err(e: anyhow::Error) -> sqlx::Error {
 sqlx::Error::Configuration(format!("{e}").into())
}

/// 解密 `api_key_enc` 到明文; 空串原样返回空; 解密失败降级空串 + warn
/// (调用方据此在 pre-flight 区分"未填 key"vs"解密失败").
fn decrypt_api_key_or_empty(master_key: &[u8; 32], enc: &str, id: &str) -> String {
 if enc.is_empty() {
 return String::new();
 }
 match crate::crypto::decrypt(master_key, enc, id) {
 Ok(s) => s,
 Err(e) => {
 tracing::warn!(
 provider_id = %id,
 error = %e,
 "api_key decrypt failed (machine-id change?); degrading to empty"
 );
 String::new()
 }
 }
}

/// Insert a new provider. `api_key` is the plaintext from IPC; it is
/// encrypted (AAD = the new provider id) before hitting the DB. The
/// legacy `api_key` column is written as `''` (kept for SQLite < 3.35
/// DROP-COLUMN reasons; never read back). Returns the inserted row
/// with server-set fields populated; `api_key` carries the plaintext
/// back for the in-memory catalog, `has_key` reflects whether a key
/// was set.
pub async fn create_provider(
 pool: &SqlitePool,
 protocol: &str,
 display_name: &str,
 base_url: &str,
 api_key: &str,
) -> Result<ProviderRow, sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let id = Uuid::new_v4().to_string();
 let master_key = crate::crypto::derive_master_key().map_err(crypto_err)?;
 let enc = crate::crypto::encrypt(&master_key, api_key, &id).map_err(crypto_err)?;
 sqlx::query(
 r#"
 INSERT INTO providers
 (id, protocol, display_name, base_url, api_key, api_key_enc, key_migrated_at,
  created_at, updated_at)
 VALUES (?, ?, ?, ?, '', ?, ?, ?, ?)
 "#,
 )
 .bind(&id)
 .bind(protocol)
 .bind(display_name)
 .bind(base_url)
 .bind(&enc)
 .bind(&now)
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
 has_key: !api_key.is_empty(),
 created_at: now.clone(),
 updated_at: now,
 })
}

/// List all providers, newest updated first. Reads `api_key_enc` and
/// decrypts to plaintext in-memory (the catalog / `build_provider`
/// path needs the plaintext; the IPC layer never sees it because
/// `ProviderRow::api_key` is `#[serde(skip)]`).
pub async fn list_providers(pool: &SqlitePool) -> Result<Vec<ProviderRow>, sqlx::Error> {
 let rows = sqlx::query(
 r#"
 SELECT id, protocol, display_name, base_url, api_key_enc, created_at, updated_at
 FROM providers
 ORDER BY updated_at DESC
 "#,
 )
 .fetch_all(pool)
 .await?;
 let master_key = crate::crypto::derive_master_key().map_err(crypto_err)?;
 rows.into_iter()
 .map(|r| {
 let id: String = r.try_get("id")?;
 let enc: String = r.try_get("api_key_enc")?;
 let api_key = decrypt_api_key_or_empty(&master_key, &enc, &id);
 Ok(ProviderRow {
 has_key: !enc.is_empty(),
 api_key,
 id,
 protocol: r.try_get("protocol")?,
 display_name: r.try_get("display_name")?,
 base_url: r.try_get("base_url")?,
 created_at: r.try_get("created_at")?,
 updated_at: r.try_get("updated_at")?,
 })
 })
 .collect()
}

/// Get a single provider by `id`. Returns `None` when the row
/// doesn't exist. Decrypts `api_key_enc` (used by `test_model` to
/// reach the provider with the live key).
pub async fn get_provider(
 pool: &SqlitePool,
 id: &str,
) -> Result<Option<ProviderRow>, sqlx::Error> {
 let row = sqlx::query(
 r#"
 SELECT id, protocol, display_name, base_url, api_key_enc, created_at, updated_at
 FROM providers
 WHERE id = ?
 "#,
 )
 .bind(id)
 .fetch_optional(pool)
 .await?;
 match row {
 None => Ok(None),
 Some(r) => {
 let master_key = crate::crypto::derive_master_key().map_err(crypto_err)?;
 let rid: String = r.try_get("id")?;
 let enc: String = r.try_get("api_key_enc")?;
 Ok(Some(ProviderRow {
 api_key: decrypt_api_key_or_empty(&master_key, &enc, &rid),
 has_key: !enc.is_empty(),
 id: rid,
 protocol: r.try_get("protocol")?,
 display_name: r.try_get("display_name")?,
 base_url: r.try_get("base_url")?,
 created_at: r.try_get("created_at")?,
 updated_at: r.try_get("updated_at")?,
 }))
 }
 }
}

/// Patch a provider by `id`. Returns `None` if the row doesn't exist.
///
/// `api_key: Option<&str>` drives the留空覆盖 UX (RULE-D-001 Decision #2):
/// - `Some(new)` → 加密覆盖 `api_key_enc`
/// - `None` → 保持原 key 不动 (用户编辑时留空 = "保持不变")
///
/// `updated_at` is bumped on every successful update. The returned
/// row is re-read via [`get_provider`] so `has_key` / `api_key` are
/// accurate (the `None` branch can't know the stored key without a
/// re-read).
pub async fn update_provider(
 pool: &SqlitePool,
 id: &str,
 protocol: &str,
 display_name: &str,
 base_url: &str,
 api_key: Option<&str>,
) -> Result<Option<ProviderRow>, sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let affected = if let Some(new_key) = api_key {
 let master_key = crate::crypto::derive_master_key().map_err(crypto_err)?;
 let enc = crate::crypto::encrypt(&master_key, new_key, id).map_err(crypto_err)?;
 sqlx::query(
 r#"
 UPDATE providers
 SET protocol = ?, display_name = ?, base_url = ?,
     api_key_enc = ?, updated_at = ?
 WHERE id = ?
 "#,
 )
 .bind(protocol)
 .bind(display_name)
 .bind(base_url)
 .bind(&enc)
 .bind(&now)
 .bind(id)
 .execute(pool)
 .await?
 .rows_affected()
 } else {
 sqlx::query(
 r#"
 UPDATE providers
 SET protocol = ?, display_name = ?, base_url = ?, updated_at = ?
 WHERE id = ?
 "#,
 )
 .bind(protocol)
 .bind(display_name)
 .bind(base_url)
 .bind(&now)
 .bind(id)
 .execute(pool)
 .await?
 .rows_affected()
 };
 if affected == 0 {
 return Ok(None);
 }
 // Re-read for accurate has_key / api_key (None branch can't infer).
 get_provider(pool, id).await
}

/// Delete a provider by `id`. Cascades to its models (FK is
/// `ON DELETE CASCADE`). Returns whether a row was actually
/// removed.
pub async fn delete_provider(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
 let res = sqlx::query("DELETE FROM providers WHERE id = ?")
 .bind(id)
 .execute(pool)
 .await?;
 Ok(res.rows_affected() > 0)
}
