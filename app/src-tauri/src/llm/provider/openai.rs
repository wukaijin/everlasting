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
    self, chat_request_to_wire, strip_unsupported, WireCapabilities, WireBlock, WireRequest,
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
    /// user-added proxy like `https://hub.example.com/v1`).
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
                super::wire::WireMessage::Assistant { blocks } => {
                    let (text_parts, tool_calls, reasoning) =
                        assistant_blocks_to_openai(blocks);
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
                    let is_reasoning_model = config.reasoning_effort.is_some()
                        || is_o1_family(&config.model);
                    if is_reasoning_model {
                        let rc = match reasoning {
                            Some(text) if !text.is_empty() => text,
                            _ => "none".to_string(),
                        };
                        msg["reasoning_content"] = json!(rc);
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
fn assistant_blocks_to_openai(
    blocks: &[WireBlock],
) -> (Vec<String>, Vec<Value>, Option<String>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut reasoning_parts: Vec<String> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        match b {
            WireBlock::Text { text, .. } => text_parts.push(text.clone()),
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
fn is_o1_family(model: &str) -> bool {
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
fn openai_caps(reasoning_effort: Option<&str>) -> WireCapabilities {
    WireCapabilities {
        supports_thinking: false,
        supports_reasoning_effort: reasoning_effort.is_some(),
        supports_thinking_signatures: false,
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
        let caps = openai_caps(self.config.reasoning_effort.as_deref());
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
                 tool_calls\". See llm-contract.md §469 Pair Atomicity."
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

// ---------------------------------------------------------------------------
// Tool call accumulation helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct ToolCallBuf {
    id: String,
    name: String,
    args_buf: String,
}

/// Accumulate one OpenAI `tool_calls` delta (`tc`) into the
/// per-index assembly map. OpenAI streams a tool call as a
/// sequence of deltas all keyed by the same `index`; we merge the
/// `id` / `function.name` / `function.arguments` fragments into a
/// single [`ToolCallBuf`] per index.
///
/// RULE-D-007 (2026-06-25): the official OpenAI API always emits
/// `index` on every tool_call delta. Some third-party
/// OpenAI-compatible proxies omit it — previously we fell back to
/// `0`, which made two index-less tool calls collide on key `0`
/// (the second overwrote the first's id/name and concatenated
/// arguments onto its `args_buf`). Now an index-less delta is
/// warned + skipped: the official API is unaffected, and a
/// misbehaving proxy drops the call rather than corrupting another.
fn accumulate_tool_call_delta(state: &mut HashMap<u32, ToolCallBuf>, tc: &Value) {
    let Some(idx) = tc.get("index").and_then(|i| i.as_u64()) else {
        tracing::warn!(
            tc = %serde_json::to_string(tc).unwrap_or_default(),
            "openai: tool_call delta missing `index`, skipping (third-party proxy?)"
        );
        return;
    };
    let idx = idx as u32;
    let entry = state.entry(idx).or_insert_with(ToolCallBuf::default);
    if let Some(id) = tc.get("id").and_then(|s| s.as_str()) {
        if !id.is_empty() {
            entry.id = id.to_string();
        }
    }
    if let Some(name) = tc
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(|s| s.as_str())
        .or_else(|| tc.get("name").and_then(|s| s.as_str()))
    {
        if !name.is_empty() {
            entry.name = name.to_string();
        }
    }
    if let Some(args) = tc
        .get("function")
        .and_then(|f| f.get("arguments"))
        .and_then(|s| s.as_str())
    {
        entry.args_buf.push_str(args);
    }
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
        // 2026-06-26 snapshot fix: cross-provider normalized
        // "total input for this request". OpenAI's
        // `prompt_tokens` is ALREADY inclusive of
        // `cached_tokens` (it's the full prompt length), so the
        // context footprint is just `input`. Do NOT add
        // `cache_read` here — that would double-count.
        context_input_tokens: input.min(u32::MAX as u64) as u32,
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

    /// DeepSeek-v4 config for RULE-D-006 tests. A reasoning-capable
    /// model (reasoning_effort set) so the `reasoning_content` field
    /// gate in `build_http_body` is open and the DeepSeek contract
    /// pin applies. Matches the prod deepseek-v4-flash OpenAI route.
    fn deepseek_cfg() -> OpenAIConfig {
        OpenAIConfig {
            base_url: "https://api.wukaijin.com".to_string(),
            model: "deepseek-v4-flash".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            reasoning_effort: Some("high".to_string()),
        }
    }

    // ---- openai_caps (RULE-D-005) ----

    #[test]
    fn openai_caps_derives_reasoning_effort_from_config() {
        // A model that opted into reasoning effort (o1/o3 with
        // thinking_effort set) keeps the capability.
        let caps = openai_caps(Some("high"));
        assert!(!caps.supports_thinking);
        assert!(caps.supports_reasoning_effort);
        assert!(!caps.supports_thinking_signatures);

        // A non-reasoning model (gpt-4o, no thinking_effort) must
        // NOT claim reasoning support — otherwise strip_unsupported
        // keeps historical Reasoning blocks and pollutes context.
        let caps = openai_caps(None);
        assert!(!caps.supports_reasoning_effort);
    }

    #[test]
    fn openai_caps_strip_drops_reasoning_for_non_reasoning_model() {
        // End-to-end of RULE-D-005: a gpt-4o provider (no
        // reasoning_effort) must drop Reasoning blocks during strip,
        // not keep them as the old hardcoded-true caps did.
        let messages = vec![WireMessage::Assistant {
            blocks: vec![
                WireBlock::Reasoning {
                    text: "thought".to_string(),
                },
                WireBlock::Text {
                    text: "answer".to_string(),
                    cache_control: None,
                },
            ],
        }];
        let caps = openai_caps(None);
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks } = &stripped[0] else {
            panic!("expected Assistant");
        };
        // Reasoning dropped (non-reasoning model), only Text remains.
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], WireBlock::Text { text, .. } if text == "answer"));
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
    // user-added proxy like `https://hub.example.com/v1`)
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
            base_url: "https://api.deepseek.com/v1".to_string(),
            ..cfg()
        };
        assert_eq!(c.endpoint(), "https://api.deepseek.com/v1/chat/completions");
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
                        cache_control: None,
                    },
                    WireBlock::ToolUse {
                        id: "call_42".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "/etc/hosts"}),
                    },
                ],
            }],
            tools: vec![],
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
        // RULE-D-006a regression guard: cfg() is gpt-4o with
        // reasoning_effort=None → a NON-reasoning model. The
        // `reasoning_content` field MUST be absent (not "none",
        // not "" — the field is entirely omitted to keep the
        // vanilla OpenAI shape). See `build_http_body` RULE-D-006a.
        assert!(
            m0.get("reasoning_content").is_none(),
            "gpt-4o (non-reasoning) must not carry reasoning_content: {m0}"
        );
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
        // RULE-D-002 regression guard: non-o1 models must NOT emit
        // the o1-only key.
        assert!(
            body.get("max_completion_tokens").is_none(),
            "non-o1 model must not emit max_completion_tokens: {body}"
        );
    }

    // ---- RULE-D-002: o1+ family uses max_completion_tokens ----

    #[test]
    fn is_o1_family_matches_reasoning_models() {
        // o1 line: o1 / o1-mini / o1-preview / o1-pro
        assert!(is_o1_family("o1"));
        assert!(is_o1_family("o1-mini"));
        assert!(is_o1_family("o1-preview"));
        assert!(is_o1_family("o1-pro"));
        // o3 line: o3 / o3-mini / o3-pro
        assert!(is_o1_family("o3"));
        assert!(is_o1_family("o3-mini"));
        assert!(is_o1_family("o3-pro"));
        // o4 line
        assert!(is_o1_family("o4-mini"));
        // case-insensitive (third-party gateways may emit caps)
        assert!(is_o1_family("O1-MINI"));
        assert!(is_o1_family("  o3-mini  ")); // trims whitespace
    }

    #[test]
    fn is_o1_family_rejects_non_reasoning_models() {
        assert!(!is_o1_family("gpt-4o"));
        assert!(!is_o1_family("gpt-4o-mini"));
        assert!(!is_o1_family("gpt-4.1"));
        assert!(!is_o1_family("chatgpt-4o-latest"));
        assert!(!is_o1_family("glm-4.7"));
    }

    #[test]
    fn build_http_body_o1_family_uses_max_completion_tokens() {
        let wire = WireRequest {
            model: "o1-mini".to_string(),
            max_tokens: Some(8192),
            system: None,
            messages: vec![WireMessage::User {
                content: "x".to_string(),
            }],
            tools: vec![],
        };
        let c = OpenAIConfig {
            model: "o3-mini".to_string(),
            max_tokens: 8192,
            ..cfg()
        };
        let body = OpenAIProvider::build_http_body(&wire, &c);
        // o1+ family MUST use max_completion_tokens ...
        assert_eq!(body["max_completion_tokens"], 8192);
        // ... and MUST NOT carry max_tokens (the server 400s on it).
        assert!(
            body.get("max_tokens").is_none(),
            "o1 family must not emit max_tokens (server 400s): {body}"
        );
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
        // 2026-06-26 snapshot fix: context_input_tokens = prompt_tokens
        // (= input). OpenAI's prompt_tokens is ALREADY inclusive of
        // cached_tokens, so adding cache_read here would double-count.
        // Verified: 200 (NOT 200 + 50 = 250).
        assert_eq!(u.context_input_tokens, 200);
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
        assert_eq!(u.context_input_tokens, 50);
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
                        cache_control: None,
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
        // signature stripped. The visible text remains, and
        // (post-PR1, RULE-D-006) the surviving `Reasoning` block
        // is lifted into the assistant message's top-level
        // `reasoning_content` field by `build_http_body` — see
        // `deepseek_reasoning_content_round_trip_*` tests below.
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

    // ---- accumulate_tool_call_delta (RULE-D-007) ----

    #[test]
    fn accumulate_tool_call_delta_skips_delta_missing_index() {
        // RULE-D-007: a tool_call delta without `index` is skipped
        // rather than falling back to key 0 (which would collide
        // with a real index-0 tool call and corrupt it).
        let mut state: HashMap<u32, ToolCallBuf> = HashMap::new();
        // first delta carries index 0
        accumulate_tool_call_delta(
            &mut state,
            &serde_json::json!({"index":0,"id":"call_a","function":{"name":"read_file","arguments":"{\"path\":"}}),
        );
        // second delta omits index (the bug surface)
        accumulate_tool_call_delta(
            &mut state,
            &serde_json::json!({"id":"call_b","function":{"name":"write_file","arguments":"\"x\""}}),
        );
        // only idx 0 present; second delta dropped, not collided
        assert_eq!(state.len(), 1, "index-less delta must not create an entry");
        let buf = &state[&0];
        assert_eq!(buf.id, "call_a");
        assert_eq!(buf.name, "read_file");
        assert_eq!(buf.args_buf, "{\"path\":");
    }

    #[test]
    fn accumulate_tool_call_delta_merges_same_index_fragments() {
        // Regression guard: the normal OpenAI contract — many deltas
        // sharing one `index` — still merges into one buffer.
        let mut state: HashMap<u32, ToolCallBuf> = HashMap::new();
        accumulate_tool_call_delta(
            &mut state,
            &serde_json::json!({"index":1,"function":{"name":"grep","arguments":"{\"a\":"}}),
        );
        accumulate_tool_call_delta(
            &mut state,
            &serde_json::json!({"index":1,"id":"call_1","function":{"arguments":"\"b\"}"}}),
        );
        let buf = &state[&1];
        assert_eq!(buf.id, "call_1");
        assert_eq!(buf.name, "grep");
        assert_eq!(buf.args_buf, "{\"a\":\"b\"}");
    }

    // ---- wire_block_to_chat_event path coverage ----

    #[test]
    fn wire_block_to_chat_event_text_path() {
        let ev = wire_block_to_chat_event(&WireBlock::Text {
            text: "hi".to_string(),
            cache_control: None,
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

    // ---- RULE-D-006 (2026-06-21): DeepSeek reasoning_content round-trip ----
    //
    // PR1 of `06-21-route-deepseek-via-openai-protocol-for-native-reasoning-content`.
    // Pre-PR1 the OpenAI adapter prepended `Reasoning` text into the content
    // string as `format!("[reasoning] {}", text)` (a hidden-comment marker).
    // That polluted the visible answer every turn and didn't satisfy
    // DeepSeek v4's `reasoning_content` field contract. PR1 lifts the
    // reasoning text into a dedicated top-level `reasoning_content` field
    // on the assistant message, sibling of `content`. Pure-text assistant
    // turns (worker memory acks, plain replies) get `reasoning_content:"none"`
    // (literal non-empty string — AstrBot PR 7823's choice for DeepSeek v4
    // strictness; harmless on real OpenAI o1/o3 which ignore unknown fields).

    #[test]
    fn reasoning_block_becomes_reasoning_content_field() {
        // The core PR1 invariant: a Reasoning block is lifted to the
        // top-level `reasoning_content` field, NOT prepended into content.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![
                    WireBlock::Reasoning {
                        text: "step 1: analyze".to_string(),
                    },
                    WireBlock::Text {
                        text: "the answer".to_string(),
                        cache_control: None,
                    },
                ],
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(msgs.len(), 1);
        let m0 = &msgs[0];
        assert_eq!(m0["role"], "assistant");
        // content carries ONLY the visible text — no `[reasoning]` marker.
        assert_eq!(m0["content"], "the answer");
        // reasoning_content carries the reasoning text verbatim.
        assert_eq!(m0["reasoning_content"], "step 1: analyze");
        // Negative regression guard: content must NOT contain the marker.
        let content_str = m0["content"].as_str().unwrap();
        assert!(
            !content_str.contains("[reasoning]"),
            "content must not carry the pre-PR1 marker: {content_str}"
        );
    }

    #[test]
    fn text_only_assistant_gets_none_reasoning_content() {
        // Pure-text assistant (worker memory ack, plain reply): no
        // reasoning block to lift. DeepSeek v4 still wants a non-empty
        // `reasoning_content` field (AstrBot PR 7823 contract), so emit
        // the literal string `"none"` — never `""`, never absent.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![WireBlock::Text {
                    text: "Understood.".to_string(),
                    cache_control: None,
                }],
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["content"], "Understood.");
        assert_eq!(m0["reasoning_content"], "none");
    }

    #[test]
    fn multiple_reasoning_blocks_joined_with_newline() {
        // The wire layer splits an Anthropic `Thinking` block into
        // `Reasoning` + `Signature`. After strip drops the signature,
        // multiple surviving `Reasoning` blocks (rare but possible when
        // an assistant turn had several Thinking blocks) are joined with
        // `\n` — matches the AstrBot PR 7823 convention and the live T4
        // probe (multi-line reasoning_content → 200).
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![
                    WireBlock::Reasoning {
                        text: "first thought".to_string(),
                    },
                    WireBlock::Reasoning {
                        text: "second thought".to_string(),
                    },
                    WireBlock::Text {
                        text: "final".to_string(),
                        cache_control: None,
                    },
                ],
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["reasoning_content"], "first thought\nsecond thought");
        assert_eq!(m0["content"], "final");
    }

    #[test]
    fn user_message_does_not_get_reasoning_content_field() {
        // Only assistant messages carry reasoning_content. A user
        // message must NOT get the field — it would be semantically
        // wrong (user messages have no reasoning) and could confuse
        // strict upstream validators.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "hi there".to_string(),
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["role"], "user");
        assert!(
            m0.get("reasoning_content").is_none(),
            "user message must not carry reasoning_content: {m0}"
        );
    }

    #[test]
    fn tool_message_does_not_get_reasoning_content_field() {
        // Same invariant for `role: "tool"` results.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Tool {
                tool_call_id: "call_1".to_string(),
                content: "result body".to_string(),
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["role"], "tool");
        assert!(
            m0.get("reasoning_content").is_none(),
            "tool message must not carry reasoning_content: {m0}"
        );
    }

    #[test]
    fn assistant_with_tool_use_only_gets_none_reasoning_content() {
        // An assistant turn that issued a tool call but produced no
        // reasoning (e.g. a deterministic tool dispatch) still needs
        // a non-empty `reasoning_content` for DeepSeek v4 — `"none"`.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![
                    WireBlock::Text {
                        text: "reading file".to_string(),
                        cache_control: None,
                    },
                    WireBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "/x"}),
                    },
                ],
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["role"], "assistant");
        assert_eq!(m0["content"], "reading file");
        assert_eq!(m0["reasoning_content"], "none");
        // tool_calls still emitted alongside reasoning_content.
        assert_eq!(m0["tool_calls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn gpt_4o_does_not_get_reasoning_content_field_even_with_reasoning_block() {
        // RULE-D-006a regression guard: the `reasoning_content` field
        // gate is based on the MODEL config (reasoning_effort /
        // is_o1_family), NOT on whether the wire payload happens to
        // carry a Reasoning block. On a non-reasoning OpenAI model
        // (gpt-4o with no reasoning_effort) the field is NEVER injected,
        // even if a Reasoning block survived into `build_http_body`
        // (e.g. a future caller forgets to strip). This keeps the
        // vanilla OpenAI request shape clean — `reasoning_content` is
        // a provider-specific extension, not a documented OpenAI field,
        // and carrying it on a gpt-4o request is a latent compat bug
        // against proxies that reserve the name.
        //
        // (In normal operation `strip_unsupported` already drops the
        // Reasoning block before this point for gpt-4o — see
        // `openai_caps_strip_drops_reasoning_for_non_reasoning_model`.
        // This test defends the build_http_body gate independently.)
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(), // wire.model is ignored by build_http_body
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![
                    WireBlock::Reasoning {
                        text: "sneaky reasoning that should not lift".to_string(),
                    },
                    WireBlock::Text {
                        text: "answer".to_string(),
                        cache_control: None,
                    },
                ],
            }],
            tools: vec![],
        };
        // cfg() = gpt-4o, reasoning_effort=None → non-reasoning model.
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["role"], "assistant");
        // The Reasoning text MUST NOT leak into content either (the
        // pre-PR1 `[reasoning]` marker is gone, and the field is gated
        // off so nothing carries the text).
        assert_eq!(m0["content"], "answer");
        let content_str = m0["content"].as_str().unwrap();
        assert!(
            !content_str.contains("sneaky reasoning"),
            "gpt-4o content must not carry reasoning text: {content_str}"
        );
        // The field is entirely absent — not "none", not "".
        assert!(
            m0.get("reasoning_content").is_none(),
            "gpt-4o (non-reasoning) must not carry reasoning_content even with a Reasoning block: {m0}"
        );
    }

    #[test]
    fn o1_family_gets_reasoning_content_field_without_explicit_effort() {
        // RULE-D-006a: an o1-family model is reasoning-capable even
        // without an explicit reasoning_effort set (the family itself
        // is the opt-in signal). The field gate must open for it so
        // o1/o3 history round-trips carry `reasoning_content`.
        let o1_cfg = OpenAIConfig {
            base_url: "https://api.openai.com".to_string(),
            model: "o1-mini".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            reasoning_effort: None, // o1 family is the signal, not effort
        };
        let wire = WireRequest {
            model: "o1-mini".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![WireBlock::Text {
                    text: "ok".to_string(),
                    cache_control: None,
                }],
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &o1_cfg);
        let m0 = &body["messages"].as_array().unwrap()[0];
        // No reasoning block → "none" (o1 family is reasoning-capable).
        assert_eq!(m0["reasoning_content"], "none");
    }

    // ---- DeepSeek v4 reasoning_content contract pin ----
    //
    // This is the PR1 acceptance contract: every assistant message in
    // the history that goes on the wire to a DeepSeek-v4 model MUST
    // carry a non-empty `reasoning_content` field. Verified live
    // (2026-06-21, wukaijin OpenAI endpoint, deepseek-v4-flash):
    //
    //   T1  no field            → 200 (lenient today; AstrBot says strict)
    //   T2  `"none"`            → 200
    //   T3  `""`                → 200 (today; AstrBot says this is rejected)
    //   T4  multi-line non-empty → 200
    //
    // We pin the AstrBot/stricter shape: every assistant has a
    // non-empty `reasoning_content`. If a future change regresses to
    // empty/missing, this test fails before the user sees a 400.

    #[test]
    fn deepseek_reasoning_content_contract_pin_mixed_history() {
        // Construct a realistic multi-turn DeepSeek history:
        //   turn 1 user      — greeting
        //   turn 2 assistant — pure text ack (worker memory ack)
        //   turn 3 user      — real question
        //   turn 4 assistant — full reasoning + answer
        //   turn 5 user      — follow-up
        // The contract: BOTH assistant turns (2 and 4) must carry
        // non-empty `reasoning_content`. Turn 2 has no reasoning block
        // → `"none"`. Turn 4 has reasoning → the joined text.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: Some("You are a coding agent.".to_string()),
            messages: vec![
                WireMessage::User {
                    content: "remember: project uses pnpm".to_string(),
                },
                WireMessage::Assistant {
                    blocks: vec![WireBlock::Text {
                        text: "Understood.".to_string(),
                        cache_control: None,
                    }],
                },
                WireMessage::User {
                    content: "how do I run tests?".to_string(),
                },
                WireMessage::Assistant {
                    blocks: vec![
                        WireBlock::Reasoning {
                            text: "user asked about tests; project uses pnpm".to_string(),
                        },
                        WireBlock::Text {
                            text: "Run `pnpm test`.".to_string(),
                            cache_control: None,
                        },
                    ],
                },
                WireMessage::User {
                    content: "thanks".to_string(),
                },
            ],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        // system + 5 user/assistant turns = 6 total.
        assert_eq!(msgs.len(), 6);
        // System message: no reasoning_content.
        assert_eq!(msgs[0]["role"], "system");
        assert!(msgs[0].get("reasoning_content").is_none());

        // Walk every message and assert the contract.
        for (i, m) in msgs.iter().enumerate() {
            let role = m["role"].as_str().unwrap();
            match role {
                "assistant" => {
                    let rc = m
                        .get("reasoning_content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_else(|| panic!("msg {i}: assistant missing reasoning_content"));
                    assert!(
                        !rc.is_empty(),
                        "msg {i}: assistant reasoning_content must be non-empty (got \"\")"
                    );
                }
                "user" | "system" | "tool" => {
                    assert!(
                        m.get("reasoning_content").is_none(),
                        "msg {i}: {role} message must not carry reasoning_content: {m}"
                    );
                }
                other => panic!("msg {i}: unexpected role {other}"),
            }
        }
        // Spot-check turn 2 (the ack) is `"none"`...
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[2]["reasoning_content"], "none");
        // ...and turn 4 (the real reasoning) is the joined text.
        assert_eq!(msgs[4]["role"], "assistant");
        assert_eq!(
            msgs[4]["reasoning_content"],
            "user asked about tests; project uses pnpm"
        );
    }

    // ---- live integration test (env-gated, no hardcoded endpoint) ----

    /// Live integration smoke test against any OpenAI-compatible endpoint.
    /// Off by default. Opt in by setting all of:
    /// - `EVERLASTING_RUN_LIVE_OPENAI_TEST=1` (master switch)
    /// - `EVERLASTING_LIVE_OPENAI_BASE_URL` (e.g. `https://api.openai.com/v1`)
    /// - `EVERLASTING_LIVE_OPENAI_API_KEY`
    /// Optional: `EVERLASTING_LIVE_OPENAI_MODEL` (default `"test-model"`).
    /// Missing any of those prints a one-line notice and returns success
    /// — keeps CI fast, offline-safe, and free of committed
    /// secrets/endpoints.
    #[tokio::test]
    async fn live_openai_compat_smoke_test() {
        if std::env::var("EVERLASTING_RUN_LIVE_OPENAI_TEST").is_err() {
            eprintln!(
                "skipping live test (set EVERLASTING_RUN_LIVE_OPENAI_TEST=1 plus \
                 EVERLASTING_LIVE_OPENAI_BASE_URL / EVERLASTING_LIVE_OPENAI_API_KEY / \
                 EVERLASTING_LIVE_OPENAI_MODEL to run)"
            );
            return;
        }
        let base_url = match std::env::var("EVERLASTING_LIVE_OPENAI_BASE_URL") {
            Ok(v) if !v.is_empty() => v,
            _ => {
                eprintln!("skipping live test (EVERLASTING_LIVE_OPENAI_BASE_URL not set or empty)");
                return;
            }
        };
        let api_key = match std::env::var("EVERLASTING_LIVE_OPENAI_API_KEY") {
            Ok(v) if !v.is_empty() => v,
            _ => {
                eprintln!("skipping live test (EVERLASTING_LIVE_OPENAI_API_KEY not set or empty)");
                return;
            }
        };
        let model = std::env::var("EVERLASTING_LIVE_OPENAI_MODEL")
            .unwrap_or_else(|_| "test-model".to_string());
        use futures_util::StreamExt;
        let c = OpenAIConfig {
            base_url,
            model,
            api_key,
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
