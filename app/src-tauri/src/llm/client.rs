//! LLM HTTP + streaming client.
//!
//! Responsibilities:
//! 1. Build the request URL from `ANTHROPIC_BASE_URL` env (default to real
//!    Anthropic when env is unset).
//! 2. POST to `/v1/messages` with `x-api-key` + `anthropic-version` headers.
//! 3. Normalize error responses per HACKING-llm.md "GLM 兼容层 3 处差异".
//! 4. Stream the response body through the SSE parser, yield [`ChatEvent`]s.
//!
//! Step 2 adds: BlockState state machine for tool_use content blocks,
//! `content_block_start` / `content_block_delta` / `content_block_stop`
//! handling, and `message_delta` for stop_reason extraction.
//!
//! Step 6 adds: extended-thinking support. `BlockState` tracks thinking
//! and redacted_thinking blocks; the SSE parser handles `thinking_delta`
//! and `signature_delta` events, emitting `ThinkingDelta` and
//! `SignatureDelta` to the frontend. `redacted_thinking` content blocks
//! emit a single `RedactedThinkingDelta` event when they close (their
//! `data` payload is opaque and undisplayable). The `ChatRequest` always
//! includes a `thinking: { type: "adaptive", display: "summarized",
//! effort: <env> }` field so the model thinks before answering (see
//! HACKING-llm.md "thinking 兼容层 note").

use async_stream::stream;
use futures_util::{Stream, StreamExt};
use std::time::Duration;

use super::error::{classify_error_response, LlmError};
use super::sse::SseParser;
use super::types::{ChatEvent, ChatMessage, ChatRequest, ThinkingConfig, ToolDef};

/// Default `max_tokens` for LLM requests. Bumped from 1024 → 16384 in
/// step 6 because extended thinking tokens count against the same budget
/// as the actual answer — 1024 was too low and would have caused
/// `stop_reason: "max_tokens"` on most non-trivial turns.
const DEFAULT_MAX_TOKENS: u32 = 16384;

/// Configuration read once at startup. Held in Tauri `State` and cloned
/// per chat invocation.
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
// chat_stream — public API
// ---------------------------------------------------------------------------

/// Stream chat completion without tools (step 1 backward compat).
#[allow(dead_code)]
pub fn chat_stream(
    config: LlmConfig,
    messages: Vec<ChatMessage>,
) -> impl Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static {
    chat_stream_with_tools(config, None, messages, vec![])
}

/// Stream chat completions, optionally with tool definitions and a system prompt.
///
/// `system` is the Anthropic Messages API `system` field. The agent loop
/// (step 4 follow-up Bug 3) constructs a per-turn-1 prompt describing the
/// session's project, working directory, and worktree state so the LLM
/// is grounded on every chat request. Subsequent turns within the same
/// agent loop iteration may reuse the same prompt — it's the caller's
/// choice whether to re-build or carry it forward.
///
/// Always emits `ChatEvent::Start` first on success, then a series of
/// `Delta`s / `ThinkingDelta`s / `SignatureDelta`s / `ToolCall`s, then
/// `Done` at the end.
pub fn chat_stream_with_tools(
    config: LlmConfig,
    system: Option<String>,
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDef>,
) -> impl Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static {
    let url = config.endpoint();
    let thinking = config.thinking_config();
    let req = ChatRequest {
        model: config.model.clone(),
        max_tokens: config.max_tokens,
        messages,
        system,
        stream: true,
        tools,
        thinking: Some(thinking),
    };

    stream! {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
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

                    // --- message_delta: extract stop_reason ---
                    "message_delta" => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&event.data) {
                            if let Some(delta) = v.get("delta") {
                                if let Some(sr) = delta.get("stop_reason").and_then(|r| r.as_str())
                                {
                                    tracing::debug!(stop_reason = %sr, "▶ message_delta");
                                    stop_reason = Some(sr.to_string());
                                }
                            }
                        }
                    }

                    "message_stop" => {
                        tracing::debug!("▶ message_stop");
                    }
                    "ping" => {
                        tracing::debug!("▶ ping (heartbeat, ignored)");
                    }
                    "message_start" => {
                        // We already emitted Start; log for debugging.
                        tracing::debug!("▶ message_start");
                    }
                    other => {
                        tracing::debug!("▶ {} (unhandled)", other);
                    }
                }
            }
        }

        yield Ok(ChatEvent::Done { stop_reason });
    }
}

// ---------------------------------------------------------------------------
// Tests — config defaults
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
        use super::super::types::ChatRequest;
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
}
