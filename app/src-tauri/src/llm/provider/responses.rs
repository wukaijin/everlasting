//! OpenAI Responses API provider (task 09-18-openai-responses-provider,
//! PR1). Third protocol alongside [`anthropic`] (Messages API) and
//! [`openai`] (Chat Completions).
//!
//! The Responses API (`POST /v1/responses`) is OpenAI's primary path
//! (Assistants API sunset 2026-08) and is natively supported by vLLM /
//! LiteLLM gateways. The HTTP + SSE + error-classification skeleton is
//! lifted from [`openai`]; the protocol-specific halves that differ are
//! (a) the request body shape and (b) the typed SSE event machine.
//!
//! Wire-behavior contract (research.md §2 + design.md §2.3-§2.5):
//!
//! | Concern | Chat Completions (`openai.rs`) | Responses (this module) |
//! |---------|-------------------------------|--------------------------|
//! | URL | `{base}/chat/completions` (base includes `/v1`) | `{base}/responses` (same base_url convention) |
//! | system | first `role: "system"` message | top-level `instructions` field |
//! | messages | `messages: [{role, content}]` | `input`: item array (full replay) |
//! | output cap | `max_tokens` / `max_completion_tokens` | `max_output_tokens` |
//! | reasoning | top-level `reasoning_effort` string | `reasoning: {effort, summary}` object |
//! | state | stateless | `store` (defaults **true**!) + `previous_response_id` — we pin `store: false` |
//! | tool decl | nested `{type: "function", function: {…}}` | flat `{type: "function", name, description, parameters, strict}` |
//! | tool call | `tool_calls[]` on the assistant message | independent `function_call` item, keyed by `call_id` |
//! | tool result | `role: "tool"` message | `function_call_output` item (same `call_id`) |
//! | text delta | `choices[0].delta.content` | `response.output_text.delta` |
//! | reasoning | `delta.reasoning_content` | `response.reasoning_summary_text.delta` (summary only) |
//! | finish | `choices[0].finish_reason` + `data: [DONE]` | **none** — stop_reason is synthesized (§2.5) |
//! | images | `image_url: {url}` part | `{type: "input_image", image_url: "data:…", detail}` part |
//! | stream format | data-only SSE | typed SSE (`event:` line + `data.type`) |
//!
//! **Stateless full replay**: the agent loop replays the whole history
//! every turn, which is exactly Responses' manual-replay mode. We never
//! send `previous_response_id` and always send `store: false` — no
//! server-side conversation state, maximal third-party gateway
//! compatibility, no data residency (research §4.1).
//!
//! **Cross-protocol strip** (AC4): an Anthropic session switched to a
//! Responses model runs `chat_request_to_wire → strip_unsupported(responses
//! caps) → build_http_body`. Two layers cooperate (review.md correction 1):
//! the strip pass drops `Signature` / `RedactedThinking` blobs
//! (`supports_thinking_signatures: false`), while **surviving** `Reasoning`
//! blocks (strip's `block_supported` is OR semantics — kept whenever
//! `reasoning.effort` is set) are dropped here in `build_http_body`'s
//! defensive skip. Unlike the Chat Completions path (RULE-D-006 replays
//! them as `reasoning_content`), Responses discards them — MVP does not
//! round-trip reasoning items (research §4.2; P3 follow-up).
//!
//! Implementation notes:
//!
//! - `ResponsesConfig` is intentionally NOT `OpenAIConfig`. The field
//!   sets coincide today, but the two protocols evolve independently
//!   (Responses will grow `include` / summary / store-related knobs);
//!   sharing the struct would couple their evolution.
//! - Speaker attribution (group chat): Responses' input messages have
//!   **no `name` field** (unlike Chat Completions), so the speaker is
//!   inlined as an `@name: ` text prefix — the exact format of
//!   `anthropic.rs`'s `apply_speaker_prefix` (review.md correction 3).
//!   Group-chat mixing stays format-stable across both protocols.
//! - Tool-call arguments truncated by `max_output_tokens` produce
//!   incomplete JSON at `output_item.done`; we degrade to
//!   `Value::String(raw)` + `warn!` instead of dropping the call
//!   (review.md correction 6 — pairs with `incomplete → max_tokens`).
//! - Refusal content parts are converted to visible `Delta`s so a
//!   refusal never produces a zero-text assistant bubble (review.md
//!   correction 5).

use async_stream::stream;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use super::streaming::parse_responses_usage;
use super::wire::{
    self, chat_request_to_wire, strip_unsupported, WireBlock, WireCapabilities, WireMessage,
    WireRequest,
};
use super::{Provider, ProviderCapabilities, ProviderProtocol};
use crate::llm::error::classify_error_response;
use crate::llm::sse::{utf8_chunk_text, SseParser};
use crate::llm::types::{ChatEvent, ChatMessage, TokenUsage, ToolDef};
use crate::llm::LlmError;

// ---------------------------------------------------------------------------
// ResponsesConfig — module-private construction via `build_provider`
// ---------------------------------------------------------------------------

/// Configuration for the OpenAI Responses adapter. Constructed by
/// `build_provider` from a `ProviderRow` + `ModelRow`. Deliberately a
/// standalone struct (not a reuse of [`super::openai::OpenAIConfig`]):
/// the shapes coincide today, but the two protocols evolve
/// independently — Responses-specific request knobs (`include`,
/// reasoning `summary` modes, store-related parameters) would force
/// dead fields onto the Chat Completions side and vice versa.
#[derive(Debug, Clone)]
pub struct ResponsesConfig {
    /// MUST include the API version prefix (e.g.
    /// `https://api.openai.com/v1`) — same convention as the `openai`
    /// protocol entries. [`ResponsesConfig::endpoint`] only appends
    /// `/responses` (never re-adds `/v1`, see the 06-09 `/v1/v1` fix).
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    /// Emitted as `max_output_tokens`. On reasoning models this budget
    /// covers reasoning tokens + visible answer combined — an exhausted
    /// budget surfaces as `response.incomplete` with
    /// `incomplete_details.reason == "max_output_tokens"`.
    pub max_tokens: u32,
    /// Sourced from `ModelRow.thinking_effort`. `None` (or a value
    /// outside the vocabulary after normalization) means "do not emit
    /// the `reasoning` object". Values are normalized to the Responses
    /// vocabulary (`minimal|low|medium|high`) by
    /// [`normalize_responses_effort`] — DB rows may carry
    /// Anthropic/DeepSeek vocabulary (`xhigh` / `max`).
    pub reasoning_effort: Option<String>,
    /// B1 (2026-08-16): from `ModelRow.supports_images` — gates the
    /// wire strip's image→text-placeholder degradation. When true,
    /// `build_http_body` emits `input_image` parts for user images.
    pub supports_images: bool,
}

impl ResponsesConfig {
    /// Trim trailing `/` from `base_url` and append the Responses
    /// endpoint. The base_url MUST already include `/v1` — same
    /// convention as the `openai` protocol (06-09 fix taught us never
    /// to re-add the version prefix; see `openai.rs` `endpoint()`).
    pub fn endpoint(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }
}

// ---------------------------------------------------------------------------
// ResponsesProvider
// ---------------------------------------------------------------------------

/// OpenAI Responses streaming adapter. Implements [`Provider`].
///
/// One provider is constructed per chat invocation (one for the
/// 20-turn agent loop); `send` is called once per turn.
pub struct ResponsesProvider {
    config: ResponsesConfig,
}

impl ResponsesProvider {
    pub fn new(config: ResponsesConfig) -> Self {
        Self { config }
    }

    /// The HTTP + SSE body for one Responses request. Pure function
    /// over the (post-strip) [`WireRequest`] so the conversion is
    /// testable without a real HTTP client.
    pub(crate) fn build_http_body(wire: &WireRequest, config: &ResponsesConfig) -> Value {
        // 1. `input` item array — full history replay, relative order
        //    preserved. See the per-variant mapping below.
        let mut items: Vec<Value> = Vec::new();
        for m in &wire.messages {
            match m {
                // Group chat (07-29-group-chat): Responses' input
                // messages have no `name` field, so the speaker is
                // inlined as an `@name: ` text prefix — the exact
                // format of anthropic.rs `apply_speaker_prefix`.
                // `None` (classic chat) → verbatim text.
                WireMessage::User { content, speaker } => {
                    items.push(json!({
                        "role": "user",
                        "content": prefix_speaker(speaker, content),
                    }));
                }
                WireMessage::UserBlocks { blocks } => {
                    // Block-shaped user message (B5 cache_control
                    // marker). Responses natively accepts a content
                    // parts array, so unlike the Chat Completions
                    // adapter we do NOT flatten to a string — text
                    // parts and image parts can coexist. `cache_control`
                    // has no Responses equivalent and is dropped.
                    let mut parts: Vec<Value> = Vec::new();
                    for b in blocks {
                        match b {
                            WireBlock::Text { text, .. } => {
                                if !text.is_empty() {
                                    parts.push(json!({ "type": "input_text", "text": text }));
                                }
                            }
                            WireBlock::Image { media_type, data } => {
                                parts.push(json!({
                                    "type": "input_image",
                                    "image_url": format!("data:{};base64,{}", media_type, data),
                                    // `detail` controls the vision
                                    // encoder's crop behavior; "auto"
                                    // lets the upstream decide (same
                                    // default as the official SDKs).
                                    "detail": "auto",
                                }));
                            }
                            // Defensive: user-role UserBlocks only
                            // carry text + image at this point
                            // (tool_results were lifted into
                            // WireMessage::Tool by the forward pass).
                            // Skip anything else rather than crash.
                            _ => {}
                        }
                    }
                    items.push(json!({ "role": "user", "content": parts }));
                }
                WireMessage::Assistant { blocks, speaker } => {
                    // One assistant message item (joined text, speaker
                    // prefixed) followed by one independent
                    // `function_call` item per ToolUse block — order:
                    // text first, then calls (research §2.2; Responses
                    // models tool calls as items, NOT as a
                    // `tool_calls[]` array on the message).
                    items.extend(assistant_blocks_to_responses_items(blocks, speaker));
                }
                WireMessage::Tool {
                    tool_call_id,
                    content,
                    images,
                } => {
                    // R4: `function_call_output.output` is string-only —
                    // tool-result images degrade to per-image notice
                    // lines prepended to the output (same wording as
                    // the Chat Completions adapter; the strip pass
                    // already handled the supports_images=false case,
                    // this arm covers vision models on an
                    // openai_responses provider).
                    let output = if images.is_empty() {
                        content.clone()
                    } else {
                        let notices: Vec<String> = images
                            .iter()
                            .map(|img| {
                                format!(
                                    "[image: {} — 当前接口不支持工具结果图片，未发送]",
                                    img.media_type
                                )
                            })
                            .collect();
                        format!("{}\n{}", notices.join("\n"), content)
                    };
                    // The call_id (not the item `id`) is what associates
                    // the output with its `function_call` item.
                    items.push(json!({
                        "type": "function_call_output",
                        "call_id": tool_call_id,
                        "output": output,
                    }));
                }
            }
        }

        // 2. tools — FLAT function declarations (no `function: {…}`
        //    envelope; that nesting is Chat Completions-only).
        //    `strict: false` is explicit: our `input_schema` is
        //    Anthropic-style free-form and does not satisfy the strict
        //    schema contract (all-fields-required +
        //    additionalProperties: false). Omitting `strict` would let
        //    the upstream attempt strict mode and silently fall back —
        //    an explicit false makes the behavior deterministic
        //    (research §4.4).
        let tools: Vec<Value> = wire
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description.clone().unwrap_or_default(),
                    "parameters": t.input_schema,
                    "strict": false,
                })
            })
            .collect();

        // 3. Top-level body. `store: false` is load-bearing: the
        //    Responses API stores responses server-side for 30 days by
        //    DEFAULT, and `previous_response_id` chaining only works
        //    against stored responses. We are stateless by design
        //    (research §4.1) — never store, never chain.
        let mut body = json!({
            "model": config.model,
            "input": items,
            "max_output_tokens": config.max_tokens,
            "store": false,
            "stream": true,
        });
        // The system prompt rides the top-level `instructions` field
        // (Chat Completions carries it as a `role: "system"` message;
        // Anthropic as a top-level `system`). Omitted entirely when
        // absent.
        if let Some(sys) = wire.system.as_deref() {
            body["instructions"] = json!(sys);
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        // Reasoning object — only when the model row opted into
        // reasoning AND the effort survives vocabulary normalization.
        // `summary: "auto"` is required for the upstream to stream
        // `reasoning_summary_text` events (our ThinkingDelta source);
        // Responses only ever sends reasoning SUMMARIES, never raw CoT.
        if let Some(effort) = normalize_responses_effort(config.reasoning_effort.as_deref()) {
            body["reasoning"] = json!({ "effort": effort, "summary": "auto" });
        }
        body
    }
}

/// Inline the group-chat speaker as an `@name: ` prefix (the exact
/// format of anthropic.rs `apply_speaker_prefix`, review.md correction
/// 3 — NOT the bare `Name: ` form). `None` → verbatim text.
fn prefix_speaker(speaker: &Option<String>, text: &str) -> String {
    match speaker {
        Some(name) if !name.is_empty() => format!("@{}: {}", name, text),
        _ => text.to_string(),
    }
}

/// Map one assistant message's blocks to Responses input items.
/// Returns `[message_item?, function_call_item…]`:
///
/// - All `Text` blocks are joined into ONE
///   `{role: "assistant", content: "<text>"}` item (speaker-prefixed
///   for group chat). An assistant turn with zero text (pure tool-use
///   turn) still emits the message item with an empty-string content —
///   the item order (message before its calls) is what the upstream
///   replay expects, and an empty content is a valid EasyInputMessage.
/// - Each `ToolUse` block becomes an independent
///   `{type: "function_call", call_id, name, arguments}` item, where
///   `arguments` is the re-serialized JSON string (the wire carries
///   parsed JSON; Responses wants the string form).
///
/// `Reasoning` / `Signature` / `RedactedThinking` blocks are skipped
/// defensively. On the Anthropic→Responses cross-protocol path the
/// strip pass alone does NOT guarantee this: `block_supported` treats
/// `Reasoning` as supported when EITHER thinking OR reasoning-effort
/// is on (wire/to_wire.rs OR semantics, review.md correction 1), so a
/// reasoning-model config keeps `Reasoning` blocks alive through strip.
/// This skip is the layer that actually drops them (AC4's guarantee is
/// strip + build cooperating, not strip alone).
fn assistant_blocks_to_responses_items(
    blocks: &[WireBlock],
    speaker: &Option<String>,
) -> Vec<Value> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut calls: Vec<Value> = Vec::new();
    for b in blocks {
        match b {
            WireBlock::Text { text, .. } => text_parts.push(text.clone()),
            WireBlock::ToolUse { id, name, input } => calls.push(json!({
                "type": "function_call",
                "call_id": id,
                "name": name,
                "arguments": serde_json::to_string(input)
                    .unwrap_or_else(|_| "{}".to_string()),
            })),
            // See the doc comment above: Reasoning survives strip on
            // reasoning-model configs and is dropped HERE; Signature /
            // RedactedThinking are opaque Anthropic blobs that the
            // strip pass already dropped (`supports_thinking_signatures:
            // false`) — this arm is their belt-and-suspenders.
            WireBlock::Reasoning { .. }
            | WireBlock::Signature { .. }
            | WireBlock::RedactedThinking { .. } => {}
            // B1: images only ride user-role messages; an assistant
            // image is not a supported shape. Skip (the forward pass
            // degrades stray assistant images before this point).
            WireBlock::Image { .. } => {}
        }
    }
    let mut items = Vec::with_capacity(calls.len() + 1);
    items.push(json!({
        "role": "assistant",
        "content": prefix_speaker(speaker, &text_parts.join("")),
    }));
    items.extend(calls);
    items
}

/// Normalize `ModelRow.thinking_effort` into the Responses
/// `reasoning.effort` vocabulary (`minimal|low|medium|high`). Returns
/// the effort to emit, or `None` to omit the whole `reasoning` object.
///
/// The DB column and API forms share the Anthropic/DeepSeek
/// vocabulary (`xhigh` / `max` are valid on both of those), but the
/// Responses API 400s on unknown values — so the adapter normalizes:
///
/// - `minimal|low|medium|high` → verbatim
/// - `xhigh|max` → `high` + `warn!` (never silent — the user asked
///   for a higher effort than we can send)
/// - anything else → `None` + `warn!` (a reasoning object with a
///   bogus effort would 400 every request; omitting it degrades to
///   default reasoning behavior instead)
/// - `None` / empty string → `None`, silent (no knob configured —
///   same "empty means unset" precedent as the Chat Completions
///   adapter's `reasoning_effort` guard)
///
/// The PR2 form-level filter (Responses rows only offer
/// `minimal|low|medium|high`) is the first line of defense; this
/// normalization is the adapter-side backstop for pre-existing rows.
pub(crate) fn normalize_responses_effort(raw: Option<&str>) -> Option<String> {
    let effort = raw?;
    match effort {
        "minimal" | "low" | "medium" | "high" => Some(effort.to_string()),
        "xhigh" | "max" => {
            tracing::warn!(
                configured = %effort,
                "responses: thinking_effort '{effort}' is outside the Responses vocabulary \
                 (minimal|low|medium|high); sending 'high' instead"
            );
            Some("high".to_string())
        }
        "" => None,
        other => {
            tracing::warn!(
                configured = %other,
                "responses: unknown thinking_effort '{other}'; omitting the reasoning object \
                 entirely (unknown values would 400 every request)"
            );
            None
        }
    }
}

/// Derive the Responses target's [`WireCapabilities`] for the
/// `strip_unsupported` pass. Structural sibling of `openai_caps`
/// (RULE-D-005): `supports_reasoning_effort` is true only when the
/// model row configured a `thinking_effort`, so historical `Reasoning`
/// blocks are dropped for non-reasoning Responses models instead of
/// polluting their context.
///
/// Why a free function taking `Option<&str>` (not
/// [`WireCapabilities::from_model_row`]): that needs `&ModelRow`, which
/// [`Provider::send`]'s signature doesn't carry — same rationale as
/// `openai_caps`. The factory already sourced `reasoning_effort` from
/// `model_row.thinking_effort`, so the signal is identical.
///
/// Reminder (design §2.2): `supports_thinking` / `supports_reasoning_effort`
/// gate the OUTGOING payload only — incoming reasoning summaries still
/// map to `ThinkingDelta` regardless of these caps.
pub(crate) fn responses_caps(
    reasoning_effort: Option<&str>,
    supports_images: bool,
) -> WireCapabilities {
    WireCapabilities {
        supports_thinking: false,
        supports_reasoning_effort: reasoning_effort.is_some(),
        supports_thinking_signatures: false,
        supports_images,
    }
}

// ---------------------------------------------------------------------------
// Streaming event state machine (design §2.4)
// ---------------------------------------------------------------------------

/// Aggregation buffer for one in-flight `function_call` output item.
/// Keyed by `output_index` (the item's position in the response's
/// output array — Responses has no Chat-Completions-style `index` on
/// tool calls; `output_index` is the discriminator, research §2.4).
/// Private to this module: unlike the shared [`ToolCallBuf`] in
/// `streaming.rs`, the field set and the keying differ, and growing
/// the streaming.rs public surface for a single consumer isn't worth
/// it (design §2.4).
#[derive(Debug, Default)]
struct FunctionCallBuf {
    call_id: String,
    name: String,
    arguments: String,
}

/// Per-stream state for the Responses SSE event machine. The `send`
/// loop feeds every parsed SSE event through [`ResponsesStreamState::handle_event`]
/// and yields whatever comes back; tests drive the state directly with
/// synthetic event sequences (tests_responses.rs), no HTTP involved.
#[derive(Debug, Default)]
pub(crate) struct ResponsesStreamState {
    /// In-flight function_call items by `output_index`.
    function_calls: HashMap<u32, FunctionCallBuf>,
    /// `response.created` → `Start`, emitted exactly once (a duplicated
    /// created event from a quirky gateway must not reset the UI).
    saw_created: bool,
    /// Set on any terminal event (`response.completed` / `.incomplete`
    /// / `.failed` / `error`); the send loop stops reading afterwards.
    finished: bool,
    /// Captured from the terminal event (`parse_responses_usage`).
    usage: Option<TokenUsage>,
}

impl ResponsesStreamState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Whether a terminal event was already processed.
    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    /// Usage captured from the terminal event (for the D5 cache-miss
    /// sentinel, which must fire before the `Done` event is yielded).
    pub(crate) fn usage(&self) -> Option<TokenUsage> {
        self.usage
    }

    /// Handle one typed SSE event and return the `ChatEvent`s (or the
    /// terminal [`LlmError`]) it maps to. `event_type` is the SSE
    /// `event:` line; the authoritative discriminator is the payload's
    /// `data.type` field (they agree on the official API, and `data.type`
    /// survives gateways that strip `event:` lines). Dozens of event
    /// types we don't consume (web_search / audio / file_search / …)
    /// fall through to the ignore arm — the SSE parser already
    /// tolerates unknown events.
    pub(crate) fn handle_event(
        &mut self,
        event_type: &str,
        data: &Value,
    ) -> Vec<Result<ChatEvent, LlmError>> {
        let typ = data
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or(event_type);
        match typ {
            "response.created" => {
                if self.saw_created {
                    return Vec::new();
                }
                self.saw_created = true;
                vec![Ok(ChatEvent::Start)]
            }
            // `response.in_progress` carries no new payload we consume;
            // Start was already emitted on `created`.
            "response.output_text.delta" => match delta_text(data) {
                Some(text) if !text.is_empty() => {
                    vec![Ok(ChatEvent::Delta {
                        text: text.to_string(),
                    })]
                }
                _ => Vec::new(),
            },
            // Reasoning summaries are the only reasoning surface on
            // Responses (no raw CoT). Emitted as ThinkingDelta so the
            // frontend's existing thinking-rendering path works — the
            // receive direction is independent of the wire caps.
            "response.reasoning_summary_text.delta" => match delta_text(data) {
                Some(text) if !text.is_empty() => {
                    vec![Ok(ChatEvent::ThinkingDelta {
                        text: text.to_string(),
                    })]
                }
                _ => Vec::new(),
            },
            "response.output_item.added" => {
                let item = data.get("item");
                if item.and_then(|i| i.get("type")).and_then(|t| t.as_str())
                    == Some("function_call")
                {
                    // Open the aggregation buffer for this output item.
                    // call_id / name may already be present on the added
                    // item; arguments arrive via `.delta` events.
                    if let Some(idx) = output_index(data) {
                        let entry = self.function_calls.entry(idx).or_default();
                        if entry.call_id.is_empty() {
                            entry.call_id = item
                                .and_then(|i| i.get("call_id"))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                        }
                        if entry.name.is_empty() {
                            entry.name = item
                                .and_then(|i| i.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                        }
                    } else {
                        tracing::warn!(
                            data = %serde_json::to_string(data).unwrap_or_default(),
                            "responses: function_call output_item.added without output_index; \
                             the call cannot be assembled"
                        );
                    }
                }
                Vec::new()
            }
            "response.function_call_arguments.delta" => {
                if let Some(idx) = output_index(data) {
                    if let Some(buf) = self.function_calls.get_mut(&idx) {
                        if let Some(frag) = delta_text(data) {
                            buf.arguments.push_str(frag);
                        }
                    } else {
                        tracing::warn!(
                            output_index = idx,
                            "responses: arguments delta for an unknown function_call; skipping"
                        );
                    }
                }
                Vec::new()
            }
            // `function_call_arguments.done` is intentionally NOT a
            // flush point — `output_item.done` is the single flush
            // (doing both would double-emit the ToolCall).
            "response.function_call_arguments.done" => Vec::new(),
            "response.output_item.done" => {
                let item = data.get("item");
                match item.and_then(|i| i.get("type")).and_then(|t| t.as_str()) {
                    Some("function_call") => match self.flush_function_call(Some(data)) {
                        Some(ev) => vec![ev],
                        None => Vec::new(),
                    },
                    Some("message") => self.handle_message_item_done(item),
                    // reasoning / web_search_call / image_generation_call
                    // / … items carry nothing we emit.
                    _ => Vec::new(),
                }
            }
            "response.completed" => {
                let response = data.get("response");
                let stop = synthesize_stop_reason(response);
                self.stop_on_terminal(parse_responses_usage(data));
                vec![Ok(ChatEvent::Done {
                    stop_reason: Some(stop),
                    usage: self.usage,
                })]
            }
            "response.incomplete" => {
                // Truncated by max_output_tokens / content filter — map
                // the documented reason to the internal Anthropic-style
                // vocabulary, defaulting to "end_turn" conservatively.
                let reason = data
                    .pointer("/response/incomplete_details/reason")
                    .and_then(|r| r.as_str())
                    .unwrap_or_default();
                let stop = if reason == "max_output_tokens" {
                    "max_tokens"
                } else {
                    "end_turn"
                };
                self.stop_on_terminal(parse_responses_usage(data));
                vec![Ok(ChatEvent::Done {
                    stop_reason: Some(stop.to_string()),
                    usage: self.usage,
                })]
            }
            "response.failed" | "error" => {
                // In-stream failure: no HTTP status exists at this
                // point, so we classify with a synthetic 500 and let
                // `classify_error_response`'s keyword match do the real
                // work (`response.error.{code,message}` follows the same
                // `{error: {code}}` convention as the Chat Completions
                // error body — a `rate_limit` code still classifies as
                // RateLimit; everything else lands on Server).
                self.finished = true;
                let err_source = data.get("response").unwrap_or(data);
                let body = serde_json::to_string(err_source).unwrap_or_default();
                tracing::warn!(event = %typ, body = %body, "← LLM stream failed (openai_responses)");
                vec![Err(classify_error_response(500, &body, None))]
            }
            // Dozens of other event types (response.queued,
            // response.in_progress, web_search_*, audio_*, …) — ignore.
            _ => Vec::new(),
        }
    }

    /// Record the terminal state (usage is what the D5 sentinel reads;
    /// the stop_reason itself travels directly on the `Done` event the
    /// caller builds) and mark the stream finished.
    fn stop_on_terminal(&mut self, usage: Option<TokenUsage>) {
        self.usage = usage;
        self.finished = true;
    }

    /// Flush one assembled `function_call` item into a
    /// `ChatEvent::ToolCall`. `event` is the whole SSE payload (the
    /// `output_item.done` envelope); the `item` object is read out of
    /// it. The arguments source is the aggregation buffer when the
    /// `.delta` events populated one, falling back to the `arguments`
    /// string on the done item itself (a gateway that only sends
    /// `output_item.done` without argument deltas is still handled).
    ///
    /// Buffer lookup key: `output_index`. In the official shape it
    /// rides the EVENT ENVELOPE — the same discriminator the added /
    /// arguments-delta events used to open and fill the buffer — so
    /// the envelope is checked first; some gateways nest it inside
    /// the item instead, so the item is checked second. Skipping this
    /// lookup (e.g. matching only the item) would silently discard
    /// the accumulated arguments whenever the done item omits them.
    ///
    /// Truncation tolerance (review.md correction 6): when
    /// `max_output_tokens` cuts the stream mid-arguments, the
    /// accumulated JSON is incomplete and `from_str` fails. Instead of
    /// dropping the call (which would orphan its `function_call_output`)
    /// or emitting `{}` (which loses the only copy of the arguments),
    /// the raw string is degraded into `Value::String(raw)` + `warn!` —
    /// the call keeps flowing and the `incomplete → max_tokens`
    /// stop_reason tells the rest of the loop why the JSON is broken.
    fn flush_function_call(
        &mut self,
        event: Option<&Value>,
    ) -> Option<Result<ChatEvent, LlmError>> {
        let item = event.and_then(|d| d.get("item"));
        let idx = event
            .and_then(output_index)
            .or_else(|| item.and_then(output_index_of_item));
        let buf = idx.and_then(|i| self.function_calls.remove(&i));
        let (call_id, name, arguments) = match buf {
            Some(buf) => {
                // Prefer the buffer; fall back per-field to the done
                // item for gateways that skipped the `.delta` events.
                let call_id = if buf.call_id.is_empty() {
                    item.and_then(|i| i.get("call_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string()
                } else {
                    buf.call_id
                };
                let name = if buf.name.is_empty() {
                    item.and_then(|i| i.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string()
                } else {
                    buf.name
                };
                let arguments = if buf.arguments.is_empty() {
                    item.and_then(|i| i.get("arguments"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string()
                } else {
                    buf.arguments
                };
                (call_id, name, arguments)
            }
            None => {
                // No buffer was opened (no output_item.added / no
                // argument deltas) — rely on the done item alone.
                (
                    item.and_then(|i| i.get("call_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    item.and_then(|i| i.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    item.and_then(|i| i.get("arguments"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                )
            }
        };
        if name.is_empty() {
            tracing::warn!(
                arguments = %arguments,
                "responses: function_call done item has no name; skipping emit"
            );
            return None;
        }
        let input: Value = if arguments.trim().is_empty() {
            json!({})
        } else {
            match serde_json::from_str(&arguments) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        arguments = %arguments,
                        error = %e,
                        "responses: function_call arguments JSON is incomplete (truncated by \
                         max_output_tokens?); degrading to raw string instead of dropping the call"
                    );
                    Value::String(arguments)
                }
            }
        };
        Some(Ok(ChatEvent::ToolCall {
            id: call_id,
            name,
            input,
        }))
    }

    /// Refusal path (review.md correction 5): a `message` output item's
    /// content parts can be `{type: "refusal", refusal: "…"}`. Without
    /// handling, a refusal turn would produce zero Deltas followed by a
    /// normal `completed` — an empty assistant bubble in the chat UI /
    /// an empty group-chat utterance. The refusal text is surfaced as a
    /// regular `Delta` (the user must SEE the refusal reason; task
    /// owner's call — not an LlmError) and the stream then completes
    /// normally.
    fn handle_message_item_done(&self, item: Option<&Value>) -> Vec<Result<ChatEvent, LlmError>> {
        let mut events = Vec::new();
        if let Some(parts) = item
            .and_then(|i| i.get("content"))
            .and_then(|c| c.as_array())
        {
            for part in parts {
                if part.get("type").and_then(|t| t.as_str()) == Some("refusal") {
                    if let Some(text) = part.get("refusal").and_then(|r| r.as_str()) {
                        if !text.is_empty() {
                            events.push(Ok(ChatEvent::Delta {
                                text: text.to_string(),
                            }));
                        }
                    }
                }
            }
        }
        events
    }
}

/// `delta` string field shared by the `*.delta` events.
fn delta_text(data: &Value) -> Option<&str> {
    data.get("delta").and_then(|d| d.as_str())
}

/// `output_index` on the event envelope (added / arguments deltas).
fn output_index(data: &Value) -> Option<u32> {
    data.get("output_index")
        .and_then(|i| i.as_u64())
        .map(|i| i as u32)
}

/// `output_index` on a done item itself (fallback flush path).
fn output_index_of_item(item: &Value) -> Option<u32> {
    item.get("output_index")
        .and_then(|i| i.as_u64())
        .map(|i| i as u32)
}

/// Synthesize the internal stop_reason for `response.completed`
/// (design §2.5). Responses has no `finish_reason` equivalent, so we
/// scan the final output items: any `function_call` item → `tool_use`
/// (the agent loop must execute the calls), otherwise `end_turn`. The
/// vocabulary is the Anthropic-style set the agent loop already
/// normalizes to (same stance as openai.rs's finish_reason mapping).
fn synthesize_stop_reason(response: Option<&Value>) -> String {
    let has_function_call = response
        .and_then(|r| r.get("output"))
        .and_then(|o| o.as_array())
        .map(|items| {
            items
                .iter()
                .any(|i| i.get("type").and_then(|t| t.as_str()) == Some("function_call"))
        })
        .unwrap_or(false);
    if has_function_call {
        "tool_use".to_string()
    } else {
        "end_turn".to_string()
    }
}

// ---------------------------------------------------------------------------
// Provider impl
// ---------------------------------------------------------------------------

impl Provider for ResponsesProvider {
    fn send(
        &self,
        system: Option<String>,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDef>,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static>> {
        // 1. Build the Anthropic-shaped ChatRequest (same placeholder
        //    trick as openai.rs) and run it through the wire layer.
        let req = crate::llm::types::ChatRequest {
            model: self.config.model.clone(),
            max_tokens: self.config.max_tokens,
            messages,
            system: system.clone(),
            stream: true,
            tools,
            thinking: None,
        };
        let wire = chat_request_to_wire(req, system);
        // Cross-protocol strip: drop blocks the Responses target can't
        // carry. Caps derived from the config (design §2.2) — note the
        // OR-semantics caveat: with `reasoning_effort` set, `Reasoning`
        // blocks SURVIVE this pass and are dropped by
        // `build_http_body`'s defensive skip instead (review.md
        // correction 1).
        let caps = responses_caps(
            self.config.reasoning_effort.as_deref(),
            self.config.supports_images,
        );
        let wire = WireRequest {
            messages: strip_unsupported(wire.messages, &caps),
            ..wire
        };

        // Wire-layer order guard (defensive diagnostic, no mutation) —
        // same rationale as openai.rs: a `function_call` item whose
        // `function_call_output` never follows (or is separated from it
        // by an unrelated message) fails upstream. The count-based
        // `orphan_tool_use_ids` inside `chat_request_to_wire` catches a
        // missing result entirely; this catches interleaving. The grep-
        // able error line beats an opaque upstream 400.
        let order_violations = wire::orphan_tool_call_order(&wire.messages);
        if !order_violations.is_empty() {
            tracing::error!(
                model = %self.config.model,
                violation_count = order_violations.len(),
                violations = ?order_violations,
                "wire: orphan tool_call order detected — an assistant(tool_calls) wire \
                 message is not immediately followed by its tool-result messages; this \
                 request may fail upstream. See llm-contract.md §Pair Atomicity."
            );
        }

        // 2. Build the HTTP body + endpoint.
        let body = Self::build_http_body(&wire, &self.config);
        let url = self.config.endpoint();
        let api_key = self.config.api_key.clone();

        let s = stream! {
            // RULE-A-011 (2026-06-19): `read_timeout` (per-read) instead
            // of `timeout` (total deadline) for SSE — identical
            // parameters and rationale as openai.rs. See
            // `.trellis/spec/backend/error-handling.md` §RULE-A-011.
            let client = match reqwest::Client::builder()
                .read_timeout(Duration::from_secs(60))
                .connect_timeout(Duration::from_secs(10))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    yield Err(crate::llm::error::LlmError::Network(format!("client build: {}", e)));
                    return;
                }
            };

            tracing::info!(
                url = %url,
                model = %body["model"],
                tools_count = %body.get("tools").map(|t| t.as_array().map(|a| a.len()).unwrap_or(0)).unwrap_or(0),
                has_system = %wire.system.is_some(),
                "→ LLM request (openai_responses)"
            );

            let resp = match client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "network error before response");
                    yield Err(crate::llm::error::LlmError::Network(e.to_string()));
                    return;
                }
            };

            let status = resp.status();
            if !status.is_success() {
                // Snapshot headers before `resp.text()` consumes the
                // response (retry_after advisory parsing, A5+).
                let headers = resp.headers().clone();
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!(status = %status, body = %body, "← LLM error (openai_responses)");
                yield Err(classify_error_response(status.as_u16(), &body, Some(&headers)));
                return;
            }

            tracing::info!("← LLM stream opened (openai_responses)");
            // NOTE: no immediate Start here — unlike Chat Completions,
            // Start is emitted when the `response.created` typed event
            // arrives (design §2.4).

            let mut byte_stream = resp.bytes_stream();
            let mut parser = SseParser::new();
            // RULE: decode with cross-chunk carry-over — TCP chunking
            // can split a multi-byte UTF-8 char across two chunks
            // (incident 3qnzktvosvxmsycoz46 turn=25, see openai.rs).
            let mut utf8_carry: Vec<u8> = Vec::new();
            let mut state = ResponsesStreamState::new();
            // CC-style `data: [DONE]` sentinel seen (defensive, see
            // below) — ends the stream; the EOF fallback after the
            // loop then closes the turn's accounting.
            let mut done_sentinel = false;

            while let Some(chunk_result) = byte_stream.next().await {
                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(crate::llm::error::LlmError::Network(format!("stream read: {}", e)));
                        return;
                    }
                };
                let text = match utf8_chunk_text(&mut utf8_carry, &bytes) {
                    Ok(Some(t)) => t,
                    Ok(None) => continue, // partial char at chunk boundary: wait
                    Err(e) => {
                        yield Err(crate::llm::error::LlmError::Network(format!("non-utf8 chunk: {}", e)));
                        return;
                    }
                };

                for event in parser.feed(&text) {
                    // Defensive: the official Responses stream never
                    // sends the Chat-Completions-style `[DONE]` sentinel,
                    // but a misbehaving gateway might splice one in.
                    // Treat it as end-of-stream (same semantics as the
                    // Chat Completions adapter) rather than feeding
                    // "[DONE]" to the JSON parser — otherwise a gateway
                    // that holds the connection open afterwards would
                    // stall until the read timeout instead of closing
                    // the turn via the EOF fallback below.
                    if event.data.trim() == "[DONE]" {
                        tracing::debug!("▶ openai_responses: [DONE]");
                        done_sentinel = true;
                        break;
                    }
                    let v: Value = match serde_json::from_str(&event.data) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                data = %event.data,
                                "openai_responses: failed to parse SSE data JSON"
                            );
                            continue;
                        }
                    };
                    tracing::debug!(raw_data = %event.data, "▶ openai_responses: SSE event");

                    for ev in state.handle_event(&event.event, &v) {
                        match ev {
                            Ok(e) => {
                                // D5 (08-31-cache-head-volatility):
                                // full-prefix cache-miss sentinel —
                                // fires BEFORE the terminal Done event
                                // (same relative order as openai.rs).
                                if matches!(e, ChatEvent::Done { .. }) {
                                    if let Some(u) = state.usage() {
                                        super::warn_on_full_prefix_cache_miss(
                                            &u,
                                            &format!(
                                                "openai_responses model={} url={}",
                                                body["model"], url
                                            ),
                                        );
                                    }
                                }
                                yield Ok(e);
                            }
                            Err(e) => {
                                yield Err(e);
                                return;
                            }
                        }
                    }
                    if state.is_finished() {
                        break;
                    }
                }
                if done_sentinel || state.is_finished() {
                    break;
                }
            }

            // Defensive EOF fallback: the stream ended without a
            // terminal event (network drop / proxy cut). Mirror
            // openai.rs and still emit Done so the agent loop's turn
            // accounting closes cleanly (usage=None skips the SQL
            // write; the sentinel above doesn't fire on None).
            if !state.is_finished() {
                yield Ok(ChatEvent::Done { stop_reason: None, usage: None });
            }
        };
        Box::pin(s)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_system_prompt: true,
            supports_tools: true,
            supports_streaming: true,
        }
    }

    fn protocol(&self) -> ProviderProtocol {
        ProviderProtocol::OpenaiResponses
    }
}
