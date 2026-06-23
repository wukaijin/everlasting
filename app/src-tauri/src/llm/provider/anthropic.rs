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
        body: serde_json::Value,
    ) -> impl Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static {
        let url = config.endpoint();
        // Pull the same observability fields the pre-fix code read off
        // `&req` — `model`, `tools_count`, `has_system` — off the JSON
        // body that the DeepSeek relay fix produced. The shape is the
        // same; the values come from the same wire payload, so log
        // content is byte-equivalent to the pre-fix logs.
        let log_model = body
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let log_tools_count = body
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let log_has_system = body.get("system").map(|v| !v.is_null()).unwrap_or(false);

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
                model = %log_model,
                tools_count = %log_tools_count,
                has_system = %log_has_system,
                "→ LLM request"
            );

            let resp = match client
                .post(&url)
                .header("x-api-key", &config.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .body(body.to_string())
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
    let mut body = serde_json::to_value(req).expect("ChatRequest → serde_json::Value is infallible");
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
        let body = apply_deepseek_reasoning_fix(&req);

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
            has_key: true,
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

    // -----------------------------------------------------------------
    // DeepSeek-Via-Anthropic-Relay reasoning_content fix
    // (task 06-20-deepseek-reasoner-reasoning-content-400 +
    // follow-up 06-21-fix-deepseek-relay-thinking-block-drop-causing-turn-2-400)
    //
    // These tests pin the contract of `apply_deepseek_reasoning_fix`:
    //
    //   (A) For assistant messages with at least one thinking block
    //       whose `thinking` text is non-empty, add a top-level
    //       `reasoning_content` field whose value is the concatenation
    //       of ALL thinking blocks' `thinking` text (joined by `\n`).
    //       Empty-signature blocks contribute their text too.
    //
    // The fix does NOT drop any thinking blocks. The previous
    // 06-20 implementation had a (B) "drop empty-signature thinking
    // blocks" step; that was based on an unverified attribution
    // ("empty sig inflates the relay's accumulated-state count")
    // that turned out to be WRONG on real-relay probing — the
    // wukaijin relay requires `content[].thinking` blocks AND a
    // top-level `reasoning_content` field TOGETHER (signatures not
    // verified), so dropping blocks triggered a new turn-2 400
    // (`content[].thinking must be passed back`). See
    // `deepseek_relay_contract_v1_v2_v3` for the pinned contract.
    //
    // User messages and tool result messages are NOT touched. The
    // top-level `thinking: adaptive` field on the request body is NOT
    // touched (Claude extended thinking depends on it). Messages with
    // no thinking blocks (pure text + tool_use) do NOT gain a
    // `reasoning_content` field (the collected buffer is empty and
    // an empty `reasoning_content: ""` would mismatch the relay's
    // content-shape contract, so the field is omitted entirely).
    //
    // See `.trellis/tasks/06-21-fix-deepseek-relay-thinking-block-drop-causing-turn-2-400/prd.md`
    // for the V1/V2/V3 probe evidence and the corrected rationale.
    // -----------------------------------------------------------------

    /// Helper: build a `ChatRequest` from a list of message JSON
    /// values, so the test bodies can focus on the message shape and
    /// not on constructing `ChatMessage` / `ContentBlock` by hand for
    /// every case. The `model` / `max_tokens` / `system` / `tools`
    /// fields are fixed at benign values; the fix doesn't touch any
    /// of them.
    fn chat_request_with_messages(
        messages: Vec<serde_json::Value>,
    ) -> ChatRequest {
        let parsed: Vec<ChatMessage> = messages
            .into_iter()
            .map(|m| serde_json::from_value(m).expect("message JSON parses"))
            .collect();
        ChatRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: 16384,
            messages: parsed,
            system: None,
            stream: true,
            tools: vec![],
            thinking: Some(ThinkingConfig::Adaptive {
                display: "summarized".to_string(),
                effort: "high".to_string(),
            }),
        }
    }

    #[test]
    fn deepseek_reasoning_fix_keeps_empty_sig_and_lifts_reasoning_content() {
        // An assistant message with both an empty-signature thinking
        // block and a non-empty-signature thinking block must KEEP
        // BOTH blocks verbatim (the relay does not verify signatures)
        // AND lift a top-level `reasoning_content` whose value is the
        // `\n`-join of ALL thinking blocks' text.
        //
        // This is the corrected contract: the previous 06-20 fix
        // DROPPED empty-signature blocks, which the wukaijin relay
        // rejects as `content[].thinking must be passed back` on the
        // next turn. Empty signatures are produced by the relay
        // itself in streaming mode (it does not emit
        // `signature_delta`), so persistence will land empty
        // signatures and the fix must round-trip them intact.
        let req = chat_request_with_messages(vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "empty sig thinking", "signature": ""},
                {"type": "thinking", "thinking": "uuid sig thinking", "signature": "uuid-sig-abc"},
                {"type": "text", "text": "visible answer"}
            ]
        })]);
        let body = apply_deepseek_reasoning_fix(&req);
        let content = body["messages"][0]["content"].as_array().expect("content array");
        // ALL 3 blocks survive (2 thinking + 1 text). No drops.
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], "");
        assert_eq!(content[0]["thinking"], "empty sig thinking");
        assert_eq!(content[1]["type"], "thinking");
        assert_eq!(content[1]["signature"], "uuid-sig-abc");
        assert_eq!(content[1]["thinking"], "uuid sig thinking");
        assert_eq!(content[2]["type"], "text");
        assert_eq!(content[2]["text"], "visible answer");
        // reasoning_content carries the text of ALL thinking blocks
        // (empty-sig block included), joined by `\n`.
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("empty sig thinking\nuuid sig thinking".to_string())
        );
    }

    #[test]
    fn deepseek_reasoning_fix_keeps_all_empty_sig_and_lifts_reasoning_content() {
        // An assistant message whose thinking blocks ALL have empty
        // signatures must STILL keep them all and STILL lift a
        // top-level `reasoning_content` whose value is the `\n`-join
        // of all their text. The previous 06-20 behavior (drop empty
        // blocks, omit `reasoning_content`) was wrong: the relay
        // requires the blocks AND the field together, and accepts
        // empty signatures without verification.
        let req = chat_request_with_messages(vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "empty 1", "signature": ""},
                {"type": "thinking", "thinking": "empty 2", "signature": ""},
                {"type": "text", "text": "answer"}
            ]
        })]);
        let body = apply_deepseek_reasoning_fix(&req);
        let content = body["messages"][0]["content"].as_array().expect("content array");
        // ALL 3 blocks survive — empty signatures are NOT a drop signal.
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], "");
        assert_eq!(content[0]["thinking"], "empty 1");
        assert_eq!(content[1]["type"], "thinking");
        assert_eq!(content[1]["signature"], "");
        assert_eq!(content[1]["thinking"], "empty 2");
        assert_eq!(content[2]["type"], "text");
        // reasoning_content = "\n"-join of all thinking blocks' text.
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("empty 1\nempty 2".to_string())
        );
    }

    #[test]
    fn deepseek_relay_contract_v1_v2_v3() {
        // PIN TEST — this test exists specifically to prevent a
        // future regression to "drop empty-signature thinking blocks"
        // (the original 06-20 fix that caused the turn-2 400). The
        // wukaijin.com relay's thinking-mode contract was verified
        // against the real relay via V1/V2/V3 probe experiments
        // (scripts `/tmp/ds_probe/v{1,2,3}*.json` in the task prd):
        //
        //   V1: drop `content[].thinking` blocks
        //       → 400 "content[].thinking must be passed back"
        //   V2: keep `content[].thinking` blocks + add `reasoning_content`
        //       → 200 ✅
        //   V3: keep `content[].thinking` blocks + NO `reasoning_content`
        //       → 400 "reasoning_content must be passed back"
        //
        // Conclusion: the relay requires blocks AND `reasoning_content`
        // TOGETHER, and does NOT cryptographically verify the
        // `signature` field (empty signatures are accepted). The
        // correct `apply_deepseek_reasoning_fix` output for any input
        // containing thinking blocks is V2.
        //
        // See `.trellis/tasks/06-21-fix-deepseek-relay-thinking-block-drop-causing-turn-2-400/prd.md`
        // for the V1/V2/V3 table and the DB evidence (session
        // `863fda30-66a1-421d-bd91-0c3a6bb9b342` seq=1 assistant
        // has `"signature": ""`).

        // Turn-2 assistant shape (DeepSeek-via-relay, empty signatures
        // because the relay's streaming mode doesn't emit
        // `signature_delta` — this is the realistic input shape).
        let turn2_assistant = serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "first reasoning", "signature": ""},
                {"type": "text", "text": "answer"}
            ]
        });

        // Sanity check what each V variant looks like relative to the
        // input, then assert the fix produces V2.
        let input = chat_request_with_messages(vec![turn2_assistant]);
        let body = apply_deepseek_reasoning_fix(&input);
        let content = body["messages"][0]["content"].as_array().expect("content array");

        // V1 invariant: NOT this — would be `content.len() == 1` with
        // only the text block. We assert against it explicitly.
        assert_ne!(
            content.len(),
            1,
            "V1 (drop thinking blocks) must NOT happen — relay 400s with 'content[].thinking must be passed back'"
        );

        // V2 (the contract): both thinking blocks AND
        // `reasoning_content` present.
        assert_eq!(
            content.len(),
            2,
            "V2: all content blocks preserved (thinking + text)"
        );
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], ""); // empty sig kept as-is
        assert_eq!(content[1]["type"], "text");
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("first reasoning".to_string()),
            "V2: reasoning_content must be present with the lifted thinking text"
        );

        // V3 invariant: NOT this — would be `content[].thinking` blocks
        // present but no top-level `reasoning_content`. We assert the
        // field is present (covered above) and assert a non-null value
        // shape explicitly so a future edit that nulls it out trips
        // here.
        assert!(
            body["messages"][0].get("reasoning_content").is_some(),
            "V3 (blocks kept, no reasoning_content) must NOT happen — relay 400s with 'reasoning_content must be passed back'"
        );
        assert!(
            body["messages"][0]["reasoning_content"].is_string(),
            "reasoning_content must be a non-null string, not null/sentinel"
        );
    }

    #[test]
    fn deepseek_reasoning_fix_keeps_nonempty_sig_and_adds_reasoning_content() {
        // Single non-empty-signature thinking block: step (B) keeps
        // it verbatim; step (A) lifts its `thinking` text to the
        // top-level `reasoning_content` field.
        let req = chat_request_with_messages(vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "let me think", "signature": "uuid-xyz"},
                {"type": "text", "text": "ok"}
            ]
        })]);
        let body = apply_deepseek_reasoning_fix(&req);
        let content = body["messages"][0]["content"].as_array().expect("content array");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], "uuid-xyz");
        assert_eq!(content[0]["thinking"], "let me think");
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("let me think".to_string())
        );
    }

    #[test]
    fn deepseek_reasoning_fix_concatenates_multiple_nonempty_blocks() {
        // Multiple non-empty-signature thinking blocks (a model can
        // emit more than one per turn). They are all preserved in
        // `content[]` AND their `thinking` text is joined with `\n`
        // into the `reasoning_content` field.
        let req = chat_request_with_messages(vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "step 1", "signature": "sig-1"},
                {"type": "thinking", "thinking": "step 2", "signature": "sig-2"},
                {"type": "text", "text": "done"}
            ]
        })]);
        let body = apply_deepseek_reasoning_fix(&req);
        let content = body["messages"][0]["content"].as_array().expect("content array");
        // Both thinking blocks preserved.
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["signature"], "sig-1");
        assert_eq!(content[1]["signature"], "sig-2");
        assert_eq!(content[2]["type"], "text");
        // reasoning_content = "step 1\nstep 2" (joined by \n).
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("step 1\nstep 2".to_string())
        );
    }

    #[test]
    fn deepseek_reasoning_fix_skips_user_messages() {
        // (R4 contract.) User-role messages must be entirely
        // untouched — content[] unchanged, no reasoning_content
        // field added, no other mutations. The fix is an
        // assistant-message-only patch.
        let req = chat_request_with_messages(vec![
            serde_json::json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "what is X?"},
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok", "is_error": false}
                ]
            }),
            serde_json::json!({
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "thinking", "signature": "sig-1"},
                    {"type": "text", "text": "X is ..."}
                ]
            }),
        ]);
        let body = apply_deepseek_reasoning_fix(&req);
        // User message: untouched.
        let user = &body["messages"][0];
        assert_eq!(user["role"], "user");
        assert_eq!(user["content"].as_array().unwrap().len(), 2);
        assert!(user.get("reasoning_content").is_none());
        // Assistant message: gets the reasoning_content field.
        let asst = &body["messages"][1];
        assert_eq!(
            asst["reasoning_content"],
            serde_json::Value::String("thinking".to_string())
        );
    }

    #[test]
    fn deepseek_reasoning_fix_no_thinking_blocks_no_reasoning_content() {
        // An assistant message with NO thinking blocks (pure text +
        // tool_use) must not gain a reasoning_content field. The fix
        // is a no-op for such messages.
        let req = chat_request_with_messages(vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "sure, let me read"},
                {"type": "tool_use", "id": "toolu_42", "name": "read_file", "input": {"path": "/etc/hosts"}}
            ]
        })]);
        let body = apply_deepseek_reasoning_fix(&req);
        let content = body["messages"][0]["content"].as_array().expect("content array");
        // Unchanged: text + tool_use, no thinking blocks.
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "tool_use");
        assert!(
            body["messages"][0].get("reasoning_content").is_none(),
            "reasoning_content must NOT appear on a no-thinking assistant message: {:?}",
            body["messages"][0]
        );
    }

    #[test]
    fn deepseek_reasoning_fix_preserves_top_level_thinking_field() {
        // The top-level `thinking: adaptive` field on the request
        // body must be preserved verbatim — Claude extended thinking
        // depends on it. The fix only mutates assistant messages'
        // `content[]` and (conditionally) adds `reasoning_content`.
        let req = ChatRequest {
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 16384,
            messages: vec![serde_json::from_value(serde_json::json!({
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "deep", "signature": "sig-abc"},
                    {"type": "text", "text": "answer"}
                ]
            })).unwrap()],
            system: Some("You are a coding agent".to_string()),
            stream: true,
            tools: vec![],
            thinking: Some(ThinkingConfig::Adaptive {
                display: "summarized".to_string(),
                effort: "high".to_string(),
            }),
        };
        let body = apply_deepseek_reasoning_fix(&req);
        // Top-level thinking field preserved verbatim.
        let thinking = body.get("thinking").expect("thinking field present");
        assert_eq!(thinking["type"], "adaptive");
        assert_eq!(thinking["display"], "summarized");
        assert_eq!(thinking["effort"], "high");
        // Sanity: the other top-level fields are untouched too.
        assert_eq!(body["model"], "claude-sonnet-4-5");
        assert_eq!(body["max_tokens"], 16384);
        assert_eq!(body["system"], "You are a coding agent");
        assert_eq!(body["stream"], true);
        // reasoning_content still attached to the assistant message.
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("deep".to_string())
        );
    }
}
