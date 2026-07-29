//! `request_mode_change` — LLM-initiated mode-switch request tool
//!
//! Subagent-clone of Claude Code's `mode_ask`: lets the agent
//! ask the user to switch the session's permission mode (edit /
//! plan / yolo). The execution is **blocking** (the agent loop's
//! turn suspends until the user allows or denies); see design
//! §3.1 + PRD §R3.
//!
//! ## Why this lives next to the regular tools (not in agent/.
//! permissions/ or agent/::permissions/::ask)
//!
//! - Shape-wise it's a tool — the LLM discovers it via
//!   `builtin_tools()`'s schema.
//! - Execution-wise it needs the agent loop's `tokio::select!`
//!   `cancel` arm (session cancel propagates), the `QuestionStore`
//!   oneshot (the same store `ask_user_question` uses — both are
//!   "interactive user-blocked" interactions, single-pending gate
//!   across both kinds per design §3.3), and the
//!   `ChatEventSink::emit_mode_change_request` trait method. All
//!   three are already threaded through `chat_loop.rs`.
//!
//! ## Why the execute path bypasses `execute_tool`
//!
//! `execute_tool_inner` is dispatch-by-name (`match name`), but it
//! has no access to `QuestionStore` or `ChatEventSink
//! ::emit_mode_change_request`. Per design §7 / PRD §R1, the
//! clean path is to keep this tool out of the dispatch table
//! and have `chat_loop.rs` recognize the tool name and call
//! `execute_blocking` directly — same shape as how
//! `dispatch_subagent` and `ask_user_question` are intercepted.
//! The `execute_tool_inner` `match` arm is intentionally absent
//! (with a doc comment pointing at `chat_loop.rs`).
//!
//! ## Why tool does NOT call `db::update_session_mode` directly
//!
//! Per design §6 decision (the BIG design change from PRD R4
//! 初稿), the **only** mode-application site in the codebase is
//! the `set_session_mode_internal` pure function (extracted
//! from the `set_session_mode` IPC handler). The
//! `request_mode_change` tool:
//!   1. validates the schema (short-circuits with `is_error: true`),
//!   2. checks noop (`target == current` → immediate return),
//!   3. records `mode_change_requested` audit,
//!   4. registers a oneshot in QuestionStore (tagged
//!      `PendingInteraction::ModeChange`),
//!   5. emits `mode:change:request` to the frontend,
//!   6. `tokio::select! { cancel | oneshot }` until resolve,
//!   7. returns the `tool_result` (allowed / cancelled /
//!      cancelled_by_session).
//!
//! The DB write happens in the `resolve_mode_change` IPC handler
//! (the front-end invokes it after the user clicks 允许 / 拒绝).
//! This keeps Yolo root guard / `mode_changed` audit / etc. in a
//! single place; the tool stays purely a "request → wait → return"
//! carrier.
//!
//! ## Wire shape (PRD §R2 schema)
//!
//! ```json
//! {
//!   "target_mode": "edit",
//!   "reason": "需要落代码"
//! }
//! ```
//!
//! Schema validation is **strict** — boundary violations
//! (`target_mode` not in {edit, plan, yolo} / empty / `reason`
//! > 500 chars) are `is_error: true` and do NOT enter the blocking
//! wait.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::agent::question_store::{
    InteractionResponse, ModeChangePayload, PendingInteraction, QuestionStore,
};
use crate::db;
use crate::llm::types::ToolDef;
use crate::state::ChatEventSink;

// ---------------------------------------------------------------------------
// Constants — schema boundaries
// ---------------------------------------------------------------------------

/// The 3 user-facing modes the LLM can request. The
/// `background` enum variant is reserved in the type system
/// for forward-compat but is NOT exposed via this tool (the
/// PRD explicitly limits the LLM to the 3 user-facing modes;
/// PRD §"目标 mode 集").
const VALID_MODES: &[&str] = &["edit", "plan", "yolo"];

/// Max `reason` text length (PRD §R2). Reasons longer than
/// this are rejected with a structured `is_error: true`.
const MAX_REASON_LEN: usize = 500;

// ---------------------------------------------------------------------------
// LLM input shape (validation handled in execute_blocking)
// ---------------------------------------------------------------------------

/// Wire input for `request_mode_change`. Snake_case fields to
/// match the LLM's trained tool-use JSON convention (per Claude
/// Code's trained `mode_ask` parity; the LLM-tool schema is
/// snake_case; the IPC payload is also snake_case — same
/// shared-struct exemption as `ToolQuestionPayload`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestModeChangeInput {
    /// Required. Must be one of `edit` / `plan` / `yolo`.
    /// The `background` enum variant is intentionally NOT
    /// accepted (the LLM can't request it; schema rejects
    /// unknown values with `is_error: true`).
    pub target_mode: String,
    /// Optional ≤500-char reason shown on the card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Tool definition — registered in builtin_tools() (low-risk
// schema; SDK-style sync execution never fires because
// chat_loop intercepts)
// ---------------------------------------------------------------------------

/// The `request_mode_change` tool schema. Name MUST match the
/// interception branch in `chat_loop.rs` — that's how the
/// agent loop dispatches to `execute_blocking` instead of
/// `execute_tool`.
pub fn definition() -> ToolDef {
    ToolDef {
        name: "request_mode_change".to_string(),
        description: Some(
            "Ask the user to switch this session's permission mode (edit / plan / \
             yolo). The user sees an inline card with the target mode and the reason \
             you provide; they choose Allow or Deny. On Allow, the mode is updated \
             and the system prompt reflects the new mode on the next turn. On Deny, \
             you get {\"cancelled_by_user\": true} and should adapt.\n\n\
             If the target mode is the current mode, the call returns \
             {\"noop\": true} and no card is shown.\n\n\
             Use this when you've completed a read-only plan and need write access \
             (request edit), or when you want to propose a change before acting on \
             it (request plan), or when the user has explicitly approved running \
             unattended (request yolo). Do not request yolo unless the task is \
             fully understood and irreversible actions are acceptable."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "target_mode": {
                    "type": "string",
                    "enum": ["edit", "plan", "yolo"],
                    "description": "The mode to request. Must be one of: edit, plan, yolo."
                },
                "reason": {
                    "type": "string",
                    "maxLength": 500,
                    "description": "Optional ≤500-char explanation shown on the card. Be specific about why this mode is needed for the next step."
                }
            },
            "required": ["target_mode"]
        }),
    }
}

// ---------------------------------------------------------------------------
// Validation result
// ---------------------------------------------------------------------------

/// Internal validation outcome. We reuse `Result<_, ValidationError>`
/// rather than a typed struct so the `execute_blocking` call sites
/// can early-return on the error path without pre-allocating
/// pending state.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ValidationError {
    #[error("`target_mode` must be non-empty")]
    EmptyTargetMode,
    #[error("`target_mode` must be one of: edit | plan | yolo (got: {got:?})")]
    UnknownTargetMode { got: String },
    #[error("`reason` exceeds {max} characters (got {got})")]
    ReasonTooLong { max: usize, got: usize },
}

/// Schema validation — does NOT enter the blocking wait. Returns
/// `Err(ValidationError)` for short-circuit failures (the agent
/// loop pushes the error string as `tool_result` content with
/// `is_error: true`).
pub(crate) fn validate(input: &RequestModeChangeInput) -> Result<(), ValidationError> {
    let t = input.target_mode.trim();
    if t.is_empty() {
        return Err(ValidationError::EmptyTargetMode);
    }
    if !VALID_MODES.contains(&t) {
        return Err(ValidationError::UnknownTargetMode {
            got: input.target_mode.clone(),
        });
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

/// Long-running execution. Pipeline (mirrors `ask_user_question`):
///
/// 1. Parse + validate. Bad input → short-circuit
///    `(error, true, _, None)`. Does NOT touch the store or
///    emit anything.
/// 2. **Noop check**: if `target_mode == current_mode` →
///    write `mode_change_requested{noop: true}` audit and
///    return `(json noop, false, _, None)` WITHOUT registering
///    or emitting (no card; PRD R7).
/// 3. Write `mode_change_requested` audit (always).
/// 4. `QuestionStore::register(session_id, tool_use_id,
///    PendingInteraction::ModeChange(payload))` — `AlreadyPending`
///    race returns a structured error string; the first pending
///    (whatever kind) stays untouched.
/// 5. `sink.emit_mode_change_request(&payload)` — fires the
///    `mode:change:request` Tauri event so the frontend renders
///    the `<RequestModeChangeCard>`.
/// 6. `tokio::select! { biased; cancel | oneshot }`:
///    - cancel arm → `store.remove()`, returns
///      `({"cancelled_by_session": true}, true, _, None)`.
///    - oneshot arm — `InteractionResponse` matched:
///      - `Answered(true)` → return `({"allowed": true,
///        "prev_mode": "...", "new_mode": "..."}, false, _, None)`.
///      - `Cancelled` → return `({"cancelled_by_user": true},
///        true, _, None)`.
///      - `Err(RecvError)` (sender dropped by the cancel arm's
///        `store.remove`) → return `({"cancelled_by_session":
///        true}, true, _, None)`.
///
/// **No retries, no timeout** — v1 keeps it simple (PRD §"Notes").
/// A user who neither allows nor denies and also doesn't Stop
/// is just stuck on a card; acceptable because the agent's
/// tokio task is blocked anyway (no LLM work happening).
///
/// `current_mode` is read off the chat_loop's
/// `session_mode` snapshot at the time the call site invokes
/// this function. The agent loop's `loaded_session.session.mode`
/// is the authoritative source for the noop check; the function
/// itself is purely a "request → wait → return" carrier (no DB
/// write — see module docstring).
#[allow(clippy::too_many_arguments)]
pub async fn execute_blocking(
    input: &serde_json::Value,
    session_id: &str,
    tool_use_id: &str,
    current_mode: db::Mode,
    db_pool: &sqlx::SqlitePool,
    store: &QuestionStore,
    sink: &Arc<dyn ChatEventSink>,
    cancel: &CancellationToken,
    // E2 (2026-07-14): per-turn seq for audit turn alignment. The
    // caller (chat_loop interceptor) passes `Some(seq)` from inside
    // the turn loop so the `mode_change_requested` audit row lands
    // in the correct turn group.
    turn_seq: Option<i64>,
) -> BlockingToolResult {
    // ---- 1. Parse + validate ----------------------------------------
    let parsed: RequestModeChangeInput = match serde_json::from_value(input.clone()) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("request_mode_change: invalid input JSON: {}", e);
            tracing::warn!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                error = %e,
                "request_mode_change: short-circuit on parse error"
            );
            return (msg, true, crate::tools::ToolContextUpdate::default(), None);
        }
    };
    if let Err(e) = validate(&parsed) {
        let msg = format!("request_mode_change: schema validation failed: {}", e);
        tracing::warn!(
            session_id = %session_id,
            tool_use_id = %tool_use_id,
            error = %e,
            "request_mode_change: short-circuit on schema validation"
        );
        return (msg, true, crate::tools::ToolContextUpdate::default(), None);
    }

    let target_mode = parsed.target_mode.trim().to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    // ---- 2. Noop check (PRD R7) --------------------------------------
    // `current_mode.as_str()` returns lowercase; `target_mode` is
    // validated to be one of the lowercase strings. Direct ==
    // is the cheap path (no `from_str_opt` round-trip).
    if target_mode == current_mode.as_str() {
        // Audit: requested with noop marker (PRD AC12 — every
        // tool call gets a `mode_change_requested` row).
        let payload = serde_json::json!({
            "target_mode": target_mode,
            "noop": true,
        })
        .to_string();
        if let Err(e) = db::record_audit_event(
            db_pool,
            session_id,
            crate::agent::permissions::AuditKind::ModeChangeRequested.as_str(),
            Some(&payload),
            turn_seq,
        )
        .await
        {
            tracing::warn!(
                error = %e,
                "request_mode_change: record_audit(mode_change_requested noop) failed"
            );
        }
        let content = serde_json::json!({
            "noop": true,
            "current_mode": target_mode,
        })
        .to_string();
        return (
            content,
            false,
            crate::tools::ToolContextUpdate::default(),
            None,
        );
    }

    // ---- 3. Build payload + requested audit ------------------------
    let payload = ModeChangePayload {
        session_id: session_id.to_string(),
        tool_use_id: tool_use_id.to_string(),
        target_mode: target_mode.clone(),
        current_mode: Some(current_mode.as_str().to_string()),
        reason: parsed.reason.clone(),
        ts: now,
    };

    // Audit: requested (always). The `reason` field carries
    // the LLM's reason; the audit is the "user-visible record"
    // of the LLM's ask.
    let audit_payload = serde_json::json!({
        "target_mode": target_mode,
        "reason": parsed.reason,
        "noop": false,
    })
    .to_string();
    if let Err(e) = db::record_audit_event(
        db_pool,
        session_id,
        crate::agent::permissions::AuditKind::ModeChangeRequested.as_str(),
        Some(&audit_payload),
        turn_seq,
    )
    .await
    {
        tracing::warn!(
            error = %e,
            "request_mode_change: record_audit(mode_change_requested) failed"
        );
    }

    // ---- 4. Register + emit -----------------------------------------
    let rx = match store
        .register(
            session_id,
            tool_use_id,
            PendingInteraction::ModeChange(payload.clone()),
        )
        .await
    {
        Ok(rx) => rx,
        Err(crate::agent::question_store::QuestionStoreError::AlreadyPending) => {
            // Same single-pending gate as ask_user_question
            // (a pending question or pending mode change blocks
            // both; the gate is across BOTH kinds per design
            // §3.3).
            let msg = "已有 pending interaction,等当前处理完成".to_string();
            tracing::warn!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                "request_mode_change: AlreadyPending — concurrent register"
            );
            return (msg, true, crate::tools::ToolContextUpdate::default(), None);
        }
        Err(e) => {
            // `NotFound` is not reachable from `register` —
            // defensive branch (register only ever returns
            // `AlreadyPending`). Kept for exhaustiveness.
            let msg = format!("request_mode_change: store error: {}", e);
            tracing::error!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                error = %e,
                "request_mode_change: unexpected register error"
            );
            return (msg, true, crate::tools::ToolContextUpdate::default(), None);
        }
    };
    // Emit AFTER register — the frontend's
    // `get_pending_interaction` fallback (used on session
    // reload) needs the entry to exist when the event arrives.
    // Symmetric with the ask_user_question flow.
    sink.emit_mode_change_request(&payload);

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
                "request_mode_change: cancelled by session token"
            );
            let content = serde_json::json!({"cancelled_by_session": true})
                .to_string();
            (content, true, crate::tools::ToolContextUpdate::default(), None)
        }
        resp = rx => {
            match resp {
                Ok(InteractionResponse::Answered(_)) => {
                    // Allowed path. The actual mode application
                    // happened in the `resolve_mode_change` IPC
                    // handler (single source of truth for mode
                    // change side effects, per design §5.5
                    // "Yolo 二次守门的一致性"). We only return
                    // the result; the new mode is now in the
                    // DB + the next LLM turn's system prompt
                    // reflects it.
                    let content = serde_json::json!({
                        "allowed": true,
                        "prev_mode": current_mode.as_str(),
                        "new_mode": target_mode,
                    })
                    .to_string();
                    (content, false, crate::tools::ToolContextUpdate::default(), None)
                }
                Ok(InteractionResponse::Cancelled) => {
                    // User clicked 拒绝 / Yolo 二次 modal 取消
                    // / Yolo root guard 触发拒绝。三种场景
                    // 统一在 audit 区分 (mode_change_denied +
                    // `reason` 字段)。
                    let content = serde_json::json!({
                        "cancelled_by_user": true,
                        "target_mode": target_mode,
                    })
                    .to_string();
                    (content, true, crate::tools::ToolContextUpdate::default(), None)
                }
                Err(_recv_err) => {
                    // Sender dropped (e.g. resolve ran on a
                    // stale session id after cancel arm cleaned
                    // the entry). Treat as session-cancelled —
                    // safe default per the permission-store
                    // parity (also uses Deny on RecvError).
                    let content = serde_json::json!({"cancelled_by_session": true})
                        .to_string();
                    (content, true, crate::tools::ToolContextUpdate::default(), None)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::question_store::{
        InteractionResponse, PendingInteraction, Question, QuestionOption, QuestionStore,
        ToolQuestionPayload,
    };
    use sqlx::SqlitePool;

    // ----- helpers for tests -----

    /// A `dyn ChatEventSink` stub that captures the latest
    /// emit so tests can assert "did we publish to IPC?".
    /// Mirrors the test infrastructure pattern used in
    /// `tools/ask_user_question.rs::tests::CapturingSink`.
    #[derive(Default)]
    struct CapturingSink {
        emitted_mode_change: std::sync::Mutex<Vec<ModeChangePayload>>,
        emitted_question: std::sync::Mutex<Vec<ToolQuestionPayload>>,
    }

    impl ChatEventSink for CapturingSink {
        fn emit_chat_event(&self, _payload: &crate::state::ChatEventPayload) {}
        fn emit_tool_call(&self, _payload: &crate::state::ToolCallPayload) {}
        fn emit_tool_result(&self, _payload: &crate::state::ToolResultPayload) {}
        fn emit_permission_ask(&self, _payload: crate::agent::permissions::PermissionAskPayload) {}
        fn emit_tool_question(&self, payload: &ToolQuestionPayload) {
            self.emitted_question.lock().unwrap().push(payload.clone());
        }
        fn emit_mode_change_request(&self, payload: &ModeChangePayload) {
            self.emitted_mode_change
                .lock()
                .unwrap()
                .push(payload.clone());
        }
    }

    fn make_sink() -> Arc<CapturingSink> {
        Arc::new(CapturingSink::default())
    }

    async fn fresh_db() -> SqlitePool {
        // Reuse the test pool helper from the broader test
        // module. (In-process SQLite + run_migrations keeps
        // the audit table present.)
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        crate::db::migrations::run_migrations(&pool).await.unwrap();
        pool
    }

    async fn seed_session(pool: &SqlitePool, session_id: &str) {
        // Create a project + session so the audit
        // `record_audit_event` calls don't fail with FK
        // violations (the audit table's `session_id` is
        // a soft-FK to `sessions.id` in the sense that the
        // `record_audit_event` SQL doesn't enforce a hard FK,
        // but the test still needs the row to exist for the
        // `assert_eq!(count, ...)` query below to work
        // without spuriously failing).
        let dir = tempfile::tempdir().expect("tempdir");
        let project_path = dir.path().to_path_buf();
        let project_id = format!("proj-{}", session_id);
        crate::db::create_project(
            pool,
            &project_id,
            project_path.to_str().unwrap(),
            false,
            None,
        )
        .await
        .expect("create_project");
        crate::db::create_session(
            pool,
            session_id,
            &project_id,
            project_path.to_str().unwrap(),
            "mock-model",
            None,
        )
        .await
        .expect("create_session");
    }

    fn make_valid_input(target: &str) -> serde_json::Value {
        serde_json::json!({
            "target_mode": target,
            "reason": "need to write code",
        })
    }

    // ----- validation short-circuits -----

    #[tokio::test]
    async fn validation_empty_target_mode_short_circuits() {
        let pool = fresh_db().await;
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = serde_json::json!({"target_mode": "  "});
        let cancel = CancellationToken::new();
        let (content, is_error, _, _) = execute_blocking(
            &input,
            "s1",
            "tu_1",
            db::Mode::Plan,
            &pool,
            &store,
            &(sink.clone() as Arc<dyn ChatEventSink>),
            &cancel,
            None,
        )
        .await;
        assert!(is_error, "empty target → is_error: true");
        assert!(content.contains("schema validation failed"));
        assert!(
            sink.emitted_mode_change.lock().unwrap().is_empty(),
            "no IPC emit on validation failure"
        );
        assert!(store.get_payload("s1").await.is_none());
    }

    #[tokio::test]
    async fn validation_target_mode_out_of_enum_short_circuits() {
        let pool = fresh_db().await;
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = serde_json::json!({"target_mode": "background"}); // reserved, not exposed
        let cancel = CancellationToken::new();
        let (content, is_error, _, _) = execute_blocking(
            &input,
            "s1",
            "tu_1",
            db::Mode::Plan,
            &pool,
            &store,
            &(sink.clone() as Arc<dyn ChatEventSink>),
            &cancel,
            None,
        )
        .await;
        assert!(is_error);
        assert!(content.contains("schema validation failed"));
        assert!(sink.emitted_mode_change.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn validation_reason_too_long_short_circuits() {
        let pool = fresh_db().await;
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = serde_json::json!({
            "target_mode": "edit",
            "reason": "x".repeat(501), // 501 > 500
        });
        let cancel = CancellationToken::new();
        let (content, is_error, _, _) = execute_blocking(
            &input,
            "s1",
            "tu_1",
            db::Mode::Plan,
            &pool,
            &store,
            &(sink.clone() as Arc<dyn ChatEventSink>),
            &cancel,
            None,
        )
        .await;
        assert!(is_error);
        assert!(content.contains("schema validation failed"));
        assert!(sink.emitted_mode_change.lock().unwrap().is_empty());
    }

    // ----- noop path -----

    #[tokio::test]
    async fn noop_target_equals_current_returns_noop_marker() {
        let pool = fresh_db().await;
        seed_session(&pool, "s1").await;
        let store = QuestionStore::new();
        let sink = make_sink();
        // current is Plan, target is Plan → noop
        let input = make_valid_input("plan");
        let cancel = CancellationToken::new();
        let sink_arc: Arc<dyn ChatEventSink> = sink.clone();
        let (content, is_error, _, _) = execute_blocking(
            &input,
            "s1",
            "tu_1",
            db::Mode::Plan,
            &pool,
            &store,
            &sink_arc,
            &cancel,
            None,
        )
        .await;
        assert!(!is_error, "noop returns is_error: false");
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["noop"], serde_json::json!(true));
        assert_eq!(parsed["current_mode"], serde_json::json!("plan"));
        // No IPC emit on noop (no card).
        assert!(sink.emitted_mode_change.lock().unwrap().is_empty());
        // No register either.
        assert!(store.get_payload("s1").await.is_none());
        // Audit row written with noop=true.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM session_audit_events WHERE kind = 'mode_change_requested'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    // ----- happy path + register-before-emit invariant -----

    #[tokio::test]
    async fn happy_path_registers_emits_and_returns_allowed() {
        let pool = fresh_db().await;
        seed_session(&pool, "s1").await;
        let store = QuestionStore::new();
        let sink = make_sink();
        // current is Plan, target is Edit → not a noop
        let input = make_valid_input("edit");
        let cancel = CancellationToken::new();
        let sink_arc: Arc<dyn ChatEventSink> = sink.clone();
        let store_clone = store.clone();
        let input_clone = input.clone();
        let cancel_clone = cancel.clone();
        let pool_clone = pool.clone();
        let exec = tokio::spawn(async move {
            execute_blocking(
                &input_clone,
                "s1",
                "tu_1",
                db::Mode::Plan,
                &pool_clone,
                &store_clone,
                &sink_arc,
                &cancel_clone,
                None,
            )
            .await
        });

        // Wait for the executor to register + emit — poll the
        // store instead of a fixed sleep (robust against CI
        // scheduling jitter; mirrors the integration suite's
        // pattern).
        let register_wait_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while store.get_payload("s1").await.is_none() {
            if std::time::Instant::now() > register_wait_deadline {
                panic!("executor never registered the pending mode change");
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        // emit was called.
        let emitted = sink.emitted_mode_change.lock().unwrap();
        assert_eq!(emitted.len(), 1, "emit_mode_change_request called once");
        assert_eq!(emitted[0].target_mode, "edit");
        assert_eq!(emitted[0].tool_use_id, "tu_1");
        drop(emitted);

        // Resolve with Answered(true) (mirrors the
        // `resolve_mode_change` IPC allow-path outcome).
        store
            .resolve("s1", InteractionResponse::Answered(serde_json::json!(true)))
            .await
            .expect("resolve ok");

        let (content, is_error, _, _) = exec.await.expect("exec ok");
        assert!(!is_error, "allowed returns is_error: false");
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["allowed"], serde_json::json!(true));
        assert_eq!(parsed["prev_mode"], serde_json::json!("plan"));
        assert_eq!(parsed["new_mode"], serde_json::json!("edit"));
    }

    // ----- cancel / cancelled paths -----

    #[tokio::test]
    async fn cancel_arm_returns_session_cancelled_marker() {
        let pool = fresh_db().await;
        seed_session(&pool, "s1").await;
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = make_valid_input("edit");
        let cancel = CancellationToken::new();
        let sink_arc: Arc<dyn ChatEventSink> = sink.clone();
        let store_clone = store.clone();
        let input_clone = input.clone();
        let cancel_clone = cancel.clone();
        let pool_clone = pool.clone();
        let exec = tokio::spawn(async move {
            execute_blocking(
                &input_clone,
                "s1",
                "tu_1",
                db::Mode::Plan,
                &pool_clone,
                &store_clone,
                &sink_arc,
                &cancel_clone,
                None,
            )
            .await
        });

        let register_wait_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while store.get_payload("s1").await.is_none() {
            if std::time::Instant::now() > register_wait_deadline {
                panic!("executor never registered the pending mode change");
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        cancel.cancel();
        let (content, is_error, _, _) = exec.await.expect("exec ok");
        assert!(is_error);
        assert!(content.contains("cancelled_by_session"));
        // Store cleaned by cancel arm.
        assert!(store.get_payload("s1").await.is_none());
    }

    #[tokio::test]
    async fn deny_path_returns_cancelled_user_marker() {
        let pool = fresh_db().await;
        seed_session(&pool, "s1").await;
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = make_valid_input("yolo");
        let cancel = CancellationToken::new();
        let sink_arc: Arc<dyn ChatEventSink> = sink.clone();
        let store_clone = store.clone();
        let input_clone = input.clone();
        let cancel_clone = cancel.clone();
        let pool_clone = pool.clone();
        let exec = tokio::spawn(async move {
            execute_blocking(
                &input_clone,
                "s1",
                "tu_1",
                db::Mode::Plan,
                &pool_clone,
                &store_clone,
                &sink_arc,
                &cancel_clone,
                None,
            )
            .await
        });

        let register_wait_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while store.get_payload("s1").await.is_none() {
            if std::time::Instant::now() > register_wait_deadline {
                panic!("executor never registered the pending mode change");
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // User clicks 拒绝 → Cancelled.
        store
            .resolve("s1", InteractionResponse::Cancelled)
            .await
            .expect("resolve ok");
        let (content, is_error, _, _) = exec.await.expect("exec ok");
        assert!(is_error, "deny returns is_error: true");
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["cancelled_by_user"], serde_json::json!(true));
        assert_eq!(parsed["target_mode"], serde_json::json!("yolo"));
    }

    // ----- AlreadyPending race -----

    #[tokio::test]
    async fn already_pending_returns_structured_error() {
        let pool = fresh_db().await;
        seed_session(&pool, "s1").await;
        let store = QuestionStore::new();
        let sink = make_sink();
        // Pre-register a pending question (any kind blocks the
        // mode change — design §3.3).
        store
            .register(
                "s1",
                "tu_pre",
                PendingInteraction::Question(ToolQuestionPayload {
                    session_id: "s1".into(),
                    tool_use_id: "tu_pre".into(),
                    questions: vec![Question {
                        question: "preexisting".into(),
                        header: None,
                        options: vec![
                            QuestionOption {
                                label: "a".into(),
                                description: None,
                                preview: None,
                            },
                            QuestionOption {
                                label: "b".into(),
                                description: None,
                                preview: None,
                            },
                        ],
                        multi_select: false,
                        allow_custom: false,
                    }],
                    ts: 0,
                }),
            )
            .await
            .expect("pre-register ok");

        let input = make_valid_input("edit");
        let cancel = CancellationToken::new();
        let sink_arc: Arc<dyn ChatEventSink> = sink.clone();
        let (content, is_error, _, _) = execute_blocking(
            &input,
            "s1",
            "tu_2",
            db::Mode::Plan,
            &pool,
            &store,
            &sink_arc,
            &cancel,
            None,
        )
        .await;
        assert!(is_error);
        assert!(content.contains("已有 pending"));
        // No emit happened for the duplicate.
        assert!(sink.emitted_mode_change.lock().unwrap().is_empty());
        // The first pending is still there.
        let got = store
            .get_payload("s1")
            .await
            .expect("pre-existing pending still present");
        assert_eq!(
            got.kind,
            crate::agent::question_store::InteractionKind::Question
        );
        match got.payload {
            PendingInteraction::Question(q) => {
                assert_eq!(q.tool_use_id, "tu_pre");
            }
            _ => panic!("expected Question payload"),
        }

        // Drain for test isolation.
        let _ = store.remove("s1").await;
    }

    // ----- schema validate export pure-fn -----

    #[test]
    fn validate_accepts_well_formed_input() {
        let input = RequestModeChangeInput {
            target_mode: "edit".to_string(),
            reason: Some("ok".to_string()),
        };
        validate(&input).expect("valid input passes");
    }

    #[test]
    fn validate_rejects_empty_target() {
        let input = RequestModeChangeInput {
            target_mode: "".to_string(),
            reason: None,
        };
        let err = validate(&input).expect_err("empty target rejected");
        assert!(matches!(err, ValidationError::EmptyTargetMode));
    }

    #[test]
    fn validate_rejects_unknown_target() {
        let input = RequestModeChangeInput {
            target_mode: "background".to_string(),
            reason: None,
        };
        let err = validate(&input).expect_err("unknown target rejected");
        assert!(matches!(err, ValidationError::UnknownTargetMode { .. }));
    }
}
