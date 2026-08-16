//! Wire types — provider-agnostic wire representation.
//!
//! See [`super`] module doc for the full wire-layer architecture.
//!
//! Split from wire.rs (08-07-large-file-splitting): this file owns the
//! type definitions shared by both conversion directions.

use crate::llm::types::CacheControl;
use serde_json::Value;

// ---------------------------------------------------------------------------
// WireCapabilities
// ---------------------------------------------------------------------------

/// Static, model-level capabilities used to drive the
/// `strip_unsupported` pass and any future capability-gated dispatch.
///
/// Distinct from [`crate::llm::provider::ProviderCapabilities`], which
/// is *protocol-level* (does the protocol support tools / streaming).
/// This struct is *model-level* — derived from `ModelRow` at
/// dispatch time:
///
/// - `supports_thinking = model_row.supports_thinking`
/// - `supports_reasoning_effort = openai-style o1/o3 reasoning
///   capability; today this is `true` iff `thinking_effort` is set
///   (which is the signal we currently use for OpenAI reasoning
///   support). A future PR may add an explicit column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireCapabilities {
    /// Whether the target can accept / emit a `Reasoning` /
    /// `thinking` block in its wire payload.
    pub supports_thinking: bool,
    /// Whether the target supports a `reasoning_effort` top-level
    /// field (OpenAI o1/o3 style). Independent of `supports_thinking`
    /// — a model could support one but not the other.
    pub supports_reasoning_effort: bool,
    /// Whether the target can round-trip the opaque Anthropic
    /// `signature` / `redacted_thinking.data` blobs. Today only
    /// Anthropic can.
    pub supports_thinking_signatures: bool,
    /// B1 (2026-08-16): whether the target model accepts image
    /// content. Derived from `ModelRow.supports_images` (explicit
    /// user configuration, no model-name heuristics). When false the
    /// strip pass **replaces** image blocks with a text placeholder
    /// (never silently drops — the model is told an image was
    /// attached but not delivered, so it doesn't hallucinate having
    /// seen one).
    pub supports_images: bool,
}

impl WireCapabilities {
    /// Derive from a [`crate::db::ModelRow`]. The decision matrix:
    ///
    /// - `supports_thinking` ← `model_row.supports_thinking`
    /// - `supports_reasoning_effort` ← `model_row.thinking_effort.is_some()`
    ///   (presence of a configured effort is the signal that the
    ///   user has opted the model into reasoning effort — works for
    ///   both Anthropic adaptive and OpenAI o1/o3)
    /// - `supports_thinking_signatures` ←
    ///   `model_row.supports_thinking && protocol is anthropic`
    ///   (only Anthropic can carry the signature blob; OpenAI
    ///   drops it on cross-protocol send)
    /// - `supports_images` ← `model_row.supports_images` (B1)
    #[allow(dead_code)] // consumed by future PRs that thread capabilities through Provider::send
    pub fn from_model_row(model: &crate::db::ModelRow, provider_protocol: &str) -> Self {
        let supports_thinking = model.supports_thinking;
        let supports_reasoning_effort = model.thinking_effort.is_some();
        let supports_thinking_signatures = supports_thinking && provider_protocol == "anthropic";
        Self {
            supports_thinking,
            supports_reasoning_effort,
            supports_thinking_signatures,
            supports_images: model.supports_images,
        }
    }
}

// ---------------------------------------------------------------------------
// WireRequest / WireMessage / WireBlock / WireTool
// ---------------------------------------------------------------------------

/// Provider-agnostic request shape. Carries the same logical
/// information as [`ChatRequest`] but with block-level fidelity that
/// lets cross-protocol conversion stay lossless.
#[derive(Debug, Clone)]
pub struct WireRequest {
    pub model: String,
    pub max_tokens: Option<u32>,
    pub system: Option<String>,
    pub messages: Vec<WireMessage>,
    pub tools: Vec<WireTool>,
}

/// One message in the conversation. Provider-agnostic — the
/// provider-wire converter picks the right shape per protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum WireMessage {
    /// A user-role message. Content is plain text (Anthropic and
    /// OpenAI both accept string content for `role: "user"`).
    ///
    /// `speaker` (group chat, 07-29-group-chat): which participant
    /// authored this message. `None` for classic-chat messages.
    /// Forward-pass threaded from [`ChatMessage::speaker`]; the
    /// OpenAI adapter emits it as the native `name` field, the
    /// Anthropic adapter consumes it to inject an `@name:` prefix
    /// (then strips it so Anthropic's API never sees the field).
    User {
        content: String,
        speaker: Option<String>,
    },
    /// A user-role message whose content MUST remain block-shaped
    /// (multi-block, or any block carrying a [`CacheControl`]
    /// marker). Anthropic serializes this as a content array; the
    /// OpenAI adapter flattens it to a string (cache_control is
    /// dropped, which is correct — OpenAI Chat Completions has no
    /// prompt-cache marker). Used by the B5 memory refactor
    /// (2026-06-11) to keep the synthetic instructions message's
    /// `cache_control: ephemeral` from being concatenated away.
    UserBlocks { blocks: Vec<WireBlock> },
    /// An assistant-role message. The model may emit text, reasoning,
    /// tool_use, signature blobs, or redacted-thinking payloads —
    /// all stored in order in `blocks`.
    ///
    /// `speaker` (group chat): see [`WireMessage::User::speaker`].
    Assistant {
        blocks: Vec<WireBlock>,
        speaker: Option<String>,
    },
    /// A tool result. Mapped to:
    /// - Anthropic: a `role: "user"` message with a `tool_result`
    ///   block.
    /// - OpenAI: a `role: "tool"` message with `tool_call_id` +
    ///   `content` (a string).
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// One content block inside an assistant message, or the
/// provider-agnostic representation of a tool result.
#[derive(Debug, Clone, PartialEq)]
pub enum WireBlock {
    Text {
        text: String,
        /// Anthropic prompt-cache breakpoint marker. When `Some`,
        /// the Anthropic adapter emits a `cache_control` field
        /// next to this text block. The wire layer preserves this
        /// block as a distinct entry (does NOT concatenate it with
        /// adjacent text) so the cache boundary is exact.
        ///
        /// The OpenAI adapter drops this field when serializing
        /// (OpenAI Chat Completions has no prompt-cache marker).
        cache_control: Option<CacheControl>,
    },
    /// Provider-agnostic reasoning block. Mapped to:
    /// - Anthropic `thinking` block (when target supports thinking).
    /// - OpenAI `reasoning_content` field of the streaming delta
    ///   (when target supports reasoning_effort).
    /// - Dropped otherwise.
    Reasoning { text: String },
    /// Anthropic-only opaque signature blob. Always paired with a
    /// preceding `Reasoning` block. Dropped on cross-protocol send
    /// to OpenAI (opaque — cannot be mapped).
    #[allow(dead_code)]
    // constructed by Anthropic-side wire parser; cross-protocol strip can drop
    Signature { data: String },
    /// Anthropic-only `redacted_thinking` opaque payload. Dropped
    /// on cross-protocol send to OpenAI.
    RedactedThinking { data: String },
    /// A model-issued tool call. `input` is already-parsed JSON.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// B1 (2026-08-16): resolved image (base64) riding a user-role
    /// message. Mapped to:
    /// - Anthropic `image` block with a base64 `source` (free via
    ///   serde — see `ContentBlock::Image`).
    /// - OpenAI `image_url` with a `data:` URL.
    /// When the target's `supports_images` cap is false, the strip
    /// pass replaces this block with a text placeholder.
    Image { media_type: String, data: String },
}

/// Tool declaration. `description` and `input_schema` are
/// `Option`-friendly at the wire layer; the provider-wire converter
/// enforces each protocol's requirement (Anthropic: both required;
/// OpenAI: both required but wrapped under `function: {…}`).
#[derive(Debug, Clone)]
pub struct WireTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

// ---------------------------------------------------------------------------
// ChatRequest → WireRequest
// ---------------------------------------------------------------------------
