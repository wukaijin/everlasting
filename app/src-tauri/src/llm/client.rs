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

use async_stream::stream;
use futures_util::{Stream, StreamExt};
use std::time::Duration;

use super::error::{classify_error_response, LlmError};
use super::sse::SseParser;
use super::types::{ChatEvent, ChatMessage, ChatRequest, ToolDef};

/// Configuration read once at startup. Held in Tauri `State` and cloned
/// per chat invocation.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub max_tokens: u32,
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
        let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "GLM-4.7".to_string());
        let max_tokens = std::env::var("LLM_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1024);
        Ok(Self {
            base_url,
            model,
            api_key,
            max_tokens,
        })
    }

    pub fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
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
/// stream. Used to know how to interpret `content_block_delta` events.
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
    chat_stream_with_tools(config, messages, vec![])
}

/// Stream chat completions, optionally with tool definitions.
///
/// Always emits `ChatEvent::Start` first on success, then a series of
/// `Delta`s / `ToolCall`s, then `Done` at the end.
pub fn chat_stream_with_tools(
    config: LlmConfig,
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDef>,
) -> impl Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static {
    let url = config.endpoint();
    let req = ChatRequest {
        model: config.model,
        max_tokens: config.max_tokens,
        messages,
        system: None,
        stream: true,
        tools,
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

        tracing::info!(url = %url, model = %req.model, tools_count = %req.tools.len(), "→ LLM request");

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
