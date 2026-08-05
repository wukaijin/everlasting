//! LLM request / response / event types.
//!
//! Step 2 extends the step 1 types to support Anthropic-style tool calling:
//! - `ContentBlock` for structured message content (text / tool_use / tool_result)
//! - `MessageContent` with custom Serde to accept both plain string and block array
//! - `ToolDef` for declaring tools in the request
//! - `ChatEvent` gains the `ToolCall` variant (tool results are pushed on the
//!   independent `tool:result` IPC channel via `ToolResultPayload`, not as
//!   `ChatEvent` variants — see `state::ChatEventSink::emit_tool_result`)
//!
//! Step 6 (this task) adds extended thinking support:
//! - `ContentBlock::Thinking` and `ContentBlock::RedactedThinking` (Anthropic
//!   extended-thinking content blocks).
//! - `ChatRequest::thinking` accepts an optional `ThinkingConfig` (currently
//!   the `adaptive` variant). When present, the request asks the model to
//!   think before answering.
//! - `ChatEvent::ThinkingDelta`, `ChatEvent::SignatureDelta` and
//!   `ChatEvent::RedactedThinkingDelta` are streamed to the frontend as the
//!   model emits `thinking_delta` / `signature_delta` SSE events and as
//!   `redacted_thinking` content blocks close.

use crate::agent::at_file::InjectionRecord;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------------

/// Conversation role. In the Anthropic Messages API, `tool_result` content
/// blocks are placed inside a `role: "user"` message, so we don't need a
/// separate `Tool` role.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

// ---------------------------------------------------------------------------
// CacheControl — Anthropic prompt cache breakpoint marker
// ---------------------------------------------------------------------------

/// A `cache_control` hint attached to a content block. Anthropic's
/// Messages API reads this field to decide where to put a cache
/// breakpoint — the LAST block in a request that carries this
/// marker is the cache boundary; everything before it becomes
/// eligible for a cache hit on the next turn (within the 5-min
/// TTL).
///
/// The B5 memory refactor (2026-06-11) attaches `Ephemeral` to
/// the first content block of the synthetic "instructions" user
/// message so the 4 instruction files (CLAUDE.md / AGENTS.md ×
/// user / project) are cached on turn 1 and read from cache on
/// turns 2..MAX_TURNS. Without this marker, Anthropic would
/// 100% miss every turn and re-bill the full instructions
/// payload.
///
/// Today only `Ephemeral` exists (5-min TTL, 1.25× write /
/// 0.1× read pricing). A future `Persistent` (1-hour TTL) variant
/// can land here without a schema break — the tagged-enum shape
/// is forward-compatible.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum CacheControl {
    Ephemeral,
}

// ---------------------------------------------------------------------------
// ContentBlock — structured message content
// ---------------------------------------------------------------------------

/// One content block inside a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        /// Optional Anthropic prompt-cache breakpoint. When `Some`,
        /// the wire layer preserves this block as a separate
        /// content block (does NOT concatenate it with adjacent
        /// text blocks) and the Anthropic adapter emits
        /// `cache_control: {"type": "ephemeral"}` next to the
        /// block. See [`CacheControl`] for the cost model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Anthropic extended-thinking content block. `thinking` is the streamed
    /// (or summarized, depending on `display`) summary text the model
    /// produces while reasoning; `signature` is the opaque, encrypted blob
    /// the model emits at the end of the block and which MUST be echoed
    /// back verbatim in subsequent turns — otherwise the API returns 400.
    Thinking { thinking: String, signature: String },
    /// Anthropic `redacted_thinking` block: emitted when the server
    /// encrypts part of a thinking block (e.g. for safety reasons). The
    /// `data` field is opaque, undisplayable, and MUST be echoed back
    /// verbatim in subsequent turns.
    RedactedThinking { data: String },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
    },
}

fn is_false(b: &bool) -> bool {
    !b
}

// ---------------------------------------------------------------------------
// MessageContent — string-or-array wrapper
// ---------------------------------------------------------------------------

/// Message content that serializes as a plain string (step 1 compat) or an
/// array of [`ContentBlock`] (step 2+ tool calling; step 6+ thinking).
#[derive(Debug, Clone, PartialEq)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Extract all *visible* text from this content — used for the
    /// denormalized `text` column in the DB and for the session-list
    /// preview. **Thinking text is intentionally excluded** so that the
    /// sidebar preview only shows user-typed / assistant-said text and the
    /// persisted `text` field stays a useful search/index surface.
    pub fn to_text(&self) -> String {
        match self {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Convenience: create a single-text-block content.
    #[allow(dead_code)]
    pub fn from_text(s: impl Into<String>) -> Self {
        MessageContent::Text(s.into())
    }
}

impl Serialize for MessageContent {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            MessageContent::Text(t) => s.serialize_str(t),
            MessageContent::Blocks(blocks) => blocks.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for MessageContent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(d)?;
        match val {
            serde_json::Value::String(s) => Ok(MessageContent::Text(s)),
            other => {
                let blocks: Vec<ContentBlock> =
                    serde_json::from_value(other).map_err(serde::de::Error::custom)?;
                Ok(MessageContent::Blocks(blocks))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ChatMessage
// ---------------------------------------------------------------------------

/// One message in a conversation. Content can be plain text (backward compat
/// with step 1) or an array of ContentBlocks (tool_use / tool_result /
/// thinking / redacted_thinking).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: MessageContent,
    /// Group chat (07-29-group-chat, 2026-07-31): which participant
    /// authored this message. `None` for classic-chat messages and
    /// for user messages — the classic single-agent path is
    /// unaffected. In a group_chat session this is set on each
    /// assistant turn so the next speaker's model can attribute
    /// prior utterances (互见性). NOT a wire `role` — Anthropic/
    /// OpenAI only accept `user`/`assistant`, so speaker identity
    /// is threaded separately and injected per-provider at the wire
    /// layer (OpenAI `name` field; Anthropic `@name:` prefix).
    ///
    /// `#[serde(default)]` so legacy persisted messages (no
    /// speaker) and existing test fixtures deserialize to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

// ---------------------------------------------------------------------------
// ToolDef — tool declaration for the request
// ---------------------------------------------------------------------------

/// Tool definition sent to the LLM in the request body (Anthropic schema).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

impl ToolDef {
    /// Test-only constructor: builds a `ToolDef` with a name
    /// and a default empty `input_schema`. Used by
    /// `agent::permissions::tests_mode::filter_tools_for_mode_*`
    /// to construct minimal tool lists without going through
    /// the real `tools::builtin_tools()` registry (which would
    /// require `AppState` context).
    #[cfg(test)]
    pub fn new_for_test(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        }
    }
}

// ---------------------------------------------------------------------------
// ThinkingConfig — request-side extended-thinking control
// ---------------------------------------------------------------------------

/// Top-level `thinking` field on a [`ChatRequest`]. The Anthropic Messages
/// API supports several modes; we currently only model `adaptive` (model
/// self-decides how much to think, controlled by `effort`).
///
/// `display: "summarized"` is set explicitly so that `thinking_delta` SSE
/// events actually stream a text summary to the client — with the default
/// `display: "omitted"` on Opus 4.7+ the summary is dropped and the UI
/// would see no thinking text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
    Adaptive { display: String, effort: String },
}

// ---------------------------------------------------------------------------
// ChatRequest
// ---------------------------------------------------------------------------

/// Anthropic Messages API request body.
///
/// NOTE: We intentionally do NOT pre-validate `max_tokens` on the client side
/// (see HACKING-llm.md "差异 3"). The server decides.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    /// When present, the model is asked to think before answering. The
    /// `signature` blobs of any thinking blocks it returns must be echoed
    /// back in subsequent assistant messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
}

// ---------------------------------------------------------------------------
// ChatEvent — events pushed to the frontend
// ---------------------------------------------------------------------------

// One thinking content block. The model can produce multiple blocks per
// turn (interleaved thinking with tool calls); each must be preserved
// in order and round-tripped back to the LLM verbatim, otherwise the
// next turn 400s. `text` is the streamed summary (or empty under
// `display: "omitted"`); `signature` is the opaque, encrypted blob.
// ---------------------------------------------------------------------------
// TokenUsage — A4 (Token Usage Tracking)
// ---------------------------------------------------------------------------

/// Token usage reported by the LLM at the end of one turn. Schema is
/// **protocol-agnostic** — both Anthropic (`message_delta.usage`) and
/// OpenAI (last-chunk `usage`) are normalized into this 4-field shape
/// by the provider before yielding `ChatEvent::Done`. The agent loop
/// reads `Option<TokenUsage>` from `ChatEvent::Done` and accumulates
/// it into `sessions` (per-session totals).
///
/// Field semantics (Anthropic schema — the reference):
///
/// - `input_tokens` — total input tokens for the request, **inclusive
///   of** `cache_creation_input_tokens` + `cache_read_input_tokens`.
///   This is the value displayed in the ChatInput hint as "current
///   context usage, not cumulative" (per Anthropic's statusline
///   convention; same shape as `sanztheo/claude-code-statusline`).
/// - `output_tokens` — tokens generated by the model. **Not** counted
///   as context pressure (the response is not context).
/// - `cache_creation_input_tokens` — newly created cache tokens
///   (eligible for a cache hit next turn).
/// - `cache_read_input_tokens` — cache tokens served from a prior
///   `cache_creation` (the cheap path). The Anthropic API bills
///   these at 0.1x input; OpenAI's `cached_tokens` at 0.5x.
///
/// OpenAI normalization (PR3 of multi-model):
///
/// - `prompt_tokens` → `input_tokens`
/// - `completion_tokens` → `output_tokens`
/// - `prompt_tokens_details.cached_tokens` → `cache_read_input_tokens`
/// - `cache_creation_input_tokens` → 0 (no OpenAI equivalent today)
///
/// `None` from a `ChatEvent::Done` means the provider did not (or
/// could not) report usage — typically cancel, network drop, or
/// pre-usage error. The agent loop treats `None` as "skip the
/// accumulation write" and logs at `info!`.
/// 2026-06-26 (token-usage snapshot fix): `context_input_tokens` is
/// the cross-provider-normalized "total input for this request" —
/// the single canonical numerator the frontend uses for the
/// "context usage %" hint. Provider mapping:
/// - Anthropic: `input_tokens + cache_creation_input_tokens +
///   cache_read_input_tokens` (Anthropic's `input_tokens` excludes
///   cache reads/creations; the true context footprint is the sum).
/// - OpenAI: `prompt_tokens` (already inclusive of
///   `cached_tokens`; OpenAI does not split cache creation out, so
///   no addition is needed — adding `cache_read` would double-count).
///
/// `#[serde(default)]` is required so that legacy
/// `subagent_runs.token_usage_json` rows written before this field
/// existed (4-field shape) still deserialize cleanly (`default = 0`).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
    #[serde(default)]
    pub context_input_tokens: u32,
}

// ---------------------------------------------------------------------------
// ChatEvent — events pushed to the frontend
// ---------------------------------------------------------------------------

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_content_serialize_text_as_string() {
        let mc = MessageContent::Text("hello".to_string());
        let json = serde_json::to_string(&mc).unwrap();
        assert_eq!(json, "\"hello\"");
    }

    #[test]
    fn message_content_deserialize_string() {
        let mc: MessageContent = serde_json::from_str("\"hello\"").unwrap();
        assert_eq!(mc, MessageContent::Text("hello".to_string()));
    }

    #[test]
    fn message_content_serialize_blocks_as_array() {
        let blocks = vec![ContentBlock::Text {
            text: "hi".to_string(),
            cache_control: None,
        }];
        let mc = MessageContent::Blocks(blocks);
        let json = serde_json::to_string(&mc).unwrap();
        assert!(json.starts_with('['));
        assert!(json.contains("\"type\":\"text\""));
    }

    #[test]
    fn message_content_deserialize_blocks() {
        let json = r#"[{"type":"text","text":"hello"}]"#;
        let mc: MessageContent = serde_json::from_str(json).unwrap();
        match mc {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert_eq!(
                    blocks[0],
                    ContentBlock::Text {
                        text: "hello".to_string(),
                        cache_control: None,
                    }
                );
            }
            _ => panic!("expected Blocks"),
        }
    }

    #[test]
    fn chat_message_backward_compat() {
        // Step 1 frontend sends {"role":"user","content":"hi"}
        let msg: ChatMessage = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content, MessageContent::Text("hi".to_string()));

        // Round-trip: serializes back as plain string
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"role":"user","content":"hi"}"#);
    }

    #[test]
    fn chat_message_with_tool_use() {
        let json = r#"{"role":"assistant","content":[
            {"type":"text","text":"let me read that"},
            {"type":"tool_use","id":"toolu_123","name":"read_file","input":{"path":"/etc/hosts"}}
        ]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        match &msg.content {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2);
                assert!(
                    matches!(&blocks[0], ContentBlock::Text { text, .. } if text == "let me read that")
                );
                assert!(
                    matches!(&blocks[1], ContentBlock::ToolUse { name, .. } if name == "read_file")
                );
            }
            _ => panic!("expected Blocks"),
        }
    }

    #[test]
    fn chat_message_with_tool_result() {
        let json = r#"{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu_123","content":"127.0.0.1 localhost"}
        ]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        match &msg.content {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert!(
                    matches!(&blocks[0], ContentBlock::ToolResult { content, is_error, .. }
                    if content == "127.0.0.1 localhost" && !is_error)
                );
            }
            _ => panic!("expected Blocks"),
        }
    }

    #[test]
    fn chat_request_tools_omitted_when_empty() {
        let req = ChatRequest {
            model: "test".to_string(),
            max_tokens: 100,
            messages: vec![],
            system: None,
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("tools"));
        assert!(!json.contains("thinking"));
    }

    #[test]
    fn chat_request_tools_present_when_nonempty() {
        let req = ChatRequest {
            model: "test".to_string(),
            max_tokens: 100,
            messages: vec![],
            system: None,
            stream: true,
            tools: vec![ToolDef {
                name: "read_file".to_string(),
                description: Some("read a file".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            thinking: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"tools\""));
        assert!(json.contains("\"read_file\""));
    }

    #[test]
    fn chat_request_thinking_omitted_when_none() {
        let req = ChatRequest {
            model: "claude-opus-4-7".to_string(),
            max_tokens: 16384,
            messages: vec![],
            system: None,
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("thinking"));
    }

    #[test]
    fn chat_request_thinking_adaptive_serializes_correctly() {
        let req = ChatRequest {
            model: "claude-opus-4-7".to_string(),
            max_tokens: 16384,
            messages: vec![],
            system: None,
            stream: true,
            tools: vec![],
            thinking: Some(ThinkingConfig::Adaptive {
                display: "summarized".to_string(),
                effort: "high".to_string(),
            }),
        };
        let json = serde_json::to_string(&req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let t = v.get("thinking").expect("thinking key present");
        assert_eq!(t.get("type").and_then(|s| s.as_str()), Some("adaptive"));
        assert_eq!(
            t.get("display").and_then(|s| s.as_str()),
            Some("summarized")
        );
        assert_eq!(t.get("effort").and_then(|s| s.as_str()), Some("high"));
    }

    #[test]
    fn message_content_to_text() {
        let blocks = vec![
            ContentBlock::Text {
                text: "hello ".to_string(),
                cache_control: None,
            },
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({}),
            },
            ContentBlock::Text {
                text: "world".to_string(),
                cache_control: None,
            },
        ];
        let mc = MessageContent::Blocks(blocks);
        assert_eq!(mc.to_text(), "hello world");
    }

    // -----------------------------------------------------------------------
    // Thinking block round-trips
    // -----------------------------------------------------------------------

    #[test]
    fn thinking_block_serializes_to_anthropic_schema() {
        let block = ContentBlock::Thinking {
            thinking: "let me think...".to_string(),
            signature: "EqQBCgIYAhIM1gbcDa...".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("type").and_then(|s| s.as_str()), Some("thinking"));
        assert_eq!(
            v.get("thinking").and_then(|s| s.as_str()),
            Some("let me think...")
        );
        assert_eq!(
            v.get("signature").and_then(|s| s.as_str()),
            Some("EqQBCgIYAhIM1gbcDa...")
        );
    }

    #[test]
    fn thinking_block_deserializes_from_anthropic_schema() {
        let json = r#"{"type":"thinking","thinking":"analyze GCD","signature":"abc123"}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(
            block,
            ContentBlock::Thinking {
                thinking: "analyze GCD".to_string(),
                signature: "abc123".to_string(),
            }
        );
    }

    #[test]
    fn redacted_thinking_block_serializes_to_anthropic_schema() {
        let block = ContentBlock::RedactedThinking {
            data: "EmwKAhIM1gbcDa9GJwZA".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.get("type").and_then(|s| s.as_str()),
            Some("redacted_thinking")
        );
        assert_eq!(
            v.get("data").and_then(|s| s.as_str()),
            Some("EmwKAhIM1gbcDa9GJwZA")
        );
    }

    #[test]
    fn redacted_thinking_block_deserializes_from_anthropic_schema() {
        let json = r#"{"type":"redacted_thinking","data":"EmwKAhIM1gbcDa9GJwZA"}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(
            block,
            ContentBlock::RedactedThinking {
                data: "EmwKAhIM1gbcDa9GJwZA".to_string(),
            }
        );
    }

    #[test]
    fn chat_message_round_trip_with_thinking_blocks() {
        // The full assistant turn: text + thinking + tool_use. Must round-trip
        // losslessly so the LLM gets the exact signature back on the next
        // turn (otherwise it 400s).
        let json = r#"{"role":"assistant","content":[
            {"type":"thinking","thinking":"need to read the file","signature":"sig_abc"},
            {"type":"text","text":"OK, reading now"},
            {"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"/etc/hosts"}}
        ]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        // Re-serialize and re-parse: must produce the same blocks.
        let re = serde_json::to_string(&msg).unwrap();
        let msg2: ChatMessage = serde_json::from_str(&re).unwrap();
        assert_eq!(msg, msg2);

        match &msg2.content {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 3);
                assert!(
                    matches!(&blocks[0], ContentBlock::Thinking { thinking, signature }
                    if thinking == "need to read the file" && signature == "sig_abc")
                );
            }
            _ => panic!("expected Blocks"),
        }
    }

    #[test]
    fn chat_message_round_trip_with_redacted_thinking() {
        let json = r#"{"role":"assistant","content":[
            {"type":"redacted_thinking","data":"EmwKAhIM1gbcDa9GJwZA"},
            {"type":"text","text":"answer"}
        ]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        let re = serde_json::to_string(&msg).unwrap();
        let msg2: ChatMessage = serde_json::from_str(&re).unwrap();
        assert_eq!(msg, msg2);
    }

    #[test]
    fn message_content_to_text_excludes_thinking() {
        // Thinking text must NOT leak into the denormalized `text` column
        // (DB text is used for sidebar previews / search).
        let blocks = vec![
            ContentBlock::Thinking {
                thinking: "secret thought".to_string(),
                signature: "sig".to_string(),
            },
            ContentBlock::Text {
                text: "visible answer".to_string(),
                cache_control: None,
            },
            ContentBlock::RedactedThinking {
                data: "redacted".to_string(),
            },
        ];
        let mc = MessageContent::Blocks(blocks);
        assert_eq!(mc.to_text(), "visible answer");
    }

    #[test]
    fn chat_event_thinking_delta_serializes_with_snake_case_kind() {
        let ev = ChatEvent::ThinkingDelta {
            text: "analyzing".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.get("kind").and_then(|s| s.as_str()),
            Some("thinking_delta")
        );
        assert_eq!(v.get("text").and_then(|s| s.as_str()), Some("analyzing"));
    }

    #[test]
    fn chat_event_signature_delta_serializes_with_snake_case_kind() {
        let ev = ChatEvent::SignatureDelta {
            signature: "sig_xyz".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.get("kind").and_then(|s| s.as_str()),
            Some("signature_delta")
        );
        assert_eq!(v.get("signature").and_then(|s| s.as_str()), Some("sig_xyz"));
    }

    #[test]
    fn chat_event_redacted_thinking_delta_serializes_with_snake_case_kind() {
        let ev = ChatEvent::RedactedThinkingDelta {
            data: "redacted_blob".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.get("kind").and_then(|s| s.as_str()),
            Some("redacted_thinking_delta")
        );
        assert_eq!(
            v.get("data").and_then(|s| s.as_str()),
            Some("redacted_blob")
        );
    }

    // -----------------------------------------------------------------------
    // A4 — TokenUsage
    // -----------------------------------------------------------------------

    #[test]
    fn token_usage_serializes_with_snake_case_fields() {
        // The IPC payload crosses the Tauri boundary camelCase by
        // default, but the inner JSON object keeps snake_case for
        // these field names (the outer `kind` discriminator and the
        // inner `stop_reason` are both snake_case in the existing
        // `ChatEvent::Done` shape — see backend/llm-contract.md
        // §"Scenario: Token Usage Tracking" §3).
        let u = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 20,
            context_input_tokens: 130,
        };
        let json = serde_json::to_string(&u).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("input_tokens"), Some(&serde_json::json!(100)));
        assert_eq!(v.get("output_tokens"), Some(&serde_json::json!(50)));
        assert_eq!(
            v.get("cache_creation_input_tokens"),
            Some(&serde_json::json!(10))
        );
        assert_eq!(
            v.get("cache_read_input_tokens"),
            Some(&serde_json::json!(20))
        );
        // 2026-06-26 snapshot fix: the new `context_input_tokens`
        // field MUST serialize as snake_case (no rename attribute
        // on the struct, but lock the contract explicitly since
        // the field is the canonical frontend "% of context_window"
        // numerator — a rename here would silently break the UI).
        assert_eq!(v.get("context_input_tokens"), Some(&serde_json::json!(130)));
    }

    #[test]
    fn token_usage_default_is_all_zero() {
        // The DB's `UPDATE col = col + ?` path doesn't see a
        // default-zero, but the `Done { usage: None }` -> no-op
        // path means the chat command never constructs a
        // default-zero usage. This test just locks the contract
        // that `Default::default() == TokenUsage { 0, 0, 0, 0 }`.
        let u = TokenUsage::default();
        assert_eq!(u.input_tokens, 0);
        assert_eq!(u.output_tokens, 0);
        assert_eq!(u.cache_creation_input_tokens, 0);
        assert_eq!(u.cache_read_input_tokens, 0);
        assert_eq!(u.context_input_tokens, 0);
    }

    #[test]
    fn token_usage_deserializes_legacy_4_field_json_with_default_context() {
        // 2026-06-26 snapshot fix (PRD decision D6): legacy
        // `subagent_runs.token_usage_json` rows written before the
        // `context_input_tokens` field existed carry only the four
        // original fields. The `#[serde(default)]` attribute on
        // `context_input_tokens` MUST make this deserialize cleanly
        // (defaulting to 0) rather than erroring — otherwise a
        // single pre-snapshot worker row would break
        // `SubagentDrawer`'s expand UI on every page load.
        let legacy_json = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_creation_input_tokens": 10,
            "cache_read_input_tokens": 20
        }"#;
        let u: TokenUsage = serde_json::from_str(legacy_json).expect(
            "legacy 4-field JSON must deserialize (#[serde(default)] on context_input_tokens)",
        );
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
        assert_eq!(u.cache_creation_input_tokens, 10);
        assert_eq!(u.cache_read_input_tokens, 20);
        assert_eq!(
            u.context_input_tokens, 0,
            "missing field defaults to 0 (not an error)"
        );
    }

    #[test]
    fn chat_event_done_carries_usage_payload() {
        // The A4 wire shape: `Done { stop_reason, usage }`. The
        // `usage` field is serialized with the inner `kind` tag
        // already supplied by the outer enum, so the payload
        // looks like:
        //
        //   { "kind": "done", "stop_reason": "end_turn",
        //     "usage": { "input_tokens": 100, ... } }
        //
        // when `Some`, and `usage: null` (or absent — we use
        // `skip_serializing_if` below for compact payloads) when
        // `None`. The agent loop checks `Some(t) => accumulate`,
        // `None => skip`.
        let ev = ChatEvent::Done {
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 25,
                context_input_tokens: 125,
            }),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("kind").and_then(|s| s.as_str()), Some("done"));
        assert_eq!(
            v.get("stop_reason").and_then(|s| s.as_str()),
            Some("end_turn")
        );
        let usage = v.get("usage").expect("usage key present");
        assert_eq!(usage.get("input_tokens"), Some(&serde_json::json!(100)));
        assert_eq!(
            usage.get("cache_read_input_tokens"),
            Some(&serde_json::json!(25))
        );
    }

    #[test]
    fn chat_event_done_with_none_usage_emits_null() {
        // Cancel / error / network drop path: usage is None.
        // The agent loop's `if let Some(t) = event.usage` check
        // skips accumulation, so the None case must be
        // distinguishable from `Some(TokenUsage::default())`
        // (which would otherwise be a no-op write, but it's
        // wasteful — we should be able to skip the SQL
        // round-trip).
        let ev = ChatEvent::Done {
            stop_reason: Some("cancelled".to_string()),
            usage: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("kind").and_then(|s| s.as_str()), Some("done"));
        // `usage` is present as JSON null (not absent) so the
        // frontend's TypeScript side can rely on the key being
        // there. (`serde(tag = "kind")` does not skip None
        // fields by default.)
        assert!(v.get("usage").map(|x| x.is_null()).unwrap_or(false));
    }

    // -----------------------------------------------------------------------
    // B2 PR3: InjectionRecord / ChatEvent::FileInjections wire shape
    // -----------------------------------------------------------------------

    /// Verify `InjectionRecord` serializes with the exact shape the
    /// frontend's `InjectionEntry` discriminated union expects:
    /// `{ path: string, action: { kind: 'injected'|'degraded'|'skipped', ... } }`.
    /// The metadata-persist path serializes via `serde_json::Value`
    /// and the rehydrate path decodes back into `InjectionRecord` —
    /// round-trip through both `String` and `Value` is verified.
    #[test]
    fn b2_pr3_injection_record_wire_shape() {
        use crate::agent::at_file::{FileKind, InjectionAction, InjectionRecord, SkipReason};
        let records = vec![
            InjectionRecord {
                path: "src/foo.ts".to_string(),
                action: InjectionAction::Injected { lines: 48 },
            },
            InjectionRecord {
                path: "bar.png".to_string(),
                action: InjectionAction::Degraded {
                    file_kind: FileKind::Image,
                },
            },
            InjectionRecord {
                path: "doc.pdf".to_string(),
                action: InjectionAction::Degraded {
                    file_kind: FileKind::Pdf,
                },
            },
            InjectionRecord {
                path: "doc.docx".to_string(),
                action: InjectionAction::Degraded {
                    file_kind: FileKind::Office,
                },
            },
            InjectionRecord {
                path: "x.zip".to_string(),
                action: InjectionAction::Degraded {
                    file_kind: FileKind::Binary,
                },
            },
            InjectionRecord {
                path: "missing.txt".to_string(),
                action: InjectionAction::Skipped {
                    reason: SkipReason::Missing,
                },
            },
            InjectionRecord {
                path: "../../etc/passwd".to_string(),
                action: InjectionAction::Skipped {
                    reason: SkipReason::OutOfRoot,
                },
            },
            InjectionRecord {
                path: "/etc/shadow".to_string(),
                action: InjectionAction::Skipped {
                    reason: SkipReason::Unreadable,
                },
            },
        ];
        let json = serde_json::to_string(&records).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = v.as_array().unwrap();
        // Injected: kind=injected, has `lines`.
        assert_eq!(arr[0]["path"], "src/foo.ts");
        assert_eq!(arr[0]["action"]["kind"], "injected");
        assert_eq!(arr[0]["action"]["lines"], 48);
        // Degraded: kind=degraded, has `file_kind` (snake_case enum).
        assert_eq!(arr[1]["action"]["kind"], "degraded");
        assert_eq!(arr[1]["action"]["file_kind"], "image");
        assert_eq!(arr[2]["action"]["file_kind"], "pdf");
        assert_eq!(arr[3]["action"]["file_kind"], "office");
        assert_eq!(arr[4]["action"]["file_kind"], "binary");
        // Skipped: kind=skipped, has `reason` (snake_case enum).
        assert_eq!(arr[5]["action"]["kind"], "skipped");
        assert_eq!(arr[5]["action"]["reason"], "missing");
        assert_eq!(arr[6]["action"]["reason"], "out_of_root");
        assert_eq!(arr[7]["action"]["reason"], "unreadable");
        // Round-trip via `String` (the IPC JSON path).
        let decoded: Vec<InjectionRecord> = serde_json::from_str(&json).unwrap();
        assert_eq!(records, decoded);
        // Round-trip via `serde_json::Value` (the
        // `update_message_metadata` persist path: `to_value`
        // → `Value::String` for the SQL column → `from_str` on
        // reload).
        let meta = serde_json::to_value(&records).unwrap();
        let meta_back: Vec<InjectionRecord> = serde_json::from_value(meta).unwrap();
        assert_eq!(records, meta_back);
    }

    /// Verify `ChatEvent::FileInjections` wire shape — the frontend
    /// `case "file_injections"` arm reads `event.message_seq` and
    /// `event.injections` off the IPC payload, then `msgs.find`
    /// patches the user message's `injections` array.
    #[test]
    fn b2_pr3_chat_event_file_injections_wire_shape() {
        use crate::agent::at_file::{InjectionAction, InjectionRecord};
        let ev = ChatEvent::FileInjections {
            request_id: "rid123".to_string(),
            message_seq: 42,
            injections: vec![InjectionRecord {
                path: "foo.txt".to_string(),
                action: InjectionAction::Injected { lines: 12 },
            }],
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // The `kind` discriminator is snake_case from the enum tag.
        assert_eq!(v["kind"], "file_injections");
        // The other 3 fields are top-level on the JSON object.
        assert_eq!(v["request_id"], "rid123");
        assert_eq!(v["message_seq"], 42);
        assert_eq!(v["injections"][0]["path"], "foo.txt");
        assert_eq!(v["injections"][0]["action"]["kind"], "injected");
        assert_eq!(v["injections"][0]["action"]["lines"], 12);
    }

    /// A5+ (2026-07-04, R8): verify `ChatEvent::Retrying` wire shape.
    /// The frontend `streamController` `case 'retrying'` arm reads
    /// `attempt` / `max_attempts` / `wait_ms` / `reason` off the IPC
    /// payload. The `kind` discriminator is snake_case (`retrying`)
    /// from the enum-level `#[serde(rename_all = "snake_case")]` tag.
    #[test]
    fn a5plus_chat_event_retrying_wire_shape() {
        let ev = ChatEvent::Retrying {
            attempt: 2,
            max_attempts: 3,
            wait_ms: 1500,
            reason: "服务器错误 (HTTP 503)".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["kind"], "retrying");
        assert_eq!(v["attempt"], 2);
        assert_eq!(v["max_attempts"], 3);
        assert_eq!(v["wait_ms"], 1500);
        assert_eq!(v["reason"], "服务器错误 (HTTP 503)");
    }
}
