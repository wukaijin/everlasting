//! LLM request / response / event types.

use serde::{Deserialize, Serialize};

/// Conversation role.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// One message in a conversation. For step 1 we only support plain text content
/// (no tool_use / tool_result blocks — that's step 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

/// Anthropic Messages API request body. We only set what we need; the server
/// fills defaults for the rest.
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
}

/// What we push to the frontend over the Tauri event channel. Single event
/// type, tagged by `kind`, keeps the frontend state machine simple.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    /// Stream started. Frontend can show a "thinking…" indicator.
    Start,
    /// Incremental text from the model.
    Delta { text: String },
    /// Stream finished cleanly. Includes Anthropic `stop_reason` if present.
    Done { stop_reason: Option<String> },
    /// Stream errored. `category` is one of the [`LlmErrorCategory`] strings so
    /// the frontend can show a friendly message.
    Error {
        message: String,
        category: LlmErrorCategory,
    },
}

/// Stable string identifiers for [`crate::error::LlmError`] variants, safe to
/// embed in IPC payloads.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LlmErrorCategory {
    Auth,
    RateLimit,
    InvalidRequest,
    Server,
    Network,
}
