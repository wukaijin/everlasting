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
//!   `mod.rs` builds it from catalog rows). `LlmConfig::from_env`
//!   is **Anthropic-only**; OpenAI has no env fallback in PR3.
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

use super::wire::{
    chat_request_to_wire, strip_unsupported, WireBlock, WireRequest,
};
use super::{Provider, ProviderCapabilities, ProviderProtocol};
use crate::llm::error::classify_error_response;
use crate::llm::sse::SseParser;
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
    /// user-added proxy like `https://hub.wukaijin.com/v1`).
    /// The upstream returns 404 "path not found: /v1/v1/chat/completions",
    /// the SSE parser never sees a stream, and `finalizeRequest`
    /// evicts the in-memory cache so the UI lands on the empty
    /// state (with the user message only in DB — exactly the
    /// symptom the user reported as "新 session 发送消息，闪一下变空").
    pub fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.base_url.trim_end_matches('/')
        )
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
    fn build_http_body(
        wire: &WireRequest,
        config: &OpenAIConfig,
    ) -> Value {
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
                super::wire::WireMessage::User { content } => {
                    msgs.push(json!({ "role": "user", "content": content }));
                }
                super::wire::WireMessage::Assistant { blocks } => {
                    let (text_parts, tool_calls) = assistant_blocks_to_openai(blocks);
                    let mut msg = json!({ "role": "assistant" });
                    if !text_parts.is_empty() {
                        msg["content"] = json!(text_parts.join(""));
                    } else {
                        msg["content"] = Value::Null;
                    }
                    if !tool_calls.is_empty() {
                        msg["tool_calls"] = json!(tool_calls);
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

        // 4. Top-level body.
        let mut body = json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "stream": true,
            "messages": msgs,
        });
        // A4: ask OpenAI to include the final usage chunk in
        // the SSE stream. Without this, OpenAI omits the
        // `usage` field on all chunks and the agent loop has
        // no per-turn token counts. See
        // backend/llm-contract.md "Scenario: Token Usage
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
        if let Some(effort) = config.reasoning_effort.as_deref() {
            if !effort.is_empty() {
                body["reasoning_effort"] = json!(effort);
            }
        }
        body
    }
}

/// Map an assistant message's blocks to the OpenAI shape.
/// Returns `(text_parts, tool_calls_json)` — text is the joined
/// string of all `WireBlock::Text` blocks (Anthropic ordering is
/// preserved within a single turn; OpenAI doesn't have an
/// explicit "block" so we just concatenate), and `tool_calls` is
/// the array of `{index, id, type, function}` objects.
///
/// Note: `Reasoning` / `Signature` / `RedactedThinking` blocks
/// have already been mapped / dropped by the wire layer; the
/// OpenAI parser emits them as `ChatEvent::ThinkingDelta` via
/// `wire_block_to_chat_event` directly when it parses a
/// `reasoning_content` field on an in-flight delta. This
/// function only handles the **request-side** mapping (history
/// being resent to OpenAI on a multi-turn conversation).
fn assistant_blocks_to_openai(blocks: &[WireBlock]) -> (Vec<String>, Vec<Value>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        match b {
            WireBlock::Text { text } => text_parts.push(text.clone()),
            WireBlock::Reasoning { text } => {
                // Reasoning from prior turns: prepend to the
                // assistant content as a hidden comment so the
                // model can see its own prior reasoning
                // (OpenAI has no native round-trip for this;
                // the comment-marker approach is the
                // documented fallback for cross-protocol
                // history).
                text_parts.push(format!("[reasoning] {}", text));
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
    (text_parts, tool_calls)
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
        // can't carry. The capabilities passed here describe
        // the *target* (this OpenAI provider + the chosen
        // model). For OpenAI: `supports_thinking = false`,
        // `supports_reasoning_effort = true` if the model row
        // had a `thinking_effort`, and signatures are never
        // supported.
        //
        // NOTE: the chat command is the canonical place to
        // thread `WireCapabilities` into the provider. For
        // PR3 we conservatively pass the most-open
        // capabilities (the wire layer will already drop
        // signatures for OpenAI because the protocol is
        // openai, and reasoning_effort is set by the
        // provider's `send` itself if appropriate). Future
        // PRs can pass model-row-derived caps through a
        // trait extension; today the OpenAI adapter is
        // permissive on input.
        let caps = super::wire::WireCapabilities {
            supports_thinking: false,
            supports_reasoning_effort: true,
            supports_thinking_signatures: false,
        };
        let wire = WireRequest {
            messages: strip_unsupported(wire.messages, &caps),
            ..wire
        };

        // 2. Build the HTTP body.
        let body = Self::build_http_body(&wire, &self.config);
        let url = self.config.endpoint();
        let api_key = self.config.api_key.clone();

        let s = stream! {
            let client = match reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
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
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!(status = %status, body = %body, "← LLM error (openai)");
                yield Err(classify_error_response(status.as_u16(), &body));
                return;
            }

            tracing::info!("← LLM stream opened (openai)");
            yield Ok(ChatEvent::Start);

            let mut byte_stream = resp.bytes_stream();
            let mut parser = SseParser::new();
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
                let text = match std::str::from_utf8(&bytes) {
                    Ok(t) => t,
                    Err(e) => {
                        yield Err(crate::llm::error::LlmError::Network(format!("non-utf8 chunk: {}", e)));
                        return;
                    }
                };

                for event in parser.feed(text) {
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
                                // reasoning_content (o1/o3).
                                // Emit as ThinkingDelta so the
                                // frontend's existing
                                // thinking-rendering path works.
                                if let Some(s) = delta
                                    .get("reasoning_content")
                                    .and_then(|c| c.as_str())
                                {
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
                                    for tc in tcs {
                                        let idx = tc
                                            .get("index")
                                            .and_then(|i| i.as_u64())
                                            .unwrap_or(0)
                                            as u32;
                                        let entry = tool_call_state
                                            .entry(idx)
                                            .or_insert_with(ToolCallBuf::default);
                                        if let Some(id) =
                                            tc.get("id").and_then(|s| s.as_str())
                                        {
                                            entry.id = id.to_string();
                                        }
                                        if let Some(name) = tc
                                            .get("function")
                                            .and_then(|f| f.get("name"))
                                            .and_then(|s| s.as_str())
                                        {
                                            entry.name = name.to_string();
                                        }
                                        if let Some(args) = tc
                                            .get("function")
                                            .and_then(|f| f.get("arguments"))
                                            .and_then(|s| s.as_str())
                                        {
                                            entry.args_buf.push_str(args);
                                        }
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

// ---------------------------------------------------------------------------
// Tool call accumulation helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct ToolCallBuf {
    id: String,
    name: String,
    args_buf: String,
}

/// Build the `ChatEvent::ToolCall` for one fully-assembled tool
/// call buffer. Returns `None` if the buffer has no name (the
/// stream never delivered the `function.name` field — defensive).
fn build_tool_call_event(buf: &ToolCallBuf, _idx: u32) -> Option<ChatEvent> {
    if buf.name.is_empty() {
        tracing::warn!(
            args_buf = %buf.args_buf,
            "openai: tool_call buffer has no name; skipping emit"
        );
        return None;
    }
    let input: Value = if buf.args_buf.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&buf.args_buf).unwrap_or_else(|e| {
            tracing::warn!(
                args_buf = %buf.args_buf,
                error = %e,
                "openai: failed to parse tool_call arguments JSON, using empty object"
            );
            json!({})
        })
    };
    Some(ChatEvent::ToolCall {
        id: buf.id.clone(),
        name: buf.name.clone(),
        input,
    })
}

// ---------------------------------------------------------------------------
// A4: parse_openai_usage — normalize OpenAI's `usage` chunk into
// the protocol-agnostic `TokenUsage` schema.
// ---------------------------------------------------------------------------

/// Parse OpenAI's `usage` payload into a protocol-agnostic
/// [`TokenUsage`]. Schema mapping (per
/// `backend/llm-contract.md` "Scenario: Token Usage Tracking"
/// §3 "OpenAI normalization"):
///
/// - `prompt_tokens` → `input_tokens`
/// - `completion_tokens` → `output_tokens`
/// - `prompt_tokens_details.cached_tokens` → `cache_read_input_tokens`
/// - `cache_creation_input_tokens` → 0 (no OpenAI equivalent
///   today; the field is documented but rarely populated)
///
/// Defensive: any field may be missing (older API versions /
/// proxies omit the cached_tokens sub-object). Missing fields
/// default to 0. Returns `None` if no recognizable integer fields
/// were present (e.g. a chunk with no `usage` key, which is the
/// common case on every non-final chunk).
fn parse_openai_usage(v: &Value) -> Option<TokenUsage> {
    let usage = v.get("usage")?;
    let input = usage
        .get("prompt_tokens")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let output = usage
        .get("completion_tokens")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let cache_read = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    if input == 0 && output == 0 && cache_read == 0 {
        // Same all-zero contract as Anthropic: a real
        // OpenAI turn with 0 prompt + 0 completion is not
        // realistic, so an all-zero payload is treated as
        // "no usage" and the agent loop's SQL write is
        // skipped.
        return None;
    }
    Some(TokenUsage {
        input_tokens: input.min(u32::MAX as u64) as u32,
        output_tokens: output.min(u32::MAX as u64) as u32,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read.min(u32::MAX as u64) as u32,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::error::LlmError;
    use crate::llm::types::{
        ChatEvent, ChatMessage, ChatRequest, ContentBlock, MessageContent, Role,
    };
    use crate::llm::provider::wire::{
        chat_request_to_wire, strip_unsupported, wire_block_to_chat_event, WireBlock,
        WireCapabilities, WireMessage, WireRequest, WireTool,
    };

    fn cfg() -> OpenAIConfig {
        OpenAIConfig {
            base_url: "https://api.openai.com".to_string(),
            model: "gpt-4o".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            reasoning_effort: None,
        }
    }

    // ---- endpoint() ----

    #[test]
    fn endpoint_trims_trailing_slash() {
        let c = OpenAIConfig {
            base_url: "https://api.openai.com/v1/".to_string(),
            ..cfg()
        };
        // The base_url already includes `/v1`; the helper only
        // appends `/chat/completions` (no leading `/v1/`).
        assert_eq!(c.endpoint(), "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn endpoint_uses_provided_base_url() {
        let c = OpenAIConfig {
            base_url: "https://proxy.example.com/openai/v1".to_string(),
            ..cfg()
        };
        assert_eq!(
            c.endpoint(),
            "https://proxy.example.com/openai/v1/chat/completions"
        );
    }

    // BUG FIX (06-09-fix-session): real OpenAI-compatible
    // providers (the seed's `https://api.openai.com/v1` and any
    // user-added proxy like `https://hub.wukaijin.com/v1`)
    // already include the `/v1` version in `base_url`. The
    // endpoint helper must NOT add another `/v1/`, otherwise
    // the upstream 404s with `path not found: /v1/v1/chat/completions`
    // and the SSE parser never sees a stream — which is the
    // root cause of the "新 session 发送消息，闪一下变空"
    // regression. The pre-fix tests above would have caught this
    // if the seed base_url had been passed in (they hard-coded
    // `https://api.openai.com/...` without the version suffix);
    // the regression test below covers the realistic base_url
    // shape.
    #[test]
    fn endpoint_does_not_double_prefix_v1_when_base_url_includes_v1() {
        let c = OpenAIConfig {
            base_url: "https://api.openai.com/v1".to_string(),
            ..cfg()
        };
        assert_eq!(c.endpoint(), "https://api.openai.com/v1/chat/completions");

        let c = OpenAIConfig {
            base_url: "https://hub.wukaijin.com/v1".to_string(),
            ..cfg()
        };
        assert_eq!(c.endpoint(), "https://hub.wukaijin.com/v1/chat/completions");
    }

    // ---- protocol() and capabilities() ----

    #[test]
    fn openai_provider_reports_openai_capabilities_and_protocol() {
        let p = OpenAIProvider::new(cfg());
        assert_eq!(p.protocol(), ProviderProtocol::Openai);
        let caps = p.capabilities();
        assert!(caps.supports_system_prompt);
        assert!(caps.supports_tools);
        assert!(caps.supports_streaming);
    }

    #[test]
    fn openai_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenAIProvider>();
    }

    // ---- build_http_body ----

    #[test]
    fn build_http_body_system_prompt_becomes_first_message() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: Some("You are a coding agent".to_string()),
            messages: vec![WireMessage::User {
                content: "hello".to_string(),
            }],
            tools: vec![],
            reasoning_effort: None,
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are a coding agent");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "hello");
    }

    #[test]
    fn build_http_body_no_system_prompt_omits_system_message() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "hi".to_string(),
            }],
            tools: vec![],
            reasoning_effort: None,
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn build_http_body_tools_wrapped_in_function_envelope() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "x".to_string(),
            }],
            tools: vec![WireTool {
                name: "read_file".to_string(),
                description: Some("read".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            reasoning_effort: None,
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let tools = body.get("tools").and_then(|t| t.as_array()).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "read_file");
        assert_eq!(tools[0]["function"]["description"], "read");
        assert!(tools[0]["function"]["parameters"].is_object());
    }

    #[test]
    fn build_http_body_tool_results_become_role_tool_messages() {
        // The wire layer lifts `tool_result` blocks out of
        // user messages into `WireMessage::Tool`; OpenAI emits
        // each as a `role: "tool"` message with `tool_call_id`.
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![
                WireMessage::User {
                    content: "looking:".to_string(),
                },
                WireMessage::Tool {
                    tool_call_id: "call_1".to_string(),
                    content: "127.0.0.1 localhost".to_string(),
                },
            ],
            tools: vec![],
            reasoning_effort: None,
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], "call_1");
        assert_eq!(msgs[1]["content"], "127.0.0.1 localhost");
    }

    #[test]
    fn build_http_body_assistant_message_carries_text_and_tool_calls() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![
                    WireBlock::Text {
                        text: "let me read".to_string(),
                    },
                    WireBlock::ToolUse {
                        id: "call_42".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "/etc/hosts"}),
                    },
                ],
            }],
            tools: vec![],
            reasoning_effort: None,
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(msgs.len(), 1);
        let m0 = &msgs[0];
        assert_eq!(m0["role"], "assistant");
        assert_eq!(m0["content"], "let me read");
        let tcs = m0["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "call_42");
        assert_eq!(tcs[0]["function"]["name"], "read_file");
        // `arguments` is a JSON string in OpenAI's wire format.
        let args = tcs[0]["function"]["arguments"].as_str().unwrap();
        assert_eq!(args, "{\"path\":\"/etc/hosts\"}");
    }

    #[test]
    fn build_http_body_omits_tools_field_when_empty() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "x".to_string(),
            }],
            tools: vec![],
            reasoning_effort: None,
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        // `tools` should be absent (not present-but-empty) so
        // the upstream doesn't get an empty `tools: []` and
        // refuse the call.
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn build_http_body_sets_model_and_max_tokens_from_config() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(8192),
            system: None,
            messages: vec![WireMessage::User {
                content: "x".to_string(),
            }],
            tools: vec![],
            reasoning_effort: None,
        };
        let c = OpenAIConfig {
            model: "gpt-4.1".to_string(),
            max_tokens: 8192,
            ..cfg()
        };
        let body = OpenAIProvider::build_http_body(&wire, &c);
        assert_eq!(body["model"], "gpt-4.1");
        assert_eq!(body["max_tokens"], 8192);
        assert_eq!(body["stream"], true);
    }

    // ---- A4: stream_options.include_usage ----

    #[test]
    fn build_http_body_includes_stream_options_for_usage() {
        // A4 (Token Usage Tracking): the request body must
        // include `stream_options: { include_usage: true }`
        // so OpenAI sends a final `usage` chunk in the SSE
        // stream. Without this, `parse_openai_usage` never
        // sees a payload and the agent loop's per-turn
        // accumulation is skipped.
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "hi".to_string(),
            }],
            tools: vec![],
            reasoning_effort: None,
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let so = body
            .get("stream_options")
            .expect("stream_options key present");
        assert_eq!(so["include_usage"], true);
    }

    // ---- A4: parse_openai_usage ----

    #[test]
    fn parse_openai_usage_full_payload() {
        // Standard OpenAI cumulative usage chunk.
        let v = serde_json::json!({
            "usage": {
                "prompt_tokens": 200,
                "completion_tokens": 30,
                "total_tokens": 230,
                "prompt_tokens_details": { "cached_tokens": 50 }
            }
        });
        let u = parse_openai_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 200);
        assert_eq!(u.output_tokens, 30);
        // OpenAI has no cache_creation field today; the
        // normalized schema still requires a value (0).
        assert_eq!(u.cache_creation_input_tokens, 0);
        assert_eq!(u.cache_read_input_tokens, 50);
    }

    #[test]
    fn parse_openai_usage_minimal_payload() {
        // Older API version / non-caching model: no
        // `prompt_tokens_details` field.
        let v = serde_json::json!({
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 10,
                "total_tokens": 60
            }
        });
        let u = parse_openai_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 50);
        assert_eq!(u.output_tokens, 10);
        assert_eq!(u.cache_read_input_tokens, 0);
    }

    #[test]
    fn parse_openai_usage_no_usage_key_returns_none() {
        // The common case on every non-final chunk: no
        // `usage` field at all. The agent loop's per-turn
        // accumulation must NOT fire on these chunks.
        let v = serde_json::json!({
            "choices": [{
                "delta": { "content": "hello" }
            }]
        });
        assert!(parse_openai_usage(&v).is_none());
    }

    #[test]
    fn parse_openai_usage_zero_returns_none() {
        // All-zero usage → "no usage", same contract as
        // Anthropic. (See parse_anthropic_usage's docstring
        // for the rationale.)
        let v = serde_json::json!({
            "usage": {
                "prompt_tokens": 0,
                "completion_tokens": 0,
                "total_tokens": 0
            }
        });
        assert!(parse_openai_usage(&v).is_none());
    }

    #[test]
    fn parse_openai_usage_empty_prompt_tokens_details() {
        // Defensive: `prompt_tokens_details: {}` is valid
        // OpenAI; we must not crash on it.
        let v = serde_json::json!({
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "total_tokens": 120,
                "prompt_tokens_details": {}
            }
        });
        let u = parse_openai_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 20);
        assert_eq!(u.cache_read_input_tokens, 0);
    }

    // ---- cross-protocol strip behavior (integration with wire) ----

    #[test]
    fn openai_strip_drops_thinking_signature_from_anthropic_history() {
        // Simulate a session that was started on a Claude
        // model (history has Thinking+Signature blocks) and
        // the user switched to gpt-4o. The OpenAI send path
        // should drop the signature.
        let req = ChatRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 16384,
            system: Some("hi".to_string()),
            messages: vec![ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Thinking {
                        thinking: "let me think".to_string(),
                        signature: "sig_xyz".to_string(),
                    },
                    ContentBlock::Text {
                        text: "the answer".to_string(),
                    },
                ]),
            }],
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, Some("hi".to_string()));
        let caps = WireCapabilities {
            supports_thinking: false,
            supports_reasoning_effort: true,
            supports_thinking_signatures: false,
        };
        let stripped = strip_unsupported(wire.messages, &caps);
        // The signature-bearing Anthropic session has its
        // signature stripped (the visible text remains,
        // emitted as a [reasoning] comment by
        // `assistant_blocks_to_openai` so the model has some
        // context that reasoning happened).
        assert_eq!(stripped.len(), 1);
        let WireMessage::Assistant { blocks } = &stripped[0] else {
            panic!("expected Assistant")
        };
        // Reasoning kept (reasoning_effort=true), Signature dropped.
        assert!(blocks.iter().any(|b| matches!(b, WireBlock::Reasoning { .. })));
        assert!(!blocks.iter().any(|b| matches!(b, WireBlock::Signature { .. })));
        assert!(blocks.iter().any(|b| matches!(b, WireBlock::Text { .. })));
    }

    // ---- error classification on OpenAI-shaped bodies ----

    #[test]
    fn openai_401_classified_as_auth() {
        // OpenAI uses `code` (not `type`) in the error body.
        // The wire shape: { error: { message, type, code, param } }.
        let body = r#"{"error":{"message":"Incorrect API key provided","type":"error","code":"invalid_api_key"}}"#;
        let err = classify_error_response(401, body);
        assert!(matches!(err, LlmError::Auth(_)));
    }

    #[test]
    fn openai_429_classified_as_rate_limit() {
        let body = r#"{"error":{"message":"Rate limit reached","type":"error","code":"rate_limit_exceeded"}}"#;
        let err = classify_error_response(429, body);
        assert!(matches!(err, LlmError::RateLimit(_)));
    }

    #[test]
    fn openai_400_with_invalid_request_code_is_invalid() {
        let body = r#"{"error":{"message":"Invalid tool definition","type":"invalid_request_error","code":"invalid_request_error"}}"#;
        let err = classify_error_response(400, body);
        assert!(matches!(err, LlmError::InvalidRequest(_)));
    }

    #[test]
    fn openai_500_classified_as_server() {
        let body = r#"{"error":{"message":"Internal server error","type":"server_error","code":"server_error"}}"#;
        let err = classify_error_response(500, body);
        assert!(matches!(err, LlmError::Server { status: 500, .. }));
    }

    // ---- tool call accumulator (offline) ----

    #[test]
    fn build_tool_call_event_parses_accumulated_arguments_json() {
        let mut buf = ToolCallBuf {
            id: "call_42".to_string(),
            name: "read_file".to_string(),
            args_buf: r#"{"path":"/etc/hosts"}"#.to_string(),
        };
        let ev = build_tool_call_event(&mut buf, 0).expect("name is set");
        match ev {
            ChatEvent::ToolCall { id, name, input } => {
                assert_eq!(id, "call_42");
                assert_eq!(name, "read_file");
                assert_eq!(input, serde_json::json!({"path":"/etc/hosts"}));
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn build_tool_call_event_handles_partial_arguments() {
        // OpenAI streams `function.arguments` as fragments
        // that may not be valid JSON until the final chunk.
        // We tolerate partial JSON by buffering and parsing
        // at emit time. Here we verify the buffer path with
        // a complete JSON after concatenation. (The
        // backslash-escaped JSON below avoids the
        // `r#"..."#` raw-string closing-delimiter collision
        // — the JSON itself contains a trailing `"` that
        // would be mistaken for the end of the raw string.)
        let mut buf = ToolCallBuf {
            id: "call_1".to_string(),
            name: "shell".to_string(),
            args_buf: "{\"cmd\":\"".to_string(),
        };
        buf.args_buf.push_str("ls\"}");
        let ev = build_tool_call_event(&mut buf, 0).expect("name is set");
        match ev {
            ChatEvent::ToolCall { name, input, .. } => {
                assert_eq!(name, "shell");
                assert_eq!(input, serde_json::json!({"cmd": "ls"}));
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn build_tool_call_event_returns_none_without_name() {
        // Defensive: an OpenAI delta never carried a name
        // for this index. Drop the event rather than emit
        // an incomplete ToolCall.
        let mut buf = ToolCallBuf {
            id: "call_x".to_string(),
            name: String::new(),
            args_buf: "{}".to_string(),
        };
        let ev = build_tool_call_event(&mut buf, 0);
        assert!(ev.is_none());
    }

    #[test]
    fn build_tool_call_event_empty_args_buf_yields_empty_object() {
        // Defensive: no arguments at all → empty object,
        // not a parse failure.
        let mut buf = ToolCallBuf {
            id: "call_x".to_string(),
            name: "ping".to_string(),
            args_buf: String::new(),
        };
        let ev = build_tool_call_event(&mut buf, 0).expect("name is set");
        match ev {
            ChatEvent::ToolCall { input, .. } => {
                assert_eq!(input, serde_json::json!({}));
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    // ---- wire_block_to_chat_event path coverage ----

    #[test]
    fn wire_block_to_chat_event_text_path() {
        let ev = wire_block_to_chat_event(&WireBlock::Text {
            text: "hi".to_string(),
        })
        .unwrap();
        assert!(matches!(ev, ChatEvent::Delta { text } if text == "hi"));
    }

    #[test]
    fn wire_block_to_chat_event_reasoning_path() {
        let ev = wire_block_to_chat_event(&WireBlock::Reasoning {
            text: "thinking...".to_string(),
        })
        .unwrap();
        assert!(matches!(ev, ChatEvent::ThinkingDelta { text } if text == "thinking..."));
    }

    // ---- live integration test (requires hub.wukaijin.com reachable) ----

    /// Live integration test against the real MiniMax OpenAI-compatible
    /// endpoint, mirroring the user's default-model configuration.
    /// Set `EVERLASTING_RUN_LIVE_OPENAI_TEST=1` to opt in (skipped by
    /// default to keep CI fast and offline-safe).
    #[tokio::test]
    async fn live_send_against_hub_wukaijin() {
        if std::env::var("EVERLASTING_RUN_LIVE_OPENAI_TEST").is_err() {
            eprintln!("skipping live test (set EVERLASTING_RUN_LIVE_OPENAI_TEST=1 to run)");
            return;
        }
        use futures_util::StreamExt;
        let c = OpenAIConfig {
            base_url: "https://hub.wukaijin.com/v1".to_string(),
            model: "MiniMax-M3".to_string(),
            api_key: "ah-22ae6bdaec4403cfe1a10fd5f56dbe0f6eeafd71661c46ad7b43a267c2922f86".to_string(),
            max_tokens: 65536,
            reasoning_effort: None,
        };
        let p = OpenAIProvider::new(c);
        let mut s = p.send(
            Some("You are a coding agent.".to_string()),
            vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("吃了吗".to_string()),
            }],
            vec![],
        );
        let mut events = Vec::new();
        while let Some(ev) = s.next().await {
            events.push(ev);
        }
        eprintln!("=== events from live send: {} total ===", events.len());
        for (i, e) in events.iter().enumerate() {
            eprintln!("  [{}] {:?}", i, e);
        }
        // Assertions
        let mut saw_start = false;
        let mut accumulated = String::new();
        let mut saw_done = false;
        for e in &events {
            match e {
                Ok(ChatEvent::Start) => saw_start = true,
                Ok(ChatEvent::Delta { text }) => accumulated.push_str(text),
                Ok(ChatEvent::Done { .. }) => saw_done = true,
                Err(e) => panic!("got error event: {:?}", e),
                _ => {}
            }
        }
        assert!(saw_start, "expected Start event");
        assert!(saw_done, "expected Done event");
        assert!(!accumulated.is_empty(), "expected non-empty text, got: {:?}", accumulated);
    }
}
