//! LLM request / response / event types.
//!
//! Step 2 extends the step 1 types to support Anthropic-style tool calling:
//! - `ContentBlock` for structured message content (text / tool_use / tool_result)
//! - `MessageContent` with custom Serde to accept both plain string and block array
//! - `ToolDef` for declaring tools in the request
//! - `ChatEvent` gains `ToolCall` and `ToolResult` variants

use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------------

/// Conversation role. In the Anthropic Messages API, `tool_result` content
/// blocks are placed inside a `role: "user"` message, so we don't need a
/// separate `Tool` role.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

// ---------------------------------------------------------------------------
// ContentBlock — structured message content
// ---------------------------------------------------------------------------

/// One content block inside a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
    },
}

fn is_false(b: &bool) -> bool {
    !b
}

// ---------------------------------------------------------------------------
// MessageContent — string-or-array wrapper
// ---------------------------------------------------------------------------

/// Message content that serializes as a plain string (step 1 compat) or an
/// array of [`ContentBlock`] (step 2+ tool calling).
#[derive(Debug, Clone, PartialEq)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Extract all text from this content, ignoring tool blocks.
    #[allow(dead_code)]
    pub fn to_text(&self) -> String {
        match self {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Convenience: create a single-text-block content.
    #[allow(dead_code)]
    pub fn from_text(s: impl Into<String>) -> Self {
        MessageContent::Text(s.into())
    }
}

impl Serialize for MessageContent {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            MessageContent::Text(t) => s.serialize_str(t),
            MessageContent::Blocks(blocks) => blocks.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for MessageContent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(d)?;
        match val {
            serde_json::Value::String(s) => Ok(MessageContent::Text(s)),
            other => {
                let blocks: Vec<ContentBlock> =
                    serde_json::from_value(other).map_err(serde::de::Error::custom)?;
                Ok(MessageContent::Blocks(blocks))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ChatMessage
// ---------------------------------------------------------------------------

/// One message in a conversation. Content can be plain text (backward compat
/// with step 1) or an array of ContentBlocks (for tool_use / tool_result).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: MessageContent,
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
}

// ---------------------------------------------------------------------------
// ChatEvent — events pushed to the frontend
// ---------------------------------------------------------------------------

/// What we push to the frontend over the Tauri event channel. Tagged by
/// `kind`, keeps the frontend state machine simple.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    /// Stream started. Frontend can show a "thinking…" indicator.
    Start,
    /// Incremental text from the model.
    Delta { text: String },
    /// LLM requested a tool call. Emitted once per tool_use block when the
    /// block is fully assembled (content_block_stop for tool_use type).
    /// Emitted on the independent `tool:call` event channel.
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool execution completed. Emitted on the independent `tool:result`
    /// event channel. Not constructed in SSE parsing — only in the agent loop.
    #[allow(dead_code)]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Stream finished cleanly. Includes Anthropic `stop_reason` if present.
    Done { stop_reason: Option<String> },
    /// Stream errored. `category` maps to [`LlmErrorCategory`] strings.
    Error {
        message: String,
        category: LlmErrorCategory,
    },
}

// ---------------------------------------------------------------------------
// LlmErrorCategory
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_content_serialize_text_as_string() {
        let mc = MessageContent::Text("hello".to_string());
        let json = serde_json::to_string(&mc).unwrap();
        assert_eq!(json, "\"hello\"");
    }

    #[test]
    fn message_content_deserialize_string() {
        let mc: MessageContent = serde_json::from_str("\"hello\"").unwrap();
        assert_eq!(mc, MessageContent::Text("hello".to_string()));
    }

    #[test]
    fn message_content_serialize_blocks_as_array() {
        let blocks = vec![ContentBlock::Text {
            text: "hi".to_string(),
        }];
        let mc = MessageContent::Blocks(blocks);
        let json = serde_json::to_string(&mc).unwrap();
        assert!(json.starts_with('['));
        assert!(json.contains("\"type\":\"text\""));
    }

    #[test]
    fn message_content_deserialize_blocks() {
        let json = r#"[{"type":"text","text":"hello"}]"#;
        let mc: MessageContent = serde_json::from_str(json).unwrap();
        match mc {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert_eq!(
                    blocks[0],
                    ContentBlock::Text {
                        text: "hello".to_string()
                    }
                );
            }
            _ => panic!("expected Blocks"),
        }
    }

    #[test]
    fn chat_message_backward_compat() {
        // Step 1 frontend sends {"role":"user","content":"hi"}
        let msg: ChatMessage = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content, MessageContent::Text("hi".to_string()));

        // Round-trip: serializes back as plain string
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"role":"user","content":"hi"}"#);
    }

    #[test]
    fn chat_message_with_tool_use() {
        let json = r#"{"role":"assistant","content":[
            {"type":"text","text":"let me read that"},
            {"type":"tool_use","id":"toolu_123","name":"read_file","input":{"path":"/etc/hosts"}}
        ]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        match &msg.content {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2);
                assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "let me read that"));
                assert!(matches!(&blocks[1], ContentBlock::ToolUse { name, .. } if name == "read_file"));
            }
            _ => panic!("expected Blocks"),
        }
    }

    #[test]
    fn chat_message_with_tool_result() {
        let json = r#"{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu_123","content":"127.0.0.1 localhost"}
        ]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        match &msg.content {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert!(matches!(&blocks[0], ContentBlock::ToolResult { content, is_error, .. }
                    if content == "127.0.0.1 localhost" && !is_error));
            }
            _ => panic!("expected Blocks"),
        }
    }

    #[test]
    fn chat_request_tools_omitted_when_empty() {
        let req = ChatRequest {
            model: "test".to_string(),
            max_tokens: 100,
            messages: vec![],
            system: None,
            stream: true,
            tools: vec![],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("tools"));
    }

    #[test]
    fn chat_request_tools_present_when_nonempty() {
        let req = ChatRequest {
            model: "test".to_string(),
            max_tokens: 100,
            messages: vec![],
            system: None,
            stream: true,
            tools: vec![ToolDef {
                name: "read_file".to_string(),
                description: Some("read a file".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"tools\""));
        assert!(json.contains("\"read_file\""));
    }

    #[test]
    fn message_content_to_text() {
        let blocks = vec![
            ContentBlock::Text {
                text: "hello ".to_string(),
            },
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({}),
            },
            ContentBlock::Text {
                text: "world".to_string(),
            },
        ];
        let mc = MessageContent::Blocks(blocks);
        assert_eq!(mc.to_text(), "hello world");
    }
}
