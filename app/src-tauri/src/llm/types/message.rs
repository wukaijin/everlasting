//! Message content primitives: `Role`, `CacheControl`, `ContentBlock`,
//! and the string-or-array `MessageContent` wrapper with its custom Serde impls.
//!
//! Split out of `llm/types.rs` (2026-08-08 batch3). These types are tightly
//! coupled — `MessageContent::Blocks` wraps `Vec<ContentBlock>`, and the
//! manual Serde impls are the only way to round-trip the string-or-array shape.

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
    Thinking { thinking: String, signature: String },
    /// Anthropic `redacted_thinking` block: emitted when the server
    /// encrypts part of a thinking block (e.g. for safety reasons). The
    /// `data` field is opaque, undisplayable, and MUST be echoed back
    /// verbatim in subsequent turns.
    RedactedThinking { data: String },
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
    /// B1 (2026-08-16): stable image **reference** — a file name inside
    /// the session's attachments directory. This is the form that
    /// lives in history / metadata / group-chat rewrites: lightweight
    /// to clone and serializable without dragging megabytes of base64
    /// through C3 compaction estimates, `role_history` clones, or SSE
    /// payloads. Never sent as-is: the request builder resolves it to
    /// [`ContentBlock::Image`] right before `provider.send` (one disk
    /// read per turn; see `agent` image resolve pass). Serde tag
    /// `"image_ref"` is internal-only — it never appears on a provider
    /// wire.
    ImageRef { file: String, media_type: String },
    /// B1: resolved pre-send image (base64). Exists only in the
    /// request copy between the resolve pass and `provider.send`.
    /// Serializes as the Anthropic-native image block
    /// (`{"type":"image","source":{"type":"base64",…}}`) because the
    /// Anthropic adapter serde-serializes the reconstructed
    /// `ChatRequest` verbatim; the OpenAI adapter maps it to
    /// `image_url` with a data URL. When the model's
    /// `supports_images` cap is false, the wire strip pass replaces
    /// this block with a text placeholder instead of dropping it (the
    /// model must know an image was attached but not delivered).
    Image { source: ImageSource },
}

/// B1 (2026-08-16): Anthropic-shaped image source payload. The
/// `source_type` is always `"base64"` today; kept as a data field
/// (not an enum) so the serde shape matches Anthropic's wire exactly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

pub(crate) fn is_false(b: &bool) -> bool {
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
