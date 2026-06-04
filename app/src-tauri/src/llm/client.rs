//! LLM HTTP + streaming client.
//!
//! Responsibilities:
//! 1. Build the request URL from `ANTHROPIC_BASE_URL` env (default to real
//!    Anthropic when env is unset).
//! 2. POST to `/v1/messages` with `x-api-key` + `anthropic-version` headers.
//! 3. Normalize error responses per HACKING-llm.md "GLM 兼容层 3 处差异".
//! 4. Stream the response body through the SSE parser, yield [`ChatEvent`]s.
//!
//! Retry / abort / cancel are intentionally out of scope for step 1 — see
//! HACKING-llm.md checklist items 10-11. They get added in step 2 (tool
//! calling) when the user actually needs a "stop generating" button.

use async_stream::stream;
use futures_util::{Stream, StreamExt};
use std::time::Duration;

use super::error::{classify_error_response, LlmError};
use super::sse::SseParser;
use super::types::{ChatEvent, ChatMessage, ChatRequest};

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

/// Stream chat completion deltas as [`ChatEvent`]s.
///
/// Always emits `ChatEvent::Start` first on success, then a series of
/// `Delta`s, then `Done` at the end. On error (network, auth, server, etc.)
/// emits exactly one `Err(LlmError)` and closes the stream.
pub fn chat_stream(
    config: LlmConfig,
    messages: Vec<ChatMessage>,
) -> impl Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static {
    let url = config.endpoint();
    let req = ChatRequest {
        model: config.model,
        max_tokens: config.max_tokens,
        messages,
        system: None,
        stream: true,
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

        tracing::info!(url = %url, model = %req.model, "→ LLM request");

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
        let mut saw_message_stop = false;

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
                    "content_block_delta" => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&event.data) {
                            if let Some(s) = v
                                .get("delta")
                                .and_then(|d| d.get("text"))
                                .and_then(|t| t.as_str())
                            {
                                yield Ok(ChatEvent::Delta { text: s.to_string() });
                            }
                        }
                    }
                    "message_stop" => {
                        saw_message_stop = true;
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

        if !saw_message_stop {
            tracing::warn!("stream ended without message_stop event");
        }
        yield Ok(ChatEvent::Done { stop_reason: None });
    }
}
