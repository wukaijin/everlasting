//! OpenAI Chat Completions provider (PR3 of the multi-model task).
//!
//! This is the PR3 successor to the PR2 Anthropic-only
//! `provider::anthropic` module. The HTTP + SSE + error-classification
//! logic is OpenAI-shaped (Chat Completions streaming protocol) and
//! is wrapped behind the [`Provider`] trait so the chat command
//! can dispatch through the catalog (`ProviderRow` +
//! `ModelRow`) for any supported protocol.
//!
//! PR3 wire-behavior contract (per the PR3 PRD §"1:1 wire behavior"
//! and §"OpenAI protocol differences" tables):
//!
//! | Concern | Anthropic (PR2) | OpenAI (PR3, this module) |
//! |---------|----------------|---------------------------|
//! | URL | `provider.base_url + "/v1/messages"` | `provider.base_url + "/v1/chat/completions"` |
//! | Auth | `x-api-key: <key>` + `anthropic-version` | `Authorization: Bearer <key>` |
//! | system | top-level `system` field | first message `role: "system"` |
//! | tools | `[ToolDef]` (Anthropic) | `[{type: "function", function: {name, description, parameters}}]` |
//! | tool calls | `tool_use` block in `content[]` | `tool_calls[]` array of `{index, id, function: {name, arguments: "<json-string>"}}` |
//! | tool result | user-message `tool_result` block | independent `role: "tool"` message, `tool_call_id` + `content` |
//! | text delta | `content_block_delta.text_delta` | `choices[0].delta.content` |
//! | reasoning | `thinking_delta` block (SSE event) | `choices[0].delta.reasoning_content` (o1/o3) |
//! | finish | `message_delta.stop_reason` + `message_stop` | `choices[0].finish_reason` + `data: [DONE]` |
//! | max_tokens | top-level | top-level (`max_tokens` field, NOT `max_completion_tokens` — that's a future o1-only change) |
//!
//! Cross-protocol strip is handled by the `wire` module: the
//! `OpenAIProvider::send` first runs
//! `chat_request_to_wire → strip_unsupported → openai-wire-converter`
//! so thinking/signature/redacted-thinking blocks from a previous
//! Anthropic session are dropped silently on the wire without
//! touching the DB. See `wire::strip_unsupported` for the rule
//! table.
//!
//! Implementation notes:
//!
//! - `OpenAIConfig` is module-private (the factory in
//!   `mod.rs` builds it from catalog rows). Both providers are
//!   catalog-only; there is no env fallback.
//! - SSE parser reuse: we use the existing [`SseParser`] in
//!   `data-only` mode (no `event:` lines — Chat Completions only
//!   emits `data: {...}\n\n`). The parser's `event_type` stays
//!   empty for every event.
//! - Multiple parallel `tool_calls`: the `BlockState` in
//!   `anthropic.rs` assumes a single in-flight tool call;
//!   OpenAI can issue several in one response. We index by
//!   `tool_call_index` and emit one `ToolCall` per index when its
//!   JSON is fully assembled.
//! - Error classification: the existing
//!   [`crate::llm::error::classify_error_response`] already
//!   parses `{error: {type, message}}` and classifies by
//!   `error.type` substring — OpenAI uses `code` instead of
//!   `type` in the body, so we extract `code` and feed it
//!   through the same classification path (the keyword matcher
//!   in `classify_error_response` is protocol-agnostic).
//!   Net effect: same 5 LlmError categories, same wire body
//!   shape `{error: {type|code, message}}`.

use async_stream::stream;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use super::streaming::{
    accumulate_tool_call_delta, build_tool_call_event, parse_openai_usage, ToolCallBuf,
};
use super::wire::{
    self, chat_request_to_wire, strip_unsupported, WireBlock, WireCapabilities, WireRequest,
};
use super::{Provider, ProviderCapabilities, ProviderProtocol};
use crate::llm::error::classify_error_response;
use crate::llm::sse::{utf8_chunk_text, SseParser};
use crate::llm::types::{ChatEvent, ChatMessage, TokenUsage, ToolDef};
use crate::llm::LlmError;

// ---------------------------------------------------------------------------
// OpenAIConfig — module-private
// ---------------------------------------------------------------------------

/// Configuration for the OpenAI adapter. Constructed by
/// `build_provider` from a `ProviderRow` + `ModelRow`. There is no
/// `from_env` path — the legacy env keys (`ANTHROPIC_*`,
/// `LLM_THINKING_EFFORT`, …) are Anthropic-only. OpenAI users must
/// configure the key via the Settings UI (PR4).
#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub max_tokens: u32,
    /// OpenAI o1/o3-style top-level `reasoning_effort` field. Sourced
    /// from `ModelRow.thinking_effort` (the same column the
    /// Anthropic adapter reads for `adaptive.effort`). `None` means
    /// "do not emit the field" — the OpenAI side will not see
    /// reasoning_effort in the request body, so non-o1/o3 models
    /// are unaffected by this knob.
    pub reasoning_effort: Option<String>,
    /// B1 (2026-08-16): from `ModelRow.supports_images` — gates the
    /// wire strip's image→text-placeholder degradation + the
    /// `image_url` emission for this model.
    pub supports_images: bool,
}

impl OpenAIConfig {
    /// Trim trailing `/` from `base_url` and append the Chat
    /// Completions endpoint. **The base_url MUST include the API
    /// version prefix** (e.g. `https://api.openai.com/v1`,
    /// `https://hub.example.com/v1`); this function only appends
    /// `/chat/completions` (no leading `/v1/`). This matches the
    /// convention used by `test_model` / `test_provider` in
    /// `lib.rs` and the OpenAI seed row (`https://api.openai.com/v1`).
    ///
    /// **BUG FIX (06-09-fix-session):** prior to this fix the
    /// helper appended `/v1/chat/completions`, producing
    /// `/v1/v1/chat/completions` against any base_url that already
    /// included the version — which is every real OpenAI-compatible
    /// provider (the `https://api.openai.com/v1` seed and any
    /// user-added proxy like `https://hub.example.com/v1`).
    /// The upstream returns 404 "path not found: /v1/v1/chat/completions",
    /// the SSE parser never sees a stream, and `finalizeRequest`
    /// evicts the in-memory cache so the UI lands on the empty
    /// state (with the user message only in DB — exactly the
    /// symptom the user reported as "新 session 发送消息，闪一下变空").
    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

// ---------------------------------------------------------------------------
// OpenAIProvider
// ---------------------------------------------------------------------------

/// OpenAI Chat Completions streaming adapter. Implements
/// [`Provider`].
///
/// One `OpenAIProvider` is constructed per chat invocation (one for
/// the 20-turn agent loop). The chat command calls
/// `send(system, messages, tools)` once per turn and consumes the
/// returned stream inside a `tokio::select!`.
pub struct OpenAIProvider {
    config: OpenAIConfig,
}

impl OpenAIProvider {
    pub fn new(config: OpenAIConfig) -> Self {
        Self { config }
    }

    /// The HTTP + SSE body for one OpenAI request. Pure
    /// function over the (post-strip) [`WireRequest`] so the
    /// conversion is testable without a real HTTP client.
    pub(crate) fn build_http_body(wire: &WireRequest, config: &OpenAIConfig) -> Value {
        // 1. messages array
        let mut msgs: Vec<Value> = Vec::new();

        // OpenAI Chat Completions carries the system prompt as
        // a first `role: "system"` message (Anthropic uses a
        // top-level `system` field). If the wire request has
        // one, prepend it.
        if let Some(sys) = wire.system.as_deref() {
            msgs.push(json!({ "role": "system", "content": sys }));
        }

        // 2. Walk the wire messages. The wire layer has already
        //    split `role: "user"` `tool_result` blocks out into
        //    `WireMessage::Tool`, so the OpenAI side emits one
        //    `role: "tool"` message per `tool_call_id`.
        for m in &wire.messages {
            match m {
                super::wire::WireMessage::User { content, speaker } => {
                    // Group chat (07-29-group-chat): OpenAI Chat
                    // Completions natively supports a `name` field
                    // on `role: "user"` to identify the speaker. We
                    // emit it when present so the model can attribute
                    // prior utterances. `None` (classic chat) → omit
                    // the field, byte-identical to pre-group-chat.
                    let mut msg = json!({ "role": "user", "content": content });
                    if let Some(name) = speaker {
                        msg["name"] = json!(name);
                    }
                    msgs.push(msg);
                }
                // B5 refactor (2026-06-11): a user message that
                // carries a `cache_control` marker on any text
                // block is emitted as `UserBlocks` (block-shaped
                // content) instead of a single string. OpenAI Chat
                // Completions has no prompt-cache marker, so we
                // drop the cache_control and flatten the text
                // blocks to a single string. This keeps OpenAI
                // behavior identical to pre-refactor for the same
                // logical content.
                super::wire::WireMessage::UserBlocks { blocks } => {
                    // B1 (2026-08-16): when the blocks carry images
                    // (a vision request), OpenAI Chat Completions
                    // requires content to be an *array* mixing
                    // `{"type":"text"}` and `{"type":"image_url"}`
                    // entries (a plain string cannot carry an image).
                    // Without images we keep the historical
                    // flatten-to-string shape, byte-identical to the
                    // B5 refactor behavior.
                    let has_image = blocks
                        .iter()
                        .any(|b| matches!(b, super::wire::WireBlock::Image { .. }));
                    if has_image {
                        let mut parts: Vec<Value> = Vec::new();
                        for b in blocks {
                            match b {
                                super::wire::WireBlock::Text { text, .. } => {
                                    if !text.is_empty() {
                                        parts.push(json!({
                                            "type": "text",
                                            "text": text,
                                        }));
                                    }
                                }
                                super::wire::WireBlock::Image { media_type, data } => {
                                    parts.push(json!({
                                        "type": "image_url",
                                        "image_url": {
                                            "url": format!("data:{};base64,{}", media_type, data),
                                        },
                                    }));
                                }
                                // Defensive: user-role UserBlocks only
                                // carry text + image blocks at this
                                // point (tool_results were lifted into
                                // WireMessage::Tool by the forward
                                // pass). Skip anything else.
                                _ => {}
                            }
                        }
                        msgs.push(json!({ "role": "user", "content": parts }));
                    } else {
                        let mut content = String::new();
                        for b in blocks {
                            if let super::wire::WireBlock::Text { text, .. } = b {
                                content.push_str(text);
                            }
                            // Defensive: a `UserBlocks` payload with
                            // non-text blocks is unexpected (the wire
                            // layer's `chat_message_to_wire_messages`
                            // only produces text blocks for this
                            // variant). Skip non-text rather than
                            // crash.
                        }
                        msgs.push(json!({ "role": "user", "content": content }));
                    }
                }
                super::wire::WireMessage::Assistant { blocks, speaker } => {
                    let (text_parts, tool_calls, reasoning) = assistant_blocks_to_openai(blocks);
                    let mut msg = json!({ "role": "assistant" });
                    if !text_parts.is_empty() {
                        msg["content"] = json!(text_parts.join(""));
                    } else {
                        msg["content"] = Value::Null;
                    }
                    if !tool_calls.is_empty() {
                        msg["tool_calls"] = json!(tool_calls);
                    }
                    // RULE-D-006 (2026-06-21): DeepSeek v4 reasoning_content
                    // round-trip. DeepSeek-v4-flash via the OpenAI protocol
                    // surfaces the model's reasoning as a top-level
                    // `reasoning_content` string field on each assistant
                    // message (both in the streaming delta and the final
                    // choice). When we send back prior assistant turns as
                    // history, DeepSeek's contract (per AstrBot PR 7823,
                    // and accepted — though not strictly required — by
                    // live wukaijin probes T1/T2/T3/T4 on 2026-06-21) is:
                    //
                    //   - assistant WITH prior reasoning → echo the joined
                    //     reasoning text into a top-level `reasoning_content`
                    //     field (sibling of `content`). NOT prepended into
                    //     the content string (the pre-PR1 code did
                    //     `format!("[reasoning] {}", text)` — that polluted
                    //     the visible answer and DeepSeek would re-tokenize
                    //     the marker every turn).
                    //   - assistant WITHOUT prior reasoning (pure text ack,
                    //     tool_result ack, etc.) → `reasoning_content="none"`
                    //     (literal non-empty string). AstrBot PR 7823 chose
                    //     `"none"` because DeepSeek rejects the empty
                    //     string `""` on its strict path; the wukaijin
                    //     proxy accepts `""` today but the stricter shape
                    //     is harmless and survives a proxy tightening.
                    //
                    // Live probes (2026-06-21, wukaijin OpenAI endpoint,
                    // `deepseek-v4-flash`):
                    //   T1  no field           → 200 (lenient today)
                    //   T2  `reasoning_content:"none"` → 200
                    //   T3  `reasoning_content:""`     → 200 (today)
                    //   T4  multi-line reasoning_content → 200
                    // We pick the AstrBot shape (`"none"`) for the
                    // no-reasoning case because it's the stricter contract
                    // and costs nothing.
                    //
                    // GATING (RULE-D-006a, regression guard for gpt-4o):
                    // The field is injected ONLY when the model opted into
                    // reasoning — `config.reasoning_effort.is_some()` OR
                    // `is_o1_family(&config.model)`. This is the same signal
                    // `openai_caps` (RULE-D-005) uses to decide whether the
                    // wire strip keeps `Reasoning` blocks, so the field
                    // injection matches what the strip pass kept.
                    //
                    // Why gate: vanilla OpenAI non-reasoning models (gpt-4o,
                    // gpt-4.1, glm-4.7) are NOT contractually required to
                    // accept `reasoning_content` (it's a provider-specific
                    // extension field, NOT a documented OpenAI field).
                    // OpenAI's official API is lenient today, but
                    // `reasoning_content` is a reserved-ish name on several
                    // OpenAI-compatible proxies — carrying it on a plain
                    // gpt-4o request is a latent compatibility bug. For a
                    // reasoning-capable model (o1/o3, deepseek with
                    // reasoning_effort set), the field is expected by the
                    // upstream and DeepSeek-v4 requires it non-empty.
                    let is_reasoning_model =
                        config.reasoning_effort.is_some() || is_o1_family(&config.model);
                    if is_reasoning_model {
                        let rc = match reasoning {
                            Some(text) if !text.is_empty() => text,
                            _ => "none".to_string(),
                        };
                        msg["reasoning_content"] = json!(rc);
                    }
                    // Group chat (07-29-group-chat): emit the native
                    // OpenAI `name` field so the model attributes
                    // prior assistant utterances to the right
                    // participant. `None` (classic chat) → omit,
                    // byte-identical to pre-group-chat.
                    if let Some(name) = speaker {
                        msg["name"] = json!(name);
                    }
                    msgs.push(msg);
                }
                super::wire::WireMessage::Tool {
                    tool_call_id,
                    content,
                } => {
                    msgs.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_call_id,
                        "content": content,
                    }));
                }
            }
        }

        // 3. tools array — wrap each in `{"type": "function",
        //    "function": {…}}` per the OpenAI spec. `parameters`
        //    is the OpenAI equivalent of Anthropic's
        //    `input_schema`.
        let tools: Vec<Value> = wire
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description.clone().unwrap_or_default(),
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();

        // 4. Top-level body. RULE-D-002 (2026-06-16): OpenAI's
        //    o1+ reasoning family (o1 / o1-mini / o1-preview /
        //    o1-pro, o3 / o3-mini / o3-pro, o4-mini) rejects the
        //    standard `max_tokens` field and requires
        //    `max_completion_tokens` — emitting the wrong key
        //    gets a 400 on every chat. Pick the key per model
        //    family; the value is the same configured cap either
        //    way.
        let mut body = json!({
            "model": config.model,
            "stream": true,
            "messages": msgs,
        });
        let max_tokens_key = if is_o1_family(&config.model) {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        body[max_tokens_key] = json!(config.max_tokens);
        // A4: ask OpenAI to include the final usage chunk in
        // the SSE stream. Without this, OpenAI omits the
        // `usage` field on all chunks and the agent loop has
        // no per-turn token counts. See
        // backend/token-usage-tracking.md "Scenario: Token Usage
        // Tracking" for the schema mapping
        // (`prompt_tokens` → `input_tokens` etc).
        body["stream_options"] = json!({ "include_usage": true });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        // OpenAI o1/o3 reasoning_effort is a top-level string.
        // Only emit it when the model row had `thinking_effort`
        // set (the same signal that the user opted the model
        // into reasoning). For other OpenAI models the field
        // is omitted entirely, which is safe across the whole
        // model family.
        //
        // RULE-D-007 (2026-06-21, deepseek OpenAI route): the
        // `reasoning_effort` field is accepted by the deepseek-v4-flash
        // model via the wukaijin OpenAI endpoint. Live probe
        // (2026-06-21, `POST /v1/chat/completions`, `deepseek-v4-flash`):
        //
        //   - `reasoning_effort:"max"`     → 200, reasoning_content present
        //   - absent                       → 200, reasoning_content present
        //                                       (deepseek turns reasoning on
        //                                       by default — `reasoning_tokens`
        //       is non-zero even without the field)
        //   - `reasoning_effort:"minimal"` → 400 `unknown variant 'minimal',
        //                                       expected one of 'high', 'low',
        //                                       'medium', 'max', 'xhigh'`
        //
        // DeepSeek's accepted enum is `{low, medium, high, xhigh, max}` —
        // a superset of OpenAI's `{low, medium, high}`. The everlasting
        // `ModelRow.thinking_effort` column already uses the same vocabulary
        // (it sources Anthropic adaptive.effort, which also allows `xhigh`
        // /`max`), so plumbing it through verbatim is correct: a deepseek
        // model row configured with `thinking_effort="max"` sends
        // `reasoning_effort:"max"` → 200.
        //
        // No suppression needed for deepseek. The existing
        // "emit only when set" guard stays.
        if let Some(effort) = config.reasoning_effort.as_deref() {
            if !effort.is_empty() {
                body["reasoning_effort"] = json!(effort);
            }
        }
        body
    }
}

/// Map an assistant message's blocks to the OpenAI shape.
/// Returns `(text_parts, tool_calls_json, reasoning_content)`:
/// - `text_parts` is the joined string of all `WireBlock::Text` blocks
///   (Anthropic ordering is preserved within a single turn; OpenAI doesn't
///   have an explicit "block" so we just concatenate).
/// - `tool_calls` is the array of `{index, id, type, function}` objects.
/// - `reasoning_content` is the joined string of all `WireBlock::Reasoning`
///   blocks' text, joined by `\n`. `None` if the assistant message has no
///   reasoning blocks. The caller (`build_http_body`) decides what to emit
///   when this is `None` — for DeepSeek v4 compatibility it's `"none"`
///   (see RULE-D-006 in `build_http_body`).
///
/// Note: `Signature` / `RedactedThinking` blocks have already been
/// mapped / dropped by the wire layer; the OpenAI parser emits
/// `Reasoning` as `ChatEvent::ThinkingDelta` via
/// `wire_block_to_chat_event` directly when it parses a
/// `reasoning_content` field on an in-flight delta. This function only
/// handles the **request-side** mapping (history being resent to OpenAI
/// on a multi-turn conversation).
///
/// Pre-PR1 behavior (replaced 2026-06-21): the `Reasoning` text was
/// prepended into `text_parts` as `format!("[reasoning] {}", text)`.
/// That polluted the visible content (the model re-tokenized the
/// marker every turn) and didn't satisfy DeepSeek v4's
/// `reasoning_content` round-trip contract. PR1 lifts it to a
/// dedicated top-level field instead.
pub(crate) fn assistant_blocks_to_openai(
    blocks: &[WireBlock],
) -> (Vec<String>, Vec<Value>, Option<String>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut reasoning_parts: Vec<String> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        match b {
            WireBlock::Text { text, .. } => text_parts.push(text.clone()),
            // B1: images only ride user-role messages; an image in an
            // assistant block is not a supported shape. Skip (the
            // forward pass degrades stray assistant images to text
            // placeholders before this point anyway).
            WireBlock::Image { .. } => {}
            WireBlock::Reasoning { text } => {
                // RULE-D-006: lift to top-level `reasoning_content`
                // field (handled by the caller), NOT into content text.
                if !text.is_empty() {
                    reasoning_parts.push(text.clone());
                }
            }
            WireBlock::Signature { .. } | WireBlock::RedactedThinking { .. } => {
                // Opaque blobs that survived strip (only happens
                // on Anthropic→Anthropic round-trip, which is
                // not a path OpenAI ever sees). On the
                // hypothetical cross-protocol pass-through
                // (Anthropic history → OpenAI), strip drops
                // these — see `wire::strip_unsupported`. So
                // this branch is unreachable in practice.
            }
            WireBlock::ToolUse { id, name, input } => {
                tool_calls.push(json!({
                    "index": i,
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": serde_json::to_string(input)
                            .unwrap_or_else(|_| "{}".to_string()),
                    }
                }));
            }
        }
    }
    let reasoning = if reasoning_parts.is_empty() {
        None
    } else {
        Some(reasoning_parts.join("\n"))
    };
    (text_parts, tool_calls, reasoning)
}

/// Whether `model` belongs to OpenAI's o1+ reasoning family —
/// o1 / o1-mini / o1-preview / o1-pro, o3 / o3-mini / o3-pro,
/// o4-mini, and any successor following the same naming. These
/// models reject the standard `max_tokens` request field and
/// require `max_completion_tokens` (RULE-D-002); see
/// `build_http_body` where the request key is picked. Matching
/// is by id prefix, lower-cased, so it tolerates casing variants
/// third-party gateways may emit.
pub(crate) fn is_o1_family(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
}

/// Derive the OpenAI target's [`WireCapabilities`] for the
/// `strip_unsupported` pass.
///
/// RULE-D-005 (2026-06-18): previously `send` hardcoded
/// `supports_reasoning_effort: true`, which kept historical
/// `Reasoning` blocks alive even for non-reasoning models (e.g.
/// gpt-4o with no `thinking_effort` configured) — polluting their
/// context. Now derived from the configured `reasoning_effort` so
/// the strip pass drops `Reasoning` blocks unless the model row
/// actually opted into reasoning.
///
/// Why a free function taking `Option<&str>` instead of
/// [`WireCapabilities::from_model_row`]? That needs `&ModelRow`,
/// but [`Provider::send`]'s signature doesn't carry it; threading
/// it through is a trait-level change out of scope here.
/// `OpenAIConfig.reasoning_effort` is already sourced from
/// `model_row.thinking_effort` in `build_provider`, so it carries
/// the same signal.
pub(crate) fn openai_caps(
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

impl Provider for OpenAIProvider {
    fn send(
        &self,
        system: Option<String>,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDef>,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static>> {
        // 1. Build the Anthropic-shaped ChatRequest. The wire
        //    layer takes a ChatRequest; converting from
        //    `Vec<ChatMessage>` directly is the same shape
        //    (just supply an empty `system` placeholder).
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
        // Cross-protocol strip: drop blocks the OpenAI target
        // can't carry. The capabilities describe the *target*
        // (this OpenAI provider + the chosen model). Derived via
        // `openai_caps` (RULE-D-005): `supports_reasoning_effort`
        // is true only when the model row had a `thinking_effort`
        // configured, so historical `Reasoning` blocks from a
        // previous Anthropic session are dropped for non-reasoning
        // OpenAI models (e.g. gpt-4o) instead of polluting their
        // context.
        let caps = openai_caps(
            self.config.reasoning_effort.as_deref(),
            self.config.supports_images,
        );
        let wire = WireRequest {
            messages: strip_unsupported(wire.messages, &caps),
            ..wire
        };

        // Wire-layer **order** guard (defensive diagnostic, no mutation):
        // OpenAI Chat Completions rejects an assistant(tool_calls) that is
        // NOT immediately followed by `role: "tool"` messages with HTTP
        // 400 "An assistant message with 'tool_calls' must be followed by
        // tool messages responding to each 'tool_call_id'". The count-based
        // `orphan_tool_use_ids` (run inside `chat_request_to_wire`) catches
        // a missing tool_result entirely; this order check catches the
        // case where every id HAS a result but a `User`/`UserBlocks`/
        // `Assistant` message is interleaved between the assistant and its
        // tool messages. Symptom: a 400 mid-session after a hint Text
        // block was inserted at the head of a user(tool_results) message
        // (loop-detection hint block) — fixed at the chat_loop layer, but
        // this guard makes any future regression grep-able via
        // `tracing::error` "wire: orphan tool_call order" instead of
        // requiring a fresh RCA against an opaque 400.
        let order_violations = wire::orphan_tool_call_order(&wire.messages);
        if !order_violations.is_empty() {
            tracing::error!(
                model = %self.config.model,
                violation_count = order_violations.len(),
                violations = ?order_violations,
                "wire: orphan tool_call order detected — an assistant(tool_calls) wire \
                 message is not immediately followed by role:tool messages; this request \
                 will fail upstream with OpenAI 400 \"insufficient tool messages following \
                 tool_calls\". See llm-contract.md §Pair Atomicity."
            );
        }

        // 2. Build the HTTP body.
        let body = Self::build_http_body(&wire, &self.config);
        let url = self.config.endpoint();
        let api_key = self.config.api_key.clone();

        let s = stream! {
            // RULE-A-011 (2026-06-19): use `read_timeout` instead of
            // `timeout` for SSE streaming. Per reqwest docs
            // (`async_impl/client.rs:1448-1459`), `.timeout()` is a
            // **total deadline** from connect to body EOF — wrong
            // for SSE where the body is unbounded and chunk rate
            // varies (extended thinking on a 3rd-party proxy can be
            // 60s+ before the first text delta). `.read_timeout()`
            // is per-read, resets on each chunk — the right tool
            // for "stalled connection when size isn't known". The
            // 60s value stays as the upper bound on silence between
            // chunks; a truly dead proxy will surface this quickly.
            // See `.trellis/spec/backend/error-handling.md` §RULE-A-011
            // and incident `mz8s3hqwx6rmqjswgte` / messages.seq=37.
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
                "→ LLM request (openai)"
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
                // Snapshot headers before `resp.text()` consumes the response —
                // `retry_after` advisory parsing needs them (A5+ retry support).
                let headers = resp.headers().clone();
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!(status = %status, body = %body, "← LLM error (openai)");
                yield Err(classify_error_response(status.as_u16(), &body, Some(&headers)));
                return;
            }

            tracing::info!("← LLM stream opened (openai)");
            yield Ok(ChatEvent::Start);

            let mut byte_stream = resp.bytes_stream();
            let mut parser = SseParser::new();
            // RULE: decode with cross-chunk carry-over — TCP chunking
            // can split a multi-byte UTF-8 char across two chunks;
            // decoding each chunk in isolation aborts a healthy turn
            // with "incomplete utf-8 byte sequence" (incident
            // 3qnzktvosvxmsycoz46 turn=25).
            let mut utf8_carry: Vec<u8> = Vec::new();
            // Map: `tool_call_index -> {id, name, args_buf}`.
            // OpenAI can emit several tool_calls in parallel
            // within a single response; the index is the
            // discriminator.
            let mut tool_call_state: HashMap<u32, ToolCallBuf> = HashMap::new();
            let mut stop_reason: Option<String> = None;
            // A4: buffer OpenAI's `usage` payload from the
            // final chunk(s) and emit it on the terminal `Done`
            // event. OpenAI's `stream_options.include_usage`
            // flag (set in `build_http_body`) makes the
            // upstream send a chunk with `usage` populated and
            // no `choices` — the chunk arrives AFTER
            // `data: [DONE]`, but most clients (incl. ours)
            // see the `usage` chunk in the same stream
            // iteration. We accumulate defensively: each
            // chunk with a `usage` field overwrites the
            // previous (the cumulative semantics match
            // Anthropic's per-turn accounting).
            let mut usage: Option<TokenUsage> = None;

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
                    // OpenAI Chat Completions sends only
                    // `data: {...}\n\n` — no `event:` lines.
                    // SseParser's `event_type` is empty, so
                    // we just parse every event's `data`.
                    if event.event.is_empty() {
                        // `data: [DONE]` signals end-of-stream.
                        if event.data.trim() == "[DONE]" {
                            tracing::debug!("▶ openai: [DONE]");
                            // Emit any in-flight tool calls that
                            // never saw a finish marker (rare
                            // but defensive). The finish-reason
                            // branch below also flushes; this
                            // is the second-line defense.
                            let keys: Vec<u32> =
                                tool_call_state.keys().copied().collect();
                            for idx in keys {
                                if let Some(buf) = tool_call_state.remove(&idx) {
                                    if let Some(ev) =
                                        build_tool_call_event(&buf, idx)
                                    {
                                        yield Ok(ev);
                                    }
                                }
                            }
                            break;
                        }
                            let v: Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::debug!(
                                        error = %e,
                                        data = %event.data,
                                        "openai: failed to parse SSE data JSON"
                                    );
                                    continue;
                                }
                            };
                            tracing::debug!(raw_data = %event.data, "▶ openai: SSE chunk");

                        // A4: OpenAI attaches a top-level
                        // `usage` field on chunks where
                        // `stream_options.include_usage` is set.
                        // The schema (cumulative per-turn):
                        //   { "usage": { "prompt_tokens": N,
                        //                "completion_tokens": N,
                        //                "total_tokens": N,
                        //                "prompt_tokens_details": { "cached_tokens": N } } }
                        // Some chunks carry ONLY `usage` (the
                        // final one, with empty `choices`); we
                        // still want to process those for the
                        // usage payload.
                        if let Some(u) = parse_openai_usage(&v) {
                            usage = Some(u);
                        }

                        // choices[0].delta is the typical shape.
                        // Some responses (final chunk) only
                        // carry choices[0].finish_reason; we
                        // capture that into `stop_reason` and
                        // don't emit any text / tool_call.
                        if let Some(choice) = v
                            .get("choices")
                            .and_then(|c| c.as_array())
                            .and_then(|a| a.first())
                        {
                            if let Some(fr) = choice
                                .get("finish_reason")
                                .and_then(|f| f.as_str())
                            {
                                // Normalize to Anthropic-style
                                // values for downstream
                                // compatibility (the chat
                                // command's done-handling is
                                // shape-agnostic).
                                let normalized = match fr {
                                    "stop" => "end_turn",
                                    "length" => "max_tokens",
                                    "tool_calls" => "tool_use",
                                    other => other,
                                };
                                tracing::debug!(stop_reason = %normalized, "▶ openai: finish_reason");
                                stop_reason = Some(normalized.to_string());
                            }

                            if let Some(delta) = choice.get("delta") {
                                // text content
                                if let Some(s) = delta
                                    .get("content")
                                    .and_then(|c| c.as_str())
                                {
                                    if !s.is_empty() {
                                        yield Ok(ChatEvent::Delta { text: s.to_string() });
                                    }
                                }
                                // reasoning_content (o1/o3) or reasoning
                                // (some OpenAI-compatible providers).
                                // Emit as ThinkingDelta so the
                                // frontend's existing
                                // thinking-rendering path works.
                                let reasoning = delta
                                    .get("reasoning_content")
                                    .and_then(|c| c.as_str())
                                    .or_else(|| delta.get("reasoning").and_then(|c| c.as_str()));
                                if let Some(s) = reasoning {
                                    if !s.is_empty() {
                                        yield Ok(ChatEvent::ThinkingDelta { text: s.to_string() });
                                    }
                                }
                                // tool_calls: array of
                                // {index, id?, type?,
                                // function: {name?, arguments?}}.
                                // Each delta may carry any
                                // subset of those fields; we
                                // accumulate the `arguments`
                                // JSON string per index.
                                if let Some(tcs) = delta
                                    .get("tool_calls")
                                    .and_then(|t| t.as_array())
                                {
                                    tracing::debug!(tool_calls = %serde_json::to_string(tcs).unwrap_or_default(), "▶ openai: tool_calls delta");
                                    for tc in tcs {
                                        accumulate_tool_call_delta(&mut tool_call_state, tc);
                                    }
                                }
                            }
                        }

                        // OpenAI signals end-of-stream by
                        // emitting a final chunk with
                        // `choices: [{..., finish_reason: "stop"}]`
                        // and no delta. We treat any
                        // `finish_reason` we see as the
                        // stream-end signal: emit any pending
                        // tool calls now.
                                if stop_reason.is_some() {
                                    tracing::debug!(
                                        stop_reason = ?stop_reason,
                                        tool_call_indices = ?tool_call_state.keys().collect::<Vec<_>>(),
                                        "▶ openai: flushing tool calls on stop"
                                    );
                                    let keys: Vec<u32> = tool_call_state.keys().copied().collect();
                            for idx in keys {
                                if let Some(buf) = tool_call_state.remove(&idx) {
                                    if let Some(ev) = build_tool_call_event(&buf, idx) {
                                        yield Ok(ev);
                                    }
                                }
                            }
                        }
                    } else {
                        tracing::debug!(
                            event_type = %event.event,
                            "▶ openai: ignored event with non-empty event type"
                        );
                    }
                }
            }

            yield Ok(ChatEvent::Done { stop_reason, usage });
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
        ProviderProtocol::Openai
    }
}
