//! `ChatMessage` and `ToolDef` — message and tool-declaration types.
//!
//! Split out of `llm/types.rs` (2026-08-08 batch3). Both reference
//! [`MessageContent`]/[`ContentBlock`] from [`super::message`] but do not
//! interlock with each other.

use serde::{Deserialize, Serialize};

use super::message::{MessageContent, Role};

// ---------------------------------------------------------------------------
// ChatMessage
// ---------------------------------------------------------------------------

/// One message in a conversation. Content can be plain text (backward compat
/// with step 1) or an array of ContentBlocks (tool_use / tool_result /
/// thinking / redacted_thinking).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: MessageContent,
    /// Group chat (07-29-group-chat, 2026-07-31): which participant
    /// authored this message. `None` for classic-chat messages and
    /// for user messages — the classic single-agent path is
    /// unaffected. In a group_chat session this is set on each
    /// assistant turn so the next speaker's model can attribute
    /// prior utterances (互见性). NOT a wire `role` — Anthropic/
    /// OpenAI only accept `user`/`assistant`, so speaker identity
    /// is threaded separately and injected per-provider at the wire
    /// layer (OpenAI `name` field; Anthropic `@name:` prefix).
    ///
    /// `#[serde(default)]` so legacy persisted messages (no
    /// speaker) and existing test fixtures deserialize to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

// ---------------------------------------------------------------------------
// ToolDef — tool declaration for the request
// ---------------------------------------------------------------------------

/// Tool definition sent to the LLM in the request body (Anthropic schema).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

impl ToolDef {
    /// Test-only constructor: builds a `ToolDef` with a name
    /// and a default empty `input_schema`. Used by
    /// `agent::permissions::tests_mode::filter_tools_for_mode_*`
    /// to construct minimal tool lists without going through
    /// the real `tools::builtin_tools()` registry (which would
    /// require `AppState` context).
    #[cfg(test)]
    pub fn new_for_test(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        }
    }
}
