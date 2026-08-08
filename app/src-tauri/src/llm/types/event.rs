//! `ChatEvent`, `RecallHit`, and `LlmErrorCategory` — streaming event types.
//!
//! Split out of `llm/types.rs` (2026-08-08 batch3). These three are grouped
//! together because `ChatEvent::Error` references [`LlmErrorCategory`] and
//! `ChatEvent::Recall` references [`RecallHit`].

use crate::agent::at_file::InjectionRecord;
use serde::{Deserialize, Serialize};

use super::usage::TokenUsage;

// ---------------------------------------------------------------------------
// LlmErrorCategory
// ---------------------------------------------------------------------------

/// Stable string identifiers for [`crate::error::LlmError`] variants, safe to
/// embed in IPC payloads.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LlmErrorCategory {
    Auth,
    RateLimit,
    InvalidRequest,
    Server,
    Network,
}

// ---------------------------------------------------------------------------
// RecallHit
// ---------------------------------------------------------------------------

/// One hit row in [`ChatEvent::Recall`]. Mirrors the
/// `MemoryRow` fields the management modal needs to render a
/// per-hit chip + the recall-event list on the main panel. The
/// `title` / `kind` are the only fields the frontend renders;
/// the `memory_id` is the join key for the modal's
/// `update_autonomous_memory_status` / `update_autonomous_memory`
/// actions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecallHit {
    pub memory_id: String,
    pub title: String,
    pub kind: String,
    /// "fts" (session-start FTS5 recall) or "pitfall"
    /// (pre-tool pitfall recall). Drives the chip's group label
    /// on the main panel.
    pub source: String,
}

// ---------------------------------------------------------------------------
// ChatEvent — events pushed to the frontend
// ---------------------------------------------------------------------------

// One thinking content block. The model can produce multiple blocks per
// turn (interleaved thinking with tool calls); each must be preserved
// in order and round-tripped back to the LLM verbatim, otherwise the
// next turn 400s. `text` is the streamed summary (or empty under
// `display: "omitted"`); `signature` is the opaque, encrypted blob.

/// What we push to the frontend over the Tauri event channel. Tagged by
/// `kind`, keeps the frontend state machine simple.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    /// Stream started. Frontend can show a "thinking…" indicator.
    Start,
    /// 08-04 group-chat follow-up: the orchestrator
    /// (`run_group_chat_loop`) emits this right before each inner
    /// speaker turn so the frontend knows whose placeholder is about
    /// to stream. The per-speaker wire events (`Delta` / `Done`) do
    /// NOT carry the speaker; the frontend stashes this value and
    /// stamps it on the placeholder's `speaker` field (renders the
    /// speaker chip live). Never emitted by a provider — the agent
    /// loop's per-event stream match drops it defensively.
    Speaker { speaker: String },
    /// Incremental text from the model.
    Delta { text: String },
    /// Incremental thinking summary from the model. Streamed via
    /// `thinking_delta` SSE events when `display: "summarized"` is set.
    ThinkingDelta { text: String },
    /// Opaque signature blob emitted at the end of a thinking block
    /// (via `signature_delta`). The frontend must keep this so it can be
    /// round-tripped to the LLM on the next turn.
    SignatureDelta { signature: String },
    /// Opaque `redacted_thinking.data` payload. Emitted once when a
    /// `redacted_thinking` content block closes. The frontend must keep
    /// this so it can be round-tripped to the LLM on the next turn; the
    /// payload is not displayable.
    RedactedThinkingDelta { data: String },
    /// LLM requested a tool call. Emitted once per tool_use block when the
    /// block is fully assembled (content_block_stop for tool_use type).
    /// Emitted on the independent `tool:call` event channel.
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Stream finished cleanly. Includes Anthropic `stop_reason` if present.
    /// `usage` is the A4 token-usage payload (normalized across protocols
    /// — see [`TokenUsage`] for the schema). `None` means the stream
    /// ended without a usage report (cancel / error / network drop);
    /// the agent loop skips the per-session accumulation write in
    /// that case.
    Done {
        stop_reason: Option<String>,
        usage: Option<TokenUsage>,
    },
    /// F5 follow-up per-turn: emitted ONCE per LLM turn (right after
    /// `persist_turn` for the assistant row). Carries the per-turn
    /// latency triple plus the thinking-phase wall clock, plus the
    /// assistant row's `seq` (assigned by the agent loop) so the
    /// frontend can key its `latencyByTurn: Map<turnIndex, TurnLatency>`
    /// and the reload `update_message_latency` IPC can fire N times
    /// per request instead of 1. All four ms fields are `Option`
    /// because a turn may have been cut before reaching the
    /// corresponding boundary (e.g. thinking-only → tool_call with
    /// no text delta; no `gen_ms` / no `ttfb_ms`).
    TurnComplete {
        /// Assistant row's `seq` (assigned by the agent loop in
        /// `agent/chat.rs` from the per-session `next_seq` counter).
        /// The frontend `update_message_latency` IPC takes this `seq`
        /// to find the row to UPDATE.
        seq: i64,
        /// First `delta` → turn `send_at` (i.e. when the LLM started
        /// returning content). `None` when no text delta arrived
        /// before the turn's `done`.
        ttfb_ms: Option<i64>,
        /// `done` (or non-thinking boundary) → first `delta`. `None`
        /// when no text delta arrived.
        gen_ms: Option<i64>,
        /// `done` (or non-thinking boundary) → turn `send_at`. Always
        /// `Some` for a turn that reached `persist_turn` (the turn
        /// must have started).
        total_ms: Option<i64>,
        /// First non-thinking boundary → first `thinking_delta` (the
        /// thinking-phase wall clock). `None` when the turn never
        /// entered the thinking phase (no `thinking_delta` SSE).
        thinking_ms: Option<i64>,
    },
    /// Stream errored. `category` maps to [`LlmErrorCategory`] strings.
    Error {
        message: String,
        category: LlmErrorCategory,
    },
    /// A5+ (2026-07-04, R8): emitted before each retry backoff sleep so
    /// the frontend can surface "↩ retrying 2/3, 2s … (reason)" instead
    /// of looking frozen. Transient UX signal — does NOT enter the DB
    /// (no AuditKind, no per-turn persist). The agent loop's
    /// `LlmRetrySink` forwards this through the regular `chat-event`
    /// channel alongside the other variants. Frontend display attaches
    /// the row to the in-flight assistant placeholder's transient
    /// `retrying` field (NOT the `messages` array, to avoid polluting
    /// the persisted history); the next `delta` / `start` / `done`
    /// clears it.
    ///
    /// Fields mirror Rust `llm::retry::RetryingEvent`:
    /// - `attempt` — 1-indexed (1 = first retry after the initial fail).
    /// - `max_attempts` — `RetryPolicy.max_retries` (the ceiling).
    /// - `wait_ms` — the computed backoff (Full Jitter or honored
    ///   server advisory), already clamped to the budget remaining.
    /// - `reason` — `LlmError::user_message()` (Chinese, user-facing).
    Retrying {
        attempt: u32,
        max_attempts: u32,
        wait_ms: u64,
        reason: String,
    },
    /// B2 PR3: per-token `@relpath` injection manifest, emitted
    /// once per user turn (right after `inject_at_tokens` runs on
    /// the last user message). The frontend `streamController`
    /// patches the matching user message's `injections` array so
    /// the hint row under the bubble renders. The same manifest
    /// is also written to `messages.metadata` (see
    /// `db::persist_turn` caller in `chat_loop.rs`) so the hint
    /// survives session reload.
    ///
    /// `request_id` + `message_seq` identify the target user
    /// message on the frontend (the controller's user messages
    /// are keyed by id, but on reload the id is `${sid}-${seq}`
    /// — `message_seq` lets the controller locate the row
    /// without a separate per-request `userMsgId` plumbing).
    FileInjections {
        request_id: String,
        message_seq: i64,
        injections: Vec<InjectionRecord>,
    },
    /// 07-06 (am-observability-panel, R2b / AC6): emitted when
    /// the agent loop's recall path surfaces one or more
    /// autonomous memories on the way to the LLM. The variant is
    /// **read-only / non-persistent** (like `Retrying`) — the
    /// `hits` are an observation, not an LLM request. The
    /// frontend uses it to drive the "本次召回" chip in the
    /// ChatPanel header so the user can see what the agent
    /// recalled (and was influenced by) during this turn.
    ///
    /// Two recall sources share this event:
    /// - `source: "fts"` — session-start FTS5 recall
    ///   (`agent::memory_recall::build_recall_text_with_rows`).
    /// - `source: "pitfall"` — pre-tool pitfall recall
    ///   (`permissions::recall_pitfall_with_hits`).
    ///
    /// Worker subagents (B6) **do not** propagate this event to
    /// the main chat — the `SubagentBufferSink` does not forward
    /// `chat-event` to the IPC channel (same isolation as
    /// `Retrying` / `Error` / `Done`; design D4). The worker's
    /// own recall still happens (it surfaces memories to the
    /// worker's LLM via `inject_recall_into_turn`), but the
    /// main chat's `lastRecallHits` chip is unaffected — AC7.
    Recall { hits: Vec<RecallHit> },
    /// E2 trace (2026-07-14): C3 context compaction observation.
    /// Emitted always-on (live panel) + persisted to
    /// `turn_trace.compaction_json` (回看). Write point:
    /// `chat_loop.rs` after `compact_messages` returns (both the
    /// normal compaction branch and the `StillOver` error branch).
    /// `degradation` is the `DegradationKind::as_str()` string
    /// (`"none"` / `"no_candidates"` / `"still_over"`).
    ContextCompacted {
        seq: i64,
        tokens_before: u32,
        tokens_after: u32,
        dropped_count: u32,
        degradation: String,
    },
    /// E2 trace (2026-07-14): C2 loop-detection soft hint (1-2
    /// consecutive hits, below the ≥3 active-intervention threshold).
    /// Emitted always-on + persisted to `turn_trace.loop_hint_json`.
    /// `verdict_kind` is `"hard"` or `"soft"`. The ≥3 intervention
    /// path already writes `loop_intervention` audit rows; this
    /// variant covers the pre-intervention soft-hint turns only.
    LoopHint {
        seq: i64,
        hit_count: u32,
        verdict_kind: String,
    },
    /// E2 trace (2026-07-14): per-turn workflow breadcrumb snapshot.
    /// Emitted always-on + persisted to `turn_trace.breadcrumb_json`.
    /// Write point: `chat_loop.rs` after `append_workflow_breadcrumb`
    /// (the trace call lives in chat_loop, not in inject.rs, so it
    /// has access to `seq` + `db` + `sink`). `task_slug` / `status`
    /// are `None` when there is no active workflow task.
    WorkflowBreadcrumb {
        seq: i64,
        task_slug: Option<String>,
        status: Option<String>,
        breadcrumb_text: String,
    },
}
