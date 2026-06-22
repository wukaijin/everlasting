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

use tauri::Emitter;

use super::transcript::{build_subagent_event_payload, TranscriptEntry, TranscriptKind};
use crate::agent::permissions::PermissionAskPayload;
use crate::llm::types::{ChatEvent, TokenUsage};
use crate::state::{ChatEventPayload, ToolCallPayload, ToolResultPayload};

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
    static TEST_COLLECTOR: RefCell<Option<Arc<StdMutex<Vec<serde_json::Value>>>>> =
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
    transcript: StdMutex<Vec<TranscriptEntry>>,
    /// Accumulated assistant text deltas. Read by `run_subagent`
    /// after `run_chat_loop` returns to extract the worker's
    /// final summary.
    text_parts: StdMutex<Vec<String>>,
    /// Per-turn `TokenUsage` accumulated from `ChatEvent::Done { usage: Some(t) }`
    /// events. Read by `run_subagent` after the worker loop returns
    /// to populate `subagent_runs.token_usage_json`.
    ///
    /// **Per-turn fold into parent's `sessions.input_tokens_total`**:
    /// does NOT happen here — `add_token_usage_streaming` is
    /// `#[allow(dead_code)]` (no production callsite; only exercised
    /// by `db/tests.rs::add_token_usage_streaming_accumulates_in_parent`).
    /// The real production fold goes through `db::add_token_usage`
    /// at `chat_loop.rs:1031` (decoupled from `skip_persist` in
    /// B6 PR2a per RULE-A-015 — the worker reuses
    /// `parent_session_id`, so the parent's `add_token_usage` call
    /// accumulates the worker's per-turn usage into the parent's
    /// `sessions.*_total` columns naturally). The parent's UI sees
    /// the counter update on the worker's terminal `Done` event,
    /// with a few seconds of lag (acceptable per
    /// `RULE-BackSubagent-002` option i).
    ///
    /// `drain_per_turn_usage` (which would invoke the streaming
    /// fold) is retained as the public API surface for a future
    /// worker↔parent session identity split (see the helper's
    /// own doc for details).
    per_turn_usage: StdMutex<Vec<TokenUsage>>,
    /// Set when the worker emitted a terminal `Error` event.
    /// `run_subagent` reads this to pick the `status: error`
    /// prefix.
    had_error: std::sync::atomic::AtomicBool,
    /// Set when the worker emitted a terminal `Done{cancelled}`
    /// event (stop_reason == "cancelled"). `run_subagent` reads
    /// this to pick the `status: cancelled` prefix.
    was_cancelled: std::sync::atomic::AtomicBool,
    /// 2026-06-21 (R2): set when the worker emitted a synthetic
    /// terminal `Done{max_turns}` event. `run_subagent` reads
    /// this to pick the `status: incomplete` prefix (vs.
    /// `Completed` for the natural end_turn exit). Mutually
    /// exclusive with `was_cancelled` and `had_error` in
    /// practice — the agent loop's `max_turns` branch fires
    /// when the worker exhausts its turn budget, which is not
    /// a cancel or an error path.
    was_incomplete: std::sync::atomic::AtomicBool,
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
    turns_completed: std::sync::atomic::AtomicU64,
    /// PR2 hotfix (B6 PR3, 2026-06-20): optional Tauri
    /// `AppHandle` used to emit the `subagent:event` IPC channel
    /// on every emit. `None` in tests (no Tauri runtime) — the
    /// emit side becomes a silent no-op, but the transcript
    /// accumulation path is unaffected.
    app_handle: Option<tauri::AppHandle>,
    /// PR2 hotfix: the worker's `run_id` (the `parent_rid-sub-<seq>`
    /// string `run_subagent` builds at chat_loop.rs:2050). Carried
    /// on the sink so each `subagent:event` payload can identify
    /// which worker run the event belongs to.
    run_id: String,
    /// PR2 hotfix: the parent session_id (worker reuses parent's
    /// session_id). Each `subagent:event` payload includes this so
    /// the frontend can route events to the right session's drawer.
    session_id: String,
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
    tool_call_received_at: StdMutex<HashMap<String, Instant>>,
}

impl SubagentBufferSink {
    /// Construct a sink with Tauri IPC. Used by production
    /// (`run_subagent` threads the parent's `AppHandle` into the
    /// worker via `run_chat_loop`'s 22nd parameter).
    pub fn new(app_handle: tauri::AppHandle, run_id: String, session_id: String) -> Self {
        Self {
            transcript: StdMutex::new(Vec::new()),
            text_parts: StdMutex::new(Vec::new()),
            per_turn_usage: StdMutex::new(Vec::new()),
            had_error: std::sync::atomic::AtomicBool::new(false),
            was_cancelled: std::sync::atomic::AtomicBool::new(false),
            was_incomplete: std::sync::atomic::AtomicBool::new(false),
            turns_completed: std::sync::atomic::AtomicU64::new(0),
            app_handle: Some(app_handle),
            run_id,
            session_id,
            tool_call_received_at: StdMutex::new(HashMap::new()),
        }
    }

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
            turns_completed: std::sync::atomic::AtomicU64::new(0),
            app_handle: None,
            run_id,
            session_id,
            tool_call_received_at: StdMutex::new(HashMap::new()),
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
        // The production path uses `app_handle.emit`; the test
        // path stores the payload in the collector. We can't have
        // both wired simultaneously through the same struct field
        // without complicating the type, so the production field
        // stays `None` for the test constructor and we route the
        // emit through a separate `emit_override` field instead.
        let sink = Self {
            transcript: StdMutex::new(Vec::new()),
            text_parts: StdMutex::new(Vec::new()),
            per_turn_usage: StdMutex::new(Vec::new()),
            had_error: std::sync::atomic::AtomicBool::new(false),
            was_cancelled: std::sync::atomic::AtomicBool::new(false),
            was_incomplete: std::sync::atomic::AtomicBool::new(false),
            turns_completed: std::sync::atomic::AtomicU64::new(0),
            app_handle: None,
            run_id,
            session_id,
            tool_call_received_at: StdMutex::new(HashMap::new()),
        };
        // Stash the collector on a thread-local for the duration
        // of the test; the record() method consults it. We use a
        // thread-local (not a field) to keep the production
        // struct unchanged — the alternative is making
        // `app_handle` an enum variant, which complicates every
        // call site.
        TEST_COLLECTOR.with(|c| {
            *c.borrow_mut() = Some(collector);
        });
        sink
    }

    fn record(&self, kind: TranscriptKind, payload_json: serde_json::Value) {
        // PR2 hotfix (B6 PR3, 2026-06-20): emit the `subagent:event`
        // IPC channel in parallel with the transcript append so the
        // frontend `<SubagentDrawer>` (PR3b) can stream the
        // worker's transcript live. The payload is a
        // `serde_json::Value` (not a typed struct) to keep the
        // Tauri channel wire shape exactly the shape documented in
        // the prd.md "PR2 hotfix" decision:
        //   { runId, sessionId, kind, payload, timestamp }
        // The kind string mirrors the Rust `TranscriptKind` enum's
        // `#[serde(rename_all = "snake_case")]` serialization
        // (`ChatEvent` / `ToolCall` / `ToolResult` / `PermissionAsk`)
        // so the TS side stays lockstep with the Rust enum.
        let ipc_payload = build_subagent_event_payload(
            &self.run_id,
            &self.session_id,
            kind,
            payload_json.clone(),
        );
        if let Some(handle) = &self.app_handle {
            if let Err(e) = handle.emit("subagent:event", ipc_payload) {
                tracing::warn!(
                    error = %e,
                    run_id = %self.run_id,
                    "subagent:event emit failed (non-fatal; transcript still recorded)"
                );
            }
        } else {
            // Test-only: forward to the in-memory collector if one
            // is armed via `new_with_collector`.
            TEST_COLLECTOR.with(|c| {
                if let Some(collector) = c.borrow().as_ref() {
                    collector.lock().unwrap().push(ipc_payload);
                }
            });
        }
        self.transcript
            .lock()
            .expect("SubagentBufferSink transcript mutex poisoned")
            .push(TranscriptEntry {
                kind,
                payload_json,
            });
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

    pub fn had_error(&self) -> bool {
        self.had_error
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn was_cancelled(&self) -> bool {
        self.was_cancelled
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 2026-06-21 (R2): set when the worker emitted a synthetic
    /// terminal `Done{max_turns}` event. `run_subagent` reads
    /// this to pick the `status: incomplete` prefix (vs.
    /// `Completed` for the natural end_turn exit).
    pub fn was_incomplete(&self) -> bool {
        self.was_incomplete
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
    /// B6 PR2: `run_subagent` would call this **once per worker turn**
    /// to fold the new turn's usage into the parent session's
    /// `sessions.input_tokens_total`. The current production
    /// implementation routes the per-turn fold through
    /// `db::add_token_usage` (decoupled from `skip_persist` —
    /// see `chat_loop.rs:907`), so the sink-side drain is
    /// not invoked by production. The method is **retained** as
    /// the public API surface (the PRD §"SubagentBufferSink"
    /// mentions streaming accumulation) and is exercised by the
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
    }
    total
}

impl crate::state::ChatEventSink for SubagentBufferSink {
    fn emit_chat_event(&self, payload: &ChatEventPayload) {
        // Track terminal signals + accumulate text deltas for the
        // final summary.
        match &payload.event {
            ChatEvent::Delta { text } => {
                self.text_parts
                    .lock()
                    .expect("SubagentBufferSink text_parts mutex poisoned")
                    .push(text.clone());
            }
            ChatEvent::Error { .. } => {
                self.had_error
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            ChatEvent::Done { stop_reason, usage } => {
                // B6 PR2: capture per-turn token usage for the
                // worker run's `subagent_runs.token_usage_json`.
                // The worker reuses `parent_session_id` and
                // `run_chat_loop`'s `db::add_token_usage` call at
                // `chat_loop.rs:1031` is OUTSIDE the `skip_persist`
                // gate (decoupled in PR2a per RULE-A-015), so the
                // parent's `sessions.*_total` columns accumulate
                // the worker's per-turn usage naturally. The sink
                // does NOT stream-fold via
                // `add_token_usage_streaming` — that helper has no
                // production callsite (only `db/tests.rs`). Per-turn
                // lag is a few seconds; accepted per
                // RULE-BackSubagent-002 option i.
                //
                // 2026-06-21 (R3): synthetic terminals
                // (`max_turns` / `cancelled`) are emitted with
                // `usage = last_usage` for `max_turns` (see
                // `chat_loop.rs:1797-1804`). The prior per-turn
                // Done for the final turn ALREADY pushed its
                // `usage: Some(t)` into the Vec; pushing again
                // here would double-count the last turn. The
                // stop_reason guard skips the push for synthetic
                // terminals so the Vec holds exactly one entry
                // per real per-turn Done, no more.
                if let Some(u) = usage {
                    if stop_reason.as_deref() != Some("cancelled")
                        && stop_reason.as_deref() != Some("max_turns")
                    {
                        self.per_turn_usage
                            .lock()
                            .expect("SubagentBufferSink per_turn_usage mutex poisoned")
                            .push(*u);
                        // 2026-06-22 (RULE-FrontSubagent-004):
                        // increment the turn counter on the SAME
                        // discriminator as the `per_turn_usage`
                        // push so the two stay 1:1
                        // (turns_completed() == per_turn_usage.len()
                        // at worker exit). Synthetic terminals
                        // (cancelled / max_turns) do NOT increment
                        // because they reuse the prior turn's
                        // usage (would double-count). The counter
                        // thus always reflects the actual count of
                        // real per-turn Dones — even when the
                        // worker exited via the soft-cap or cancel.
                        self.turns_completed
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                if stop_reason.as_deref() == Some("cancelled")
                    || stop_reason.as_deref() == Some("max_turns")
                {
                    // Treat max_turns as a soft "ran out of budget"
                    // — the worker did useful work but didn't
                    // cleanly finish. The summary still carries
                    // whatever it produced. Status prefix =
                    // "incomplete" with a note appended (R2
                    // 2026-06-21); for cancelled (user Stop
                    // propagated to worker) we use status=cancelled.
                    if stop_reason.as_deref() == Some("cancelled") {
                        self.was_cancelled
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    } else if stop_reason.as_deref() == Some("max_turns") {
                        // 2026-06-21 (R2): distinct from
                        // `was_cancelled` so `run_subagent`'s
                        // status picker can distinguish the
                        // budget-exhaustion path from the
                        // clean-failure path. Mutually exclusive
                        // with `was_cancelled` in practice.
                        self.was_incomplete
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
            _ => {}
        }
        let payload_json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self.record(TranscriptKind::ChatEvent, payload_json);
    }

    fn emit_tool_call(&self, payload: &ToolCallPayload) {
        // B6 PR3 redesign (2026-06-21): record the `Instant` of this
        // tool_call so the paired `emit_tool_result` can compute the
        // wall-clock `duration_ms`. The frontend drawer pairs the
        // two transcript entries by `tool_use_id` and renders the
        // duration in the merged card header (see
        // `.trellis/tasks/06-21-redesign-subagent-drawer-entry-as-toolcard-style/prd.md`
        // §"Technical Approach"). The Instant is the wall-clock now
        // (`Instant::now()`), not the message's emit timestamp —
        // matches the main panel's `ToolCallCard` duration contract
        // (F5), which is "from tool_call to tool_result wall-clock".
        let mut map = self
            .tool_call_received_at
            .lock()
            .expect("SubagentBufferSink tool_call_received_at mutex poisoned");
        map.insert(payload.id.clone(), Instant::now());
        // Defensive cap: if a worker ever produces a runaway number
        // of distinct tool_use_ids without results landing (e.g. an
        // error-loop worker spamming tool_use), bound the map. The
        // 1024 cap is generous for the 20-turn worker's realistic
        // case (a busy tool-heavy turn produces ~5-10 distinct
        // tool_use_ids). The eviction policy is "drop oldest entry"
        // to keep the most recent measurements intact.
        if map.len() > 1024 {
            if let Some(oldest_key) = map
                .iter()
                .min_by_key(|(_, v)| v.elapsed())
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest_key);
            }
        }
        drop(map);
        let payload_json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        // Inject the `tool_use_id` field at the top level of
        // payload_json so the frontend can pair tool_call with the
        // matching tool_result. The original `ToolCallPayload` does
        // not serialize `id` separately (it has `request_id` and
        // `id`, but the frontend `TranscriptEntry` projection
        // historically only exposed `payload_json.{name,input}` for
        // tool_call — see `subagentRuns.ts:TranscriptEntry`). Adding
        // the field at serialization time keeps the Rust struct
        // stable for cross-process Tauri commands (no DB migration
        // needed — see PRD §"Cross-layer Decision Points").
        let mut payload_obj = match payload_json {
            serde_json::Value::Object(m) => m,
            other => {
                tracing::warn!(
                    tool_use_id = %payload.id,
                    "tool_call payload_json not an object; wrapping as-is"
                );
                let mut m = serde_json::Map::new();
                m.insert("raw".into(), other);
                m
            }
        };
        payload_obj.insert("tool_use_id".into(), serde_json::Value::String(payload.id.clone()));
        let enriched = serde_json::Value::Object(payload_obj);
        self.record(TranscriptKind::ToolCall, enriched);
    }

    fn emit_tool_result(&self, payload: &ToolResultPayload) {
        // B6 PR3 redesign (2026-06-21): look up the matching
        // `tool_call` Instant, compute the wall-clock gap, and embed
        // it (plus `tool_use_id`) into payload_json so the frontend
        // drawer can render the per-tool duration on the merged
        // card header. Orphan tool_result (no matching tool_call —
        // possible if the IPC `subagent:event` was lost or the
        // transcript was truncated at the 4 MiB cap) falls back to
        // `duration_ms = 0` with a `tracing::warn!`; the entry
        // still lands in the transcript so the user sees the result,
        // the drawer's pairing layer treats it as a standalone
        // "orphan result" card.
        let mut map = self
            .tool_call_received_at
            .lock()
            .expect("SubagentBufferSink tool_call_received_at mutex poisoned");
        let duration_ms: u64 = if let Some(start) = map.remove(&payload.tool_use_id) {
            let ms = start.elapsed().as_millis();
            // Saturating cast — a `u128` ms value cannot realistically
            // exceed `u64::MAX`, but the saturating cast keeps the
            // conversion safe under any pathological clock behavior.
            u64::try_from(ms).unwrap_or(u64::MAX)
        } else {
            tracing::warn!(
                tool_use_id = %payload.tool_use_id,
                "tool_result arrived without matching tool_call; duration_ms=0"
            );
            0
        };
        drop(map);
        let payload_json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        // Enrich payload_json with `tool_use_id` (top-level) +
        // `duration_ms` so the frontend pairing layer can locate the
        // matching call and render the duration. The Rust struct
        // `ToolResultPayload` does not derive `tool_use_id` at the
        // top level (it has `request_id` + `tool_use_id` as separate
        // fields, but the original `TranscriptEntry` projection in
        // `subagentRuns.ts` only exposed
        // `payload_json.{content,is_error}`). Adding the field at
        // serialization time keeps the Rust struct stable.
        let mut payload_obj = match payload_json {
            serde_json::Value::Object(m) => m,
            other => {
                tracing::warn!(
                    tool_use_id = %payload.tool_use_id,
                    "tool_result payload_json not an object; wrapping as-is"
                );
                let mut m = serde_json::Map::new();
                m.insert("raw".into(), other);
                m
            }
        };
        payload_obj.insert(
            "tool_use_id".into(),
            serde_json::Value::String(payload.tool_use_id.clone()),
        );
        payload_obj.insert(
            "duration_ms".into(),
            serde_json::Value::Number(duration_ms.into()),
        );
        let enriched = serde_json::Value::Object(payload_obj);
        self.record(TranscriptKind::ToolResult, enriched);
    }

    fn emit_permission_ask(&self, payload: PermissionAskPayload) {
        // 2026-06-22 (RULE-FrontSubagent-003 fix): worker asks now
        // go through the full interactive round-trip
        // (`register_ask + tokio::select!{cancel, timeout, oneshot}`)
        // instead of auto-denying at the Tier 4 is_worker collapse.
        //
        // The ask is delivered to the frontend over TWO channels:
        //   1. `permission:ask` (emitted below when AppHandle is
        //      present) → consumed by `usePermissionsStore` →
        //      live pending entry (`pendingWorkerByRunId`) →
        //      interactive Allow/Deny card in `<SubagentDrawer>`
        //      + `<WorkerAskBanner>` counter.
        //   2. `subagent:event` (via self.record below) →
        //      transcript, consumed by `useSubagentRunsStore` →
        //      historical render in the drawer (also captures
        //      the ask when no AppHandle is wired, e.g. in unit
        //      tests that use the test collector).
        //
        // Both channels carry the same rid; the drawer dedups by
        // rid (interactive while the permissions store has it
        // pending, historical once resolved). The dual emit is
        // the correct separation: worker chat events stay on
        // `subagent:event` (don't pollute the main chat), while
        // `permission:ask` is the shared approval channel both
        // main-chat and worker asks use.
        //
        // The resolve side (user Allow / Deny / timeout / cancel)
        // does NOT write a follow-up audit row to the parent's
        // `session_audit_events` per RULE-A-016 — the transcript
        // is the worker's audit-like record.
        //
        // PR1.5 (2026-06-22): emit BEFORE `record()` so the
        // frontend permissions store is armed before/alongside
        // the transcript entry (avoids a render race where the
        // transcript card appears historical before the live
        // entry lands). Both are synchronous emits so ordering
        // is minor, but emit-first is the safer choice.
        if let Some(handle) = &self.app_handle {
            if let Err(e) = handle.emit("permission:ask", payload.clone()) {
                tracing::warn!(
                    error = %e,
                    run_id = %self.run_id,
                    "permission:ask emit failed (non-fatal; transcript still recorded)"
                );
            }
        }
        // Test-only: when no app_handle is wired, the payload is
        // still captured via the transcript record below (test
        // collectors inspect transcript entries). The IPC emit
        // path is exercised in integration, not unit, tests.
        let payload_json = serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null);
        self.record(TranscriptKind::PermissionAsk, payload_json);
    }

    /// 2026-06-22 (RULE-WorkerAsk-001): trait override of
    /// `ChatEventSink::emit_permission_ask_resolved`. Records the
    /// worker's `PermissionAsk` resolve outcome as a
    /// `PermissionAskResolved` transcript entry. Called by
    /// `ask_path`'s worker branch AFTER its `tokio::select!` arm
    /// returns its outcome.
    ///
    /// **Transcript-only** (no dual IPC emit). The live
    /// interaction card's disappearance is driven by the
    /// permissions store removing the pending entry on resolve
    /// (Session 62 `89e5ba1`). This transcript entry is the
    /// **historical-replay record** — when the user reopens the
    /// drawer after the worker exits, the frontend pairs this
    /// entry to the matching ask by `rid` and surfaces the
    /// outcome as a badge on the card.
    ///
    /// **No audit** (RULE-A-016): worker resolve events stay in
    /// the transcript, NOT in `session_audit_events`.
    ///
    /// `outcome` is one of `"allow"` / `"deny"` / `"timeout"` /
    /// `"cancel"` (DEBT-locked four-state wire). The caller
    /// (`ask_path` worker branch) maps its `tokio::select!` arm
    /// to the appropriate outcome string before calling this.
    fn emit_permission_ask_resolved(&self, rid: &str, outcome: &str) {
        self.record_permission_ask_resolved(rid, outcome);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ChatEventSink;

    // ---- helpers ----

    fn done_with_usage(input: u32, output: u32) -> ChatEventPayload {
        ChatEventPayload {
            request_id: "rid-u".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("end_turn".to_string()),
                usage: Some(TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
            },
        }
    }

    fn sink_with_resolved(rid: &str, outcome: &str) -> Vec<TranscriptEntry> {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_permission_ask_resolved(rid, outcome);
        sink.transcript_snapshot()
    }

    // ---- basic sink behavior ----

    #[test]
    fn buffer_sink_accumulates_text_deltas() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        let rid = "rid-test".to_string();
        for t in ["hello", " ", "world"] {
            sink.emit_chat_event(&ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Delta {
                    text: t.to_string(),
                },
            });
        }
        assert_eq!(sink.final_text(), "hello world");
    }

    #[test]
    fn buffer_sink_tracks_cancelled_done() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        let rid = "rid-cancel".to_string();
        sink.emit_chat_event(&ChatEventPayload {
            request_id: rid.clone(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        assert!(sink.was_cancelled());
        assert!(!sink.had_error());
    }

    #[test]
    fn buffer_sink_tracks_error_event() {
        use crate::llm::LlmErrorCategory;
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        let rid = "rid-err".to_string();
        sink.emit_chat_event(&ChatEventPayload {
            request_id: rid.clone(),
            event: ChatEvent::Error {
                message: "boom".to_string(),
                category: LlmErrorCategory::Server,
            },
        });
        assert!(sink.had_error());
        assert!(!sink.was_cancelled());
    }

    #[test]
    fn buffer_sink_records_transcript_entries() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        let rid = "rid-transcript".to_string();
        sink.emit_chat_event(&ChatEventPayload {
            request_id: rid.clone(),
            event: ChatEvent::Start,
        });
        sink.emit_tool_call(&ToolCallPayload {
            request_id: rid.clone(),
            id: "toolu_1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "/x"}),
        });
        sink.emit_tool_result(&ToolResultPayload {
            request_id: rid,
            tool_use_id: "toolu_1".to_string(),
            content: "ok".to_string(),
            is_error: false,
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 3);
        assert_eq!(transcript[0].kind, TranscriptKind::ChatEvent);
        assert_eq!(transcript[1].kind, TranscriptKind::ToolCall);
        assert_eq!(transcript[2].kind, TranscriptKind::ToolResult);
    }

    // ---- token usage accumulation (B6 PR2) ----

    #[test]
    fn buffer_sink_accumulates_token_usage_per_turn() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&done_with_usage(50, 10));
        let total = sink.cumulative_usage();
        assert_eq!(total.input_tokens, 350);
        assert_eq!(total.output_tokens, 90);
    }

    #[test]
    fn buffer_sink_drain_per_turn_usage_clears_buffer() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(10, 5));
        let drained = sink.drain_per_turn_usage();
        assert_eq!(drained.input_tokens, 10);
        assert_eq!(drained.output_tokens, 5);
        // After drain, the cumulative is zero.
        let after = sink.cumulative_usage();
        assert_eq!(after.input_tokens, 0);
        assert_eq!(after.output_tokens, 0);
    }

    #[test]
    fn buffer_sink_done_without_usage_does_not_accumulate() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        let total = sink.cumulative_usage();
        assert_eq!(total.input_tokens, 0);
        assert_eq!(total.output_tokens, 0);
    }

    // ---- R3 (2026-06-21) max_turns terminal-patch regression tests ----

    /// R3 regression: the synthetic terminal `Done{max_turns, usage:
    /// last_usage}` must NOT double-count the last turn (the guard
    /// skips the push for synthetic terminals).
    #[test]
    fn buffer_sink_max_turns_terminal_does_not_double_count_last_turn() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&done_with_usage(50, 10));
        let t_last = TokenUsage {
            input_tokens: 50,
            output_tokens: 10,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("max_turns".to_string()),
                usage: Some(t_last),
            },
        });
        let total = sink.cumulative_usage();
        assert_eq!(
            total.input_tokens, 350,
            "cumulative input = 100+200+50 (synthetic terminal must not double-count)"
        );
        assert_eq!(
            total.output_tokens, 90,
            "cumulative output = 50+30+10 (synthetic terminal must not double-count)"
        );
    }

    /// R3 mirror: cancelled synthetic terminal must NOT affect
    /// cumulative_usage().
    #[test]
    fn buffer_sink_cancelled_terminal_does_not_affect_cumulative_usage() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        let total = sink.cumulative_usage();
        assert_eq!(total.input_tokens, 300);
        assert_eq!(total.output_tokens, 80);
    }

    /// RULE-FrontSubagent-004: `turns_completed()` increments once
    /// per REAL per-turn Done (synthetic terminals do NOT bump).
    #[test]
    fn buffer_sink_turns_completed_tracks_real_per_turn_dones() {
        // (a) Clean end_turn: 3 per-turn Dones → turns_completed == 3.
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&done_with_usage(50, 10));
        assert_eq!(sink.turns_completed(), 3, "3 real per-turn Dones → counter == 3");

        // (b) Cancelled: 2 per-turn Dones + 1 synthetic cancelled.
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        assert_eq!(
            sink.turns_completed(),
            2,
            "cancelled synthetic terminal must NOT increment"
        );

        // (c) max_turns: 200 per-turn Dones + 1 synthetic max_turns.
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        for _ in 0..200 {
            sink.emit_chat_event(&done_with_usage(100, 50));
        }
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("max_turns".to_string()),
                usage: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
            },
        });
        assert_eq!(
            sink.turns_completed(),
            200,
            "max_turns synthetic terminal must NOT increment (counter == real turn budget)"
        );
    }

    /// RULE-FrontSubagent-004: turns_completed() and per_turn_usage
    /// stay 1:1 (same discriminator guards both).
    #[test]
    fn buffer_sink_turns_completed_equals_per_turn_usage_len() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&done_with_usage(50, 10));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        assert_eq!(sink.turns_completed(), 3);
        let total = sink.cumulative_usage();
        assert_eq!(total.input_tokens, 350);
        assert_eq!(total.output_tokens, 90);
    }

    /// R3 was_incomplete: set on synthetic `Done{max_turns}`.
    #[test]
    fn buffer_sink_max_turns_terminal_sets_was_incomplete() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("max_turns".to_string()),
                usage: None,
            },
        });
        assert!(sink.was_incomplete(), "max_turns terminal must set was_incomplete=true");
        assert!(!sink.was_cancelled(), "max_turns must NOT also set was_cancelled");
        assert!(!sink.had_error(), "max_turns must NOT also set had_error");
    }

    /// R3 was_cancelled: set on synthetic `Done{cancelled}`.
    #[test]
    fn buffer_sink_cancelled_terminal_sets_was_cancelled_only() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        assert!(sink.was_cancelled(), "cancelled terminal must set was_cancelled=true");
        assert!(!sink.was_incomplete(), "cancelled must NOT also set was_incomplete");
    }

    /// R3: clean `end_turn` exit sets neither flag.
    #[test]
    fn buffer_sink_end_turn_terminal_does_not_set_incomplete_or_cancelled() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("end_turn".to_string()),
                usage: None,
            },
        });
        assert!(!sink.was_incomplete(), "end_turn terminal must NOT set was_incomplete");
        assert!(!sink.was_cancelled(), "end_turn terminal must NOT set was_cancelled");
    }

    // ---- PR2 hotfix: subagent:event IPC payload ----

    /// Each `emit_*` appends a transcript entry AND (when armed via
    /// `new_with_collector`) the matching IPC payload.
    #[test]
    fn subagent_buffer_sink_emits_ipc_event_per_emit() {
        TEST_COLLECTOR.with(|c| *c.borrow_mut() = None);
        let collector: Arc<StdMutex<Vec<serde_json::Value>>> = Arc::new(StdMutex::new(Vec::new()));
        let sink = SubagentBufferSink::new_with_collector(
            "rid-pr2".into(),
            "sid-pr2".into(),
            collector.clone(),
        );

        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid-pr2".into(),
            event: ChatEvent::Start,
        });
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid-pr2".into(),
            id: "toolu_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/x"}),
        });
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid-pr2".into(),
            tool_use_id: "toolu_1".into(),
            content: "ok".into(),
            is_error: false,
        });
        sink.emit_permission_ask(crate::agent::permissions::PermissionAskPayload {
            rid: "ask-rid".into(),
            session_id: "sid-pr2".into(),
            tool_use_id: "toolu_1".into(),
            tool_name: "shell".into(),
            tool_input: serde_json::json!({"command": "rm -rf /"}),
            risk: crate::agent::permissions::Risk::High,
            reason: Some("dangerous".into()),
            path: None,
            worker_run_id: None,
        });

        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 4);
        assert_eq!(transcript[0].kind, TranscriptKind::ChatEvent);
        assert_eq!(transcript[1].kind, TranscriptKind::ToolCall);
        assert_eq!(transcript[2].kind, TranscriptKind::ToolResult);
        assert_eq!(transcript[3].kind, TranscriptKind::PermissionAsk);

        let collected = collector.lock().unwrap().clone();
        assert_eq!(collected.len(), 4, "every emit must produce 1 IPC payload");
        assert_eq!(collected[0]["kind"], "chat_event");
        assert_eq!(collected[1]["kind"], "tool_call");
        assert_eq!(collected[2]["kind"], "tool_result");
        assert_eq!(collected[3]["kind"], "permission_ask");
        for (i, p) in collected.iter().enumerate() {
            assert_eq!(p["runId"], "rid-pr2", "payload #{i} runId");
            assert_eq!(p["sessionId"], "sid-pr2", "payload #{i} sessionId");
            assert!(
                p["payload"].is_object() || p["payload"].is_null(),
                "payload #{i} shape"
            );
            assert!(
                p["timestamp"].as_str().unwrap().contains('T'),
                "payload #{i} timestamp is RFC 3339"
            );
        }

        TEST_COLLECTOR.with(|c| *c.borrow_mut() = None);
    }

    /// `new_without_app_handle` does NOT emit IPC events.
    #[test]
    fn subagent_buffer_sink_without_app_handle_does_not_emit_ipc() {
        TEST_COLLECTOR.with(|c| *c.borrow_mut() = None);
        let sink = SubagentBufferSink::new_without_app_handle("rid-noop".into(), "sid-noop".into());
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid-noop".into(),
            event: ChatEvent::Start,
        });
        assert_eq!(sink.transcript_snapshot().len(), 1);
        TEST_COLLECTOR.with(|c| {
            assert!(c.borrow().is_none(), "no collector armed → no IPC attempted");
        });
    }

    // ---- B6 PR3 redesign: tool_use_id + duration_ms payload fields ----

    #[test]
    fn tool_call_payload_json_includes_tool_use_id() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid".into(),
            id: "toolu_42".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/foo"}),
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 1);
        let entry = &transcript[0];
        assert_eq!(entry.kind, TranscriptKind::ToolCall);
        let pj = entry.payload_json.as_object().expect("payload_json is object");
        assert_eq!(
            pj.get("tool_use_id").and_then(|v| v.as_str()),
            Some("toolu_42"),
            "tool_call payload_json must carry top-level tool_use_id"
        );
        assert_eq!(
            pj.get("id").and_then(|v| v.as_str()),
            Some("toolu_42"),
            "original `id` field preserved"
        );
        assert_eq!(pj.get("name").and_then(|v| v.as_str()), Some("read_file"));
        assert!(pj.get("input").is_some(), "input preserved");
    }

    #[test]
    fn tool_result_payload_json_includes_duration_ms() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid".into(),
            id: "toolu_p".into(),
            name: "shell".into(),
            input: serde_json::json!({"command": "ls"}),
        });
        std::thread::sleep(std::time::Duration::from_millis(5));
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid".into(),
            tool_use_id: "toolu_p".into(),
            content: "ok".into(),
            is_error: false,
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 2);
        let result_entry = &transcript[1];
        assert_eq!(result_entry.kind, TranscriptKind::ToolResult);
        let pj = result_entry
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("tool_use_id").and_then(|v| v.as_str()),
            Some("toolu_p"),
            "tool_result payload_json carries top-level tool_use_id"
        );
        let duration = pj
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .expect("duration_ms is u64");
        assert!(duration >= 4, "duration_ms must reflect wall-clock gap, got {duration}");
        assert!(duration < 5_000, "duration_ms unreasonably large: {duration}");
        assert_eq!(pj.get("content").and_then(|v| v.as_str()), Some("ok"));
        assert_eq!(pj.get("is_error").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn orphan_tool_result_gets_duration_ms_zero() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid".into(),
            tool_use_id: "toolu_orphan".into(),
            content: "partial".into(),
            is_error: false,
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 1);
        let pj = transcript[0]
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("tool_use_id").and_then(|v| v.as_str()),
            Some("toolu_orphan"),
        );
        assert_eq!(
            pj.get("duration_ms").and_then(|v| v.as_u64()),
            Some(0),
            "orphan tool_result must have duration_ms=0"
        );
    }

    #[test]
    fn consecutive_pairs_get_independent_durations() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid".into(),
            id: "toolu_a".into(),
            name: "read_file".into(),
            input: serde_json::json!({}),
        });
        std::thread::sleep(std::time::Duration::from_millis(2));
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid".into(),
            tool_use_id: "toolu_a".into(),
            content: "a".into(),
            is_error: false,
        });
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid".into(),
            id: "toolu_b".into(),
            name: "read_file".into(),
            input: serde_json::json!({}),
        });
        std::thread::sleep(std::time::Duration::from_millis(8));
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid".into(),
            tool_use_id: "toolu_b".into(),
            content: "b".into(),
            is_error: false,
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 4);
        let dur_a = transcript[1]
            .payload_json
            .as_object()
            .unwrap()
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap();
        let dur_b = transcript[3]
            .payload_json
            .as_object()
            .unwrap()
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert!(
            dur_b >= dur_a,
            "second pair ({dur_b}ms) should be at least as long as first ({dur_a}ms)"
        );
        assert!(dur_a >= 1, "dur_a < 1ms is implausible, got {dur_a}");
        assert!(dur_b >= 4, "dur_b < 4ms is implausible (we slept 8ms), got {dur_b}");
    }

    // ---- emit_permission_ask (RULE-FrontSubagent-003) ----

    /// PR1.5: `emit_permission_ask` produces a PermissionAsk
    /// transcript entry whose payload carries the PARENT session id.
    #[test]
    fn emit_permission_ask_populates_transcript_with_parent_session_id() {
        let sink = SubagentBufferSink::new_without_app_handle(
            "worker-rid-1".into(),
            "parent-sess-1".into(),
        );
        sink.emit_permission_ask(crate::agent::permissions::PermissionAskPayload {
            rid: "ask-rid-1".into(),
            session_id: "parent-sess-1".into(),
            tool_use_id: "toolu_w1".into(),
            tool_name: "write_file".into(),
            tool_input: serde_json::json!({"path": "/repo/outside/foo.rs"}),
            risk: crate::agent::permissions::Risk::High,
            reason: Some("requires confirmation".into()),
            path: Some("/repo/outside/foo.rs".into()),
            worker_run_id: Some("worker-run-1".into()),
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(
            transcript.len(),
            1,
            "emit_permission_ask must produce exactly 1 transcript entry"
        );
        let entry = &transcript[0];
        assert_eq!(entry.kind, TranscriptKind::PermissionAsk);
        let pj = entry
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("sessionId").and_then(|v| v.as_str()),
            Some("parent-sess-1"),
            "transcript payload must carry parent session_id (PR1.5 cross-layer fix)"
        );
        assert_eq!(
            pj.get("workerRunId").and_then(|v| v.as_str()),
            Some("worker-run-1"),
            "transcript payload must carry workerRunId camelCase"
        );
        assert_eq!(pj.get("rid").and_then(|v| v.as_str()), Some("ask-rid-1"),);
        assert_eq!(pj.get("toolName").and_then(|v| v.as_str()), Some("write_file"),);
        assert_eq!(pj.get("toolUseId").and_then(|v| v.as_str()), Some("toolu_w1"),);
    }

    // ---- emit_permission_ask_resolved (RULE-WorkerAsk-001) ----

    #[test]
    fn emit_permission_ask_resolved_allow_records_entry() {
        let transcript = sink_with_resolved("ask-rid-allow", "allow");
        assert_eq!(
            transcript.len(),
            1,
            "emit_permission_ask_resolved must produce exactly 1 transcript entry"
        );
        let entry = &transcript[0];
        assert_eq!(
            entry.kind,
            TranscriptKind::PermissionAskResolved,
            "kind must be PermissionAskResolved"
        );
        let pj = entry
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("rid").and_then(|v| v.as_str()),
            Some("ask-rid-allow"),
            "rid must match the input"
        );
        assert_eq!(
            pj.get("outcome").and_then(|v| v.as_str()),
            Some("allow"),
            "outcome must be 'allow' for AllowOnce/AllowAlways arm"
        );
    }

    #[test]
    fn emit_permission_ask_resolved_deny_records_entry() {
        let transcript = sink_with_resolved("ask-rid-deny", "deny");
        assert_eq!(transcript.len(), 1);
        let entry = &transcript[0];
        assert_eq!(entry.kind, TranscriptKind::PermissionAskResolved);
        let pj = entry.payload_json.as_object().expect("payload_json is object");
        assert_eq!(pj.get("rid").and_then(|v| v.as_str()), Some("ask-rid-deny"));
        assert_eq!(
            pj.get("outcome").and_then(|v| v.as_str()),
            Some("deny"),
            "outcome must be 'deny' for user-initiated Deny arm"
        );
    }

    #[test]
    fn emit_permission_ask_resolved_timeout_records_entry() {
        let transcript = sink_with_resolved("ask-rid-timeout", "timeout");
        assert_eq!(transcript.len(), 1);
        let entry = &transcript[0];
        assert_eq!(entry.kind, TranscriptKind::PermissionAskResolved);
        let pj = entry.payload_json.as_object().expect("payload_json is object");
        assert_eq!(
            pj.get("rid").and_then(|v| v.as_str()),
            Some("ask-rid-timeout"),
        );
        assert_eq!(
            pj.get("outcome").and_then(|v| v.as_str()),
            Some("timeout"),
            "outcome must be 'timeout' for the 120s ASK_TIMEOUT arm"
        );
    }

    #[test]
    fn emit_permission_ask_resolved_cancel_records_entry() {
        let transcript = sink_with_resolved("ask-rid-cancel", "cancel");
        assert_eq!(transcript.len(), 1);
        let entry = &transcript[0];
        assert_eq!(entry.kind, TranscriptKind::PermissionAskResolved);
        let pj = entry.payload_json.as_object().expect("payload_json is object");
        assert_eq!(
            pj.get("rid").and_then(|v| v.as_str()),
            Some("ask-rid-cancel"),
        );
        assert_eq!(
            pj.get("outcome").and_then(|v| v.as_str()),
            Some("cancel"),
            "outcome must be 'cancel' for parent-token cancel arm"
        );
    }

    /// The trait default is a no-op for sinks that do NOT override it.
    #[test]
    fn emit_permission_ask_resolved_default_is_noop_on_non_buffer_sink() {
        struct NoopSink;
        impl crate::state::ChatEventSink for NoopSink {
            fn emit_chat_event(&self, _: &ChatEventPayload) {}
            fn emit_tool_call(&self, _: &ToolCallPayload) {}
            fn emit_tool_result(&self, _: &ToolResultPayload) {}
            fn emit_permission_ask(&self, _: PermissionAskPayload) {}
            // emit_permission_ask_resolved: default no-op.
        }
        let sink = NoopSink;
        // Must not panic.
        sink.emit_permission_ask_resolved("rid", "allow");
        sink.emit_permission_ask_resolved("rid", "deny");
        sink.emit_permission_ask_resolved("rid", "timeout");
        sink.emit_permission_ask_resolved("rid", "cancel");
    }

    /// Multiple outcomes for the same rid produce multiple entries
    /// (the sink does NOT deduplicate by rid).
    #[test]
    fn emit_permission_ask_resolved_multiple_outcomes_for_same_rid() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_permission_ask_resolved("same-rid", "allow");
        sink.emit_permission_ask_resolved("same-rid", "deny");
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].kind, TranscriptKind::PermissionAskResolved);
        assert_eq!(transcript[1].kind, TranscriptKind::PermissionAskResolved);
        assert_eq!(
            transcript[0].payload_json.get("outcome").and_then(|v| v.as_str()),
            Some("allow"),
        );
        assert_eq!(
            transcript[1].payload_json.get("outcome").and_then(|v| v.as_str()),
            Some("deny"),
        );
    }
}
