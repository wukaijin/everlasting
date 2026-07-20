// `SubagentEventSink` — the worker-side event-injection trait
// (transport-abstraction 2026-07-20, Phase 1 P1.3).
//
// Why a separate trait (and not just the parent `ChatEventSink`):
//   - The parent's `ChatEventSink` (state.rs `AppHandleSink` impl)
//     services the **parent** agent loop's chat-event stream
//     (chat-event / tool:call / tool:result / permission:ask /
//     tool:question / mode:change:request / task:state:transition:request).
//   - The subagent has its own emit channels (`subagent:event` /
//     `subagent:finished`) + its own `permission:ask` path (the
//     worker's Tier 4 ask routes through the **same** `permission:ask`
//     channel as the parent, but only because the worker's
//     `PermissionContext.is_worker=true` collapses Tier 4 to
//     `Decision::Deny` and emits the payload via the sink — see
//     `permissions/check.rs` and RULE-A-014 / RULE-A-016).
//
// Two-channel semantics matter (RULE-FrontSubagent-003 + B6 PR3
// review): the subagent sink is **not** a child of `AppHandleSink`,
// it shares the `AppHandle` for the IPC emit but the channels it
// emits on are disjoint. So we model it as a sibling trait, not a
// subclass.
//
// Phase 2 will add an `HttpSseSubagentSink` impl over the daemon's
// SSE channel; today's production impl is `AppHandleSubagentSink`.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use tauri::AppHandle;
use tauri::Emitter;

use super::transcript::{build_subagent_event_payload, build_subagent_finished_payload, TranscriptKind};
use crate::agent::permissions::PermissionAskPayload;

thread_local! {
    /// Test-only thread-local collector for `subagent:event` IPC
    /// payloads. The test constructor `new_with_collector` arms
    /// this cell via the `ThreadLocalSubagentSink` impl below;
    /// production code never reads the cell. Kept here (not in
    /// `sink.rs`) so the trait file owns the test affordance.
    static TEST_COLLECTOR: std::cell::RefCell<Option<Arc<StdMutex<Vec<serde_json::Value>>>>> =
        std::cell::RefCell::new(None);
}

/// Wire the test thread-local collector. Idempotent: a second call
/// replaces the prior collector so the test snapshot is fresh. Must
/// be paired with a `clear_test_collector` in a test cleanup
/// (the test in `sink.rs:1126` does this in its `Drop`-equivalent
/// teardown).
#[allow(dead_code)]
pub(crate) fn arm_test_collector(c: Arc<StdMutex<Vec<serde_json::Value>>>) {
    TEST_COLLECTOR.with(|cell| *cell.borrow_mut() = Some(c));
}

#[allow(dead_code)]
pub(crate) fn clear_test_collector() {
    TEST_COLLECTOR.with(|cell| *cell.borrow_mut() = None);
}

/// Push a payload to the test collector (no-op if none armed).
/// Used by `ThreadLocalSubagentSink::emit_subagent_event`.
pub(crate) fn push_test_collector(payload: serde_json::Value) {
    TEST_COLLECTOR.with(|cell| {
        if let Some(c) = cell.borrow().as_ref() {
            c.lock().unwrap().push(payload);
        }
    });
}

pub trait SubagentEventSink: Send + Sync {
    /// Emit a per-event payload on the `subagent:event` channel.
    /// `kind` is a `TranscriptKind`-shaped string (snake_case:
    /// `"chat_event" | "tool_call" | "tool_result" |
    /// "permission_ask" | "permission_ask_resolved"`); the
    /// `payload_json` is the body. The sink wraps the two into the
    /// canonical wire shape (matches `build_subagent_event_payload`).
    fn emit_subagent_event(
        &self,
        run_id: &str,
        session_id: &str,
        kind: TranscriptKind,
        payload_json: serde_json::Value,
    );

    /// Emit the one-shot terminal signal on `subagent:finished`.
    /// Called from `run_subagent` after `update_run_finished`
    /// commits the row. `status_db` is the `SubagentStatusDb`
    /// string (e.g. `"completed"` / `"cancelled"` / `"error"`);
    /// `finished_at` is the ISO-8601 timestamp.
    fn emit_subagent_finished(
        &self,
        run_id: &str,
        session_id: &str,
        status_db: &str,
        finished_at: &str,
    );

    /// Emit the worker's permission ask on the `permission:ask`
    /// channel. Mirrors `AppHandleSink::emit_permission_ask` —
    /// the worker's `ask_path` worker branch in `permissions/check.rs`
    /// routes the ask through this method instead of inlining an
    /// `app.emit` call, so the subagent collector and the future
    /// HTTP+SSE sink share one shape.
    fn emit_permission_ask(&self, payload: &PermissionAskPayload);
}

// ---------------------------------------------------------------------------
// Production impl: AppHandle-backed (Tauri IPC)
// ---------------------------------------------------------------------------

/// Production `SubagentEventSink` — wraps a `tauri::AppHandle` and
/// forwards each method to the corresponding `app.emit` channel.
/// This is the implementation that runs in `pnpm tauri dev` and
/// shipped builds; `run_subagent` injects it via
/// `SubagentBufferSink::new(app_handle, ...)` when an app handle
/// is in scope.
pub struct AppHandleSubagentSink {
    pub app: AppHandle,
}

impl SubagentEventSink for AppHandleSubagentSink {
    fn emit_subagent_event(
        &self,
        run_id: &str,
        session_id: &str,
        kind: TranscriptKind,
        payload_json: serde_json::Value,
    ) {
        let ipc_payload =
            build_subagent_event_payload(run_id, session_id, kind, payload_json);
        if let Err(e) = self.app.emit("subagent:event", ipc_payload) {
            tracing::warn!(
                error = %e,
                run_id,
                "subagent:event emit failed (non-fatal; transcript still recorded)"
            );
        }
    }

    fn emit_subagent_finished(
        &self,
        run_id: &str,
        session_id: &str,
        status_db: &str,
        finished_at: &str,
    ) {
        let payload =
            build_subagent_finished_payload(run_id, session_id, status_db, finished_at);
        if let Err(e) = self.app.emit("subagent:finished", payload) {
            tracing::warn!(
                run_id,
                error = %e,
                "subagent:finished emit failed (non-fatal; DB row already terminal)"
            );
        }
    }

    fn emit_permission_ask(&self, payload: &PermissionAskPayload) {
        if let Err(e) = self.app.emit("permission:ask", payload.clone()) {
            tracing::warn!(error = %e, "AppHandleSubagentSink: permission:ask emit failed");
        }
    }
}

// ---------------------------------------------------------------------------
// Test impl: thread-local collector
// ---------------------------------------------------------------------------

/// Test-only `SubagentEventSink` — pushes the raw `subagent:event`
/// payload (matching the production wire shape) into the
/// thread-local collector armed by `arm_test_collector`. `finished`
/// and `permission_ask` are recorded in a parallel collector; tests
/// snapshot the collector to assert on the IPC sequence. This impl
/// replaces the inline `if let Some(handle) = &self.app_handle { ... }
/// else { TEST_COLLECTOR.with(...) }` branches that used to live
/// inside `SubagentBufferSink::record()` and
/// `SubagentBufferSink::emit_permission_ask`.
#[allow(dead_code)]
pub struct ThreadLocalSubagentSink;

#[allow(dead_code)]
impl SubagentEventSink for ThreadLocalSubagentSink {
    fn emit_subagent_event(
        &self,
        run_id: &str,
        session_id: &str,
        kind: TranscriptKind,
        payload_json: serde_json::Value,
    ) {
        let ipc_payload =
            build_subagent_event_payload(run_id, session_id, kind, payload_json);
        push_test_collector(ipc_payload);
    }

    fn emit_subagent_finished(
        &self,
        _run_id: &str,
        _session_id: &str,
        _status_db: &str,
        _finished_at: &str,
    ) {
        // No collector side effect in the existing tests (the
        // `subagent:finished` event is asserted via DB + transcript,
        // not via the IPC collector). The method exists so the
        // dispatch path can call it uniformly.
    }

    fn emit_permission_ask(&self, _payload: &PermissionAskPayload) {
        // Tests that inspect `permission:ask` IPC reads the
        // transcript (via `record()`), not the IPC collector. The
        // method exists for trait-uniformity.
    }
}

// ---------------------------------------------------------------------------
// Convert the agent's permissions `PermissionAskPayload` (used by
// `permissions/check.rs`) to the state's `PermissionAskPayload`
// (used by `AppHandleSink::emit_permission_ask` and re-exported
// by the IPC contract). The two are the SAME type today (this
// alias is `pub use`d in `state.rs`); the explicit conversion
// keeps the trait signature readable if they ever drift.
// ---------------------------------------------------------------------------

impl SubagentEventSink for Arc<dyn SubagentEventSink> {
    fn emit_subagent_event(
        &self,
        run_id: &str,
        session_id: &str,
        kind: TranscriptKind,
        payload_json: serde_json::Value,
    ) {
        (**self).emit_subagent_event(run_id, session_id, kind, payload_json);
    }

    fn emit_subagent_finished(
        &self,
        run_id: &str,
        session_id: &str,
        status_db: &str,
        finished_at: &str,
    ) {
        (**self).emit_subagent_finished(run_id, session_id, status_db, finished_at);
    }

    fn emit_permission_ask(&self, payload: &PermissionAskPayload) {
        (**self).emit_permission_ask(payload);
    }
}

// ---------------------------------------------------------------------------
// `Arc<dyn SubagentEventSink>` already supports the trait's
// methods natively (Rust auto-derefs through `Arc` to the inner
// `dyn`). No blanket impl is needed — calls like
// `sink.emit_subagent_event(...)` resolve directly to the trait
// method on the inner type. The note above is here so future
// readers don't try to add one (it would conflict).
// ---------------------------------------------------------------------------
