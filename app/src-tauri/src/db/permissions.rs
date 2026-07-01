//! A2 + B7 (Permission system + per-session Mode) — DB CRUD.
//!
//! Three concerns live here:
//!
//! 1. **Per-session Mode persistence** ([`update_session_mode`]) —
//! the `sessions.mode` column that drives both the ⑨ 关 permission
//! policy and the ⑧a Mode check (tool list filtering + system
//! prompt prefix + runtime intercept).
//!
//! 2. **Per-session "always allow" set**
//! ([`grant_tool_permission`] / [`has_tool_permission`] /
//! [`revoke_tool_permission`]) — backs the ⑨ 关 Tier 4 short-circuit
//! ("this session has previously granted this tool, don't ask again").
//! All three `match_kind` values are now live:
//! - `tool` — whole-tool grant (e.g. web_fetch); `match_value IS NULL`.
//! - `path` — sqlite GLOB on a filesystem path (path tools); checked
//!   by `permissions::check_path_grant`.
//! - `prefix` — exact command-prefix match for `shell` (e.g. `cargo`);
//!   the write side (`match_value_for_allow_always`) has existed since
//!   the re-grill; the read side (`permissions::check_prefix_grant`)
//!   was wired in the 三档分类 refactor (2026-06-14), closing the old
//!   "stored but never queried" gap.
//!
//! 3. **Audit log persistence** ([`record_audit_event`] /
//! [`list_audit_events`]) — every ⑨ 关 decision path hits the
//! audit hook with a typed [`AuditKind`] and a JSON payload. The
//! UI query side (C4) is out of scope for A2; PR1 only writes
//! the rows.
//!
//! All functions return `Result<T, sqlx::Error>` (no logging) so
//! the caller decides how to surface the error (the agent loop
//! wraps each call in `tracing::warn!` on failure).

use serde::Serialize;
use sqlx::{Row, SqlitePool};

use super::Mode;

// ---------------------------------------------------------------------------
// Per-session Mode persistence
// ---------------------------------------------------------------------------

/// Update the `mode` column on a session row. Called by the
/// `set_session_mode` Tauri command on every user toggle of the
/// `ModeSelect.vue` dropdown. The function is a single UPDATE
/// statement; the new value is `mode.as_str()` (lowercase string).
///
/// `updated_at` is bumped to the current time so the session
/// list re-sorts correctly (matches `update_session_model_id` /
/// `rename_session` / `set_session_color` semantics).
///
/// Returns `Ok(())` on success; `Err(sqlx::Error)` if the DB
/// write fails. The frontend surfaces the error as a toast.
pub async fn update_session_mode(
 pool: &SqlitePool,
 session_id: &str,
 mode: Mode,
) -> Result<(), sqlx::Error> {
 let now = chrono::Utc::now().to_rfc3339();
 sqlx::query(
 r#"
 UPDATE sessions
 SET mode = ?, updated_at = ?
 WHERE id = ?
 "#,
 )
 .bind(mode.as_str())
 .bind(&now)
 .bind(session_id)
 .execute(pool)
 .await?;
 Ok(())
}

// ---------------------------------------------------------------------------
// Per-session "always allow" set
// ---------------------------------------------------------------------------

/// Grant a "always allow" permission for `(session_id, tool_name)`
/// with the given `match_kind` and `match_value`. UPSERT semantics:
/// re-granting the same `(session_id, tool_name, match_kind,
/// match_value)` row updates `granted_at` to the current time
/// instead of inserting a duplicate.
///
/// MVP scope: `match_kind = 'tool'`, `match_value = NULL`. The
/// 3-kind schema is reserved for a future PR (`prefix` for shell
/// command prefixes, `path` for file path globs).
///
/// Returns `Ok(())` on success. The frontend re-loads
/// `session_tool_permissions` lazily (no separate IPC — the agent
/// loop's next turn reads from DB on every tool_use decision).
pub async fn grant_tool_permission(
 pool: &SqlitePool,
 session_id: &str,
 tool_name: &str,
 match_kind: &str,
 match_value: Option<&str>,
) -> Result<(), sqlx::Error> {
 sqlx::query(
 r#"
 INSERT INTO session_tool_permissions
 (session_id, tool_name, match_kind, match_value, granted_at)
 VALUES (?, ?, ?, ?, datetime('now'))
 ON CONFLICT(session_id, tool_name, match_kind, match_value)
 DO UPDATE SET granted_at = datetime('now')
 "#,
 )
 .bind(session_id)
 .bind(tool_name)
 .bind(match_kind)
 .bind(match_value)
 .execute(pool)
 .await?;
 Ok(())
}

/// Returns `true` if the session has an "always allow" row for
/// `tool_name`. Used by ⑨ 关 Tier 3 to short-circuit the
/// permission modal: the user previously clicked "始终允许" on
/// this tool, so subsequent calls of the same tool on this
/// session go straight to Tier 6 (audit) and execute.
///
/// MVP matches `match_kind = 'tool'` + `match_value IS NULL`
/// only — the future `prefix` / `path` kinds are not yet
/// consulted.
pub async fn has_tool_permission(
 pool: &SqlitePool,
 session_id: &str,
 tool_name: &str,
) -> Result<bool, sqlx::Error> {
 let row: Option<(i64,)> = sqlx::query_as(
 r#"
 SELECT 1 FROM session_tool_permissions
 WHERE session_id = ? AND tool_name = ?
 AND match_kind = 'tool' AND match_value IS NULL
 LIMIT 1
 "#,
 )
 .bind(session_id)
 .bind(tool_name)
 .fetch_optional(pool)
 .await?;
 Ok(row.is_some())
}

/// Remove ONE "always allow" row identified by its full PK
/// `(session_id, tool_name, match_kind, match_value)`. Wired to
/// the permission-grant management UI's per-row "撤销" button
/// (task 07-01-permission-grant-list-ui).
///
/// **NULL match_value (design D2)**: `match_kind = 'tool'` rows
/// store `match_value IS NULL`. SQLite evaluates
/// `match_value = NULL` as always-false, so a naive
/// `WHERE match_value = ?` bound to NULL would silently delete 0
/// rows (revoke looks successful but the grant survives). We
/// branch: `None` → `match_value IS NULL`, `Some(v)` →
/// `match_value = ?`. Covered by the
/// `revoke_tool_permission_null_value_tool_kind` test.
///
/// Only the exact PK row is deleted; sibling grants for the same
/// `tool_name` under a different `match_kind`/`match_value` (e.g.
/// other path globs on `read_file`) are preserved.
pub async fn revoke_tool_permission(
 pool: &SqlitePool,
 session_id: &str,
 tool_name: &str,
 match_kind: &str,
 match_value: Option<&str>,
) -> Result<(), sqlx::Error> {
 match match_value {
 None => {
 sqlx::query(
 r#"
 DELETE FROM session_tool_permissions
 WHERE session_id = ? AND tool_name = ?
 AND match_kind = ? AND match_value IS NULL
 "#,
 )
 .bind(session_id)
 .bind(tool_name)
 .bind(match_kind)
 .execute(pool)
 .await?;
 }
 Some(v) => {
 sqlx::query(
 r#"
 DELETE FROM session_tool_permissions
 WHERE session_id = ? AND tool_name = ?
 AND match_kind = ? AND match_value = ?
 "#,
 )
 .bind(session_id)
 .bind(tool_name)
 .bind(match_kind)
 .bind(v)
 .execute(pool)
 .await?;
 }
 }
 Ok(())
}

/// Read every "always allow" row for `session_id`, newest first.
/// Wired to the permission-grant management UI's "load on open"
/// call (task 07-01-permission-grant-list-ui). The row set is the
/// raw `session_tool_permissions` rows; the frontend renders each
/// row's `match_kind` + `match_value` (path glob / prefix token)
/// so the user can distinguish multiple grants on the same tool.
///
/// Empty / missing session returns an empty `Vec` (NOT an error)
/// — the modal renders its empty-state placeholder. The
/// `ORDER BY granted_at DESC, rowid DESC` is a stable sort:
/// `granted_at` is `datetime('now')` (1-second resolution), so
/// same-second grants tie on `granted_at` and break on `rowid`
/// (SQLite's implicit monotonic insertion id).
pub async fn list_tool_permissions(
 pool: &SqlitePool,
 session_id: &str,
) -> Result<Vec<PermissionGrantRow>, sqlx::Error> {
 let rows = sqlx::query(
 r#"
 SELECT session_id, tool_name, match_kind, match_value, granted_at
 FROM session_tool_permissions
 WHERE session_id = ?
 ORDER BY granted_at DESC, rowid DESC
 "#,
 )
 .bind(session_id)
 .fetch_all(pool)
 .await?;
 rows.into_iter()
 .map(|r| {
 Ok(PermissionGrantRow {
 session_id: r.try_get("session_id")?,
 tool_name: r.try_get("tool_name")?,
 match_kind: r.try_get("match_kind")?,
 match_value: r.try_get("match_value")?,
 granted_at: r.try_get("granted_at")?,
 })
 })
 .collect()
}

// ---------------------------------------------------------------------------
// Audit log persistence
// ---------------------------------------------------------------------------

/// Append one row to `session_audit_events`. Called from the
/// agent loop's `permission::check` after each ⑨ 关 decision path
/// (Allow / Deny / Ask / Timeout). `kind` is a stringified
/// [`crate::agent::permissions::AuditKind`] (e.g.
/// `"tool_allowed"`, `"tool_denied"`, `"tool_permission_ask"`,
/// `"permission_granted"`, `"mode_changed"`,
/// `"yolo_entered"`, `"yolo_exited"`, `"tool_denied_yolo"`,
/// `"permission_timeout"`, `"request_cancelled"`). `payload_json`
/// is a free-form JSON object the caller builds — typically
/// `{ "tool_name": "...", "tool_input": {...}, "reason": "..." }`.
///
/// MVP scope: write-only. Read-side UI (C4 audit log panel)
/// is out of scope for A2; [`list_audit_events`] is here as a
/// future hook.
pub async fn record_audit_event(
 pool: &SqlitePool,
 session_id: &str,
 kind: &str,
 payload_json: Option<&str>,
) -> Result<(), sqlx::Error> {
 sqlx::query(
 r#"
 INSERT INTO session_audit_events
 (session_id, ts, kind, payload_json)
 VALUES (?, datetime('now'), ?, ?)
 "#,
 )
 .bind(session_id)
 .bind(kind)
 .bind(payload_json)
 .execute(pool)
 .await?;
 Ok(())
}

/// Read all audit events for `session_id`, newest first. Wired to
/// the C4 audit-log UI's `list_session_audit_events` Tauri command
/// (2026-06-14). Sorted by `ts DESC` (the schema's index supports
/// this — `idx_session_audit_events_session_ts`).
pub async fn list_audit_events(
 pool: &SqlitePool,
 session_id: &str,
) -> Result<Vec<AuditEventRow>, sqlx::Error> {
 let rows = sqlx::query(
 r#"
        SELECT id, session_id, ts, kind, payload_json
        FROM session_audit_events
        WHERE session_id = ?
        ORDER BY ts DESC
        "#,
 )
 .bind(session_id)
 .fetch_all(pool)
 .await?;
 rows.into_iter()
 .map(|r| {
 Ok(AuditEventRow {
 id: r.try_get("id")?,
 session_id: r.try_get("session_id")?,
 ts: r.try_get("ts")?,
 kind: r.try_get("kind")?,
 payload_json: r.try_get("payload_json")?,
 })
 })
 .collect()
}

/// Row shape for [`list_audit_events`] and the
/// `list_session_audit_events` Tauri command. The `payload_json`
/// column stays a `String` (raw JSON text) so callers can re-parse
/// per row rather than committing to a typed shape upfront —
/// different `kind` values carry different payload schemas today,
/// and a locked struct would force premature commitments.
///
/// C4 PR1 follow-up (check phase, 2026-06-14): the struct uses
/// `#[serde(rename_all = "camelCase")]` — NOT plain snake_case —
/// matching every other `db::*Row` type that crosses the IPC
/// boundary (`SessionRow`, `SessionSummary`, `ProviderRow`,
/// `ModelRow`). The Rust fields stay snake_case (per Rust style)
/// but the wire shape is camelCase: the C4 audit-log UI's TS
/// interface reads `sessionId` / `payloadJson`, not `session_id`
/// / `payload_json`. This is mandated by
/// `.trellis/spec/backend/database-guidelines.md` ("All Serialize
/// structs that cross the IPC boundary have
/// #[serde(rename_all = \"camelCase\")]") and confirmed by every
/// existing Row struct in `db/types.rs`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEventRow {
 pub id: i64,
 pub session_id: String,
 pub ts: String,
 pub kind: String,
 pub payload_json: Option<String>,
}

/// Row shape for [`list_tool_permissions`] and the
/// `list_session_tool_permissions` Tauri command (task
/// 07-01-permission-grant-list-ui). `match_value` is `None` for
/// `match_kind = 'tool'` (whole-tool grants); `Some(glob)` for
/// `path`; `Some(prefix_token)` for `prefix`. Mirrors
/// [`AuditEventRow`]'s wire convention —
/// `#[serde(rename_all = "camelCase")]` per
/// `.trellis/spec/backend/database-guidelines.md` (the frontend TS
/// reads `matchKind` / `matchValue`, not snake_case).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionGrantRow {
 pub session_id: String,
 pub tool_name: String,
 pub match_kind: String,
 pub match_value: Option<String>,
 pub granted_at: String,
}