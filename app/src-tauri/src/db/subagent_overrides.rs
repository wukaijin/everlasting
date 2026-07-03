//! subagent_model_overrides — global per-subagent model preference.
//!
//! 2026-07-03 (task 07-03-subagent-per-agent-model-ui): priority
//! `DB override > frontmatter > parent` for per-subagent model
//! selection. The DB row stores a user-managed preference keyed by
//! `agent_name` (the subagent's `SubagentDef.name`); builtin
//! subagents (which have NO frontmatter file to edit) get
//! configuration via this table. User / project subagents can also
//! be configured here — the DB row wins over their frontmatter
//! `model:` line per the priority chain (handy for "force this
//! agent to a specific model for this workspace regardless of
//! what the `.md` says").
//!
//! **Scope is global** (single row per `agent_name`, shared across
//! all projects) — consistent with builtin subagents being global
//! by definition, and the simplest schema (no project FK). Per-
//! project override is a follow-up.
//!
//! `model_id` is a *logical* reference to `models.id` (UUID). It
//! has NO FK constraint — soft reference, matching
//! `sessions.model_id` (see `database-guidelines.md` "Soft FK
//! pattern"). A deleted model leaves a dangling `model_id`; the
//! dispatch path's `resolve_worker_provider` already logs `warn!`
//! + falls back to the parent provider on catalog miss, so the
//! failure is graceful. The Settings UI surfaces invalid overrides
//! with a red "model 已删除" label so the user can fix it.
//!
//! Schema:
//! ```sql
//! CREATE TABLE IF NOT EXISTS subagent_model_overrides (
//!   agent_name TEXT NOT NULL PRIMARY KEY,
//!   model_id   TEXT NOT NULL,
//!   updated_at TEXT NOT NULL
//! )
//! ```

use chrono::Utc;
use sqlx::{FromRow, SqlitePool};

// ---------------------------------------------------------------------------
// Row type (camelCase on the wire for IPC parity with the rest of the
// project; see `database-guidelines.md` "When you add a new user-managed
// catalog" checklist).
// ---------------------------------------------------------------------------

/// One DB row from `subagent_model_overrides`. `model_id` is the
/// `models.id` UUID; `updated_at` is RFC 3339. The Settings UI
/// receives this shape verbatim and maps `model_id` → `display_name`
/// via the existing `useModelsStore` list.
///
/// `allow(dead_code)` because the current production IPC surface
/// (the `list_subagents_with_model` + `set_subagent_model` commands
/// in `commands::subagents`) projects the row into
/// `SubagentWithModelRow` rather than passing this type across the
/// IPC boundary directly. The struct is kept here as the canonical
/// DB-row shape so future commands (e.g. a `list_subagent_overrides`
/// audit endpoint) can re-use it without a second projection.
#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SubagentModelOverrideRow {
    pub agent_name: String,
    pub model_id: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Public API — best-effort, no transactions
//
// All four helpers return `Result<_, sqlx::Error>` per the project's
// db-layer convention (see `database-guidelines.md` "Error handling").
// The Tauri command layer wraps them via `?` + `anyhow::Error`; the
// `set_subagent_model_override` command's caller treats the result
// as a hard error (the user-facing IPC, so a failure must surface
// as a toast).
// ---------------------------------------------------------------------------

/// Look up the override for a given subagent name. Returns `Ok(None)`
/// when no row exists (the common case — most agents have no DB
/// override and inherit parent / frontmatter).
pub async fn get_subagent_model_override(
    pool: &SqlitePool,
    agent_name: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT model_id FROM subagent_model_overrides WHERE agent_name = ?")
            .bind(agent_name)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(m,)| m))
}

/// Set (insert or update) the override for a given subagent name.
/// UPSERT via `ON CONFLICT(agent_name) DO UPDATE` — the
/// `agent_name` is the PRIMARY KEY, so a second call replaces the
/// previous model without a race window. `updated_at` is stamped
/// fresh on every write.
pub async fn set_subagent_model_override(
    pool: &SqlitePool,
    agent_name: &str,
    model_id: &str,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO subagent_model_overrides (agent_name, model_id, updated_at)
        VALUES (?, ?, ?)
        ON CONFLICT(agent_name) DO UPDATE SET
            model_id = excluded.model_id,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(agent_name)
    .bind(model_id)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Clear the override for a given subagent name. Resolves to
/// `Ok(())` regardless of whether a row existed (mirrors the
/// project's defensive delete-everywhere semantics — same shape as
/// `delete_message_metadata` / `clear_session_permissions`).
pub async fn clear_subagent_model_override(
    pool: &SqlitePool,
    agent_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM subagent_model_overrides WHERE agent_name = ?")
        .bind(agent_name)
        .execute(pool)
        .await?;
    Ok(())
}

/// List all overrides as `(agent_name, model_id)` tuples. The
/// Settings UI uses this to decorate each row with a "DB override"
/// badge + to surface the resolved model without round-tripping
/// each agent through a separate `get_subagent_model_override` call.
///
/// The query is `ORDER BY agent_name` for a stable UI render order
/// (a HashMap iteration would be non-deterministic; the IPC contract
/// needs a stable order so the UI doesn't reshuffle on re-fetch).
pub async fn list_subagent_model_overrides(
    pool: &SqlitePool,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT agent_name, model_id FROM subagent_model_overrides ORDER BY agent_name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
