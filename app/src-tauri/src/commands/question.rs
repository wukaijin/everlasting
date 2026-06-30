//! ask_user_question Tauri command surface (2026-06-30).
//!
//! Two thin commands back the frontend's `<AskUserQuestionCard>`:
//!
//! - [`resolve_tool_question`] — frontend invokes with the user's
//!   answer (or `cancelled: true` on `跳过`); we
//!   forward to `QuestionStore::resolve` which sends the
//!   oneshot → the agent loop's `tokio::select!` returns.
//!   The shape of the result unwraps into the 2-way
//!   `QuestionResponse::Answered | Cancelled` enum (session
//!   cancel is handled by `execute_blocking`'s cancel arm
//!   directly, never via this IPC path).
//! - [`get_pending_question`] — frontend invokes on session
//!   switch / `rehydrateMessages` to recover the live pending
//!   question (so a switched-back session renders the still-
//!   unanswered card). Returns `Option<ToolQuestionPayload>` —
//!   `None` if no pending. The store is the source of truth;
//!   the frontend `pendingBySession` cache is overruled by this
//!   result (`get_pending_question` > cache > empty).

use std::sync::Arc;

use tauri::State;

use crate::agent::question_store::{
    QuestionAnswer, QuestionResponse, ToolQuestionPayload,
};
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
#[tauri::command]
pub async fn resolve_tool_question(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tool_use_id: String,
    answer: Option<Vec<QuestionAnswer>>,
    cancelled: Option<bool>,
) -> Result<(), String> {
    // Accepted for routing parity with the wire shape; the
    // store keys on session_id alone (single-pending).
    let _ = tool_use_id;
    let response = resolve_response_from_args(cancelled, answer);
    state
        .question_store
        .resolve(&session_id, response)
        .await
        .map_err(|e| e.to_string())
}

/// Map the scalar IPC args to a `QuestionResponse`. Pure
/// function extracted from `resolve_tool_question` so the
/// `cancelled`-vs-`answer` branch is unit-testable without a
/// Tauri `mock_app` (which this project doesn't use — see the
/// "Why scalar args" note on `resolve_tool_question` for why
/// the invoke serde boundary itself is covered by the
/// `permission_response` precedent + manual `tauri dev`
/// verification, not a unit test).
pub(crate) fn resolve_response_from_args(
    cancelled: Option<bool>,
    answer: Option<Vec<QuestionAnswer>>,
) -> QuestionResponse {
    if cancelled.unwrap_or(false) {
        QuestionResponse::Cancelled
    } else {
        QuestionResponse::Answered(answer.unwrap_or_default())
    }
}

/// Read-only frontend hook for session switch + initial load.
/// Returns the `ToolQuestionPayload` for the session if a
/// question is still pending (so the frontend can re-inject the
/// card on a session the user switched back to), or `None` if
/// no question is pending (so the frontend doesn't render a
/// card for an already-resolved or never-pending session).
///
/// The store is the source of truth — `Option<None>` here
/// means "the question was resolved (or never existed)". The
/// `pendingBySession` Pinia cache is a memoization layer that
/// gets corrected (via this command) on every session switch.
#[tauri::command]
pub async fn get_pending_question(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<ToolQuestionPayload>, String> {
    Ok(state.question_store.get_payload(&session_id).await)
}
