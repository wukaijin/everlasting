//! ask_user_question + request_mode_change Tauri command surface
//! (2026-06-30 / 2026-07-07).
//!
//! Three commands back the frontend's inline cards:
//!
//! - [`resolve_tool_question`] — frontend invokes with the user's
//!   answer (or `cancelled: true` on `跳过`); we forward to
//!   `QuestionStore::resolve` which sends the oneshot → the
//!   agent loop's `tokio::select!` returns. The result is
//!   unwrapped into the 2-way `InteractionResponse::Answered |
//!   Cancelled` enum (session cancel is handled by
//!   `execute_blocking`'s cancel arm directly, never via this
//!   IPC path).
//! - [`resolve_mode_change`] — frontend invokes on `允许` /
//!   `拒绝` of a `<RequestModeChangeCard>`; same pattern,
//!   routes to the internal `set_session_mode_internal` (the
//!   pure function extracted from the `set_session_mode` IPC
//!   handler) BEFORE resolving the store oneshot, so the
//!   permission boundary is enforced at the same place as
//!   user-driven mode switches (single source of truth).
//! - [`get_pending_interaction`] — frontend invokes on session
//!   switch / `rehydrateMessages` to recover the live pending
//!   interaction (so a switched-back session renders the still-
//!   unanswered card). Returns
//!   `Option<PendingInteractionEntry>` — `None` if no
//!   interaction pending. The store is the source of truth;
//!   the frontend `pendingBySession` cache is overruled by this
//!   result (`get_pending_interaction` > cache > empty).
//! - [`get_pending_question`] — legacy thin shim that returns
//!   only the `ToolQuestionPayload` for backward compat with
//!   callers that pre-date the unified IPC. New code should use
//!   `get_pending_interaction` + the tagged `PendingInteraction`
//!   enum.

use std::sync::Arc;

use tauri::State;

use crate::agent::permissions::AuditKind;
use crate::agent::question_store::{
    InteractionResponse, PendingInteractionEntry, QuestionAnswer, ToolQuestionPayload,
};
// Test-only imports — gated by `#[cfg(test)]` so non-test builds
// (e.g. `cargo check`) don't flag them as unused. `use super::*;`
// inside `mod tests` then re-imports them via the parent scope.
#[cfg(test)]
use crate::agent::question_store::{
    InteractionKind, ModeChangePayload, PendingInteraction, TaskStateTransitionPayload,
};
use crate::agent::workflow::state::{
    set_task_state as workflow_set_task_state, StateTransitionError,
};
use crate::agent::workflow::task::read_task as read_workflow_task;
use crate::db;
use crate::error::AppCommandError;
use crate::state::AppState;

/// Forward the user's answer (or cancel) to the
/// `QuestionStore`, which resolves the oneshot the agent loop
/// is awaiting. The agent loop's `tokio::select!` arm fires,
/// returning either the JSON-serialized answer array (success)
/// or `{"cancelled": true}` (cancelled = true path). See
/// `tools/ask_user_question.rs::execute_blocking` for the
/// consumer-side wire shape.
///
/// # Why scalar args (not a struct payload)
///
/// Per `database-guidelines.md`'s IPC checklist, Tauri 2
/// auto-converts JS camelCase → Rust snake_case for command
/// **arguments**, but does NOT rename fields inside a struct
/// parameter (that needs `#[serde(rename_all = "camelCase")]`
/// on the struct, which the shared `Question` type can't use —
/// see `question_store::ToolQuestionPayload`'s exemption note).
/// So this command takes scalar args — the frontend's
/// `invoke("resolve_tool_question", { sessionId, toolUseId,
/// answer, cancelled })` round-trips correctly because each
/// scalar arg crosses the camelCase↔snake_case boundary via
/// Tauri's arg-level conversion. This mirrors
/// `permission_response` (the established frontend→backend
/// resolve pattern in this codebase). `tool_use_id` is
/// accepted for routing parity but the store keys on
/// `session_id` alone (single-pending invariant).
///
/// `answer` is `Option<Vec<QuestionAnswer>>` — the frontend
/// omits it on the `跳过` path (Tauri maps `undefined` to
/// `None`). `cancelled` is `Option<bool>` for the same reason.
/// `QuestionAnswer`'s fields are snake_case on both sides
/// (same shared-struct exemption), so the nested answer array
/// deserializes without rename.
///
/// # Errors
///
/// Returns `Err(String)` for `NotFound` — no pending question
/// for `session_id` (race: the session-cancel arm already
/// cleared the entry, or the user clicked 跳过 on a card
/// already resolved by another path). The frontend treats this
/// as a no-op (the card is either gone or never was visible).
pub async fn resolve_tool_question_inner(
    state: &Arc<AppState>,
    session_id: String,
    tool_use_id: String,
    answer: Option<Vec<QuestionAnswer>>,
    cancelled: Option<bool>,
) -> Result<(), AppCommandError> {
    // Accepted for routing parity with the wire shape; the
    // store keys on session_id alone (single-pending).
    let _ = tool_use_id;
    let response = resolve_response_from_args(cancelled, answer)?;
    state.question_store.resolve(&session_id, response).await?;
    Ok(())
}

#[tauri::command]
pub async fn resolve_tool_question(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tool_use_id: String,
    answer: Option<Vec<QuestionAnswer>>,
    cancelled: Option<bool>,
) -> Result<(), AppCommandError> {
    resolve_tool_question_inner(&state, session_id, tool_use_id, answer, cancelled).await
}

/// Map the scalar IPC args to an `InteractionResponse` for the
/// `ask_user_question` case. Pure function extracted from
/// `resolve_tool_question` so the `cancelled`-vs-`answer`
/// branch is unit-testable without a Tauri `mock_app` (which
/// this project doesn't use — see the "Why scalar args" note
/// on `resolve_tool_question` for why the invoke serde
/// boundary itself is covered by the `permission_response`
/// precedent + manual `tauri dev` verification, not a unit
/// test).
pub(crate) fn resolve_response_from_args(
    cancelled: Option<bool>,
    answer: Option<Vec<QuestionAnswer>>,
) -> Result<InteractionResponse, AppCommandError> {
    if cancelled.unwrap_or(false) {
        Ok(InteractionResponse::Cancelled)
    } else {
        // Serialize the answer array to `serde_json::Value` so
        // the unified `InteractionResponse::Answered(serde_json::Value)`
        // channel can carry it. The `execute_blocking` side
        // re-parses this back into `Vec<QuestionAnswer>` (or
        // surfaces a serialize error).
        let value = serde_json::to_value(answer.unwrap_or_default()).map_err(|e| {
            AppCommandError::new(
                crate::error::ErrorCategory::InvalidRequest,
                format!("resolve_response_from_args: serialize error: {}", e),
            )
        })?;
        Ok(InteractionResponse::Answered(value))
    }
}

/// Forward the `request_mode_change` decision to the
/// `QuestionStore` oneshot AFTER applying the mode change via
/// the internal `set_session_mode_internal` function (the
/// single source of truth for mode application — the user-
/// driven IPC `set_session_mode` is the only other writer).
///
/// Why we resolve AFTER applying (not before):
/// 1. The permission boundary (`is_running_as_root` Yolo guard
///    + mode_changed audit) is the IPC handler's job; the
///    tool's `tokio::select!` arm awaits the resolution and
///    sees the **authoritative** outcome (allowed vs denied).
/// 2. If the user denies (frontend `allow=false` path) we
///    skip `set_session_mode_internal` entirely and just
///    resolve as `Cancelled`. No audit, no DB write.
/// 3. If the user allows but the DB call fails (Yolo root
///    guard / DB error), we still resolve as `Cancelled` so
///    the agent loop doesn't hang — the failure surfaces via
///    audit + tool_result (`cancelled_by_user: true,
///    reason: "Cannot enable Yolo as root" | db_error`).
///
/// The `session_id` is read off the `ModeChangePayload` that
/// the IPC caller fetched via `get_pending_interaction` (so
/// the store's `session_id` matches the user-visible session
/// — the IPC `session_id` arg is for the audit / DB target,
/// which is the SAME session by construction; the
/// `ModeChangePayload.session_id` field is the source of
/// truth).
pub async fn resolve_mode_change_inner(
    state: &Arc<AppState>,
    session_id: String,
    tool_use_id: String,
    target_mode: String,
    allow: bool,
) -> Result<db::SessionRow, AppCommandError> {
    // Accepted for routing parity with the wire shape (the
    // store keys on session_id alone, single-pending gate).
    let _ = tool_use_id;
    resolve_mode_change_internal(
        &state.db,
        &state.question_store,
        &session_id,
        &target_mode,
        allow,
    )
    .await
}

#[tauri::command]
pub async fn resolve_mode_change(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tool_use_id: String,
    target_mode: String,
    allow: bool,
) -> Result<db::SessionRow, AppCommandError> {
    resolve_mode_change_inner(&state, session_id, tool_use_id, target_mode, allow).await
}

/// Pure-Rust core of [`resolve_mode_change`] — extracted into a
/// free-standing function so it can be unit-tested WITHOUT a
/// `tauri::test::mock_app` (which this project doesn't use; see
/// the existing `permission_response` precedent). The IPC
/// wrapper is a thin shell that just threads the `&Arc<AppState>`
/// deps through.
///
/// `target_mode` is the raw IPC string ("plan" / "yolo" /
/// "edit" / "background" / anything else). Lenient parse
/// (unknown → Edit per `db::types::Mode::from_str_opt`).
///
/// `allow` is the user's decision. `false` skips
/// `set_session_mode_internal` entirely (the deny path
/// MUST NOT touch DB mode); `true` applies via the shared
/// internal function (single source of truth for mode
/// application + Yolo root guard + mode_changed audit).
///
/// In both paths the `QuestionStore` entry is resolved so the
/// agent loop's `tokio::select!` arm fires — Cancelled on deny
/// / root-guard / db-error, Answered on success.
pub(crate) async fn resolve_mode_change_internal(
    db_pool: &sqlx::SqlitePool,
    store: &crate::agent::question_store::QuestionStore,
    session_id: &str,
    target_mode: &str,
    allow: bool,
) -> Result<db::SessionRow, AppCommandError> {
    // 1. Parse + validate the target_mode (lenient parse —
    //    unknown → Edit per `db::types::Mode::from_str_opt`).
    let new_mode = match target_mode {
        "plan" => db::Mode::Plan,
        "yolo" => db::Mode::Yolo,
        "background" => db::Mode::Background,
        _ => db::Mode::Edit,
    };

    // 2. If denied, just resolve as Cancelled + write the
    //    denied audit. No DB write.
    if !allow {
        // Denied audit (always — even on Yolo confirm-cancel).
        let payload = serde_json::json!({
            "target_mode": target_mode,
            "reason": "user denied",
        })
        .to_string();
        if let Err(e) = db::record_audit_event(
            db_pool,
            session_id,
            crate::agent::permissions::AuditKind::ModeChangeDenied.as_str(),
            Some(&payload),
            None,
        )
        .await
        {
            tracing::warn!(error = %e, "resolve_mode_change: denied audit failed");
        }
        // Resolve the oneshot so the agent loop unblocks.
        store
            .resolve(session_id, InteractionResponse::Cancelled)
            .await?;
        // Reload the row so the IPC caller can refresh
        // `currentSession` (mode unchanged on the deny path).
        return load_session_row(db_pool, session_id).await;
    }

    // 3. Allow path — apply the mode via the internal pure
    //    function. Yolo root guard is enforced inside
    //    `set_session_mode_internal` (mirrors the
    //    user-driven IPC path; see design §5.5 "Yolo root
    //    guard 一致性").
    let apply_result = set_session_mode_internal(db_pool, session_id, new_mode).await;
    match apply_result {
        Ok(row) => {
            // Allowed audit on top of the mode_changed audit
            // emitted by `set_session_mode_internal`.
            let payload = serde_json::json!({
                "prev_mode": row.mode_prev.as_deref().unwrap_or(""),
                "new_mode": row.session.mode.as_str(),
                "target_mode": target_mode,
            })
            .to_string();
            if let Err(e) = db::record_audit_event(
                db_pool,
                session_id,
                crate::agent::permissions::AuditKind::ModeChangeAllowed.as_str(),
                Some(&payload),
                None,
            )
            .await
            {
                tracing::warn!(error = %e, "resolve_mode_change: allowed audit failed");
            }
            // Resolve the oneshot so the agent loop unblocks.
            store
                .resolve(
                    session_id,
                    InteractionResponse::Answered(serde_json::json!(true)),
                )
                .await?;
            Ok(row.session)
        }
        Err(e) => {
            // Apply failed (Yolo root guard / DB error). The
            // tool still needs to unblock — resolve as
            // Cancelled + write the denied audit with the
            // specific reason. We match on the error message
            // text (no `Display` impl on `AppCommandError` —
            // see `error.rs`; the `message` field is the
            // stable wire shape).
            let reason = if e.category == crate::error::ErrorCategory::InvalidRequest
                && e.message == "Cannot enable Yolo as root"
            {
                "yolo_root_guard"
            } else {
                "db_error"
            };
            let payload = serde_json::json!({
                "target_mode": target_mode,
                "reason": reason,
            })
            .to_string();
            if let Err(e2) = db::record_audit_event(
                db_pool,
                session_id,
                crate::agent::permissions::AuditKind::ModeChangeDenied.as_str(),
                Some(&payload),
                None,
            )
            .await
            {
                tracing::warn!(error = %e2, "resolve_mode_change: denied audit failed (post-apply-fail)");
            }
            // Resolve as Cancelled — the agent loop sees
            // `cancelled_by_user: true` (a generic cancel
            // marker; the specific reason is in audit only —
            // the tool layer formats the cancel wire as
            // `{"cancelled_by_user": true}` and the LLM
            // decides what to do).
            let _ = store
                .resolve(session_id, InteractionResponse::Cancelled)
                .await;
            // Surface the apply error to the IPC caller (so
            // the frontend can toast it).
            Err(e)
        }
    }
}

/// Apply `mode` to `session_id` + write audit rows. Internal
/// pure function extracted from the `set_session_mode` IPC
/// handler so both the user-driven IPC path AND the LLM-driven
/// `request_mode_change` tool path share the EXACT same mode
/// application + audit behavior (RULE-A-006 "single source of
/// truth"; design §6 decision `set_session_mode` is the only
/// drop-in place for `mode_changed` audit + Yolo root guard +
/// session-mode DB write).
///
/// Returns the freshly-loaded session row + the prev-mode
/// string (so the caller can write a `mode_change_allowed`
/// audit row with the prev→new transition without re-querying).
pub(crate) async fn set_session_mode_internal(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    new_mode: db::Mode,
) -> Result<SetSessionModeResult, AppCommandError> {
    // Yolo safety guard (same as `set_session_mode` IPC).
    // `is_running_as_root` lives in `commands::permissions` —
    // re-import the path so the internal function is the
    // single source of truth for mode application (no
    // Yolo-guard drift between user-driven IPC and the
    // LLM-driven `request_mode_change` tool).
    if new_mode == db::Mode::Yolo && crate::commands::permissions::is_running_as_root() {
        return Err(AppCommandError::new(
            crate::error::ErrorCategory::InvalidRequest,
            "Cannot enable Yolo as root",
        ));
    }

    // Read current mode for prev/transition audit.
    let loaded = db::load_session(pool, session_id)
        .await
        .map_err(|e| {
            AppCommandError::new(
                crate::error::ErrorCategory::Server,
                format!("set_session_mode_internal: load_session failed: {}", e),
            )
        })?
        .ok_or_else(|| {
            AppCommandError::new(
                crate::error::ErrorCategory::InvalidRequest,
                format!(
                    "set_session_mode_internal: session '{}' not found",
                    session_id
                ),
            )
        })?;
    let prev_mode = loaded.session.mode;

    // Write the new mode.
    db::update_session_mode(pool, session_id, new_mode)
        .await
        .map_err(|e| {
            AppCommandError::new(
                crate::error::ErrorCategory::Server,
                format!("set_session_mode_internal: db update failed: {}", e),
            )
        })?;

    // Audit: mode_changed (always).
    let payload = serde_json::json!({
        "prev_mode": prev_mode.as_str(),
        "new_mode": new_mode.as_str(),
    })
    .to_string();
    if let Err(e) =
        db::record_audit_event(pool, session_id, "mode_changed", Some(&payload), None).await
    {
        tracing::warn!(error = %e, "set_session_mode_internal: record_audit_event(mode_changed) failed");
    }

    // Audit: yolo_entered / yolo_exited (only on the
    // transition, not on every set call).
    let transition_kind = match (prev_mode, new_mode) {
        (db::Mode::Yolo, db::Mode::Yolo) => None,
        (_, db::Mode::Yolo) => Some("yolo_entered"),
        (db::Mode::Yolo, _) => Some("yolo_exited"),
        _ => None,
    };
    if let Some(kind) = transition_kind {
        if let Err(e) = db::record_audit_event(pool, session_id, kind, Some(&payload), None).await {
            tracing::warn!(
                error = %e,
                kind = %kind,
                "set_session_mode_internal: record_audit_event(transition) failed"
            );
        }
    }

    // Re-load so the IPC return matches the typical CRUD
    // shape (caller updates `currentSession` with the returned
    // row).
    let updated = db::load_session(pool, session_id)
        .await
        .map_err(|e| {
            AppCommandError::new(
                crate::error::ErrorCategory::Server,
                format!("set_session_mode_internal: re-load failed: {}", e),
            )
        })?
        .ok_or_else(|| {
            AppCommandError::new(
                crate::error::ErrorCategory::InvalidRequest,
                format!(
                    "set_session_mode_internal: session '{}' disappeared mid-call",
                    session_id
                ),
            )
        })?;

    Ok(SetSessionModeResult {
        session: updated.session,
        mode_prev: Some(prev_mode.as_str().to_string()),
    })
}

/// Return shape of [`set_session_mode_internal`]. Carries the
/// freshly-loaded `SessionRow` (for the IPC caller to update
/// `currentSession`) + the prev-mode string (so the LLM-driven
/// `request_mode_change` tool can write a `mode_change_allowed`
/// audit with the prev→new transition without re-querying).
pub(crate) struct SetSessionModeResult {
    pub session: db::SessionRow,
    pub mode_prev: Option<String>,
}

/// Read-only frontend hook for session switch + initial load.
/// Returns the `PendingInteractionEntry` (typed union) for
/// the session if a question / mode change is still pending,
/// or `None` if no interaction is pending.
///
/// The store is the source of truth — `Option<None>` here
/// means "the interaction was resolved (or never existed)".
/// The `pendingBySession` Pinia cache is a memoization layer
/// that gets corrected (via this command) on every session
/// switch.
pub async fn get_pending_interaction_inner(
    state: &Arc<AppState>,
    session_id: String,
) -> Result<Option<PendingInteractionEntry>, AppCommandError> {
    Ok(state.question_store.get_payload(&session_id).await)
}

#[tauri::command]
pub async fn get_pending_interaction(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<PendingInteractionEntry>, AppCommandError> {
    get_pending_interaction_inner(&state, session_id).await
}

/// Legacy IPC shim (back-compat). Returns just the
/// `ToolQuestionPayload` for the session if a question is
/// still pending AND the pending interaction is a question
/// (returns `None` for a pending mode change — callers that
/// care about mode change should migrate to
/// `get_pending_interaction`). New code should use
/// `get_pending_interaction`; this shim is kept so pre-unified
/// callers don't break.
pub async fn get_pending_question_inner(
    state: &Arc<AppState>,
    session_id: String,
) -> Result<Option<ToolQuestionPayload>, AppCommandError> {
    Ok(state.question_store.get_question_payload(&session_id).await)
}

#[tauri::command]
#[deprecated(note = "use get_pending_interaction instead")]
pub async fn get_pending_question(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<ToolQuestionPayload>, AppCommandError> {
    get_pending_question_inner(&state, session_id).await
}

/// Reload a session row by id; the thin wrapper around
/// `db::load_session` that the `resolve_mode_change` handler
/// uses on the deny path (and any other handler that needs to
/// return a `SessionRow` to the IPC caller without applying a
/// mode change).
async fn load_session_row(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<db::SessionRow, AppCommandError> {
    let loaded = db::load_session(pool, session_id)
        .await
        .map_err(|e| {
            AppCommandError::new(
                crate::error::ErrorCategory::Server,
                format!("load_session_row: load_session failed: {}", e),
            )
        })?
        .ok_or_else(|| {
            AppCommandError::new(
                crate::error::ErrorCategory::InvalidRequest,
                format!("load_session_row: session '{}' not found", session_id),
            )
        })?;
    Ok(loaded.session)
}

// ---------------------------------------------------------------------------
// `resolve_task_state_transition` IPC (Phase 3 Step 3.1 of
// `07-08-workflow-integration`, 2026-07-08).
//
// Mirrors the dual-IPC pattern of `resolve_mode_change`:
// 1. Parse + validate the target state string.
// 2. If denied, just resolve as `Cancelled`. No DB write.
// 3. If allowed, look up the project + task slug →
//    `workflow::set_task_state(...)` (apply BEFORE resolve) →
//    resolve as `Answered(true)`.
//
// The actual `task.json` mutation + `from → to` Rust hook
// dispatch (trigger_spec_distillation / preflight_implement_check)
// happens in `set_task_state` — the IPC handler is the single
// source of truth for state-transition side effects (per design
// §5.2 M-A / Q9 "Rust 固定 hook 嵌入 set_task_state 写入路径").
// ---------------------------------------------------------------------------

/// Forward the user's allow / deny decision for a
/// `request_task_state_transition` card to the `QuestionStore`
/// oneshot AFTER applying the state transition via
/// `workflow::set_task_state` (the single source of truth for
/// `from → to` transitions + hook dispatch — design §5.2
/// M-A). Mirrors `resolve_mode_change`'s apply-BEFORE-resolve
/// pattern.
///
/// Why resolve AFTER applying:
/// 1. The `from → to` hook dispatch (e.g.
///    `trigger_spec_distillation` on `in_progress → Done`) is the
///    function `set_task_state`'s job; the tool's
///    `tokio::select!` arm awaits the resolution and sees the
///    **authoritative** outcome (allowed / denied / hook fired).
/// 2. If the user denies we skip `set_task_state` entirely and
///    just resolve as `Cancelled`. No `task.json` write, no
///    hook, no audit (other than `task_state_transition_denied`).
/// 3. If the user allows but `set_task_state` fails (invalid
///    target state / IO error) we still resolve as `Cancelled`
///    so the agent loop doesn't hang — the failure surfaces
///    via audit + `tool_result`.
///
/// The `session_id` arg targets the audit + DB writes (the
/// `set_task_state` call's project is resolved off the session
/// row, which we read with `db::load_session`).
pub async fn resolve_task_state_transition_inner(
    state: &Arc<AppState>,
    session_id: String,
    tool_use_id: String,
    target_state: String,
    slug: String,
    allow: bool,
) -> Result<db::SessionRow, AppCommandError> {
    // Accepted for routing parity with the wire shape (the
    // store keys on session_id alone, single-pending gate).
    let _ = tool_use_id;
    let project_path = state_to_project_path(&state, &session_id).await?;
    resolve_task_state_transition_internal(
        &state.db,
        &state.question_store,
        &session_id,
        &target_state,
        &slug,
        allow,
        project_path.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn resolve_task_state_transition(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tool_use_id: String,
    target_state: String,
    slug: String,
    allow: bool,
) -> Result<db::SessionRow, AppCommandError> {
    resolve_task_state_transition_inner(&state, session_id, tool_use_id, target_state, slug, allow)
        .await
}

/// Pure-Rust core of [`resolve_task_state_transition`].
/// Extracted into a free-standing function so it can be
/// unit-tested WITHOUT a `tauri::test::mock_app`.
///
/// `project_path` MUST be the absolute path to the project
/// root (i.e. the directory containing `.everlasting/`). The
/// IPC layer resolves this from the session's `project_id`.
pub(crate) async fn resolve_task_state_transition_internal(
    db_pool: &sqlx::SqlitePool,
    store: &crate::agent::question_store::QuestionStore,
    session_id: &str,
    target_state: &str,
    slug: &str,
    allow: bool,
    project_path: Option<&std::path::Path>,
) -> Result<db::SessionRow, AppCommandError> {
    // 1. Parse + validate target_state (lenient — unknown
    //    → Err so the resolve can short-circuit).
    let new_state = match crate::agent::workflow::parse_target_state(target_state) {
        Ok(s) => s,
        Err(e) => {
            // Audited as denied (invalid target) — the user
            // can't have authorized an invalid target.
            record_state_transition_audit(
                db_pool,
                session_id,
                target_state,
                slug,
                AuditKind::TaskStateTransitionDenied,
                Some("invalid_target_state"),
                None,
            )
            .await;
            // Surface the error to the caller; resolve the
            // oneshot as `Cancelled` so the agent loop
            // unblocks.
            let _ = store
                .resolve(session_id, InteractionResponse::Cancelled)
                .await;
            return Err(AppCommandError::new(
                crate::error::ErrorCategory::InvalidRequest,
                format!(
                    "resolve_task_state_transition: parse_target_state failed: {}",
                    e
                ),
            ));
        }
    };

    // 2. If denied, just resolve as `Cancelled` + write the
    //    denied audit. No `set_task_state` call (the on-disk
    //    `task.json` is NOT mutated).
    if !allow {
        record_state_transition_audit(
            db_pool,
            session_id,
            target_state,
            slug,
            AuditKind::TaskStateTransitionDenied,
            Some("user denied"),
            None,
        )
        .await;
        // Resolve the oneshot so the agent loop unblocks.
        store
            .resolve(session_id, InteractionResponse::Cancelled)
            .await?;
        // Reload the row so the IPC caller can refresh
        // `currentSession` (state unchanged on the deny path).
        return load_session_row(db_pool, session_id).await;
    }

    // 3. Allow path — apply the state transition via the
    //    pure `workflow::set_task_state`. We need the
    //    project's path + the current task's status.
    let project_path = match project_path {
        Some(p) => p,
        None => {
            // Without a project path we can't locate
            // `<project>/.everlasting/tasks/<slug>/task.json`.
            // Audit denied + surface error.
            record_state_transition_audit(
                db_pool,
                session_id,
                target_state,
                slug,
                AuditKind::TaskStateTransitionDenied,
                Some("missing_project_path"),
                None,
            )
            .await;
            let _ = store
                .resolve(session_id, InteractionResponse::Cancelled)
                .await;
            return Err(AppCommandError::new(
                crate::error::ErrorCategory::InvalidRequest,
                "resolve_task_state_transition: project path not available \
                 (no project loaded for the session)",
            ));
        }
    };

    // Resolve the current task so we know `from` for the
    // set_task_state signature. Read fresh off disk (the
    // IPC handler is the source of truth for the user's
    // allow decision; the LLM-provided current_state in
    // the payload is informational and may be stale under
    // multi-session concurrent edits).
    let current_status = match read_workflow_task(project_path, slug) {
        Ok(t) => t.status,
        Err(e) => {
            // Audit denied (task not found / corrupted).
            record_state_transition_audit(
                db_pool,
                session_id,
                target_state,
                slug,
                AuditKind::TaskStateTransitionDenied,
                Some("task_read_failed"),
                Some(&format!("{:?}", e)),
            )
            .await;
            let _ = store
                .resolve(session_id, InteractionResponse::Cancelled)
                .await;
            return Err(AppCommandError::new(
                crate::error::ErrorCategory::Server,
                format!(
                    "resolve_task_state_transition: read_task failed for slug {:?}: {}",
                    slug, e
                ),
            ));
        }
    };

    // Apply the state transition (single source of truth for
    // the `from → to` hook dispatch).
    let apply_result = workflow_set_task_state(project_path, slug, &current_status, &new_state);
    match apply_result {
        Ok(_updated_task) => {
            // Allowed audit (on top of any side-effect audits
            // written inside set_task_state — currently none,
            // so this is the row that records "user allowed
            // transition X→Y").
            let audit_payload = serde_json::json!({
                "target_state": target_state,
                "from": current_status.as_str(),
                "to": new_state.as_str(),
                "slug": slug,
            })
            .to_string();
            if let Err(e) = db::record_audit_event(
                db_pool,
                session_id,
                AuditKind::TaskStateTransitionAllowed.as_str(),
                Some(&audit_payload),
                None,
            )
            .await
            {
                tracing::warn!(
                    error = %e,
                    "resolve_task_state_transition: allowed audit failed"
                );
            }
            // Resolve the oneshot so the agent loop unblocks.
            store
                .resolve(
                    session_id,
                    InteractionResponse::Answered(serde_json::json!(true)),
                )
                .await?;
            // Reload the session row so the IPC caller can
            // refresh `currentSession` (status is on the
            // task.json, not the session row, but the
            // SessionRow is the established return shape for
            // the resolve-mode-change pattern).
            load_session_row(db_pool, session_id).await
        }
        Err(StateTransitionError::InvalidTargetState(s)) => {
            // Defensive — `parse_target_state` already
            // validated above; should be unreachable.
            record_state_transition_audit(
                db_pool,
                session_id,
                target_state,
                slug,
                AuditKind::TaskStateTransitionDenied,
                Some("set_task_state_invalid_target"),
                Some(&s),
            )
            .await;
            let _ = store
                .resolve(session_id, InteractionResponse::Cancelled)
                .await;
            Err(AppCommandError::new(
                crate::error::ErrorCategory::InvalidRequest,
                format!(
                    "resolve_task_state_transition: set_task_state rejected target {:?}",
                    s
                ),
            ))
        }
        Err(e) => {
            // Apply failed (IO / malformed / not-found).
            // The tool still needs to unblock — resolve as
            // `Cancelled` + write the denied audit with the
            // specific reason.
            record_state_transition_audit(
                db_pool,
                session_id,
                target_state,
                slug,
                AuditKind::TaskStateTransitionDenied,
                Some("set_task_state_failed"),
                Some(&format!("{}", e)),
            )
            .await;
            let _ = store
                .resolve(session_id, InteractionResponse::Cancelled)
                .await;
            Err(AppCommandError::new(
                crate::error::ErrorCategory::Server,
                format!(
                    "resolve_task_state_transition: set_task_state failed: {}",
                    e
                ),
            ))
        }
    }
}

/// Write a state-transition audit row. Best-effort (matches
/// the rest of the audit surface — failures log + swallow so
/// the user-facing flow isn't blocked). Extracted into a
/// helper so the 4 call sites in
/// `resolve_task_state_transition_internal` stay readable.
async fn record_state_transition_audit(
    db_pool: &sqlx::SqlitePool,
    session_id: &str,
    target_state: &str,
    slug: &str,
    kind: AuditKind,
    reason: Option<&str>,
    detail: Option<&str>,
) {
    let payload = serde_json::json!({
        "target_state": target_state,
        "slug": slug,
        "reason": reason,
        "detail": detail,
    })
    .to_string();
    if let Err(e) =
        db::record_audit_event(db_pool, session_id, kind.as_str(), Some(&payload), None).await
    {
        tracing::warn!(
            error = %e,
            kind = %kind.as_str(),
            "resolve_task_state_transition: record_audit_event failed"
        );
    }
}

/// Resolve the project path from the session row. Used by the
/// IPC handler to locate `<project>/.everlasting/tasks/<slug>/`
/// for the `set_task_state` call. Returns
/// `Err(InvalidRequest)` if the session has no project loaded
/// (e.g. orphan session).
async fn state_to_project_path(
    state: &Arc<AppState>,
    session_id: &str,
) -> Result<Option<std::path::PathBuf>, AppCommandError> {
    let loaded = db::load_session(&state.db, session_id)
        .await
        .map_err(|e| {
            AppCommandError::new(
                crate::error::ErrorCategory::Server,
                format!("state_to_project_path: load_session failed: {}", e),
            )
        })?
        .ok_or_else(|| {
            AppCommandError::new(
                crate::error::ErrorCategory::InvalidRequest,
                format!("state_to_project_path: session '{}' not found", session_id),
            )
        })?;
    let project = db::get_project(&state.db, &loaded.session.project_id)
        .await
        .map_err(|e| {
            AppCommandError::new(
                crate::error::ErrorCategory::Server,
                format!("state_to_project_path: get_project failed: {}", e),
            )
        })?;
    Ok(project.map(|p| std::path::PathBuf::from(p.path)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_response_from_args_cancelled() {
        let r = resolve_response_from_args(Some(true), None).expect("ok");
        assert!(matches!(r, InteractionResponse::Cancelled));
    }

    #[test]
    fn resolve_response_from_args_answered() {
        let answers = vec![QuestionAnswer {
            question: "q".into(),
            header: None,
            options: vec!["a".into()],
            multi_select: false,
        }];
        let r = resolve_response_from_args(None, Some(answers.clone())).expect("ok");
        match r {
            InteractionResponse::Answered(v) => {
                let parsed: Vec<QuestionAnswer> = serde_json::from_value(v).unwrap();
                assert_eq!(parsed, answers);
            }
            other => panic!("expected Answered, got {:?}", other),
        }
    }

    #[test]
    fn resolve_response_from_args_default_answered_empty() {
        let r = resolve_response_from_args(None, None).expect("ok");
        match r {
            InteractionResponse::Answered(v) => {
                let parsed: Vec<QuestionAnswer> = serde_json::from_value(v).unwrap();
                assert!(parsed.is_empty());
            }
            other => panic!("expected Answered, got {:?}", other),
        }
    }

    #[test]
    fn interaction_kind_round_trip() {
        // sanity: as_str() matches the wire lowercase form
        // (the IPC consumer reads these as the `kind` field).
        assert_eq!(InteractionKind::Question.as_str(), "question");
        assert_eq!(InteractionKind::ModeChange.as_str(), "mode_change");
        assert_eq!(
            InteractionKind::TaskStateTransition.as_str(),
            "task_state_transition"
        );
    }

    #[test]
    fn pending_interaction_kind_helper() {
        // The kind() helper is the single source of truth for
        // which variant a `PendingInteraction` carries — used
        // by the store's `register` to populate the
        // `PendingQuestion.kind` field without re-pattern-matching.
        let q = PendingInteraction::Question(ToolQuestionPayload {
            session_id: "s1".into(),
            tool_use_id: "tu".into(),
            questions: vec![],
            ts: 0,
        });
        let m = PendingInteraction::ModeChange(ModeChangePayload {
            session_id: "s1".into(),
            tool_use_id: "tu".into(),
            target_mode: "edit".into(),
            current_mode: None,
            reason: None,
            ts: 0,
        });
        let t = PendingInteraction::TaskStateTransition(TaskStateTransitionPayload {
            session_id: "s1".into(),
            tool_use_id: "tu".into(),
            target_state: "in_progress".into(),
            current_state: Some("planning".into()),
            slug: Some("my-feat".into()),
            reason: None,
            ts: 0,
        });
        assert_eq!(q.kind(), InteractionKind::Question);
        assert_eq!(m.kind(), InteractionKind::ModeChange);
        assert_eq!(t.kind(), InteractionKind::TaskStateTransition);
    }
}

// ---------------------------------------------------------------------------
// Phase E2 + E3 (07-07-request-mode-change-tool) — IPC handler unit tests.
//
// Two sibling test modules:
// - `tests_get_pending_interaction` (E2) — covers the
//   `get_pending_interaction` IPC behavior by exercising
//   `QuestionStore::get_payload` directly (the IPC handler is
//   a thin wrapper around it; `mock_app` is not used in this
//   codebase per the `permission_response` precedent).
// - `tests_resolve_mode_change` (E3) — covers the
//   `resolve_mode_change` IPC behavior by exercising
//   `resolve_mode_change_internal` (the pure-Rust core extracted
//   from the IPC handler in this same module; the IPC wrapper
//   threads `&state.db` + `&state.question_store` through).
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests_get_pending_interaction.rs"]
mod tests_get_pending_interaction;

#[cfg(test)]
#[path = "tests_resolve_mode_change.rs"]
mod tests_resolve_mode_change;
