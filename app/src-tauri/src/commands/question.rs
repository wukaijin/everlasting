//! ask_user_question Tauri command surface (2026-06-30).
//!
//! Two thin commands back the frontend's `<AskUserQuestionCard>`:
//!
//! - [`resolve_tool_question`] — frontend invokes with the user's
//!   answer (or `cancelled: true` on `跳过`); we
//!   forward to `QuestionStore::resolve` which sends the
//!   oneshot → the agent loop's `tokio::select!` returns.
//!   The shape of the result unwraps into the existing 3-way
//!   `QuestionResponse::Answered | Cancelled | SessionCancelled`
//!   enum (the third arm is reached only via `token.cancelled()`
//!   inside `ask_user_question::execute_blocking`, never via
//!   this IPC path).
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
    QuestionResponse, ToolQuestionPayload, ToolQuestionResolvePayload,
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
/// # Errors
///
/// Returns `Err(String)` for:
/// - `NotFound` — no pending question for `session_id` (race:
///   the session-cancel arm already cleared the entry, or the
///   user clicked 跳过 on a card that's already been resolved
///   by another path). Frontend renders this as a no-op (no
///   card visible).
/// - `AlreadyResolved` — defensive (a double-resolve from a
///   stale IPC). Same UX as NotFound (no visible card).
///
/// The frontend should treat both as success (the card is
/// either gone or never was visible).
#[tauri::command]
pub async fn resolve_tool_question(
    state: State<'_, Arc<AppState>>,
    payload: ToolQuestionResolvePayload,
) -> Result<(), String> {
    let response = if payload.cancelled {
        QuestionResponse::Cancelled
    } else {
        QuestionResponse::Answered(payload.answer)
    };
    state
        .question_store
        .resolve(&payload.session_id, response)
        .await
        .map_err(|e| e.to_string())
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
