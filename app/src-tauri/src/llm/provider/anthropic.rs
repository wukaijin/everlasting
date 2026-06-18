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
//! - `LlmConfig` is now PRIVATE to this module (it's the
//!   Anthropic-adapter's config). The chat command no longer
//!   constructs it directly; the factory in `mod.rs` builds it from
//!   catalog rows. The `from_env` constructor is preserved as a
//!   cold-start fallback and re-exported via the parent module
//!   re-export for any code that needs to read env values
//!   (`AppState::load` still calls it for the env-fallback path).
//! - `chat_stream_with_tools` is a private free function reused by
//!   `AnthropicProvider::send`. The public surface of the
//!   `chat` command is now `provider.send(system, messages, tools)`.

use async_stream::stream;
use futures_util::{Stream, StreamExt};
use std::pin::Pin;
use std::time::Duration;

use super::wire::{
    chat_request_to_wire, strip_unsupported, wire_messages_to_chat_messages, WireCapabilities,
};
use super::{Provider, ProviderCapabilities, ProviderProtocol};
use crate::llm::error::{classify_error_response, LlmError};
use crate::llm::sse::SseParser;
use crate::llm::types::{ChatEvent, ChatMessage, ChatRequest, ThinkingConfig, TokenUsage, ToolDef};

/// Default `max_tokens` for LLM requests. Bumped from 1024 → 16384 in
/// step 6 because extended thinking tokens count against the same budget
/// as the actual answer — 1024 was too low and would have caused
/// `stop_reason: "max_tokens"` on most non-trivial turns.
const DEFAULT_MAX_TOKENS: u32 = 16384;

/// Configuration for the Anthropic adapter. Constructed by
/// `build_provider` from a `ProviderRow` + `ModelRow`; cold-start
/// fallback path (`LlmConfig::from_env`) is preserved so
/// `AppState::load` can still read env values for `state.config`.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub max_tokens: u32,
    /// `effort` value for adaptive thinking. `low` / `medium` / `high`
    /// / `xhigh` / `max` (Anthropic schema). Defaults to `"high"`.
    pub thinking_effort: String,
}

impl LlmConfig {
    /// Read config from environment. Used by `AppState::load` for
    /// the env-fallback path. PR2's `chat` command no longer
    /// constructs an `LlmConfig` from this — it builds one from
    /// catalog rows.
    pub fn from_env() -> Result<Self, LlmError> {
        // Accept either ANTHROPIC_API_KEY (Anthropic SDK convention) or
        // ANTHROPIC_AUTH_TOKEN (older Claude Code env convention).
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_AUTH_TOKEN"))
            .map_err(|_| LlmError::Auth("ANTHROPIC_API_KEY not set".into()))?;
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
        let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "MiniMax-M2.7".to_string());
        let max_tokens = std::env::var("LLM_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_TOKENS);
        let thinking_effort = std::env::var("LLM_THINKING_EFFORT")
            .unwrap_or_else(|_| "high".to_string());
        Ok(Self {
            base_url,
            model,
            api_key,
            max_tokens,
            thinking_effort,
        })
    }

    pub fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    /// Build the `thinking` field we always send with the request.
    /// Adaptive mode (Opus 4.7 / 4.8) — `display: "summarized"` is
    /// explicit so that `thinking_delta` SSE events actually flow
    /// (otherwise the default `display: "omitted"` on those models would
    /// drop the summary text).
    fn thinking_config(&self) -> ThinkingConfig {
        ThinkingConfig::Adaptive {
            display: "summarized".to_string(),
            effort: self.thinking_effort.clone(),
        }
    }

    /// Sentinel for "ANTHROPIC_API_KEY wasn't set at startup". We construct
    /// the app even in this case so the UI loads and the user sees a
    /// helpful error rather than a crash on launch.
    pub fn unconfigured() -> Self {
        Self {
            base_url: String::new(),
            model: String::new(),
            api_key: String::new(),
            max_tokens: 0,
            thinking_effort: String::new(),
        }
    }

    /// `is_unconfigured` is used by tests + (pre-PR2) the chat
    /// command's pre-flight check. The post-PR2 chat command goes
    /// through `resolve_chat_provider` instead, so this method
    /// is only exercised by tests today. `#[allow(dead_code)]`
    /// is the lightest signal that the method is intentional
    /// (still on `LlmConfig` because the env-fallback path in
    /// `AppState::load` may want to query it via the `LlmConfig`
    /// struct's public surface in a future PR).
    #[allow(dead_code)]
    pub fn is_unconfigured(&self) -> bool {
        self.api_key.is_empty()
    }
}

// ---------------------------------------------------------------------------
// BlockState — tracks what content block is being streamed
// ---------------------------------------------------------------------------

/// State machine for the current content block being received from the SSE
/// stream. Used to know how to interpret `content_block_delta` events and
/// to assemble the right payload on `content_block_stop`.
#[derive(Debug)]
enum BlockState {
    /// Not inside any content block.
    Idle,
    /// Inside a text block.
    Text,
    /// Inside a tool_use block — accumulate JSON fragments.
    ToolUse {
        id: String,
        name: String,
        json_buf: String,
    },
    /// Inside a thinking block — accumulate thinking text and the opaque
    /// signature blob (delivered via `signature_delta` just before stop).
    Thinking {
        thinking_buf: String,
        signature_buf: String,
    },
    /// Inside a redacted_thinking block. The block carries only an opaque
    /// `data` payload (no streaming deltas); we treat the buffer as the
    /// fully-assembled payload once `content_block_stop` fires.
    RedactedThinking {
        data_buf: String,
    },
}

// ---------------------------------------------------------------------------
// AnthropicProvider
// ---------------------------------------------------------------------------

/// Anthropic Messages API adapter. Implements [`Provider`].
///
/// One `AnthropicProvider` is constructed per chat invocation (one
/// for the 20-turn agent loop). The chat command calls
/// `send(system, messages, tools)` once per turn and consumes the
/// returned stream inside a `tokio::select!`.
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
    fn chat_stream_with_tools(
        config: LlmConfig,
        req: ChatRequest,
    ) -> impl Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static {
        let url = config.endpoint();

        stream! {
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
                    yield Err(LlmError::Network(format!("client build: {}", e)));
                    return;
                }
            };

            tracing::info!(
                url = %url,
                model = %req.model,
                tools_count = %req.tools.len(),
                has_system = %req.system.is_some(),
                "→ LLM request"
            );

            let resp = match client
                .post(&url)
                .header("x-api-key", &config.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&req)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "network error before response");
                    yield Err(LlmError::Network(e.to_string()));
                    return;
                }
            };

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!(status = %status, body = %body, "← LLM error");
                yield Err(classify_error_response(status.as_u16(), &body));
                return;
            }

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
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&event.data) {
                                if let Some(cb) = v.get("content_block") {
                                    match cb.get("type").and_then(|t| t.as_str()) {
                                        Some("tool_use") => {
                                            let id = cb
                                                .get("id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("unknown")
                                                .to_string();
                                            let name = cb
                                                .get("name")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("unknown")
                                                .to_string();
                                            tracing::debug!(id = %id, name = %name, "▶ tool_use block start");
                                            block_state = BlockState::ToolUse {
                                                id,
                                                name,
                                                json_buf: String::new(),
                                            };
                                        }
                                        Some("thinking") => {
                                            // The initial signature is usually
                                            // an empty string in the start
                                            // event; it gets filled in by the
                                            // `signature_delta` event just
                                            // before stop. We don't need to
                                            // seed the buf from `content_block.signature`
                                            // — Anthropic guarantees the
                                            // signature is fully delivered via
                                            // the delta. (Defensive seed
                                            // preserved in case the schema
                                            // ever ships the whole thing up
                                            // front.)
                                            let initial_sig = cb
                                                .get("signature")
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let initial_thinking = cb
                                                .get("thinking")
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            tracing::debug!("▶ thinking block start");
                                            block_state = BlockState::Thinking {
                                                thinking_buf: initial_thinking,
                                                signature_buf: initial_sig,
                                            };
                                        }
                                        Some("redacted_thinking") => {
                                            // The `data` field is the full
                                            // opaque payload (no streaming
                                            // deltas for this block type).
                                            let data = cb
                                                .get("data")
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            tracing::debug!("▶ redacted_thinking block start");
                                            block_state = BlockState::RedactedThinking {
                                                data_buf: data,
                                            };
                                        }
                                        Some("text") | _ => {
                                            block_state = BlockState::Text;
                                        }
                                    }
                                }
                            }
                        }

                        // --- content_block_delta: incremental data ---
                        "content_block_delta" => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&event.data) {
                                if let Some(delta) = v.get("delta") {
                                    match delta.get("type").and_then(|t| t.as_str()) {
                                        Some("text_delta") => {
                                            if let Some(s) = delta.get("text").and_then(|t| t.as_str())
                                            {
                                                yield Ok(ChatEvent::Delta {
                                                    text: s.to_string(),
                                                });
                                            }
                                        }
                                        Some("input_json_delta") => {
                                            if let Some(partial) =
                                                delta.get("partial_json").and_then(|p| p.as_str())
                                            {
                                                if let BlockState::ToolUse { json_buf, .. } =
                                                    &mut block_state
                                                {
                                                    json_buf.push_str(partial);
                                                }
                                            }
                                        }
                                        Some("thinking_delta") => {
                                            if let Some(s) =
                                                delta.get("thinking").and_then(|t| t.as_str())
                                            {
                                                if let BlockState::Thinking { thinking_buf, .. } =
                                                    &mut block_state
                                                {
                                                    thinking_buf.push_str(s);
                                                }
                                                yield Ok(ChatEvent::ThinkingDelta {
                                                    text: s.to_string(),
                                                });
                                            }
                                        }
                                        Some("signature_delta") => {
                                            // Buffer only — emit the
                                            // assembled `SignatureDelta` once
                                            // on `content_block_stop`. This
                                            // protects the frontend's
                                            // `currentThinkingBlock` invariant
                                            // ("one signature per block")
                                            // even if the server ever splits
                                            // the signature across multiple
                                            // events. (Today Anthropic sends
                                            // exactly one `signature_delta`
                                            // per thinking block — see
                                            // research/anthropic-thinking-api.md
                                            // §6 — but we don't want to depend
                                            // on that.)
                                            if let Some(s) = delta
                                                .get("signature")
                                                .and_then(|t| t.as_str())
                                            {
                                                if let BlockState::Thinking { signature_buf, .. } =
                                                    &mut block_state
                                                {
                                                    signature_buf.push_str(s);
                                                }
                                            }
                                        }
                                        other => {
                                            tracing::debug!(
                                                "▶ content_block_delta with unknown delta type: {:?}",
                                                other
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // --- content_block_stop: finish a block ---
                        "content_block_stop" => {
                            match std::mem::replace(&mut block_state, BlockState::Idle) {
                                BlockState::ToolUse {
                                    id,
                                    name,
                                    json_buf,
                                } => {
                                    // Parse accumulated JSON; default to {} if empty or broken.
                                    let input: serde_json::Value = if json_buf.trim().is_empty() {
                                        serde_json::json!({})
                                    } else {
                                        serde_json::from_str(&json_buf).unwrap_or_else(|e| {
                                            tracing::warn!(
                                                json_buf = %json_buf,
                                                error = %e,
                                                "failed to parse tool_use input JSON, using empty object"
                                            );
                                            serde_json::json!({})
                                        })
                                    };
                                    tracing::debug!(id = %id, name = %name, "▶ tool_use block complete");
                                    yield Ok(ChatEvent::ToolCall {
                                        id,
                                        name,
                                        input,
                                    });
                                }
                                BlockState::Thinking {
                                    signature_buf,
                                    ..
                                } => {
                                    // Emit the fully-assembled signature as a
                                    // single `SignatureDelta` event — the
                                    // frontend's `currentThinkingBlock` and
                                    // the agent loop's `pending_thinking`
                                    // both rely on the invariant that there's
                                    // at most one `SignatureDelta` per
                                    // thinking block, otherwise the frontend
                                    // would open a fresh (corrupted) block on
                                    // each subsequent chunk and the agent
                                    // loop's `pending_thinking` would never
                                    // see the full signature in one event.
                                    //
                                    // `thinking_delta` events were already
                                    // streamed as they arrived; the frontend
                                    // appends them to the in-flight thinking
                                    // block's `text` directly.
                                    tracing::debug!(
                                        signature_len = signature_buf.len(),
                                        "▶ thinking block complete"
                                    );
                                    if !signature_buf.is_empty() {
                                        yield Ok(ChatEvent::SignatureDelta {
                                            signature: signature_buf,
                                        });
                                    }
                                }
                                BlockState::RedactedThinking { data_buf } => {
                                    // Emit the full opaque payload as a single
                                    // event so the frontend (and persistence)
                                    // can record it. The data is not
                                    // displayable; the agent loop stores it
                                    // verbatim for round-trip back to the
                                    // LLM.
                                    tracing::debug!(data_len = data_buf.len(), "▶ redacted_thinking block complete");
                                    if !data_buf.is_empty() {
                                        yield Ok(ChatEvent::RedactedThinkingDelta {
                                            data: data_buf,
                                        });
                                    }
                                }
                                BlockState::Text | BlockState::Idle => {}
                            }
                        }

                        // --- message_delta: extract stop_reason + usage ---
                        "message_delta" => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&event.data) {
                                if let Some(delta) = v.get("delta") {
                                    if let Some(sr) = delta.get("stop_reason").and_then(|r| r.as_str())
                                    {
                                        tracing::debug!(stop_reason = %sr, "▶ message_delta");
                                        stop_reason = Some(sr.to_string());
                                    }
                                }
                                // A4: usage is at the top level of
                                // `message_delta`, not under `delta`.
                                // Anthropic schema (cumulative per-turn):
                                //   { "type": "message_delta",
                                //     "delta": { "stop_reason": "..." },
                                //     "usage": { "input_tokens": N,
                                //                "output_tokens": N,
                                //                "cache_creation_input_tokens": N,
                                //                "cache_read_input_tokens": N } }
                                // The first `message_delta` event for a
                                // turn typically reports
                                // `output_tokens: 1`; later ones carry
                                // the cumulative value. We keep the
                                // last seen non-null payload (defensive
                                // — a per-event accumulator would
                                // also work, but the chat command
                                // only writes on `Done` anyway).
                                if let Some(usage_value) = v.get("usage") {
                                    if let Some(u) = parse_anthropic_usage(usage_value) {
                                        usage = Some(u);
                                    }
                                }
                            }
                        }

                        // Some proxies (and the Anthropic SDK's
                        // pre-stream `message_start`) attach
                        // `usage: { ... }` at the top level of
                        // `message_start`. We treat it as the
                        // initial baseline; the subsequent
                        // `message_delta.usage` (above) is the
                        // authoritative cumulative payload and
                        // overwrites this. Without this, a
                        // connection that errored out before the
                        // first `message_delta` would never get a
                        // `usage` report.
                        "message_start" => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&event.data) {
                                if let Some(usage_value) = v.get("message").and_then(|m| m.get("usage")) {
                                    if let Some(u) = parse_anthropic_usage(usage_value) {
                                        if usage.is_none() {
                                            usage = Some(u);
                                        }
                                    }
                                } else if let Some(usage_value) = v.get("usage") {
                                    if let Some(u) = parse_anthropic_usage(usage_value) {
                                        if usage.is_none() {
                                            usage = Some(u);
                                        }
                                    }
                                }
                            }
                            // We already emitted Start; log for debugging.
                            tracing::debug!("▶ message_start");
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

/// Parse Anthropic's `usage` payload into a protocol-agnostic
/// [`TokenUsage`]. Defensive: any of the four fields may be missing
/// (older Anthropic API versions / proxies only emitted a subset);
/// missing fields default to 0. Returns `None` if no recognizable
/// integer fields were present (e.g. a totally malformed payload).
fn parse_anthropic_usage(v: &serde_json::Value) -> Option<TokenUsage> {
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
        // Anthropic target: supports everything. We pass
        // permissive capabilities so `strip_unsupported` is a
        // no-op for the Anthropic→Anthropic path; the function
        // is the **single place** that encodes the strip rules,
        // and running it costs nothing.
        let caps = WireCapabilities {
            supports_thinking: true,
            supports_reasoning_effort: true,
            supports_thinking_signatures: true,
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

        Box::pin(Self::chat_stream_with_tools(config, req))
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::ChatRequest;
    use crate::db;

    #[test]
    fn default_max_tokens_is_16384_not_1024() {
        // Extended thinking tokens count against max_tokens; 1024 was
        // bumped to 16384 in step 6 to cover a typical thinking + reply
        // turn without truncation.
        assert_eq!(DEFAULT_MAX_TOKENS, 16384);
    }

    #[test]
    fn thinking_config_is_adaptive_summarized_with_configured_effort() {
        let config = LlmConfig {
            base_url: "https://example.com".to_string(),
            model: "claude-opus-4-7".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            thinking_effort: "xhigh".to_string(),
        };
        let tc = config.thinking_config();
        match tc {
            ThinkingConfig::Adaptive { display, effort } => {
                assert_eq!(display, "summarized");
                assert_eq!(effort, "xhigh");
            }
        }
    }

    #[test]
    fn unconfigured_has_empty_thinking_effort() {
        let config = LlmConfig::unconfigured();
        assert!(config.thinking_effort.is_empty());
        assert!(config.is_unconfigured());
    }

    /// Step 4 follow-up Bug 3: when the agent loop builds a system
    /// prompt for the current session, that string must make it into
    /// the request body's top-level `system` field (Anthropic's
    /// schema). Verified by serializing a `ChatRequest` with the
    /// `system` field populated and checking the wire shape.
    #[test]
    fn chat_request_system_field_serializes_when_some() {
        let req = ChatRequest {
            model: "test".to_string(),
            max_tokens: 100,
            messages: vec![],
            system: Some("You are a coding agent in worktree /foo".to_string()),
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.get("system").and_then(|s| s.as_str()),
            Some("You are a coding agent in worktree /foo")
        );
    }

    /// The `AnthropicProvider` reports Anthropic as its protocol and
    /// supports the three capabilities the chat command cares about
    /// (system prompt, tools, streaming).
    #[test]
    fn anthropic_provider_reports_capabilities_and_protocol() {
        let p = AnthropicProvider::new(LlmConfig {
            base_url: "https://api.anthropic.com".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            thinking_effort: "high".to_string(),
        });
        assert_eq!(p.protocol(), ProviderProtocol::Anthropic);
        let caps = p.capabilities();
        assert!(caps.supports_system_prompt);
        assert!(caps.supports_tools);
        assert!(caps.supports_streaming);
    }

    /// Two `AnthropicProvider`s built from the same `LlmConfig` are
    /// interchangeable — the chat command could in principle clone
    /// the provider for the 20-turn loop, but in practice we just
    /// call `send` on the same instance. The relevant invariant:
    /// `Send + Sync` (the trait's super-trait) is satisfied, so the
    /// chat command's `Box<dyn Provider>` can move into a
    /// `tauri::async_runtime::spawn` task.
    #[test]
    fn anthropic_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AnthropicProvider>();
    }

    /// Sanity: the factory in `mod.rs` constructs an
    /// `AnthropicProvider` whose internal `LlmConfig` is wired from
    /// the catalog rows. We re-check the protocol + capabilities
    /// here (the catalog-driven path), distinct from the
    /// hand-built `AnthropicProvider::new` test above.
    #[test]
    fn factory_built_provider_reports_anthropic_capabilities() {
        let p = crate::db::ProviderRow {
            id: "pid-1".to_string(),
            protocol: "anthropic".to_string(),
            display_name: "Anthropic 官方".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: "sk-test".to_string(),
            created_at: "2026-06-09T00:00:00Z".to_string(),
            updated_at: "2026-06-09T00:00:00Z".to_string(),
        };
        let m = db::ModelRow {
            id: "mid-1".to_string(),
            provider_id: "pid-1".to_string(),
            model_name: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            max_tokens: Some(8192),
            thinking_effort: Some("high".to_string()),
            supports_thinking: true,
            context_window: 200_000,
            created_at: "2026-06-09T00:00:00Z".to_string(),
            updated_at: "2026-06-09T00:00:00Z".to_string(),
        };
        let provider = super::super::build_provider(&p, &m).expect("anthropic is implemented");
        assert_eq!(provider.protocol(), ProviderProtocol::Anthropic);
        let caps = provider.capabilities();
        assert!(caps.supports_system_prompt);
        assert!(caps.supports_tools);
        assert!(caps.supports_streaming);
    }

    // ---- A4: parse_anthropic_usage ----

    #[test]
    fn parse_anthropic_usage_full_payload() {
        // Anthropic's `message_delta.usage` (cumulative per-turn).
        let v = serde_json::json!({
            "input_tokens": 1234,
            "output_tokens": 56,
            "cache_creation_input_tokens": 100,
            "cache_read_input_tokens": 200,
        });
        let u = parse_anthropic_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 1234);
        assert_eq!(u.output_tokens, 56);
        assert_eq!(u.cache_creation_input_tokens, 100);
        assert_eq!(u.cache_read_input_tokens, 200);
    }

    #[test]
    fn parse_anthropic_usage_minimal_payload() {
        // Pre-caching Anthropic / older proxy / non-thinking
        // call: only the two core fields are present. Defaults
        // fill the cache fields to 0.
        let v = serde_json::json!({
            "input_tokens": 42,
            "output_tokens": 7,
        });
        let u = parse_anthropic_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 42);
        assert_eq!(u.output_tokens, 7);
        assert_eq!(u.cache_creation_input_tokens, 0);
        assert_eq!(u.cache_read_input_tokens, 0);
    }

    #[test]
    fn parse_anthropic_usage_zero_returns_none() {
        // An all-zero payload is treated as "no usage
        // information" so the agent loop's
        // `if let Some(t) = usage { ... }` path correctly skips
        // the SQL write. See the function's docstring for the
        // rationale.
        let v = serde_json::json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0,
        });
        assert!(parse_anthropic_usage(&v).is_none());
    }

    #[test]
    fn parse_anthropic_usage_empty_object_returns_none() {
        // A `usage: {}` event (defensive — Anthropic doesn't
        // emit this, but a proxy might) is treated as
        // "no usage".
        let v = serde_json::json!({});
        assert!(parse_anthropic_usage(&v).is_none());
    }
}
