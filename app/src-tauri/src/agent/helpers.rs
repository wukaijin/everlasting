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
use std::time::Duration;

use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, oneshot};
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
///
/// Dead code as of 2026-06-15 (RULE-A-006 closure): the agent loop
/// body now lives in `chat_loop::run_chat_loop` and dispatches
/// every chat-event through the `ChatEventSink` trait
/// (`emit_chat_event_via_sink`). Kept here for future
/// helper-callers that may need to emit from a non-`run_chat_loop`
/// context (e.g. a future IPC command that synthesizes a Done
/// event without running the full loop).
#[allow(dead_code)]
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

/// Cancel an in-flight chat request for `session_id`, if any, and
/// return its "agent loop exited" signal. Called at the entry of
/// `delete_session` / `detach_worktree` / `delete_worktree` so a
/// streaming LLM can't write into a half-destroyed session/worktree.
/// No-op when the session isn't streaming. The cancellation is
/// best-effort: the agent loop notices on its next event boundary and
/// bails out cleanly, emitting a `done` event with `stop_reason:
/// "cancelled"`.
///
/// **RULE-E-005 (2026-06-15)**: cancelling the token only *sets* the
/// flag — it does NOT wait for the agent loop to actually exit. The
/// loop checks cancel at stream-event boundaries and *after* a tool
/// executes (`chat_loop.rs:670`), so when cancel fires there can still
/// be one in-flight tool that runs to completion before the loop
/// returns. If the destructive command deletes the worktree in that
/// window, the in-flight write hits a deleted dir (ENOENT / panic /
/// orphaned fingerprint). To close the race we also return the
/// `oneshot::Receiver` that the `chat` spawn closure `.send(())`s when
/// `run_chat_loop` returns; the caller `await`s it (with a timeout,
/// see the call sites) BEFORE the destructive work runs.
///
/// Returns `None` when the session has no in-flight request (the
/// common case) OR when a second destructive op already drained the
/// receiver (single-consumer — concurrent destructive ops on the same
/// session are a degraded case; the second one proceeds without
/// awaiting, which is safe because the first already gated the exit).
///
/// The `cancellations` / `session_active_request` / `inflight_exits`
/// arguments are pulled out of `AppState` (rather than taking
/// `&AppState` directly) so this helper can be `pub` and unit-tested
/// with bare `Arc<Mutex<HashMap<...>>>` values — see
/// `crate::agent::tests::cancel_inflight_for_session_*`.
pub async fn cancel_inflight_for_session(
    cancellations: &Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    session_active_request: &Arc<Mutex<std::collections::HashMap<String, String>>>,
    inflight_exits: &Arc<Mutex<std::collections::HashMap<String, oneshot::Receiver<()>>>>,
    session_id: &str,
) -> Option<oneshot::Receiver<()>> {
    let request_id = {
        let map = session_active_request.lock().await;
        map.get(session_id).cloned()
    };
    let Some(rid) = request_id else {
        return None;
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
    // RULE-E-005: take the exit receiver out of the map (single-
    // consumer) so the caller can `await` it. The `chat` spawn
    // closure `.send(())`s when `run_chat_loop` returns; if the
    // caller drops the receiver without awaiting, or the session
    // wasn't in-flight, this is `None` and the spawn closure still
    // removes the map entry after its own `.send`.
    let exit_rx = {
        let mut map = inflight_exits.lock().await;
        map.remove(&rid)
    };
    // The `session_active_request` + `cancellations` entries are
    // removed by the `CancellationGuard` on Drop, after the agent
    // loop exits. We don't remove them here — the in-flight loop
    // still uses them during cleanup.
    exit_rx
}

/// RULE-E-005 (2026-06-15): await the agent loop's exit signal
/// returned by [`cancel_inflight_for_session`], with a defensive
/// timeout so a hung loop can't block a destructive command
/// forever. `None` (the session had no in-flight request, or a
/// concurrent destructive op already drained the single-consumer
/// receiver) returns immediately.
///
/// Outcomes:
/// - `Ok(Ok(()))` / `Ok(Err(_))` — the loop exited (the `Err` arm
///   is the sender dropping without sending, e.g. the spawn task
///   panicked; still "exited"). Proceed.
/// - `Err(_)` (timeout) — log a warning and proceed anyway. The
///   user explicitly invoked the destructive op, so we never block
///   indefinitely; the 10 s bound comfortably exceeds one tool
///   execution (the worst-case window between cancel and exit).
///
/// `label` is the op name for the timeout warning log (e.g.
/// `"delete_worktree"`) so the warning is attributable.
pub async fn await_inflight_exit(exit_rx: Option<oneshot::Receiver<()>>, label: &str) {
    let Some(rx) = exit_rx else {
        return;
    };
    match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(())) | Ok(Err(_)) => {}
        Err(_) => {
            tracing::warn!(
                op = %label,
                timeout_secs = 10,
                "destructive op: agent loop did not exit within timeout — proceeding anyway"
            );
        }
    }
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

/// RULE-A-007 (2026-06-17): symmetric counterpart to
/// [`CANCELLED_MARKER`] for the **error** path. When the LLM
/// stream emits a `ChatEvent::Error` mid-turn, the agent loop
/// now persists whatever partial content accumulated
/// (`text_parts` / `finalized_thinking` / `tool_calls`) —
/// matching the cancel path's behavior — and appends this
/// marker to the text so a reload shows the user where the
/// turn broke. The two markers share the same bracketed-text
/// style so the rehydrate path can render either uniformly.
pub const ERROR_MARKER: &str = "[生成出错中断]";

// ---------------------------------------------------------------------------
// Sink-based emit helpers (P1 RULE-A-006)
//
// The agent loop body in `chat_loop::run_chat_loop` emits on
// three Tauri channels: `chat-event` / `tool:call` / `tool:result`
// / `permission:ask`. The `ChatEventSink` trait abstracts the
// underlying emitter so the loop can run against a `MockEmitter`
// in tests AND against the production `AppHandleSink` in the
// `chat` Tauri command (P1 RULE-A-006 closure, 2026-06-15).
// `emit_chat_event_via_sink` is the per-call-site helper for the
// `chat-event` channel; the `tool:call` / `tool:result` /
// `permission:ask` channels are dispatched through the
// `sink.emit_*` methods directly.
//
// The legacy `AppHandle` variants ([`emit_chat_event`] and
// [`emit_tool_result`]) survive as dead code for future
// helper-callers — paths that pre-date the sink abstraction or
// future IPC commands that synthesize events without running
// the full loop.
// ---------------------------------------------------------------------------

/// Emit a `ChatEvent` on the `chat-event` channel via the
/// supplied `ChatEventSink`. Mirrors [`emit_chat_event`] but
/// dispatches through the trait so the agent loop body is
/// testable without a Tauri `AppHandle`.
///
/// Used by the production agent loop body in
/// [`crate::agent::chat_loop::run_chat_loop`] (P1 RULE-A-006
/// closure, 2026-06-15). Every chat-event emit on the agent
/// loop's per-event select! arm goes through this helper.
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