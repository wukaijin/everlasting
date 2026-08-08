//! `request_task_state_transition` — LLM-initiated workflow
//! state-transition request tool (Phase 3 Step 3.1 of
//! `07-08-workflow-integration`, 2026-07-08).
//!
//! Subagent-clone of the workflow state-machine's user-confirmation
//! gate. Lets the agent ask the user to move the current task's
//! `task.json.status` from one workflow state to another
//! (`planning` → `in_progress` → `done` for the dev plugin). The
//! execution is **blocking** (the agent loop's turn
//! suspends until the user allows / denies), mirroring
//! `request_mode_change`'s structure.
//!
//! ## Why this lives next to the regular tools (not in agent/.
//! workflow/ or agent/::workflow/::state)
//!
//! - Shape-wise it's a tool — the LLM discovers it via
//!   `builtin_tools()`'s schema.
//! - Execution-wise it needs the agent loop's `tokio::select!`
//!   `cancel` arm (session cancel propagates), the `QuestionStore`
//!   oneshot (the same store `ask_user_question` and
//!   `request_mode_change` use — all three are "interactive
//!   user-blocked" interactions, single-pending gate across ALL
//!   kinds per design §3.3), and the
//!   `ChatEventSink::emit_task_state_transition` trait method.
//!   All three are already threaded through `chat_loop.rs`.
//!
//! ## Why the execute path bypasses `execute_tool`
//!
//! `execute_tool_inner` is dispatch-by-name (`match name`), but it
//! has no access to `QuestionStore` or `ChatEventSink
//! ::emit_task_state_transition`. Per design §7 / PRD §R1, the
//! clean path is to keep this tool out of the dispatch table and
//! have `chat_loop.rs` recognize the tool name and call
//! `execute_blocking` directly — same shape as how
//! `dispatch_subagent` / `ask_user_question` /
//! `request_mode_change` are intercepted. The
//! `execute_tool_inner` `match` arm is intentionally absent (with a
//! doc comment pointing at `chat_loop.rs`).
//!
//! ## Why tool does NOT mutate `task.json` directly
//!
//! Per design §5.2 (the BIG design decision from M-A) and PRD §R8:
//! the **only** place that calls `agent::workflow::set_task_state`
//! is the `resolve_task_state_transition` IPC handler
//! (`commands/question.rs`). The
//! `request_task_state_transition` tool:
//!   1. validates the schema (short-circuits with `is_error: true`),
//!   2. noop-checks (`target == current` → immediate return),
//!   3. records `task_state_transition_requested` audit,
//!   4. registers a oneshot in `QuestionStore` (tagged
//!      `PendingInteraction::TaskStateTransition`),
//!   5. emits `task:state:transition:request` to the frontend,
//!   6. `tokio::select! { cancel | oneshot }` until resolve,
//!   7. returns the `tool_result` (allowed / cancelled /
//!      cancelled_by_session).
//!
//! The on-disk `task.json` write happens in the
//! `resolve_task_state_transition` IPC handler (apply BEFORE
//! resolve — the "double-IPC pattern" of `resolve_mode_change`).
//! This keeps the `from → to` hook dispatch (`trigger_spec_distillation`
//! on `in_progress → Done`; `preflight_implement_check` on
//! `Planning → in_progress`) in a single place — the IPC handler.
//! Without this separation, hooks would fire BEFORE the user
//! confirms (a regression that the Q9 design explicitly rules out:
//! "Rust 固定 hook 嵌入 set_task_state 写入路径").
//!
//! ## Wire shape
//!
//! ```json
//! {
//!   "target_state": "in_progress",
//!   "slug": "my-feat",
//!   "reason": "research complete; ready to implement"
//! }
//! ```
//!
//! Schema validation is **strict** — boundary violations
//! (`target_state` not in `planning`/`in_progress`/`done` /
//! empty / not a valid slug / `reason` > 500 chars) are
//! `is_error: true` and do NOT enter the blocking wait.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::agent::permissions::AuditKind;
use crate::agent::question_store::{
    InteractionResponse, PendingInteraction, QuestionStore, TaskStateTransitionPayload,
};
use crate::agent::workflow::task::validate_slug;
use crate::agent::workflow::{can_transition, TaskStatus, WorkflowDef};
use crate::db;
use crate::llm::types::ToolDef;
use crate::state::ChatEventSink;

// ---------------------------------------------------------------------------
// Constants — schema boundaries
// ---------------------------------------------------------------------------

// C0 (`07-26-taskstatus-custom-state`): the old hard-coded
// `VALID_STATES = ["planning", "in_progress", "done"]` constant
// is GONE. The dev-only enum rejected review's
// `intake`/`reviewing`/... states outright at `validate` time,
// which is exactly the failure C0 fixes. Legality is now decided
// in `execute_blocking` via `can_transition` against the session's
// `WorkflowDef` (workflow session), or by rejecting
// `TaskStatus::Custom` outright (non-workflow session — see
// design §3). `validate` only enforces non-emptiness + slug +
// reason length now. Full rationale lives on `validate` and
// `execute_blocking`'s doc comments.

/// Max `reason` text length (same as `request_mode_change`'s
/// `MAX_REASON_LEN`). Reasons longer than this are rejected with
/// a structured `is_error: true`.
const MAX_REASON_LEN: usize = 500;

// ---------------------------------------------------------------------------
// LLM input shape (validation handled in execute_blocking)
// ---------------------------------------------------------------------------

/// Wire input for `request_task_state_transition`. Snake_case
/// fields to match the LLM's trained tool-use JSON convention
/// (mirroring `request_mode_change`'s `RequestModeChangeInput`
/// schema posture).
///
/// `slug` is the task identifier the IPC handler needs to locate
/// `<project>/.everlasting/tasks/<slug>/task.json`. The
/// blocking tool reads it from `WorkflowCtx.current_task.slug`
/// at call-entry so the LLM doesn't have to remember it (the
/// schema makes it optional — the tool validates presence and
/// matches against the workflow_ctx-loaded task).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestTaskStateTransitionInput {
    /// Required. Must be one of `planning` / `in_progress` /
    /// `done` (matches `WorkflowDef::states` for the
    /// `dev` plugin — the dev plugin's locked 3-state
    /// machine is the SchemaOfTruth for this tool).
    pub target_state: String,
    /// Required. The slug of the current task. The tool
    /// validates `validate_slug` (so the IPC handler can
    /// `read_task(project_path, slug)` directly without
    /// re-validating). When the LLM forgets it, the tool
    /// passes the caller's `current_slug` (read off
    /// `workflow_ctx.current_task`) as a fallback so the
    /// schema's `required` doesn't trip.
    #[serde(default)]
    pub slug: String,
    /// Optional ≤500-char reason shown on the card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Tool definition — registered in builtin_tools()
// ---------------------------------------------------------------------------

/// The `request_task_state_transition` tool schema. Name MUST
/// match the interception branch in `chat_loop.rs` — that's how
/// the agent loop dispatches to `execute_blocking` instead of
/// `execute_tool`.
pub fn definition() -> ToolDef {
    ToolDef {
        name: "request_task_state_transition".to_string(),
        description: Some(
            "Ask the user to transition the current task to a new workflow state. \
             The valid target states are those declared by the current \
             workflow plugin's workflow.json (dev: planning → in_progress → \
             done; review: intake → reviewing → revising → reported). The \
             user sees an inline card with the target state, the current \
             state, and your reason; they choose Allow or Deny. On Allow, \
             the on-disk task.json is updated and the per-transition Rust \
             hook (spec-distillation on in_progress→done, preflight on \
             planning→in_progress for the dev plugin) fires automatically. \
             On Deny, you get {\"cancelled_by_user\": true} and should \
             adapt.\n\n\
             Only available in workflow sessions. If the target state is the \
             current state, the call returns {\"noop\": true} and no card is \
             shown. If the target state is not reachable from the current \
             state per the plugin's declared transitions, the call returns \
             {\"invalid_transition\": true} with is_error.\n\n\
             Use this when you've completed a workflow phase (research, \
             implementation, review) and need the user to confirm the \
             transition to the next phase. Do NOT self-advance the state — \
             the user must approve each transition."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "target_state": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The state to transition to. Must be a state declared by the current workflow plugin's workflow.json and reachable from the current state via a declared transition (e.g. dev: planning | in_progress | done; review: intake | reviewing | revising | reported)."
                },
                "slug": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 64,
                    "description": "The task slug (matches /[a-z0-9-]{1,64}/). The tool carries the current workflow session's task slug forward — pass an empty string to use the workflow session's current task."
                },
                "reason": {
                    "type": "string",
                    "maxLength": 500,
                    "description": "Optional ≤500-char explanation shown on the card. Be specific about why this state transition is appropriate."
                }
            },
            "required": ["target_state"]
        }),
    }
}

// ---------------------------------------------------------------------------
// Validation result
// ---------------------------------------------------------------------------

/// Internal validation outcome. We reuse `Result<_, ValidationError>`
/// rather than a typed struct so the `execute_blocking` call sites
/// can early-return on the error path without pre-allocating pending
/// state.
#[allow(dead_code)] // `EmptySlug` + `UnknownTargetState` reserved — see notes per variant
#[derive(Debug, thiserror::Error)]
pub(crate) enum ValidationError {
    #[error("`target_state` must be non-empty")]
    EmptyTargetState,
    /// C0: validate no longer rejects unknown target_state values
    /// — they flow through as `TaskStatus::Custom` and legality is
    /// decided in `execute_blocking` by `can_transition`. Kept for
    /// exhaustiveness + a future schema-tightening if the wire
    /// enum ever needs to come back.
    #[error("`target_state` is not a valid workflow state (got: {got:?})")]
    UnknownTargetState { got: String },
    #[error("`slug` is empty — pass the current workflow task's slug explicitly, or set it via WorkflowCtx")]
    EmptySlug,
    #[error("`slug` {0:?} is not a valid slug (expected [a-z0-9-]{{1,64}})")]
    InvalidSlug(String),
    #[error("`reason` exceeds {max} characters (got {got})")]
    ReasonTooLong { max: usize, got: usize },
}

/// Schema validation — does NOT enter the blocking wait. Returns
/// `Err(ValidationError)` for short-circuit failures (the agent
/// loop pushes the error string as `tool_result` content with
/// `is_error: true`).
///
/// **C0 (`07-26-taskstatus-custom-state`)**: this no longer
/// rejects unknown `target_state` strings. Plugin-defined states
/// (review's `intake`/`reviewing`/...) pass through; legality is
/// decided in [`execute_blocking`] via
/// [`crate::agent::workflow::can_transition`] against the
/// session's `WorkflowDef` (workflow session), or by rejecting
/// `TaskStatus::Custom` outright (non-workflow session). Only
/// non-emptiness + slug shape + reason length are checked here.
pub(crate) fn validate(input: &RequestTaskStateTransitionInput) -> Result<(), ValidationError> {
    let t = input.target_state.trim();
    if t.is_empty() {
        return Err(ValidationError::EmptyTargetState);
    }
    // slug is empty → caller didn't supply one. The chat_loop
    // passes the workflow_ctx's current task slug via the
    // `current_slug` parameter (which has higher precedence).
    // We only validate the LLM-supplied slug here.
    if !input.slug.is_empty() {
        if let Err(_e) = validate_slug(&input.slug) {
            return Err(ValidationError::InvalidSlug(input.slug.clone()));
        }
    }
    if let Some(r) = input.reason.as_ref() {
        if r.chars().count() > MAX_REASON_LEN {
            return Err(ValidationError::ReasonTooLong {
                max: MAX_REASON_LEN,
                got: r.chars().count(),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// execute_blocking — the ONLY execution entry point
// ---------------------------------------------------------------------------

/// Result tuple shape — matches the agent loop's existing
/// `execute_tool` return shape so the call site at
/// `chat_loop.rs` can build the same `ContentBlock::ToolResult`.
/// `Option<i32>` (exit_code) is always `None` here — no shell
/// spawned.
pub type BlockingToolResult = (
    /* content */ String,
    /* is_error */ bool,
    /* tool_context_update */ crate::tools::ToolContextUpdate,
    /* exit_code */ Option<i32>,
);

/// Long-running execution. Pipeline (mirrors
/// `request_mode_change::execute_blocking`):
///
/// 1. Parse + validate. Bad input → short-circuit
///    `(error, true, _, None)`. Does NOT touch the store or
///    emit anything.
/// 2. **Noop check**: if `target_state == current_state` →
///    write `task_state_transition_requested{noop: true}` audit
///    and return `(json noop, false, _, None)` WITHOUT
///    registering or emitting (no card; matches PRD R7).
/// 3. Write `task_state_transition_requested` audit (always).
/// 4. `QuestionStore::register(session_id, tool_use_id,
///    PendingInteraction::TaskStateTransition(payload))` —
///    `AlreadyPending` race returns a structured error string;
///    the first pending (whatever kind) stays untouched.
/// 5. `sink.emit_task_state_transition(&payload)` — fires the
///    `task:state:transition:request` Tauri event so the
///    frontend renders the
///    `<RequestTaskStateTransitionCard>`.
/// 6. `tokio::select! { biased; cancel | oneshot }`:
///    - cancel arm → `store.remove()`, returns
///      `({"cancelled_by_session": true}, true, _, None)`.
///    - oneshot arm — `InteractionResponse` matched:
///      - `Answered(true)` → return `({"allowed": true,
///        "prev_state": "...", "new_state": "..."}, false, _,
///        None)`.
///      - `Cancelled` → return `({"cancelled_by_user": true,
///        "target_state": "..."}, true, _, None)`.
///      - `Err(RecvError)` (sender dropped by the cancel arm's
///        `store.remove`) → return
///        `({"cancelled_by_session": true}, true, _, None)`.
///
/// **No retries, no timeout** — v1 keeps it simple (PRD §"Notes").
///
/// `current_state` is read off `workflow_ctx.current_task.status`
/// at the time the call site invokes this function. The
/// `current_slug` is the workflow ctx's task slug; if the LLM
/// supplied a non-empty `slug` it must match (else
/// `StateTransitionError` short-circuit). The function itself
/// is purely a "request → wait → return" carrier (no DB / disk
/// write — see module docstring).
///
/// **Workflow-session gate**: when `workflow_ctx = None` (the
/// caller isn't a workflow session) the function short-circuits
/// with `is_error: true` and a structured message. Same posture
/// as how `update_checklist`'s workflow branch is gated — the
/// tool exists for workflow sessions only; non-workflow
/// sessions can't migrate state.
#[allow(clippy::too_many_arguments)]
pub async fn execute_blocking(
    input: &serde_json::Value,
    session_id: &str,
    tool_use_id: &str,
    current_state: Option<TaskStatus>,
    current_slug: Option<String>,
    db_pool: &sqlx::SqlitePool,
    store: &QuestionStore,
    sink: &Arc<dyn ChatEventSink>,
    cancel: &CancellationToken,
    // C0 (`07-26-taskstatus-custom-state`): the session's
    // WorkflowDef. `Some` for workflow sessions — used to validate
    // the `from → to` transition via `can_transition` (the plugin's
    // `transitions` is the source of truth for legality). `None`
    // for non-workflow sessions — then `TaskStatus::Custom` targets
    // are rejected (no plugin = no basis for custom states).
    workflow_def: Option<&WorkflowDef>,
    // E2 (2026-07-14): per-turn seq for audit turn alignment. The
    // caller (chat_loop interceptor) passes `Some(seq)` from inside
    // the turn loop so the `task_state_transition_requested` audit
    // row lands in the correct turn group.
    turn_seq: Option<i64>,
) -> BlockingToolResult {
    // ---- 1. Parse + validate ----------------------------------------
    let parsed: RequestTaskStateTransitionInput = match serde_json::from_value(input.clone()) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("request_task_state_transition: invalid input JSON: {}", e);
            tracing::warn!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                error = %e,
                "request_task_state_transition: short-circuit on parse error"
            );
            return (msg, true, crate::tools::ToolContextUpdate::default(), None);
        }
    };
    if let Err(e) = validate(&parsed) {
        let msg = format!(
            "request_task_state_transition: schema validation failed: {}",
            e
        );
        tracing::warn!(
            session_id = %session_id,
            tool_use_id = %tool_use_id,
            error = %e,
            "request_task_state_transition: short-circuit on schema validation"
        );
        return (msg, true, crate::tools::ToolContextUpdate::default(), None);
    }

    let target_state_str = parsed.target_state.trim().to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    // ---- 1b. Resolve slug + workflow session gate -------------------
    let resolved_slug = if !parsed.slug.is_empty() {
        // LLM supplied a slug — it must match the workflow
        // session's current task slug, otherwise we're
        // transitioning a task we don't own (a security risk:
        // a misbehaving LLM could flip state on a project the
        // user isn't actively editing).
        match &current_slug {
            Some(cs) if cs == &parsed.slug => parsed.slug.clone(),
            _ => {
                let msg = format!(
                    "request_task_state_transition: supplied slug {:?} doesn't match the current workflow task slug {:?}",
                    parsed.slug, current_slug
                );
                tracing::warn!(
                    session_id = %session_id,
                    tool_use_id = %tool_use_id,
                    "request_task_state_transition: supplied slug mismatch"
                );
                return (msg, true, crate::tools::ToolContextUpdate::default(), None);
            }
        }
    } else {
        // LLM didn't supply — fall back to the workflow ctx.
        match &current_slug {
            Some(s) => s.clone(),
            None => {
                // No current task at all → the session isn't a
                // workflow session with an active task.
                // Short-circuit with the structured "no current
                // task" marker (same posture as the gate
                // below when workflow_ctx is None).
                let msg = "request_task_state_transition: no active workflow task; \
                           this tool requires an in-progress task in a workflow session";
                tracing::warn!(
                    session_id = %session_id,
                    tool_use_id = %tool_use_id,
                    "request_task_state_transition: no current_slug supplied and no workflow task"
                );
                return (
                    msg.to_string(),
                    true,
                    crate::tools::ToolContextUpdate::default(),
                    None,
                );
            }
        }
    };

    // The LLM's intent — parsed into a TaskStatus (already
    // validated against VALID_STATES). Used for the noop check
    // + audit payload.
    let target_state = match crate::agent::workflow::parse_target_state(&target_state_str) {
        Ok(s) => s,
        // Should be unreachable — `validate` already accepted it.
        Err(e) => {
            let msg = format!(
                "request_task_state_transition: parse_target_state failed post-validate: {}",
                e
            );
            tracing::error!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                error = %e,
                "request_task_state_transition: unreachable parse post-validate"
            );
            return (msg, true, crate::tools::ToolContextUpdate::default(), None);
        }
    };

    // ---- 2. Noop check (PRD R7, mirroring request_mode_change) -----
    // `current_state` is the workflow ctx's current task
    // status. If the LLM is requesting a transition to the
    // same state, return immediately with `noop: true` and
    // no card. Matches the `request_mode_change` noop
    // posture.
    let noop = matches!(current_state.as_ref(), Some(s) if s == &target_state);
    if noop {
        // Audit: requested with noop marker (every tool call
        // gets a `*_requested` audit row).
        let audit_payload = serde_json::json!({
            "target_state": target_state_str,
            "current_state": current_state.as_ref().map(|s| s.as_str()).unwrap_or(""),
            "slug": resolved_slug,
            "noop": true,
        })
        .to_string();
        if let Err(e) = db::record_audit_event(
            db_pool,
            session_id,
            AuditKind::TaskStateTransitionRequested.as_str(),
            Some(&audit_payload),
            turn_seq,
        )
        .await
        {
            tracing::warn!(
                error = %e,
                "request_task_state_transition: record_audit(requested noop) failed"
            );
        }
        let content = serde_json::json!({
            "noop": true,
            "current_state": target_state_str,
        })
        .to_string();
        return (
            content,
            false,
            crate::tools::ToolContextUpdate::default(),
            None,
        );
    }

    // ---- 2b. Transition legality check (C0, design §3) -------------
    // `parse_target_state` no longer rejects unknown values (they
    // become TaskStatus::Custom), so legality must be decided HERE.
    // - Workflow session: the plugin's `WorkflowDef::transitions` is
    //   the source of truth — `can_transition(def, from, to)` returns
    //   true iff the edge is declared. This is what lets review's
    //   intake→reviewing pass while planning→done (no such edge)
    //   fails, all driven by the plugin's workflow.json.
    // - Non-workflow session (no plugin): fall back to the legacy
    //   posture — reject Custom target_state. Without a plugin there's
    //   no basis for a custom state; the dev accept-list still applies.
    //
    // `from` resolves from current_state (string form via as_str); if
    // the session has no current task yet we use the plugin's declared
    // `initial` state — that's the only sensible "from" for a fresh
    // task and matches how `set_task_state`'s caller-side snapshot
    // would resolve it.
    if let Some(def) = workflow_def {
        let from_str = current_state
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or(def.initial.as_str());
        if !can_transition(def, from_str, &target_state_str) {
            tracing::warn!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                from = %from_str,
                to = %target_state_str,
                plugin = %def.name,
                "request_task_state_transition: undeclared transition rejected \
                 (allowed transitions are defined in the plugin's workflow.json)"
            );
            return (
                serde_json::json!({
                    "invalid_transition": true,
                    "from": from_str,
                    "target_state": target_state_str,
                    "plugin": def.name,
                })
                .to_string(),
                true,
                crate::tools::ToolContextUpdate::default(),
                None,
            );
        }
    } else if matches!(target_state, TaskStatus::Custom(_)) {
        // Non-workflow session + Custom target: no plugin to back the
        // custom state. Reject — mirrors the legacy "must be a known
        // TaskStatus" posture for non-workflow callers.
        tracing::warn!(
            session_id = %session_id,
            tool_use_id = %tool_use_id,
            target_state = %target_state_str,
            "request_task_state_transition: Custom target rejected in non-workflow session \
             (no workflow plugin loaded to define a custom state)"
        );
        return (
            serde_json::json!({
                "invalid_transition": true,
                "target_state": target_state_str,
                "reason": "no workflow plugin loaded",
            })
            .to_string(),
            true,
            crate::tools::ToolContextUpdate::default(),
            None,
        );
    }

    // ---- 3. Build payload + requested audit ------------------------
    let payload = TaskStateTransitionPayload {
        session_id: session_id.to_string(),
        tool_use_id: tool_use_id.to_string(),
        target_state: target_state_str.clone(),
        current_state: current_state.as_ref().map(|s| s.as_str().to_string()),
        slug: Some(resolved_slug.clone()),
        reason: parsed.reason.clone(),
        ts: now,
    };

    // Audit: requested (always). The audit payload is the
    // "user-visible record" of the LLM's ask.
    let audit_payload = serde_json::json!({
        "target_state": target_state_str,
        "current_state": current_state.as_ref().map(|s| s.as_str()).unwrap_or(""),
        "slug": resolved_slug,
        "reason": parsed.reason,
        "noop": false,
    })
    .to_string();
    if let Err(e) = db::record_audit_event(
        db_pool,
        session_id,
        AuditKind::TaskStateTransitionRequested.as_str(),
        Some(&audit_payload),
        turn_seq,
    )
    .await
    {
        tracing::warn!(
            error = %e,
            "request_task_state_transition: record_audit(requested) failed"
        );
    }

    // ---- 4. Register + emit -----------------------------------------
    let rx = match store
        .register(
            session_id,
            tool_use_id,
            PendingInteraction::TaskStateTransition(payload.clone()),
        )
        .await
    {
        Ok(rx) => rx,
        Err(crate::agent::question_store::QuestionStoreError::AlreadyPending) => {
            // Same single-pending gate as ask_user_question +
            // request_mode_change (a pending question / mode
            // change / state transition blocks all kinds; the
            // gate is across ALL THREE kinds per design §3.3).
            let msg = "已有 pending interaction,等当前处理完成".to_string();
            tracing::warn!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                "request_task_state_transition: AlreadyPending — concurrent register"
            );
            return (msg, true, crate::tools::ToolContextUpdate::default(), None);
        }
        Err(e) => {
            // `NotFound` is not reachable from `register` —
            // defensive branch (register only ever returns
            // `AlreadyPending`). Kept for exhaustiveness.
            let msg = format!("request_task_state_transition: store error: {}", e);
            tracing::error!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                error = %e,
                "request_task_state_transition: unexpected register error"
            );
            return (msg, true, crate::tools::ToolContextUpdate::default(), None);
        }
    };
    // Emit AFTER register — the frontend's
    // `get_pending_interaction` fallback (used on session
    // reload) needs the entry to exist when the event arrives.
    // Symmetric with the ask_user_question +
    // request_mode_change flows.
    sink.emit_task_state_transition(&payload);

    // ---- 5. Wait for resolve / cancel -------------------------------
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            // Session cancel (user Stop / app shutdown).
            // Drop the oneshot receiver so the sender's resolve
            // (if it arrives late) becomes a no-op. Clean the
            // store entry.
            store.remove(session_id).await;
            tracing::info!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                "request_task_state_transition: cancelled by session token"
            );
            let content = serde_json::json!({"cancelled_by_session": true})
                .to_string();
            (content, true, crate::tools::ToolContextUpdate::default(), None)
        }
        resp = rx => {
            match resp {
                Ok(InteractionResponse::Answered(_)) => {
                    // Allowed path. The actual
                    // `set_task_state` (with `from → to` hook
                    // dispatch) happened in the
                    // `resolve_task_state_transition` IPC
                    // handler (single source of truth for
                    // state-transition side effects per design
                    // §5.2 M-A / Q9 "Rust 固定 hook 嵌入
                    // set_task_state"). We only return the
                    // result; the new status is now in
                    // `task.json` + the next LLM turn's
                    // `WorkflowCtx` re-resolves it via
                    // `build_workflow_ctx` (the breadcrumb
                    // reflects the new state).
                    let content = serde_json::json!({
                        "allowed": true,
                        "prev_state": current_state.as_ref().map(|s| s.as_str().to_string()).unwrap_or_default(),
                        "new_state": target_state_str,
                    })
                    .to_string();
                    (content, false, crate::tools::ToolContextUpdate::default(), None)
                }
                Ok(InteractionResponse::Cancelled) => {
                    // User clicked 拒绝. The `from → to` hook
                    // did NOT fire (the IPC handler guards:
                    // !allow → skip set_task_state).
                    let content = serde_json::json!({
                        "cancelled_by_user": true,
                        "target_state": target_state_str,
                    })
                    .to_string();
                    (content, true, crate::tools::ToolContextUpdate::default(), None)
                }
                Err(_recv_err) => {
                    // Sender dropped (e.g. resolve ran on a
                    // stale session id after cancel arm cleaned
                    // the entry). Treat as session-cancelled —
                    // safe default per the
                    // permission-store / mode-change parity.
                    let content = serde_json::json!({"cancelled_by_session": true})
                        .to_string();
                    (content, true, crate::tools::ToolContextUpdate::default(), None)
                }
            }
        }
    }
}
