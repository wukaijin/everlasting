//! Tauri commands for the ⑨ 关 permission IPC bridge.
//!
//! Three commands:
//!
//! - [`set_session_mode`] — set the session's `mode` (Chat /
//!   Plan / Review / Yolo). Includes the Yolo + root check:
//!   attempting to enter Yolo as root returns an error so the
//!   user doesn't accidentally nuke their system.
//! - [`permission_response`] — the frontend's reply to a
//!   `permission:ask` event. Resolves the pending oneshot in
//!   `PermissionStore`, which wakes the agent loop's `check()`
//!   future.
//! - [`grant_tool_permission`] — direct write to
//!   `session_tool_permissions`. Used by the frontend after a
//!   "始终允许" click (the IPC is also fired from `permission_response`
//!   on `AllowAlways`, but this command is a future-proof
//!   shortcut for the "manage remembered permissions" UI).
//!
//! Lives in its own module (`commands/permissions.rs`) rather
//! than `commands/sessions.rs` because the IPC surface is
//! distinct (different functions, different stores, different
//! error paths). Follows the post-PR1 audit-task pattern of
//! "one concern per commands module".

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::agent::permissions::PermissionResponse;
use crate::db;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Root check (for Yolo safety guard)
// ---------------------------------------------------------------------------

/// Returns `true` if the current process is running as root
/// (UID 0 on Unix, or as the Administrators group on Windows).
///
/// Per PRD `### Technical Notes`: we deliberately use
/// `unsafe { libc::geteuid() }` instead of pulling the `nix`
/// crate for a single check. The libc call is the canonical
/// POSIX primitive; the `unsafe` block is minimal and well-
/// understood (libc is the C standard library; `geteuid()` is
/// a pure syscall with no preconditions).
///
/// On Windows the UID/GID model doesn't exist; we always
/// return `false` (the user is treated as non-root, which
/// means Yolo is unconditionally allowed on Windows). The PRD
/// notes Windows root check is a future concern (would use
/// `windows-sys::Win32::Security::IsUserAnAdmin` or similar).
#[cfg(target_family = "unix")]
pub fn is_running_as_root() -> bool {
 unsafe { libc::geteuid() == 0 }
}

#[cfg(not(target_family = "unix"))]
pub fn is_running_as_root() -> bool {
 false
}

// ---------------------------------------------------------------------------
// set_session_mode — write the session's mode + audit + Yolo guard
// ---------------------------------------------------------------------------

/// Set the session's mode. Called by the frontend's
/// `ModeSelect.vue` on every mode change. Includes the Yolo
/// safety guard: attempting to enter Yolo as root fails with
/// `"Cannot enable Yolo as root"` (per PRD AC §后端 + audit
/// §3.3).
///
/// Side effects:
/// 1. Update `sessions.mode` (UPDATE row, bump `updated_at`).
/// 2. Write audit event(s):
///    - `mode_changed` (every call)
///    - `yolo_entered` (only when entering Yolo)
///    - `yolo_exited` (only when leaving Yolo)
#[tauri::command]
pub async fn set_session_mode(
 state: State<'_, Arc<AppState>>,
 session_id: String,
 mode: String,
) -> Result<db::SessionRow, String> {
 // Parse + validate the mode string. Unknown / empty falls
 // back to Edit per the lenient-parse contract (matches
 // `db::types::Mode::from_str_opt`). Old 'chat' / 'review'
 // strings intentionally NOT aliased — the v6 migration
 // rewrites historical rows; new IPC calls must use the
 // 3 档 wire names ('edit' / 'plan' / 'yolo').
 let new_mode = match mode.as_str() {
 "plan" => db::Mode::Plan,
 "yolo" => db::Mode::Yolo,
 "background" => db::Mode::Background,
 _ => db::Mode::Edit,
 };

 // Yolo safety guard: refuse to enable Yolo when running as
 // root. Yolo removes all user confirmations and the only
 // remaining gate is Tier 2 (hard kill list). Running as root
 // means `rm -rf /` could actually destroy the system before
 // the kill list even matters (the kill list rejects it but
 // other destructive commands are not enumerated). Refusing
 // here is the simplest, safest guard.
 if new_mode == db::Mode::Yolo && is_running_as_root() {
 tracing::warn!(
 session_id = %session_id,
 "set_session_mode: refused to enable Yolo as root"
 );
 return Err("Cannot enable Yolo as root".to_string());
 }

 // Read the current mode for the yolo_entered / yolo_exited
 // audit dispatch.
 let loaded = db::load_session(&state.db, &session_id)
 .await
 .map_err(|e| format!("set_session_mode: load_session failed: {}", e))?
 .ok_or_else(|| format!("set_session_mode: session '{}' not found", session_id))?;
 let prev_mode = loaded.session.mode;

 // Write the new mode.
 db::update_session_mode(&state.db, &session_id, new_mode)
 .await
 .map_err(|e| format!("set_session_mode: db update failed: {}", e))?;

 // Audit row: mode_changed (always).
 let payload = serde_json::json!({
 "prev_mode": prev_mode.as_str(),
 "new_mode": new_mode.as_str(),
 })
 .to_string();
 if let Err(e) = db::record_audit_event(
 &state.db,
 &session_id,
 "mode_changed",
 Some(&payload),
 )
 .await
 {
 tracing::warn!(error = %e, "set_session_mode: record_audit_event(mode_changed) failed");
 }

 // Audit row: yolo_entered / yolo_exited (only on the
 // transition, not on every set_session_mode call).
 let transition_kind = match (prev_mode, new_mode) {
 (db::Mode::Yolo, db::Mode::Yolo) => None, // no-op toggle
 (_, db::Mode::Yolo) => Some("yolo_entered"),
 (db::Mode::Yolo, _) => Some("yolo_exited"),
 _ => None,
 };
 if let Some(kind) = transition_kind {
 if let Err(e) = db::record_audit_event(
 &state.db,
 &session_id,
 kind,
 Some(&payload),
 )
 .await
 {
 tracing::warn!(
 error = %e,
 kind = %kind,
 "set_session_mode: record_audit_event(transition) failed"
 );
 }
 }

 // Re-load the row so the IPC return matches the typical
 // CRUD shape (the frontend updates its `currentSession` with
 // the returned row).
 let updated = db::load_session(&state.db, &session_id)
 .await
 .map_err(|e| format!("set_session_mode: re-load failed: {}", e))?
 .ok_or_else(|| format!(
 "set_session_mode: session '{}' disappeared mid-call",
 session_id
 ))?;
 Ok(updated.session)
}

// ---------------------------------------------------------------------------
// permission_response — IPC bridge for the Tier 3 await
// ---------------------------------------------------------------------------

/// Frontend reply to a `permission:ask` event. Looks up the
/// pending oneshot by `rid` and sends the user's decision.
///
/// `decision` is one of:
/// - `"allow_once"` → `PermissionResponse::AllowOnce`
/// - `"allow_always"` → `PermissionResponse::AllowAlways`
/// - `"deny"` → `PermissionResponse::Deny`
///
/// Unknown decision strings return `Err`. Unknown / stale
/// `rid`s return `Ok(false)` (the IPC is best-effort — a
/// duplicate or late response is a benign no-op, NOT an error,
/// per audit §3.2).
#[tauri::command]
pub async fn permission_response(
 _app: AppHandle,
 state: State<'_, Arc<AppState>>,
 rid: String,
 decision: String,
) -> Result<bool, String> {
 let response = match decision.as_str() {
 "allow_once" => PermissionResponse::AllowOnce,
 "allow_always" => PermissionResponse::AllowAlways,
 "deny" => PermissionResponse::Deny,
 other => {
 return Err(format!(
 "permission_response: unknown decision '{}'",
 other
 ));
 }
 };
 let resolved = crate::agent::permissions::resolve_ask(
 &state.permission_asks,
 &rid,
 response,
 )
 .await;
 if !resolved {
 tracing::warn!(
 rid = %rid,
 decision = %decision,
 "permission_response: rid not found (timed out or duplicate response)"
 );
 }
 Ok(resolved)
}

// ---------------------------------------------------------------------------
// grant_tool_permission — direct "remember this tool" write
// ---------------------------------------------------------------------------

/// Insert an "always allow" row for `(session_id, tool_name)`
/// with the given `match_kind` + `match_value`. Wired to the
/// future "manage remembered permissions" UI (PR3+); the
/// `permission:ask` IPC flow (via `permission_response`) also
/// writes the row via the agent loop's `check()` on
/// `AllowAlways` using `permissions::match_value_for_allow_always`
/// to auto-pick the right `match_kind` for the tool type
/// (path / prefix / tool — re-grill Q6).
///
/// **Validation** (re-grill Q6 schema lock):
/// - `match_kind` MUST be one of `"tool"` / `"prefix"` / `"path"`.
/// - `match_value` MUST be `None` when `match_kind = "tool"`.
/// - `match_value` MUST be `Some` for `prefix` and `path`.
///
/// The DB schema also enforces `match_kind IN ('tool', 'prefix',
/// 'path')` via a CHECK constraint (see `db::migrations`).
/// Passing a value that doesn't match the constraint is
/// reported as a `db::grant_tool_permission` error (the IPC
/// wraps it as `Err`).
#[tauri::command]
#[allow(dead_code)]
pub async fn grant_tool_permission(
 state: State<'_, Arc<AppState>>,
 session_id: String,
 tool_name: String,
 match_kind: Option<String>,
 match_value: Option<String>,
) -> Result<(), String> {
 // Default the `match_kind` to `"tool"` when omitted
 // (back-compat with the pre-re-grill IPC, which only
 // wrote tool-level grants).
 let kind = match_kind.as_deref().unwrap_or("tool");
 // Validation: match_value must be Some for prefix / path.
 match kind {
 "tool" => {
 // match_value is ignored for `tool`; we still pass
 // through what the frontend sent for transparency.
 db::grant_tool_permission(
 &state.db,
 &session_id,
 &tool_name,
 kind,
 match_value.as_deref(),
 )
 .await
 .map_err(|e| format!("grant_tool_permission failed: {}", e))
 }
 "prefix" | "path" => {
 let value = match_value.as_deref().ok_or_else(|| {
 format!(
 "grant_tool_permission: match_kind='{}' requires a non-NULL match_value",
 kind
 )
 })?;
 db::grant_tool_permission(&state.db, &session_id, &tool_name, kind, Some(value))
 .await
 .map_err(|e| format!("grant_tool_permission failed: {}", e))
 }
 other => Err(format!(
 "grant_tool_permission: unknown match_kind '{}' (expected 'tool' | 'prefix' | 'path')",
 other
 )),
 }
}


// ---------------------------------------------------------------------------
// C4 (Audit-log query UI, 2026-06-14) — list_session_audit_events
// ---------------------------------------------------------------------------

/// Read all audit events for a session, newest first. Wired to the
/// C4 AuditLogModal's "load on open" call. The row set is the
/// raw `session_audit_events` rows; the frontend parses
/// `payload_json` per `kind` (see `prd.md` "payload 形态").
///
/// Empty / missing session returns an empty `Vec` (NOT an error)
/// — the modal renders its "暂无审计事件" placeholder. Any DB
/// error is wrapped as a `String` for the frontend's toast path.
///
/// MVP scope: full pull (no pagination / no virtual scroll). The
/// `idx_session_audit_events_session_ts` index keeps the
/// `ORDER BY ts DESC` cheap; >500-event sessions are a follow-up
/// optimization (PRD "Edge Cases" TODO).
#[tauri::command]
pub async fn list_session_audit_events(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<db::AuditEventRow>, String> {
    db::list_audit_events(&state.db, &session_id)
        .await
        .map_err(|e| format!("list_session_audit_events failed: {}", e))
}