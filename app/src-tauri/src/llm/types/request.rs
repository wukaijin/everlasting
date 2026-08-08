//! `ThinkingConfig` and `ChatRequest` — request-side types.
//!
//! Split out of `llm/types.rs` (2026-08-08 batch3).

use serde::{Deserialize, Serialize};

use super::chat::{ChatMessage, ToolDef};

// ---------------------------------------------------------------------------
// ThinkingConfig — request-side extended-thinking control
// ---------------------------------------------------------------------------

/// Top-level `thinking` field on a [`ChatRequest`]. The Anthropic Messages
/// API supports several modes; we currently only model `adaptive` (model
/// self-decides how much to think, controlled by `effort`).
///
/// `display: "summarized"` is set explicitly so that `thinking_delta` SSE
/// events actually stream a text summary to the client — with the default
/// `display: "omitted"` on Opus 4.7+ the summary is dropped and the UI
/// would see no thinking text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
    Adaptive { display: String, effort: String },
}

// ---------------------------------------------------------------------------
// ChatRequest
// ---------------------------------------------------------------------------

/// Anthropic Messages API request body.
///
/// NOTE: We intentionally do NOT pre-validate `max_tokens` on the client side
/// (see HACKING-llm.md "差异 3"). The server decides.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    /// When present, the model is asked to think before answering. The
    /// `signature` blobs of any thinking blocks it returns must be echoed
    /// back in subsequent assistant messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
}
