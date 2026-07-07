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
use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

// Re-export the internal `set_session_mode_internal` from
// `commands::question` so callers (incl. `set_session_mode`)
// can use the same drop-in place for mode application. Kept
// in `commands::question` because that file owns the
// `request_mode_change` tool's `resolve_mode_change` IPC
// handler — the two paths (user-driven IPC + LLM-driven
// tool) must share a single mode-application point to avoid
// audit / Yolo guard drift.
pub(crate) use super::question::set_session_mode_internal;

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
/// Side effects (delegated to
/// `set_session_mode_internal` to keep the user-driven IPC +
/// LLM-driven `request_mode_change` tool paths in lockstep —
/// RULE-A-006 single source of truth):
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
) -> Result<db::SessionRow, AppCommandError> {
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

    // Delegate the apply + audit + Yolo root guard to the
    // shared internal function. The IPC wrapper's only job is
    // the lenient-parse + the IPC return shape.
    let result = set_session_mode_internal(&state.db, &session_id, new_mode).await?;
    Ok(result.session)
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
    reason: Option<String>,
) -> Result<bool, AppCommandError> {
    let response = match decision.as_str() {
        "allow_once" => PermissionResponse::AllowOnce,
        "allow_always" => PermissionResponse::AllowAlways,
        // `reason` is the user's optional "拒绝并说明" feedback.
        // Empty / None → plain deny. The agent loop surfaces this as
        // the tool_result(is_error) content for the LLM.
        "deny" => PermissionResponse::Deny {
            reason: reason.unwrap_or_default(),
        },
        other => {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!("permission_response: unknown decision '{}'", other),
            ))
        }
    };
    let resolved =
        crate::agent::permissions::resolve_ask(&state.permission_asks, &rid, response).await;
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
) -> Result<(), AppCommandError> {
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
            .map_err(|e| anyhow::anyhow!("grant_tool_permission failed: {}", e).into())
        }
        "prefix" | "path" => {
            let value = match_value.as_deref().ok_or_else(|| {
                AppCommandError::new(
                    ErrorCategory::InvalidRequest,
                    format!(
                        "grant_tool_permission: match_kind='{}' requires a non-NULL match_value",
                        kind
                    ),
                )
            })?;
            db::grant_tool_permission(&state.db, &session_id, &tool_name, kind, Some(value))
                .await
                .map_err(|e| anyhow::anyhow!("grant_tool_permission failed: {}", e).into())
        }
        other => Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
 "grant_tool_permission: unknown match_kind '{}' (expected 'tool' | 'prefix' | 'path')",
 other
 ),
        )),
    }
}

// ---------------------------------------------------------------------------
// Permission-grant management UI (task 07-01-permission-grant-list-ui)
// ---------------------------------------------------------------------------

/// Read every "always allow" row for a session, newest first. Wired
/// to the permission-grant management modal's "load on open" call.
/// Each row carries its `match_kind` + `match_value` so the UI can
/// render path globs / shell prefixes distinctly and revoke a single
/// PK row without touching sibling grants. Empty / missing session
/// returns an empty `Vec` (NOT an error) — the modal renders its
/// empty-state placeholder.
#[tauri::command]
pub async fn list_session_tool_permissions(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<db::PermissionGrantRow>, AppCommandError> {
    db::list_tool_permissions(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("list_session_tool_permissions failed: {}", e).into())
}

/// Revoke ONE "always allow" row by its full PK. `match_value` is
/// `None` for `match_kind = "tool"` (matches the NULL the DB stores);
/// `Some(...)` for `prefix` / `path`. The NULL branch is handled in
/// `db::revoke_tool_permission` (design D2): passing `None` here must
/// reach the DB as `IS NULL`, not as a bound NULL parameter (which
/// would silently match nothing). `match_kind` is validated to keep
/// parity with `grant_tool_permission`.
#[tauri::command]
pub async fn revoke_tool_permission(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tool_name: String,
    match_kind: String,
    match_value: Option<String>,
) -> Result<(), AppCommandError> {
    match match_kind.as_str() {
        "tool" | "prefix" | "path" => {}
        other => {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!(
                    "revoke_tool_permission: unknown match_kind '{}' (expected 'tool' | 'prefix' | 'path')",
                    other
                ),
            ))
        }
    }
    db::revoke_tool_permission(
        &state.db,
        &session_id,
        &tool_name,
        &match_kind,
        match_value.as_deref(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("revoke_tool_permission failed: {}", e).into())
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
) -> Result<Vec<db::AuditEventRow>, AppCommandError> {
    db::list_audit_events(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("list_session_audit_events failed: {}", e).into())
}
