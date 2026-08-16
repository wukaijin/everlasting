//! Anthropic Messages API provider (PR2 of the multi-model task).
//!
//! This is the PR2 successor to the step 1/2/6 Anthropic-only
//! `client.rs`. The HTTP + SSE + error-classification logic is
//! unchanged; it is now wrapped behind the [`Provider`] trait so the
//! chat command can dispatch through the catalog (`ProviderRow` +
//! `ModelRow`) instead of a single env-derived `LlmConfig`.
//!
//! Per the PR2 PRD: behavior must be 1:1 identical to the legacy
//! `chat_stream_with_tools` for every Anthropic request:
//! - URL = `provider.base_url + "/v1/messages"`
//! - headers = `x-api-key: <provider.api_key>` +
//!   `anthropic-version: 2023-06-01`
//! - `thinking` field is always
//!   `{type: "adaptive", display: "summarized", effort: <model.thinking_effort || "high">}`
//! - the 4 HACKING-llm pitfalls are preserved (GLM compat,
//!   thinking signature round-trip, `display: "summarized"` for
//!   Opus 4.7+, orphan tool_use handling).
//!
//! Implementation notes:
//! - `LlmConfig` is PRIVATE to this module (it's the
//!   Anthropic-adapter's config). The chat command never constructs
//!   it directly; the factory in `mod.rs` builds it from catalog rows.
//! - `chat_stream_with_tools` is a private free function reused by
//!   `AnthropicProvider::send`. The public surface of the
//!   `chat` command is now `provider.send(system, messages, tools)`.

use async_stream::stream;
use futures_util::{Stream, StreamExt};
use std::pin::Pin;

use super::wire::{
    chat_request_to_wire, strip_unsupported, wire_messages_to_chat_messages, WireCapabilities,
};
use super::{Provider, ProviderCapabilities, ProviderProtocol};
use crate::llm::error::LlmError;
use crate::llm::sse::SseParser;
use crate::llm::types::{ChatEvent, ChatMessage, ChatRequest, ThinkingConfig, TokenUsage, ToolDef};

/// Default `max_tokens` for LLM requests. Bumped from 1024 → 16384 in
/// step 6 because extended thinking tokens count against the same budget
/// as the actual answer — 1024 was too low and would have caused
/// `stop_reason: "max_tokens"` on most non-trivial turns.
///
/// `pub(crate)` so the `build_provider` factory (`provider::mod`) reuses
/// this single source of truth for the catalog default instead of
/// duplicating the literal `16384`.
pub(crate) const DEFAULT_MAX_TOKENS: u32 = 16384;

pub(crate) mod events;
pub(crate) mod transport;

#[allow(unused_imports)]
pub(crate) use events::*;
#[allow(unused_imports)]
pub(crate) use transport::*;

/// Configuration for the Anthropic adapter. Constructed by
/// `build_provider` from a `ProviderRow` + `ModelRow` (catalog rows);
/// there is no env-derived fallback anymore.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub max_tokens: u32,
    /// `effort` value for adaptive thinking. `low` / `medium` / `high`
    /// / `xhigh` / `max` (Anthropic schema). Defaults to `"high"`.
    pub thinking_effort: String,
    /// B1 (2026-08-16): from `ModelRow.supports_images` — gates the
    /// wire strip's image→text-placeholder degradation for this model.
    pub supports_images: bool,
}

impl LlmConfig {
    pub fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    /// Build the `thinking` field we always send with the request.
    /// Adaptive mode (Opus 4.7 / 4.8) — `display: "summarized"` is
    /// explicit so that `thinking_delta` SSE events actually flow
    /// (otherwise the default `display: "omitted"` on those models would
    /// drop the summary text).
    pub(crate) fn thinking_config(&self) -> ThinkingConfig {
        ThinkingConfig::Adaptive {
            display: "summarized".to_string(),
            effort: self.thinking_effort.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// BlockState — tracks what content block is being streamed
// ---------------------------------------------------------------------------

pub struct AnthropicProvider {
    config: LlmConfig,
}

impl AnthropicProvider {
    pub fn new(config: LlmConfig) -> Self {
        Self { config }
    }

    /// Stream chat completions, optionally with tool definitions and a system prompt.
    ///
    /// `req` is the fully-built Anthropic Messages API request body
    /// (the caller — `AnthropicProvider::send` — has already run it
    /// through the wire layer, set `thinking`, and reconstructed
    /// the Anthropic-shaped messages). The body is logged verbatim
    /// with the model / tool count / system-prompt presence so
    /// observability is preserved 1:1 with pre-PR3.
    ///
    /// Always emits `ChatEvent::Start` first on success, then a series of
    /// `Delta`s / `ThinkingDelta`s / `SignatureDelta`s / `ToolCall`s, then
    /// `Done` at the end.
    /// 提取请求体的 observability 字段(model / tools 数 / system 有无),
    /// 供 `→ LLM request` 日志使用(提取自 chat_stream_with_tools 阶段 0,
    /// 08-08-a-class-anthropic-split)。
    fn chat_stream_with_tools(
        config: LlmConfig,
        body: serde_json::Value,
    ) -> impl Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static {
        let url = config.endpoint();

        stream! {
            // 阶段 A-D:client 构建 + 请求日志 + HTTP 发送 + 非 2xx 检查
            // (提取至 `send_request`;错误统一经 Err 返回,yield 点收敛于此)。
            let resp = match send_request(&config, &url, &body).await {
                Ok(r) => r,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            tracing::info!("← LLM stream opened");
            yield Ok(ChatEvent::Start);

            let mut byte_stream = resp.bytes_stream();
            let mut parser = SseParser::new();
            let mut block_state = BlockState::Idle;
            let mut stop_reason: Option<String> = None;
            // A4: buffer Anthropic's `message_delta.usage` payload
            // and emit it on the final `Done` event. Anthropic
            // sends usage on the `message_delta` event (or
            // sometimes on `message_stop`); some proxies also
            // attach a `usage` field to the SSE `message_start`
            // event. We treat `message_delta.usage` as the
            // authoritative source (it's the cumulative usage
            // for the turn) and a `message_start.usage` (if
            // present) as the initial baseline that subsequent
            // `message_delta.usage` overwrites. See
            // `parse_anthropic_usage` for the per-field handling.
            let mut usage: Option<TokenUsage> = None;

            while let Some(chunk_result) = byte_stream.next().await {
                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(LlmError::Network(format!("stream read: {}", e)));
                        return;
                    }
                };
                let text = match std::str::from_utf8(&bytes) {
                    Ok(t) => t,
                    Err(e) => {
                        yield Err(LlmError::Network(format!("non-utf8 chunk: {}", e)));
                        return;
                    }
                };

                for event in parser.feed(text) {
                    match event.event.as_str() {
                        // --- content_block_start: begin a new block ---
                        "content_block_start" => {
                            handle_content_block_start(&event.data, &mut block_state);
                        }

                        // --- content_block_delta: incremental data ---
                        "content_block_delta" => {
                            if let Some(ev) =
                                handle_content_block_delta(&event.data, &mut block_state)
                            {
                                yield Ok(ev);
                            }
                        }

                        // --- content_block_stop: finish a block ---
                        "content_block_stop" => {
                            if let Some(ev) = handle_content_block_stop(&mut block_state) {
                                yield Ok(ev);
                            }
                        }

                        // --- message_delta: extract stop_reason + usage ---
                        "message_delta" => {
                            handle_message_delta(&event.data, &mut stop_reason, &mut usage);
                        }

                        "message_start" => {
                            handle_message_start(&event.data, &mut usage);
                        }

                        "message_stop" => {
                            tracing::debug!("▶ message_stop");
                        }
                        "ping" => {
                            tracing::debug!("▶ ping (heartbeat, ignored)");
                        }
                        other => {
                            tracing::debug!("▶ {} (unhandled)", other);
                        }
                    }
                }
            }

            yield Ok(ChatEvent::Done { stop_reason, usage });
        }
    }
}

/// DeepSeek-Via-Anthropic-Relay (wukaijin.com passthrough) thinking
/// block fix (task 06-20-deepseek-reasoner-reasoning-content-400 +
/// follow-up 06-21-fix-deepseek-relay-thinking-block-drop-causing-turn-2-400).
///
/// Background: the wukaijin.com relay does a thin passthrough of
/// Anthropic's `/v1/messages` schema to DeepSeek V4. The relay's
/// thinking-mode contract (verified via V1/V2/V3 probe experiments
/// against the real relay, see prd of
/// `06-21-fix-deepseek-relay-thinking-block-drop-causing-turn-2-400`)
/// is:
///
/// | assistant shape returned on next turn               | relay response |
/// | --------------------------------------------------- | -------------- |
/// | `content[].thinking` blocks dropped                | 400 `content[].thinking must be passed back` |
/// | `content[].thinking` blocks kept, NO `reasoning_content` | 400 `reasoning_content must be passed back` |
/// | `content[].thinking` blocks kept, WITH `reasoning_content` | **200 ✅** |
///
/// In other words the relay requires **both** `content[].thinking`
/// blocks **and** a top-level `reasoning_content` field, and the
/// signature is NOT cryptographically verified (empty-signature
/// blocks are accepted by the relay). The original task 06-20 fix
/// dropped empty-signature thinking blocks (an unverified attribution
/// — "empty sig inflates the relay's accumulated-state count" — that
/// turned out to be wrong on real-relay probing) and that drop
/// produced the new turn-2 400 `content[].thinking must be passed
/// back`.
///
/// The corrected contract is: keep every `thinking` block verbatim
/// (empty-signature or not), AND lift a top-level `reasoning_content`
/// field on assistant messages whose collected `thinking` text is
/// non-empty.
///
/// This helper applies that single surgical patch to the
/// Anthropic-shaped request body so the same wire payload is also
/// DeepSeek-Via-relay-friendly, while staying invisible to the native
/// Anthropic API (which ignores unknown top-level fields on assistant
/// messages):
///
/// **Lift `reasoning_content` from every thinking block** — for each
/// assistant message that has at least one `thinking` block whose
/// `thinking` text is non-empty, add a top-level `reasoning_content`
/// string field whose value is the concatenation of **all** thinking
/// blocks' `thinking` text (joined by `\n`). Empty-signature blocks
/// contribute their text too (the relay doesn't verify signatures,
/// and dropping them was the turn-2 regression).
///
/// Native Anthropic Claude path stays 1:1 with the pre-fix body in
/// all observable ways: every `thinking` block is preserved verbatim,
/// the top-level `thinking: adaptive` field is untouched (Claude
/// extended thinking needs it), and the only added field on assistant
/// messages is `reasoning_content`, which Anthropic ignores.
///
/// Pure function: takes a borrowed [`ChatRequest`], returns the
/// transformed [`serde_json::Value`] body. No IO, no allocation
/// beyond the JSON tree. Tested by
/// `deepseek_reasoning_fix_tests::*` — see the test module at the
/// bottom of this file.
pub(crate) fn apply_deepseek_reasoning_fix(req: &ChatRequest) -> serde_json::Value {
    let mut body =
        serde_json::to_value(req).expect("ChatRequest → serde_json::Value is infallible");
    let messages = match body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        Some(arr) => arr,
        None => return body,
    };
    for msg in messages.iter_mut() {
        // Only assistant-role messages carry thinking blocks.
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        // `content` may be either a string (pre-step-2 back-compat) or
        // an array of blocks. The thinking-block handling only applies
        // to the array form — a plain-string content has no blocks to
        // walk. The `reasoning_content` top-level field is still safe
        // to add (relays that care about it read it as a sibling of
        // `content` regardless of shape), but there's no `thinking`
        // text to extract from a string content, so we skip the whole
        // message.
        let arr = match msg.get_mut("content").and_then(|c| c.as_array_mut()) {
            Some(a) => a,
            None => continue,
        };
        // (A) Collect the `thinking` text of ALL thinking blocks —
        // empty-signature blocks INCLUDED. The wukaijin relay requires
        // `content[].thinking` blocks AND a top-level `reasoning_content`
        // field together (verified by V1/V2/V3 probe experiments; see
        // the task 06-21 prd for the table). Dropping empty-signature
        // blocks was the turn-2 regression root cause and must NOT
        // return. The signature is not cryptographically verified by
        // the relay, so an empty signature is not a drop signal.
        let mut reasoning_buf = String::new();
        for block in arr.iter() {
            if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                if let Some(text) = block.get("thinking").and_then(|t| t.as_str()) {
                    if !reasoning_buf.is_empty() {
                        reasoning_buf.push('\n');
                    }
                    reasoning_buf.push_str(text);
                }
            }
        }
        // Only attach `reasoning_content` when we actually have
        // non-empty reasoning text to attach. A message with zero
        // thinking blocks (pure text + tool_use) must NOT gain a
        // `reasoning_content: ""` field — that would be a sentinel
        // the relay would mismatch against the actual content shape,
        // so we omit the field entirely. Assistant messages that DO
        // carry thinking blocks always get the field (their collected
        // text is non-empty by construction unless every block had an
        // empty `thinking` string, in which case there's still nothing
        // useful to lift).
        if !reasoning_buf.is_empty() {
            msg["reasoning_content"] = serde_json::Value::String(reasoning_buf);
        }
    }
    body
}

/// Group chat (07-29-group-chat, 2026-07-31): inject speaker
/// identity as an `@name:` text prefix and strip the raw `speaker`
/// field. Anthropic's Messages API has no native `name` field
/// (unlike OpenAI), so a participant's identity must be folded
/// into the visible content. The `speaker` field itself (serialized
/// from [`crate::llm::types::ChatMessage::speaker`]) is an unknown
/// field to Anthropic and would cause a 400, so it is always
/// removed — even when no prefix is injected (classic-chat rows
/// never carry `speaker`, so this is a no-op there).
///
/// Handles both content shapes Anthropic accepts:
/// - string content → `format!("@{}: {}", name, original)`
/// - array content → prepend a `Text` block carrying the prefix
///
/// Mirrors `apply_deepseek_reasoning_fix`'s "walk the serialized
/// JSON body in place" pattern (cheaper than re-deriving the
/// ChatRequest, and the body is already a serde_json::Value here).
pub(crate) fn apply_speaker_prefix(body: &mut serde_json::Value) {
    let messages = match body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        Some(arr) => arr,
        None => return,
    };
    for msg in messages.iter_mut() {
        // Take ownership of the speaker field, removing it from the
        // object (so Anthropic never sees an unknown field).
        let speaker = match msg.get_mut("speaker") {
            Some(v) => v.take(),
            None => continue, // no speaker field — nothing to inject
        };
        let speaker = match speaker.as_str() {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => {
                // speaker was null/empty — still must remove the key.
                if let Some(obj) = msg.as_object_mut() {
                    obj.remove("speaker");
                }
                continue;
            }
        };
        // Remove the speaker key (take() left it as null in place).
        if let Some(obj) = msg.as_object_mut() {
            obj.remove("speaker");
        }
        let prefix = format!("@{}: ", speaker);
        // Inject the prefix into content (string or array form).
        match msg.get_mut("content") {
            Some(serde_json::Value::String(s)) => {
                s.insert_str(0, &prefix);
            }
            Some(serde_json::Value::Array(blocks)) => {
                // Prepend a dedicated text block so we never mutate
                // an existing block's structure (e.g. a tool_result).
                blocks.insert(0, serde_json::json!({ "type": "text", "text": prefix }));
            }
            _ => {
                // content missing or null — nothing to prefix (rare).
            }
        }
    }
}

/// Parse Anthropic's `usage` payload into a protocol-agnostic
/// [`TokenUsage`]. Defensive: any of the four fields may be missing
/// (older Anthropic API versions / proxies only emitted a subset);
/// missing fields default to 0. Returns `None` if no recognizable
/// integer fields were present (e.g. a totally malformed payload).
pub(crate) fn parse_anthropic_usage(v: &serde_json::Value) -> Option<TokenUsage> {
    let input = v.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
    let output = v.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
    let cache_creation = v
        .get("cache_creation_input_tokens")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let cache_read = v
        .get("cache_read_input_tokens")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    if input == 0 && output == 0 && cache_creation == 0 && cache_read == 0 {
        // Distinguish "no usage payload" from "all-zero usage".
        // A real Anthropic response with 0 input/output is
        // extremely unlikely (a `0` `output_tokens` is only
        // possible on `stop_reason: "max_tokens"` hitting the
        // thinking budget before any visible answer, which is
        // a server-config issue, not a normal case). Treat
        // all-zero as "no payload" so the agent loop sees
        // `usage: None` and skips the SQL write.
        return None;
    }
    Some(TokenUsage {
        input_tokens: input.min(u32::MAX as u64) as u32,
        output_tokens: output.min(u32::MAX as u64) as u32,
        cache_creation_input_tokens: cache_creation.min(u32::MAX as u64) as u32,
        cache_read_input_tokens: cache_read.min(u32::MAX as u64) as u32,
        // 2026-06-26 snapshot fix: cross-provider normalized
        // "total input for this request". Anthropic's
        // `input_tokens` EXCLUDES cache reads/creations (it's the
        // uncached-input billable line), so the true context
        // footprint is the sum of all three. The u64 sum is
        // saturated to `u32::MAX` to match the field type.
        context_input_tokens: (input + cache_creation + cache_read).min(u32::MAX as u64) as u32,
    })
}

impl Provider for AnthropicProvider {
    fn send(
        &self,
        system: Option<String>,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDef>,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static>> {
        // Each `send` call constructs a fresh `LlmConfig` clone so
        // the provider's `&self` config is read-only (Provider is
        // `Send + Sync` so concurrent access must be safe). The
        // clone is cheap (5 String fields) and the inner `async_stream`
        // owns the config for the lifetime of the stream.
        let config = self.config.clone();

        // PR3 cross-protocol symmetry: the wire layer
        // (`provider::wire`) is the single place that knows how to
        // map the Anthropic-shaped `ChatRequest` to /
        // from a provider-agnostic representation. We run the
        // request through the wire layer first so:
        //
        // 1. The Anthropic provider is architecturally symmetric
        //    with the OpenAI provider (decision D1 of the PR3
        //    spec). Future protocols (Gemini, Ollama) plug in
        //    identically.
        // 2. Cross-protocol strip runs once and is observable in
        //    the Anthropic path's request payload too — if a
        //    future caller hands the Anthropic provider a request
        //    that includes non-Anthropic blocks, they'd be
        //    dropped at the wire layer rather than reaching the
        //    legacy `chat_stream_with_tools` parser and crashing.
        //
        // The wire layer's inverse path (`wire_messages_to_chat_messages`)
        // reconstitutes the Anthropic-shaped `ChatRequest` that
        // the legacy SSE parser understands, so the rest of the
        // call chain is byte-for-byte the same as pre-PR3.
        let req = ChatRequest {
            model: config.model.clone(),
            max_tokens: config.max_tokens,
            messages,
            system: system.clone(),
            stream: true,
            tools,
            thinking: None,
        };
        let mut wire = chat_request_to_wire(req, system);
        // Anthropic target: the protocol itself supports everything,
        // but model-level caps still apply — B1 (2026-08-16) threads
        // `ModelRow.supports_images` through `LlmConfig` so a
        // non-vision Anthropic-protocol model gets the image→text
        // placeholder degradation in `strip_unsupported` (the single
        // place that encodes strip rules).
        let caps = WireCapabilities {
            supports_thinking: true,
            supports_reasoning_effort: true,
            supports_thinking_signatures: true,
            supports_images: config.supports_images,
        };
        wire.messages = strip_unsupported(wire.messages, &caps);
        // Reconstruct the Anthropic-shaped ChatRequest that
        // `chat_stream_with_tools` consumes. The wire
        // round-trip preserves the same field set; the only
        // structural change is that `tool_result` blocks
        // lifted into `WireMessage::Tool` come back as
        // separate `role: "user"` messages with one
        // `tool_result` block each (the inverse of
        // `chat_message_to_wire_messages`).
        let req = ChatRequest {
            model: wire.model,
            max_tokens: wire.max_tokens.unwrap_or(config.max_tokens),
            messages: wire_messages_to_chat_messages(wire.messages),
            system: wire.system,
            stream: true,
            tools: wire
                .tools
                .into_iter()
                .map(|t| ToolDef {
                    name: t.name,
                    description: t.description,
                    input_schema: t.input_schema,
                })
                .collect(),
            thinking: Some(config.thinking_config()),
        };

        // DeepSeek-Via-Anthropic-Relay (wukaijin.com passthrough)
        // thinking block fix (task 06-20-deepseek-reasoner-reasoning-content-400):
        // The wukaijin.com relay does a thin passthrough of Anthropic's
        // `/v1/messages` schema to DeepSeek V4. DeepSeek V4's thinking
        // mode contract requires every assistant message to carry a
        // top-level `reasoning_content` field (sibling of `content`)
        // — Anthropic's standard `thinking` block + `signature` blob
        // alone is not enough, and the relay's accumulated-state check
        // surfaces as a 400 with the message
        // `"The reasoning_content in the thinking mode must be passed
        // back to the API."`.
        //
        // Two surgical patches make the Anthropic-shaped body also
        // DeepSeek-Via-relay-friendly, while staying invisible to the
        // native Anthropic API (which ignores unknown top-level fields
        // on assistant messages):
        //
        // (A) For every assistant message that has at least one
        //     **non-empty-signature** `thinking` block, add a
        //     top-level `reasoning_content` string field whose value
        //     is the concatenation of those blocks' `thinking` text
        //     (joined by `\n`). The relay extracts `reasoning_content`
        //     to feed DeepSeek V4's per-turn contract.
        //
        // (B) Filter out `{"type":"thinking","signature":""}` blocks
        //     from `content[]`. They contribute no usable signal to
        //     DeepSeek (empty signature is opaque) and they inflate
        //     the relay's accumulated-state count, which is one of
        //     the failure modes we observed in production (3/4
        //     DeepSeek sessions hit 400; the surviving session's
        //     early turns had empty signatures that the relay
        //     didn't trip on, but later turns did).
        //
        // The native Anthropic Claude path stays 1:1 with the pre-fix
        // body in all observable ways: the `thinking` blocks with
        // non-empty signatures are preserved verbatim, the top-level
        // `thinking: adaptive` field is untouched (Claude extended
        // thinking needs it), and the only added field on assistant
        // messages is `reasoning_content`, which Anthropic ignores.
        let mut body = apply_deepseek_reasoning_fix(&req);
        // Group chat (07-29-group-chat): Anthropic's Messages API
        // has no `name` field (unlike OpenAI), so speaker identity
        // is injected as a `@name:` text prefix and the raw `speaker`
        // field is stripped (Anthropic would 400 on an unknown
        // field). No-op for classic-chat messages (no `speaker`).
        apply_speaker_prefix(&mut body);

        Box::pin(Self::chat_stream_with_tools(config, body))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_system_prompt: true,
            supports_tools: true,
            supports_streaming: true,
        }
    }

    fn protocol(&self) -> ProviderProtocol {
        ProviderProtocol::Anthropic
    }
}
