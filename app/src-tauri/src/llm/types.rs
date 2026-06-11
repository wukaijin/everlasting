//! LLM request / response / event types.
//!
//! Step 2 extends the step 1 types to support Anthropic-style tool calling:
//! - `ContentBlock` for structured message content (text / tool_use / tool_result)
//! - `MessageContent` with custom Serde to accept both plain string and block array
//! - `ToolDef` for declaring tools in the request
//! - `ChatEvent` gains `ToolCall` and `ToolResult` variants
//!
//! Step 6 (this task) adds extended thinking support:
//! - `ContentBlock::Thinking` and `ContentBlock::RedactedThinking` (Anthropic
//!   extended-thinking content blocks).
//! - `ChatRequest::thinking` accepts an optional `ThinkingConfig` (currently
//!   the `adaptive` variant). When present, the request asks the model to
//!   think before answering.
//! - `ChatEvent::ThinkingDelta`, `ChatEvent::SignatureDelta` and
//!   `ChatEvent::RedactedThinkingDelta` are streamed to the frontend as the
//!   model emits `thinking_delta` / `signature_delta` SSE events and as
//!   `redacted_thinking` content blocks close.

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
// CacheControl — Anthropic prompt cache breakpoint marker
// ---------------------------------------------------------------------------

/// A `cache_control` hint attached to a content block. Anthropic's
/// Messages API reads this field to decide where to put a cache
/// breakpoint — the LAST block in a request that carries this
/// marker is the cache boundary; everything before it becomes
/// eligible for a cache hit on the next turn (within the 5-min
/// TTL).
///
/// The B5 memory refactor (2026-06-11) attaches `Ephemeral` to
/// the first content block of the synthetic "instructions" user
/// message so the 4 instruction files (CLAUDE.md / AGENTS.md ×
/// user / project) are cached on turn 1 and read from cache on
/// turns 2..MAX_TURNS. Without this marker, Anthropic would
/// 100% miss every turn and re-bill the full instructions
/// payload.
///
/// Today only `Ephemeral` exists (5-min TTL, 1.25× write /
/// 0.1× read pricing). A future `Persistent` (1-hour TTL) variant
/// can land here without a schema break — the tagged-enum shape
/// is forward-compatible.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum CacheControl {
    Ephemeral,
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
        /// Optional Anthropic prompt-cache breakpoint. When `Some`,
        /// the wire layer preserves this block as a separate
        /// content block (does NOT concatenate it with adjacent
        /// text blocks) and the Anthropic adapter emits
        /// `cache_control: {"type": "ephemeral"}` next to the
        /// block. See [`CacheControl`] for the cost model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Anthropic extended-thinking content block. `thinking` is the streamed
    /// (or summarized, depending on `display`) summary text the model
    /// produces while reasoning; `signature` is the opaque, encrypted blob
    /// the model emits at the end of the block and which MUST be echoed
    /// back verbatim in subsequent turns — otherwise the API returns 400.
    Thinking {
        thinking: String,
        signature: String,
    },
    /// Anthropic `redacted_thinking` block: emitted when the server
    /// encrypts part of a thinking block (e.g. for safety reasons). The
    /// `data` field is opaque, undisplayable, and MUST be echoed back
    /// verbatim in subsequent turns.
    RedactedThinking {
        data: String,
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
/// array of [`ContentBlock`] (step 2+ tool calling; step 6+ thinking).
#[derive(Debug, Clone, PartialEq)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Extract all *visible* text from this content — used for the
    /// denormalized `text` column in the DB and for the session-list
    /// preview. **Thinking text is intentionally excluded** so that the
    /// sidebar preview only shows user-typed / assistant-said text and the
    /// persisted `text` field stays a useful search/index surface.
    pub fn to_text(&self) -> String {
        match self {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
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
/// with step 1) or an array of ContentBlocks (tool_use / tool_result /
/// thinking / redacted_thinking).
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
    Adaptive {
        display: String,
        effort: String,
    },
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

// ---------------------------------------------------------------------------
// ChatEvent — events pushed to the frontend
// ---------------------------------------------------------------------------

/// One thinking content block. The model can produce multiple blocks per
/// turn (interleaved thinking with tool calls); each must be preserved
/// in order and round-tripped back to the LLM verbatim, otherwise the
/// next turn 400s. `text` is the streamed summary (or empty under
/// `display: "omitted"`); `signature` is the opaque, encrypted blob. */
// ---------------------------------------------------------------------------
// TokenUsage — A4 (Token Usage Tracking)
// ---------------------------------------------------------------------------

/// Token usage reported by the LLM at the end of one turn. Schema is
/// **protocol-agnostic** — both Anthropic (`message_delta.usage`) and
/// OpenAI (last-chunk `usage`) are normalized into this 4-field shape
/// by the provider before yielding `ChatEvent::Done`. The agent loop
/// reads `Option<TokenUsage>` from `ChatEvent::Done` and accumulates
/// it into `sessions` (per-session totals).
///
/// Field semantics (Anthropic schema — the reference):
///
/// - `input_tokens` — total input tokens for the request, **inclusive
///   of** `cache_creation_input_tokens` + `cache_read_input_tokens`.
///   This is the value displayed in the ChatInput hint as "current
///   context usage, not cumulative" (per Anthropic's statusline
///   convention; same shape as `sanztheo/claude-code-statusline`).
/// - `output_tokens` — tokens generated by the model. **Not** counted
///   as context pressure (the response is not context).
/// - `cache_creation_input_tokens` — newly created cache tokens
///   (eligible for a cache hit next turn).
/// - `cache_read_input_tokens` — cache tokens served from a prior
///   `cache_creation` (the cheap path). The Anthropic API bills
///   these at 0.1x input; OpenAI's `cached_tokens` at 0.5x.
///
/// OpenAI normalization (PR3 of multi-model):
///
/// - `prompt_tokens` → `input_tokens`
/// - `completion_tokens` → `output_tokens`
/// - `prompt_tokens_details.cached_tokens` → `cache_read_input_tokens`
/// - `cache_creation_input_tokens` → 0 (no OpenAI equivalent today)
///
/// `None` from a `ChatEvent::Done` means the provider did not (or
/// could not) report usage — typically cancel, network drop, or
/// pre-usage error. The agent loop treats `None` as "skip the
/// accumulation write" and logs at `info!`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
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
    /// Incremental thinking summary from the model. Streamed via
    /// `thinking_delta` SSE events when `display: "summarized"` is set.
    ThinkingDelta { text: String },
    /// Opaque signature blob emitted at the end of a thinking block
    /// (via `signature_delta`). The frontend must keep this so it can be
    /// round-tripped to the LLM on the next turn.
    SignatureDelta { signature: String },
    /// Opaque `redacted_thinking.data` payload. Emitted once when a
    /// `redacted_thinking` content block closes. The frontend must keep
    /// this so it can be round-tripped to the LLM on the next turn; the
    /// payload is not displayable.
    RedactedThinkingDelta { data: String },
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
    /// `usage` is the A4 token-usage payload (normalized across protocols
    /// — see [`TokenUsage`] for the schema). `None` means the stream
    /// ended without a usage report (cancel / error / network drop);
    /// the agent loop skips the per-session accumulation write in
    /// that case.
    Done {
        stop_reason: Option<String>,
        usage: Option<TokenUsage>,
    },
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
            cache_control: None,
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
                        text: "hello".to_string(),
                        cache_control: None,
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
                assert!(matches!(&blocks[0], ContentBlock::Text { text, .. } if text == "let me read that"));
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
            thinking: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("tools"));
        assert!(!json.contains("thinking"));
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
            thinking: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"tools\""));
        assert!(json.contains("\"read_file\""));
    }

    #[test]
    fn chat_request_thinking_omitted_when_none() {
        let req = ChatRequest {
            model: "claude-opus-4-7".to_string(),
            max_tokens: 16384,
            messages: vec![],
            system: None,
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("thinking"));
    }

    #[test]
    fn chat_request_thinking_adaptive_serializes_correctly() {
        let req = ChatRequest {
            model: "claude-opus-4-7".to_string(),
            max_tokens: 16384,
            messages: vec![],
            system: None,
            stream: true,
            tools: vec![],
            thinking: Some(ThinkingConfig::Adaptive {
                display: "summarized".to_string(),
                effort: "high".to_string(),
            }),
        };
        let json = serde_json::to_string(&req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let t = v.get("thinking").expect("thinking key present");
        assert_eq!(t.get("type").and_then(|s| s.as_str()), Some("adaptive"));
        assert_eq!(
            t.get("display").and_then(|s| s.as_str()),
            Some("summarized")
        );
        assert_eq!(t.get("effort").and_then(|s| s.as_str()), Some("high"));
    }

    #[test]
    fn message_content_to_text() {
        let blocks = vec![
            ContentBlock::Text {
                text: "hello ".to_string(),
                cache_control: None,
            },
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({}),
            },
            ContentBlock::Text {
                text: "world".to_string(),
                cache_control: None,
            },
        ];
        let mc = MessageContent::Blocks(blocks);
        assert_eq!(mc.to_text(), "hello world");
    }

    // -----------------------------------------------------------------------
    // Thinking block round-trips
    // -----------------------------------------------------------------------

    #[test]
    fn thinking_block_serializes_to_anthropic_schema() {
        let block = ContentBlock::Thinking {
            thinking: "let me think...".to_string(),
            signature: "EqQBCgIYAhIM1gbcDa...".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("type").and_then(|s| s.as_str()), Some("thinking"));
        assert_eq!(
            v.get("thinking").and_then(|s| s.as_str()),
            Some("let me think...")
        );
        assert_eq!(
            v.get("signature").and_then(|s| s.as_str()),
            Some("EqQBCgIYAhIM1gbcDa...")
        );
    }

    #[test]
    fn thinking_block_deserializes_from_anthropic_schema() {
        let json = r#"{"type":"thinking","thinking":"analyze GCD","signature":"abc123"}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(
            block,
            ContentBlock::Thinking {
                thinking: "analyze GCD".to_string(),
                signature: "abc123".to_string(),
            }
        );
    }

    #[test]
    fn redacted_thinking_block_serializes_to_anthropic_schema() {
        let block = ContentBlock::RedactedThinking {
            data: "EmwKAhIM1gbcDa9GJwZA".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.get("type").and_then(|s| s.as_str()),
            Some("redacted_thinking")
        );
        assert_eq!(
            v.get("data").and_then(|s| s.as_str()),
            Some("EmwKAhIM1gbcDa9GJwZA")
        );
    }

    #[test]
    fn redacted_thinking_block_deserializes_from_anthropic_schema() {
        let json = r#"{"type":"redacted_thinking","data":"EmwKAhIM1gbcDa9GJwZA"}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(
            block,
            ContentBlock::RedactedThinking {
                data: "EmwKAhIM1gbcDa9GJwZA".to_string(),
            }
        );
    }

    #[test]
    fn chat_message_round_trip_with_thinking_blocks() {
        // The full assistant turn: text + thinking + tool_use. Must round-trip
        // losslessly so the LLM gets the exact signature back on the next
        // turn (otherwise it 400s).
        let json = r#"{"role":"assistant","content":[
            {"type":"thinking","thinking":"need to read the file","signature":"sig_abc"},
            {"type":"text","text":"OK, reading now"},
            {"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"/etc/hosts"}}
        ]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        // Re-serialize and re-parse: must produce the same blocks.
        let re = serde_json::to_string(&msg).unwrap();
        let msg2: ChatMessage = serde_json::from_str(&re).unwrap();
        assert_eq!(msg, msg2);

        match &msg2.content {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 3);
                assert!(matches!(&blocks[0], ContentBlock::Thinking { thinking, signature }
                    if thinking == "need to read the file" && signature == "sig_abc"));
            }
            _ => panic!("expected Blocks"),
        }
    }

    #[test]
    fn chat_message_round_trip_with_redacted_thinking() {
        let json = r#"{"role":"assistant","content":[
            {"type":"redacted_thinking","data":"EmwKAhIM1gbcDa9GJwZA"},
            {"type":"text","text":"answer"}
        ]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        let re = serde_json::to_string(&msg).unwrap();
        let msg2: ChatMessage = serde_json::from_str(&re).unwrap();
        assert_eq!(msg, msg2);
    }

    #[test]
    fn message_content_to_text_excludes_thinking() {
        // Thinking text must NOT leak into the denormalized `text` column
        // (DB text is used for sidebar previews / search).
        let blocks = vec![
            ContentBlock::Thinking {
                thinking: "secret thought".to_string(),
                signature: "sig".to_string(),
            },
            ContentBlock::Text {
                text: "visible answer".to_string(),
                cache_control: None,
            },
            ContentBlock::RedactedThinking {
                data: "redacted".to_string(),
            },
        ];
        let mc = MessageContent::Blocks(blocks);
        assert_eq!(mc.to_text(), "visible answer");
    }

    #[test]
    fn chat_event_thinking_delta_serializes_with_snake_case_kind() {
        let ev = ChatEvent::ThinkingDelta {
            text: "analyzing".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.get("kind").and_then(|s| s.as_str()),
            Some("thinking_delta")
        );
        assert_eq!(v.get("text").and_then(|s| s.as_str()), Some("analyzing"));
    }

    #[test]
    fn chat_event_signature_delta_serializes_with_snake_case_kind() {
        let ev = ChatEvent::SignatureDelta {
            signature: "sig_xyz".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.get("kind").and_then(|s| s.as_str()),
            Some("signature_delta")
        );
        assert_eq!(
            v.get("signature").and_then(|s| s.as_str()),
            Some("sig_xyz")
        );
    }

    #[test]
    fn chat_event_redacted_thinking_delta_serializes_with_snake_case_kind() {
        let ev = ChatEvent::RedactedThinkingDelta {
            data: "redacted_blob".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.get("kind").and_then(|s| s.as_str()),
            Some("redacted_thinking_delta")
        );
        assert_eq!(
            v.get("data").and_then(|s| s.as_str()),
            Some("redacted_blob")
        );
    }

    // -----------------------------------------------------------------------
    // A4 — TokenUsage
    // -----------------------------------------------------------------------

    #[test]
    fn token_usage_serializes_with_snake_case_fields() {
        // The IPC payload crosses the Tauri boundary camelCase by
        // default, but the inner JSON object keeps snake_case for
        // these field names (the outer `kind` discriminator and the
        // inner `stop_reason` are both snake_case in the existing
        // `ChatEvent::Done` shape — see backend/llm-contract.md
        // §"Scenario: Token Usage Tracking" §3).
        let u = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 20,
        };
        let json = serde_json::to_string(&u).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("input_tokens"), Some(&serde_json::json!(100)));
        assert_eq!(v.get("output_tokens"), Some(&serde_json::json!(50)));
        assert_eq!(
            v.get("cache_creation_input_tokens"),
            Some(&serde_json::json!(10))
        );
        assert_eq!(
            v.get("cache_read_input_tokens"),
            Some(&serde_json::json!(20))
        );
    }

    #[test]
    fn token_usage_default_is_all_zero() {
        // The DB's `UPDATE col = col + ?` path doesn't see a
        // default-zero, but the `Done { usage: None }` -> no-op
        // path means the chat command never constructs a
        // default-zero usage. This test just locks the contract
        // that `Default::default() == TokenUsage { 0, 0, 0, 0 }`.
        let u = TokenUsage::default();
        assert_eq!(u.input_tokens, 0);
        assert_eq!(u.output_tokens, 0);
        assert_eq!(u.cache_creation_input_tokens, 0);
        assert_eq!(u.cache_read_input_tokens, 0);
    }

    #[test]
    fn chat_event_done_carries_usage_payload() {
        // The A4 wire shape: `Done { stop_reason, usage }`. The
        // `usage` field is serialized with the inner `kind` tag
        // already supplied by the outer enum, so the payload
        // looks like:
        //
        //   { "kind": "done", "stop_reason": "end_turn",
        //     "usage": { "input_tokens": 100, ... } }
        //
        // when `Some`, and `usage: null` (or absent — we use
        // `skip_serializing_if` below for compact payloads) when
        // `None`. The agent loop checks `Some(t) => accumulate`,
        // `None => skip`.
        let ev = ChatEvent::Done {
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 25,
            }),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("kind").and_then(|s| s.as_str()), Some("done"));
        assert_eq!(
            v.get("stop_reason").and_then(|s| s.as_str()),
            Some("end_turn")
        );
        let usage = v.get("usage").expect("usage key present");
        assert_eq!(usage.get("input_tokens"), Some(&serde_json::json!(100)));
        assert_eq!(usage.get("cache_read_input_tokens"), Some(&serde_json::json!(25)));
    }

    #[test]
    fn chat_event_done_with_none_usage_emits_null() {
        // Cancel / error / network drop path: usage is None.
        // The agent loop's `if let Some(t) = event.usage` check
        // skips accumulation, so the None case must be
        // distinguishable from `Some(TokenUsage::default())`
        // (which would otherwise be a no-op write, but it's
        // wasteful — we should be able to skip the SQL
        // round-trip).
        let ev = ChatEvent::Done {
            stop_reason: Some("cancelled".to_string()),
            usage: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("kind").and_then(|s| s.as_str()), Some("done"));
        // `usage` is present as JSON null (not absent) so the
        // frontend's TypeScript side can rely on the key being
        // there. (`serde(tag = "kind")` does not skip None
        // fields by default.)
        assert!(v.get("usage").map(|x| x.is_null()).unwrap_or(false));
    }
}
