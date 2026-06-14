//! Shared helpers used by the agent loop and the worktree
//! commands.
//!
//! - [`tool_result_envelope`] — REQ-16: wrap tool result content
//!   in a JSON envelope that also carries the worktree's current
//!   `cwd` so the LLM can correlate results with worktree state.
//! - [`build_synthetic_tool_result_message`] — BUG FIX (2013
//!   tool_use orphan): on cancel, persist a synthetic
//!   `user(tool_result)` block per emitted `tool_use` to keep the
//!   history self-consistent.
//! - [`persist_turn_cwd`] — write the agent's final cwd at turn
//!   end (not per shell call).
//! - [`emit_chat_event`] — thin wrapper around `AppHandle::emit`
//!   for the `chat-event` channel.
//! - [`cancel_inflight_for_session`] — destructive-op helper used
//!   by `delete_session`, `detach_worktree`, `delete_worktree`.

use std::sync::Arc;

use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::llm::{ChatEvent, ChatMessage, ContentBlock, MessageContent, Role};
use crate::state::{ChatEventPayload, ChatEventSink, ToolResultPayload};

// ---------------------------------------------------------------------------
// Tool result envelope (REQ-16)
// ---------------------------------------------------------------------------

/// Step 4 follow-up (REQ-16): wrap a tool result `content` string
/// in a JSON envelope that also carries the worktree's current
/// `cwd`. The LLM uses the `cwd` field to understand "this file
/// was at this path on disk when the tool ran" — important after
/// worktree transitions (attach/detach), when the agent's mental
/// model of the worktree can drift from the actual on-disk state.
/// The `result` field holds the legacy content string so
/// downstream tool-specific prompts continue to work without
/// re-parsing.
///
/// Extracted as a free function (not inlined in the chat loop)
/// so it can be unit-tested for the round-trip shape — see
/// `crate::agent::tests::tool_result_envelope_round_trip`. The
/// frontend has a matching lenient parser in `extractToolResultDisplay`
/// (`app/src/utils/messageFormat.ts`).
pub fn tool_result_envelope(content: &str, worktree_path: &std::path::Path) -> String {
    let cwd_str = worktree_path.to_string_lossy().to_string();
    serde_json::json!({
        "result": content,
        "cwd": cwd_str,
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Synthetic tool_result on cancel (BUG FIX 2013)
// ---------------------------------------------------------------------------

/// BUG FIX (2013 tool_use orphan): build a synthetic `user`-role
/// [`ChatMessage`] carrying one `ContentBlock::ToolResult` per
/// `(id, name, _input)` triple the LLM emitted before the user
/// cancelled. The block's `content` tells the LLM the tool never
/// ran; `is_error: true` makes the failure explicit on the wire.
///
/// Why a helper: the inline shape is verbose, and the invariant
/// we care about (one ToolResult block per tool_use, with matching
/// `tool_use_id`, is_error=true, role=User) is what unblocks the
/// Anthropic 2013 error on the next `send()`. Extracting it lets
/// a unit test assert the invariant end-to-end without spinning
/// up an LLM stream, Tauri AppHandle, or real DB. See
/// `crate::agent::tests::synthetic_tool_result_message_*`.
///
/// The `Role` is `User` (not `Tool`) per the Anthropic Messages
/// API contract: `tool_result` blocks only ever appear inside
/// `role: "user"` messages.
pub fn build_synthetic_tool_result_message(
    tool_calls: &[(String, String, serde_json::Value)],
) -> ChatMessage {
    let blocks: Vec<ContentBlock> = tool_calls
        .iter()
        .map(|(id, name, _input)| {
            let content = format!(
                "Tool execution was interrupted: the user stopped the request or the \
session was cancelled before the tool could run. The tool {} did not run.",
                name
            );
            ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content,
                is_error: true,
            }
        })
        .collect();
    ChatMessage {
        role: Role::User,
        content: MessageContent::Blocks(blocks),
    }
}

// ---------------------------------------------------------------------------
// CWD persistence
// ---------------------------------------------------------------------------

/// Persist the final cwd of a turn. Called once at turn end (not
/// after every shell call). We compare against the DB-stored value
/// to avoid a no-op write when the agent stayed put.
///
/// `last_cwd` is the latest validated canonical path reported by
/// the shell tool's `ToolContextUpdate`. We store the path as a
/// string — the next turn's `assert_within_root` call will
/// canonicalize it again on read, so the DB stays
/// canonical-encoding-agnostic.
pub async fn persist_turn_cwd(
    db: &SqlitePool,
    session_id: &str,
    last_cwd: Option<&std::path::Path>,
) {
    let Some(new_cwd) = last_cwd else {
        return;
    };
    let new_cwd_str = new_cwd.to_string_lossy().into_owned();
    // Cheap "did it change?" guard. We compare against the
    // just-loaded session rather than re-querying.
    if let Ok(Some(loaded)) = db::load_session(db, session_id).await {
        if loaded.session.current_cwd == new_cwd_str {
            return;
        }
    }
    if let Err(e) = db::update_session_cwd(db, session_id, &new_cwd_str).await {
        tracing::warn!(error = %e, "failed to persist turn cwd");
    }
}

// ---------------------------------------------------------------------------
// Event emission
// ---------------------------------------------------------------------------

/// Emit a `ChatEvent` on the `chat-event` channel. The frontend's
/// Pinia store listens for this channel and updates its
/// `messages[]` + `sending` state. Failures are logged but not
/// surfaced to the caller — the agent loop is best-effort about
/// streaming; if the AppHandle is gone we can't do anything useful.
pub fn emit_chat_event(app: &AppHandle, rid: &str, event: &ChatEvent) {
    let payload = ChatEventPayload {
        request_id: rid.to_string(),
        event: event.clone(),
    };
    if let Err(e) = app.emit("chat-event", payload) {
        tracing::warn!(error = %e, "failed to emit chat-event");
    }
}

/// Emit a `ToolResultPayload` on the `tool:result` channel. The
/// frontend renders this as the result card under the tool call.
/// Failures are logged but not surfaced (mirrors [`emit_chat_event`]).
#[allow(dead_code)] // kept for future helper-callers
pub fn emit_tool_result(app: &AppHandle, payload: &ToolResultPayload) {
    if let Err(e) = app.emit("tool:result", payload.clone()) {
        tracing::warn!(error = %e, "failed to emit tool:result");
    }
}

// ---------------------------------------------------------------------------
// Destructive-op cancel hook
// ---------------------------------------------------------------------------

/// Cancel an in-flight chat request for `session_id`, if any.
/// Called at the entry of `delete_session` / `detach_worktree` /
/// `delete_worktree` so a streaming LLM can't write into a
/// half-destroyed session/worktree. No-op when the session isn't
/// streaming. The cancellation is best-effort: the agent loop
/// notices on its next event boundary and bails out cleanly,
/// emitting a `done` event with `stop_reason: "cancelled"`.
///
/// The `cancellations` and `session_active_request` arguments are
/// pulled out of `AppState` (rather than taking `&AppState`
/// directly) so this helper can be `pub` and unit-tested with
/// bare `Arc<Mutex<HashMap<...>>>` values — see
/// `crate::agent::tests::cancel_inflight_for_session_*`.
pub async fn cancel_inflight_for_session(
    cancellations: &Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    session_active_request: &Arc<Mutex<std::collections::HashMap<String, String>>>,
    session_id: &str,
) {
    let request_id = {
        let map = session_active_request.lock().await;
        map.get(session_id).cloned()
    };
    let Some(rid) = request_id else {
        return;
    };
    let token = {
        let map = cancellations.lock().await;
        map.get(&rid).cloned()
    };
    if let Some(t) = token {
        t.cancel();
        tracing::info!(
            session_id = %session_id,
            request_id = %rid,
            "destructive op: cancelled in-flight chat"
        );
    }
    // The `session_active_request` entry is removed by the
    // `CancellationGuard` on Drop, after the agent loop exits.
    // We don't remove it here — the in-flight loop still uses it
    // during cleanup.
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Sentinel string appended to the assistant message's text on
/// cancel. The UI (rehydrate path) leaves the marker in place; the
/// bubble just renders it inline. A literal "🛑" was considered
/// but it would be inlined as part of markdown; the bracketed text
/// survives DOMPurify unchanged and is locale-friendly.
pub const CANCELLED_MARKER: &str = "[已停止]";

// ---------------------------------------------------------------------------
// Sink-based emit helpers (P1 RULE-A-006)
//
// The agent loop's `chat` Tauri command emits on three Tauri
// channels: `chat-event` / `tool:call` / `tool:result`. The
// `ChatEventSink` trait abstracts the underlying emitter so the
// loop can run against a `MockEmitter` in tests. These helpers
// take a `&dyn ChatEventSink` and are the per-call sites'
// replacement for the legacy `AppHandle` variants
// ([`emit_chat_event`] and [`emit_tool_result`]) — which the
// Tauri command still uses for paths that pre-date the sink
// abstraction (the pre-flight error emits at the top of `chat`).
// ---------------------------------------------------------------------------

/// Emit a `ChatEvent` on the `chat-event` channel via the
/// supplied `ChatEventSink`. Mirrors [`emit_chat_event`] but
/// dispatches through the trait so the agent loop body is
/// testable without a Tauri `AppHandle`.
///
/// Currently called only from `chat_loop::run_chat_loop` (which
/// is itself test-gated per P1 RULE-A-006). The non-test build
/// treats it as dead code; the `#[allow(dead_code)]` keeps it
/// available without forcing a `#[cfg(test)]` re-gate at every
/// call site.
#[allow(dead_code)]
pub fn emit_chat_event_via_sink(
    sink: &Arc<dyn ChatEventSink>,
    rid: &str,
    event: &ChatEvent,
) {
    let payload = ChatEventPayload {
        request_id: rid.to_string(),
        event: event.clone(),
    };
    sink.emit_chat_event(&payload);
}