//! `SubagentBufferSink` — the worker-side `ChatEventSink` that
//! records every worker emit into an in-memory transcript + tracks
//! the worker's final assistant text (the summary).
//!
//! Extracted from `subagent.rs` (split 2026-06-23). The sink does
//! NOT forward to the parent sink (worker isolation); it also fires
//! the `subagent:event` / `permission:ask` Tauri channels on every
//! emit so the frontend `<SubagentDrawer>` can stream the worker
//! live.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use super::transcript::{TranscriptEntry, TranscriptKind};
use crate::llm::types::TokenUsage;

pub(crate) mod events;

#[allow(unused_imports)]
pub(crate) use events::*;

// Test-only thread-local collector for `subagent:event` IPC
// payloads. The test constructor `SubagentBufferSink::new_with_collector`
// arms this cell; `record()` forwards the IPC payload here when
// no `app_handle` is wired. Production code never reads the
// cell (the cell is always `None`). The
// `Arc<StdMutex<Vec>>` lets the test snapshot the collected
// payloads after the run.
//
// The thread-local is declared at module scope (not under
// `#[cfg(test)]`) because `record()` consults it from the
// production code path — without the declaration, a non-test
// binary that constructs a sink with `app_handle = None` (which
// the codebase never does in production, but the compiler still
// has to verify the code path) would fail to compile. The cell
// stays `None` for the entire production lifetime; only test
// code arms it.
thread_local! {
    pub(crate) static TEST_COLLECTOR: RefCell<Option<Arc<StdMutex<Vec<serde_json::Value>>>>> =
        const { RefCell::new(None) };
}

/// `ChatEventSink` impl that records every worker emit into an
/// in-memory `Vec<TranscriptEntry>` + tracks the worker's final
/// assistant text (the summary).
///
/// The sink does **NOT** forward to the parent sink — doing so
/// would flood the parent's frontend with worker stream events
/// (Claude Code convention: the worker is isolated from the main
/// UI; only the final summary returns as a tool_result). The
/// parent's frontend sees `dispatch_subagent` as a single opaque
/// tool_use/tool_result pair; the worker's transcript is
/// retrievable separately (PR2: `subagent_runs.transcript`;
/// PR3: ToolCallCard expand UI).
///
/// **PR2 hotfix (B6 PR3, 2026-06-20)**: each emit ALSO fires the
/// `subagent:event` Tauri event on the parent `AppHandle`, so the
/// frontend `<SubagentDrawer>` (PR3b) can stream the worker's
/// transcript live (debounced 200ms in the frontend store) without
/// waiting for the worker to finish. The `app_handle` is `None` in
/// tests where no Tauri runtime is present — the emit becomes a
/// no-op and the transcript-only path still works (test coverage
/// of `transcript_snapshot` is unchanged).
pub struct SubagentBufferSink {
    pub(crate) transcript: StdMutex<Vec<TranscriptEntry>>,
    /// Accumulated assistant text deltas. Read by `run_subagent`
    /// after `run_chat_loop` returns to extract the worker's
    /// final summary.
    pub(crate) text_parts: StdMutex<Vec<String>>,
    /// Per-turn `TokenUsage` accumulated from `ChatEvent::Done { usage: Some(t) }`
    /// events. Read by `run_subagent` after the worker loop returns
    /// to populate `subagent_runs.token_usage_json`.
    ///
    /// **Worker isolation (2026-06-26 reversal of RULE-A-015/PR2a)**:
    /// the parent session's `last_*` snapshot columns are NOT
    /// updated by the worker. PR2a pulled `add_token_usage` OUT of
    /// the `!skip_persist` gate so the worker's per-turn usage
    /// streamed into the parent's cumulative total — the 2026-06-26
    /// snapshot fix reverses this: `update_last_turn_usage` is BACK
    /// inside the gate, so the worker's per-turn usage stays
    /// isolated in this `per_turn_usage` buffer + the eventual
    /// `subagent_runs.token_usage_json` write (worker token usage
    /// visible to the parent only via `<SubagentDrawer>`, not via
    /// the parent's ChatInput hint).
    ///
    /// `drain_per_turn_usage` (the legacy streaming-fold surface)
    /// is retained as `#[allow(dead_code)]` for the future
    /// worker↔parent session identity split (see the helper's own
    /// doc). It has no production callsite.
    pub(crate) per_turn_usage: StdMutex<Vec<TokenUsage>>,
    /// Set when the worker emitted a terminal `Error` event.
    /// `run_subagent` reads this to pick the `status: error`
    /// prefix.
    pub(crate) had_error: std::sync::atomic::AtomicBool,
    /// Set when the worker emitted a terminal `Done{cancelled}`
    /// event (stop_reason == "cancelled"). `run_subagent` reads
    /// this to pick the `status: cancelled` prefix.
    pub(crate) was_cancelled: std::sync::atomic::AtomicBool,
    /// 2026-06-21 (R2): set when the worker emitted a synthetic
    /// terminal `Done{max_turns}` event. `run_subagent` reads
    /// this to pick the `status: incomplete` prefix (vs.
    /// `Completed` for the natural end_turn exit). Mutually
    /// exclusive with `was_cancelled` and `had_error` in
    /// practice — the agent loop's `max_turns` branch fires
    /// when the worker exhausts its turn budget, which is not
    /// a cancel or an error path.
    pub(crate) was_incomplete: std::sync::atomic::AtomicBool,
    /// C2+ (2026-07-05, task `07-05-c2-loop-active-intervention`
    /// PR3): set when the worker emitted a terminal
    /// `Done { stop_reason: "loop_terminated" }` event. C2+
    /// triggers after `loop_hit_count >= 3` consecutive loop-
    /// detection verdicts; the worker path takes a direct
    /// short-circuit (no `QuestionStore` round-trip, no audit
    /// row) and emits the `loop_terminated` stop_reason. The
    /// `run_subagent` caller reads this to:
    ///   1. pick the `status: incomplete` prefix (the worker
    ///      didn't cleanly finish — it was force-stopped by
    ///      the harness mid-loop),
    ///   2. append the
    ///      `[loop terminated: worker 因循环重复操作被自动终止]`
    ///      line to the `dispatch_result` content so the parent
    ///      LLM sees the loop-termination signal and can decide
    ///      whether to retry / change strategy / accept.
    /// Mutually exclusive with `was_cancelled`, `had_error`, and
    /// `was_incomplete` in practice — the worker's C2+ branch
    /// (`chat_loop.rs`) emits exactly one terminal `Done` event.
    pub(crate) was_loop_terminated: std::sync::atomic::AtomicBool,
    /// 2026-06-22 (RULE-FrontSubagent-004): count of REAL per-turn
    /// `Done` events the worker received. Incremented once per
    /// completed LLM turn iteration (the natural per-turn Done
    /// carrying that turn's `usage`). Synthetic terminals
    /// (`cancelled` / `max_turns`) do NOT increment — the counter
    /// always reflects the actual turn count at worker exit, even
    /// when the exit was triggered by the soft-cap or cancel. Read
    /// by `run_subagent` after the worker loop returns to populate
    /// `subagent_runs.turn_count` via `update_run_finished`.
    /// Matches the `per_turn_usage` push guard (the same real-Done
    /// discriminator) so the two stay 1:1: turns_completed.len() ==
    /// per_turn_usage.len() at exit.
    pub(crate) turns_completed: std::sync::atomic::AtomicU64,
    /// transport-abstraction 2026-07-20 (P1.3): the worker's
    /// event-injection sink. Replaces the `Option<AppHandle>` +
    /// inline `app.emit` + `TEST_COLLECTOR` branches that used to
    /// live in `record()` and `emit_permission_ask`. Production
    /// injects `Arc::new(AppHandleSubagentSink { app: app_handle })`;
    /// tests inject `Arc::new(ThreadLocalSubagentSink)`. The
    /// `Option<tauri::AppHandle>` field is kept below ONLY because
    /// the existing test constructor `new_with_collector` (which
    /// routes through a thread-local cell) was easier to thread
    /// through this way; new code should use
    /// `new_with_event_sink` instead.
    pub(crate) event_sink: Arc<dyn super::SubagentEventSink>,
    /// PR2 hotfix (B6 PR3, 2026-06-20): kept for the
    /// `new_with_collector` test constructor (which arms a
    /// thread-local collector; the collector path predates the
    /// `SubagentEventSink` trait). Production constructors
    /// (`new` / `new_without_app_handle`) leave this as `None`.
    /// The field is no longer read by `record()` /
    /// `emit_permission_ask` (those route through the trait) but
    /// stays for `new_with_collector`'s use.
    #[allow(dead_code)]
    pub(crate) app_handle: Option<tauri::AppHandle>,
    /// PR2 hotfix: the worker's `run_id` (the `parent_rid-sub-<seq>`
    /// string `run_subagent` builds at subagent/dispatch.rs). Carried
    /// on the sink so each `subagent:event` payload can identify
    /// which worker run the event belongs to.
    pub(crate) run_id: String,
    /// PR2 hotfix: the parent session_id (worker reuses parent's
    /// session_id). Each `subagent:event` payload includes this so
    /// the frontend can route events to the right session's drawer.
    pub(crate) session_id: String,
    /// B6 PR3 redesign (2026-06-21): per-`tool_use_id` `Instant` of
    /// the matching `emit_tool_call` arrival, used to measure the
    /// wall-clock gap to the paired `emit_tool_result` so the
    /// `tool_result` payload_json can carry a `duration_ms` field for
    /// the frontend drawer to render per-tool latency. The map is
    /// mutated only on the same thread that calls `record()` (the
    /// `ChatEventSink` impl methods all route through `record()`,
    /// which is `&self` — but since the sink lives for the duration
    /// of a single worker invocation, no cross-thread races occur).
    /// Entries older than the matching result (or unreachable due
    /// to a lost tool_call event) are removed on result arrival;
    /// see `record_tool_result` for the orphan-fallback path.
    pub(crate) tool_call_received_at: StdMutex<HashMap<String, Instant>>,
    /// C1 (07-26-subagent-resume): the worker's final `Vec<ChatMessage>`
    /// snapshot, captured once by `record_worker_messages` on the chat
    /// loop's normal completion path — and, since
    /// 09-01-subagent-network-resume, ALSO on the drive loop's
    /// stream-error exit (the partial turn is pair-safe there; see
    /// `drive.rs`). Read by `run_subagent` after the worker loop
    /// returns to persist into `subagent_runs.messages_json` (via
    /// `update_run_finished`) so a later `dispatch_subagent` can
    /// resume the conversation. Stays empty for cancel exits and
    /// pre-loop setup failures — those never call
    /// `record_worker_messages`, so the snapshot is the empty
    /// default and resume falls back to fresh dispatch (design §5).
    pub(crate) worker_messages: StdMutex<Vec<crate::llm::types::ChatMessage>>,
}

impl SubagentBufferSink {
    /// Construct a sink without Tauri IPC (test path). The emit
    /// side becomes a silent no-op; transcript accumulation works
    /// identically.
    #[allow(dead_code)] // exposed for unit tests that exercise the sink in isolation
    pub fn new_without_app_handle(run_id: String, session_id: String) -> Self {
        Self {
            transcript: StdMutex::new(Vec::new()),
            text_parts: StdMutex::new(Vec::new()),
            per_turn_usage: StdMutex::new(Vec::new()),
            had_error: std::sync::atomic::AtomicBool::new(false),
            was_cancelled: std::sync::atomic::AtomicBool::new(false),
            was_incomplete: std::sync::atomic::AtomicBool::new(false),
            was_loop_terminated: std::sync::atomic::AtomicBool::new(false),
            turns_completed: std::sync::atomic::AtomicU64::new(0),
            event_sink: Arc::new(super::ThreadLocalSubagentSink),
            app_handle: None,
            run_id,
            session_id,
            tool_call_received_at: StdMutex::new(HashMap::new()),
            worker_messages: StdMutex::new(Vec::new()),
        }
    }

    /// Construct a sink with an explicitly-injected
    /// `SubagentEventSink`. P2.4 C5 (2026-07-22): this is now the
    /// SINGLE production constructor — `dispatch.rs` injects the
    /// transport's sink (Tauri `AppHandleSubagentSink` / daemon
    /// `HttpSseSubagentSink` / test `ThreadLocalSubagentSink`)
    /// here, replacing the old `new` / `new_without_app_handle`
    /// AppHandle split.
    pub fn new_with_event_sink(
        run_id: String,
        session_id: String,
        event_sink: Arc<dyn super::SubagentEventSink>,
    ) -> Self {
        Self {
            transcript: StdMutex::new(Vec::new()),
            text_parts: StdMutex::new(Vec::new()),
            per_turn_usage: StdMutex::new(Vec::new()),
            had_error: std::sync::atomic::AtomicBool::new(false),
            was_cancelled: std::sync::atomic::AtomicBool::new(false),
            was_incomplete: std::sync::atomic::AtomicBool::new(false),
            was_loop_terminated: std::sync::atomic::AtomicBool::new(false),
            turns_completed: std::sync::atomic::AtomicU64::new(0),
            event_sink,
            app_handle: None,
            run_id,
            session_id,
            tool_call_received_at: StdMutex::new(HashMap::new()),
            worker_messages: StdMutex::new(Vec::new()),
        }
    }

    /// Construct a sink whose IPC path is delegated to an injected
    /// collector. The collector runs in place of `app_handle.emit`
    /// so tests can assert the exact IPC payload shape without
    /// needing a real Tauri runtime. Used by the
    /// `subagent_buffer_sink_emits_ipc_event` test to lock the
    /// `subagent:event` wire shape end-to-end.
    #[cfg(test)]
    pub fn new_with_collector(
        run_id: String,
        session_id: String,
        collector: Arc<StdMutex<Vec<serde_json::Value>>>,
    ) -> Self {
        // transport-abstraction 2026-07-20 (P1.3): wire the
        // collector through the `SubagentEventSink` trait
        // (`arm_test_collector` arms a thread-local cell that
        // `ThreadLocalSubagentSink` reads). The `app_handle`
        // field stays `None` so the test path is unchanged
        // from the caller's perspective.
        let sink = Self {
            transcript: StdMutex::new(Vec::new()),
            text_parts: StdMutex::new(Vec::new()),
            per_turn_usage: StdMutex::new(Vec::new()),
            had_error: std::sync::atomic::AtomicBool::new(false),
            was_cancelled: std::sync::atomic::AtomicBool::new(false),
            was_incomplete: std::sync::atomic::AtomicBool::new(false),
            was_loop_terminated: std::sync::atomic::AtomicBool::new(false),
            turns_completed: std::sync::atomic::AtomicU64::new(0),
            event_sink: Arc::new(super::ThreadLocalSubagentSink),
            app_handle: None,
            run_id,
            session_id,
            tool_call_received_at: StdMutex::new(HashMap::new()),
            worker_messages: StdMutex::new(Vec::new()),
        };
        crate::agent::subagent::arm_test_collector(collector);
        sink
    }

    pub(crate) fn record(&self, kind: TranscriptKind, payload_json: serde_json::Value) {
        // transport-abstraction 2026-07-20 (P1.3): route the
        // `subagent:event` IPC emit through the
        // `SubagentEventSink` trait instead of branching on
        // `Option<AppHandle>`. The `kind` / `payload_json` body is
        // exactly the same; the trait wraps it in the canonical
        // wire shape (matches `build_subagent_event_payload`).
        self.event_sink.emit_subagent_event(
            &self.run_id,
            &self.session_id,
            kind,
            payload_json.clone(),
        );
        self.transcript
            .lock()
            .expect("SubagentBufferSink transcript mutex poisoned")
            .push(TranscriptEntry { kind, payload_json });
    }

    /// Snapshot of the worker's accumulated text deltas, joined.
    /// Called by `run_subagent` after the worker loop returns.
    pub fn final_text(&self) -> String {
        let guard = self
            .text_parts
            .lock()
            .expect("SubagentBufferSink text_parts mutex poisoned");
        guard.join("")
    }

    /// C1 (07-26-subagent-resume): the worker's final `Vec<ChatMessage>`
    /// snapshot, captured by `record_worker_messages` on the chat loop's
    /// normal completion path. Called by `run_subagent` after the worker
    /// loop returns to persist into `subagent_runs.messages_json`. Empty
    /// for cancel/error/incomplete exits (those skip the snapshot call).
    pub fn worker_messages(&self) -> Vec<crate::llm::types::ChatMessage> {
        self.worker_messages
            .lock()
            .expect("SubagentBufferSink worker_messages mutex poisoned")
            .clone()
    }

    pub fn had_error(&self) -> bool {
        self.had_error.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn was_cancelled(&self) -> bool {
        self.was_cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 2026-06-21 (R2): set when the worker emitted a synthetic
    /// terminal `Done{max_turns}` event. `run_subagent` reads
    /// this to pick the `status: incomplete` prefix (vs.
    /// `Completed` for the natural end_turn exit).
    pub fn was_incomplete(&self) -> bool {
        self.was_incomplete
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// C2+ (2026-07-05, task `07-05-c2-loop-active-intervention`
    /// PR3): set when the worker emitted a terminal
    /// `Done { stop_reason: "loop_terminated" }` event. The
    /// `run_subagent` caller reads this to pick the
    /// `status: incomplete` prefix and append the
    /// `[loop terminated: ...]` line to the dispatch_result.
    pub fn was_loop_terminated(&self) -> bool {
        self.was_loop_terminated
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 2026-06-22 (RULE-FrontSubagent-004): actual completed LLM
    /// turn count at worker exit. Incremented once per REAL per-turn
    /// `Done` (same discriminator as the `per_turn_usage` push —
    /// synthetic `cancelled` / `max_turns` terminals do NOT
    /// increment). `run_subagent` reads this to populate
    /// `subagent_runs.turn_count` via `update_run_finished`.
    pub fn turns_completed(&self) -> u64 {
        self.turns_completed
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// transport-abstraction 2026-07-20 (P1.3): forward
    /// `subagent:finished` to the injected `SubagentEventSink`.
    /// `run_subagent` calls this after `update_run_finished`
    /// commits the terminal row. Public so dispatch.rs can call
    /// it without reaching into the private `event_sink` field.
    pub fn emit_subagent_finished(
        &self,
        run_id: &str,
        session_id: &str,
        status_db: &str,
        finished_at: &str,
    ) {
        self.event_sink
            .emit_subagent_finished(run_id, session_id, status_db, finished_at);
    }

    /// Snapshot of the transcript (clone). Used by future PR2/PR3
    /// to persist into `subagent_runs.transcript_json`.
    #[allow(dead_code)] // PR2: persists transcript; PR3: expands it.
    pub fn transcript_snapshot(&self) -> Vec<TranscriptEntry> {
        self.transcript
            .lock()
            .expect("SubagentBufferSink transcript mutex poisoned")
            .clone()
    }

    /// Drain the accumulated per-turn `TokenUsage` entries. Returns
    /// the union sum and clears the sink's buffer (the sink is
    /// single-shot — the caller is `run_subagent`, which runs once
    /// per worker dispatch).
    ///
    /// B6 PR2: this was the intended per-turn fold surface for the
    /// parent session's `sessions.input_tokens_total`. The 2026-06-26
    /// snapshot fix isolates worker token usage from the parent's
    /// snapshot entirely (reversal of RULE-A-015/PR2a), so this
    /// method has NO production callsite. It is **retained** as
    /// `#[allow(dead_code)]` API surface for a future worker↔parent
    /// session identity split and is exercised by the
    /// `buffer_sink_drain_per_turn_usage_clears_buffer` test in
    /// this module.
    #[allow(dead_code)]
    pub fn drain_per_turn_usage(&self) -> TokenUsage {
        let mut guard = self
            .per_turn_usage
            .lock()
            .expect("SubagentBufferSink per_turn_usage mutex poisoned");
        let drained: Vec<TokenUsage> = guard.drain(..).collect();
        sum_usage(&drained)
    }

    /// Cumulative per-turn `TokenUsage` snapshot (no drain). Read
    /// by `run_subagent` at worker exit to populate
    /// `subagent_runs.token_usage_json`.
    pub fn cumulative_usage(&self) -> TokenUsage {
        let guard = self
            .per_turn_usage
            .lock()
            .expect("SubagentBufferSink per_turn_usage mutex poisoned");
        sum_usage(&guard)
    }
}

/// Sum a slice of `TokenUsage` into one. Helper for the sink's
/// `drain_per_turn_usage` / `cumulative_usage` paths.
fn sum_usage(items: &[TokenUsage]) -> TokenUsage {
    let mut total = TokenUsage::default();
    for u in items {
        total.input_tokens = total.input_tokens.saturating_add(u.input_tokens);
        total.output_tokens = total.output_tokens.saturating_add(u.output_tokens);
        total.cache_creation_input_tokens = total
            .cache_creation_input_tokens
            .saturating_add(u.cache_creation_input_tokens);
        total.cache_read_input_tokens = total
            .cache_read_input_tokens
            .saturating_add(u.cache_read_input_tokens);
        // 2026-06-26 snapshot fix: sum the normalized field too.
        // (Worker context_input_tokens is per-turn; the cumulative
        // is the worker's TOTAL context pressure across all its
        // turns — used for `subagent_runs.token_usage_json`, not
        // for the parent UI hint.)
        total.context_input_tokens = total
            .context_input_tokens
            .saturating_add(u.context_input_tokens);
    }
    total
}

impl SubagentBufferSink {
    /// 2026-06-22 (RULE-WorkerAsk-001): record the resolve outcome of
    /// a worker's `PermissionAsk` as a `PermissionAskResolved`
    /// transcript entry. Called by `ask_path`'s worker branch AFTER
    /// the `tokio::select!{cancel, timeout, rx}` returns its outcome.
    ///
    /// **Transcript-only** (no dual IPC emit). The live interaction
    /// card's disappearance is driven by the permissions store
    /// removing the pending entry on resolve (Session 62 `89e5ba1`).
    /// This transcript entry is the **historical-replay record** —
    /// when the user reopens the drawer after the worker exits, the
    /// frontend pairs this entry to the matching ask by `rid` and
    /// surfaces the outcome as a badge on the card.
    ///
    /// **No audit** (RULE-A-016): worker resolve events stay in the
    /// transcript, NOT in `session_audit_events`. Same invariant as
    /// `emit_permission_ask`.
    ///
    /// `outcome` is one of `"allow"` / `"deny"` / `"timeout"` /
    /// `"cancel"` (DEBT-locked four-state wire). The caller
    /// (`ask_path` worker branch) maps its `tokio::select!` arm to
    /// the appropriate outcome string before calling this.
    ///
    /// This is the inner helper (free function) invoked by the
    /// trait override `emit_permission_ask_resolved` above + by
    /// tests that want to exercise the recording path directly
    /// without going through the trait dispatch.
    pub(crate) fn record_permission_ask_resolved(&self, rid: &str, outcome: &str) {
        let payload_json = serde_json::json!({
            "rid": rid,
            "outcome": outcome,
        });
        self.record(TranscriptKind::PermissionAskResolved, payload_json);
    }
}
