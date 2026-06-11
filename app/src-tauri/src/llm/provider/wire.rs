//! Provider-agnostic wire representation (PR3 of the multi-model task).
//!
//! Both the Anthropic adapter and the OpenAI adapter convert from
//! [`crate::llm::types::ChatRequest`] / [`crate::llm::types::ChatEvent`]
//! (Anthropic-shaped) to / from this intermediate [`WireRequest`] /
//! [`WireMessage`] / [`WireBlock`] form, then to / from the actual
//! provider wire format. The wire module is the single place that
//! knows how to:
//!
//! 1. Map the Anthropic-shaped `ChatMessage` / `ContentBlock` types
//!    into a provider-agnostic shape that has explicit variants for
//!    things only Anthropic supports (signature blobs, redacted
//!    thinking) and things only OpenAI supports (Reasoning).
//! 2. Run a `strip_unsupported` pass that drops blocks the target
//!    protocol cannot represent, driven by the target's
//!    [`WireCapabilities`]. This is the **silent degradation** the
//!    parent PRD §Q5 H1 decision locked in: switching from a
//!    `supports_thinking=true` Anthropic model to a non-thinking
//!    OpenAI model silently drops the thinking blocks (they stay in
//!    the DB; only the wire payload this turn omits them).
//!
//! The wire layer is **purely in-memory** — no IO, no DB. The
//! provider's `send` is the single call site that invokes it:
//!
//! ```text
//! ChatRequest  --(chat_request_to_wire)-->  WireRequest
//!                     |
//!                     v
//!           (strip_unsupported, target_caps)
//!                     |
//!                     v
//!               WireRequest
//!                     |
//!                     v
//!          (provider-wire converter)
//!                     |
//!                     v
//!         actual upstream HTTP body
//! ```
//!
//! Conversely the stream is converted block-by-block: each
//! `WireBlock` arriving from the provider's parser is mapped to a
//! [`ChatEvent`] (or None for blocks that shouldn't surface to the
//! frontend) via [`wire_block_to_chat_event`].

use serde_json::Value;

use crate::llm::types::{
    CacheControl, ChatEvent, ChatMessage, ChatRequest, ContentBlock, MessageContent, Role, ToolDef,
};

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
    #[allow(dead_code)] // consumed by future PRs that thread capabilities through Provider::send
    pub fn from_model_row(
        model: &crate::db::ModelRow,
        provider_protocol: &str,
    ) -> Self {
        let supports_thinking = model.supports_thinking;
        let supports_reasoning_effort = model.thinking_effort.is_some();
        let supports_thinking_signatures =
            supports_thinking && provider_protocol == "anthropic";
        Self {
            supports_thinking,
            supports_reasoning_effort,
            supports_thinking_signatures,
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
    /// OpenAI-style `reasoning_effort` (e.g. "low" / "medium" / "high").
    /// `None` means "no reasoning effort requested" — neither Anthropic
    /// nor OpenAI will see a reasoning field. Anthropic's adaptive
    /// thinking is signalled separately via [`crate::llm::types::ThinkingConfig`]
    /// outside the wire layer.
    #[allow(dead_code)] // populated by `chat_request_to_wire`; consumer reads it in OpenAI adapter
    pub reasoning_effort: Option<String>,
}

/// One message in the conversation. Provider-agnostic — the
/// provider-wire converter picks the right shape per protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum WireMessage {
    /// A user-role message. Content is plain text (Anthropic and
    /// OpenAI both accept string content for `role: "user"`).
    User { content: String },
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
    Assistant { blocks: Vec<WireBlock> },
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
    #[allow(dead_code)] // constructed by Anthropic-side wire parser; cross-protocol strip can drop
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

/// Convert the Anthropic-shaped [`ChatRequest`] into the
/// provider-agnostic [`WireRequest`].
///
/// - `system` is passed through verbatim (it has the same semantic
///   on both protocols).
/// - `max_tokens` is preserved as `Option<u32>` at the wire layer
///   (the OpenAI side decides whether to send it as
///   `max_tokens` vs `max_completion_tokens`).
/// - Messages are walked and each `ContentBlock` is mapped to the
///   right `WireBlock` variant. `tool_result` blocks are lifted out
///   into a `WireMessage::Tool` (Anthropic's `tool_result` lives
///   inside a `role: "user"` message with content blocks; OpenAI's
///   `role: "tool"` is a separate message).
/// - `reasoning_effort` is initialized to `None`; the caller (the
///   provider's `send` method) sets it from
///   `model_row.thinking_effort` if appropriate.
pub fn chat_request_to_wire(
    req: ChatRequest,
    system: Option<String>,
) -> WireRequest {
    let messages = req
        .messages
        .into_iter()
        .flat_map(chat_message_to_wire_messages)
        .collect();
    let tools = req
        .tools
        .into_iter()
        .map(|t| WireTool {
            name: t.name,
            description: t.description,
            input_schema: t.input_schema,
        })
        .collect();
    WireRequest {
        model: req.model,
        max_tokens: Some(req.max_tokens),
        system,
        messages,
        tools,
        reasoning_effort: None,
    }
}

/// Convert one [`ChatMessage`] into zero, one, or more
/// [`WireMessage`]s. A `role: "user"` message containing a
/// `tool_result` block fans out into separate `Tool` messages
/// (one per block) so the OpenAI side can emit one `role: "tool"`
/// message per `tool_call_id`. A `role: "user"` message containing
/// only text stays a single `User` message.
fn chat_message_to_wire_messages(msg: ChatMessage) -> Vec<WireMessage> {
    match msg.role {
        Role::User => match msg.content {
            MessageContent::Text(s) => vec![WireMessage::User { content: s }],
            MessageContent::Blocks(blocks) => {
                // B5 refactor (2026-06-11): if any text block in
                // the user role carries a `cache_control` marker,
                // we must keep block boundaries (concatenation
                // would silently drop the cache marker, and
                // Anthropic would 100% miss every turn). The
                // legacy concatenation path is preserved for
                // everything else.
                let has_cacheable = blocks.iter().any(|b| {
                    matches!(
                        b,
                        ContentBlock::Text {
                            cache_control: Some(_),
                            ..
                        }
                    )
                });

                if has_cacheable {
                    let mut out: Vec<WireMessage> = Vec::new();
                    let mut pending: Vec<WireBlock> = Vec::new();
                    for block in blocks {
                        match block {
                            ContentBlock::Text {
                                text,
                                cache_control,
                            } => {
                                pending.push(WireBlock::Text {
                                    text,
                                    cache_control,
                                });
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                if !pending.is_empty() {
                                    out.push(WireMessage::UserBlocks {
                                        blocks: std::mem::take(&mut pending),
                                    });
                                }
                                out.push(WireMessage::Tool {
                                    tool_call_id: tool_use_id,
                                    content,
                                });
                            }
                            ContentBlock::Thinking { .. }
                            | ContentBlock::RedactedThinking { .. }
                            | ContentBlock::ToolUse { .. } => {
                                tracing::debug!(
                                    "chat_message_to_wire_messages: skipping unexpected assistant block in user-role message"
                                );
                            }
                        }
                    }
                    if !pending.is_empty() {
                        out.push(WireMessage::UserBlocks { blocks: pending });
                    }
                    out
                } else {
                    let mut out: Vec<WireMessage> = Vec::new();
                    let mut pending_text = String::new();
                    for block in blocks {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                pending_text.push_str(&text);
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                if !pending_text.is_empty() {
                                    out.push(WireMessage::User {
                                        content: std::mem::take(&mut pending_text),
                                    });
                                }
                                out.push(WireMessage::Tool {
                                    tool_call_id: tool_use_id,
                                    content,
                                });
                            }
                            ContentBlock::Thinking { .. }
                            | ContentBlock::RedactedThinking { .. }
                            | ContentBlock::ToolUse { .. } => {
                                tracing::debug!(
                                    "chat_message_to_wire_messages: skipping unexpected assistant block in user-role message"
                                );
                            }
                        }
                    }
                    if !pending_text.is_empty() {
                        out.push(WireMessage::User {
                            content: pending_text,
                        });
                    }
                    out
                }
            }
        },
        Role::Assistant => {
            let blocks: Vec<WireBlock> = match msg.content {
                MessageContent::Text(s) => vec![WireBlock::Text {
                    text: s,
                    cache_control: None,
                }],
                MessageContent::Blocks(blocks) => blocks
                    .into_iter()
                    .flat_map(content_block_to_wire_block)
                    .collect(),
            };
            vec![WireMessage::Assistant { blocks }]
        }
    }
}

fn content_block_to_wire_block(block: ContentBlock) -> Vec<WireBlock> {
    match block {
        ContentBlock::Text {
            text,
            cache_control,
        } => vec![WireBlock::Text {
            text,
            cache_control,
        }],
        ContentBlock::Thinking { thinking, signature } => {
            // The Anthropic-side Thinking block carries both
            // `thinking` text and an opaque `signature`. We split
            // the signature out into a separate `Signature` block
            // so cross-protocol strip can drop it independently of
            // the visible text. The inverse path
            // (`wire_message_to_chat_messages`) recombines a
            // consecutive `Reasoning`+`Signature` pair into a
            // single `Thinking { thinking, signature }` block so
            // the Anthropic round-trip is 1:1 with the pre-PR3
            // shape.
            //
            // An empty signature (defensive — a hand-built
            // ChatMessage could in theory have one) stays as a
            // single `Reasoning` block, no `Signature` block.
            if signature.is_empty() {
                vec![WireBlock::Reasoning { text: thinking }]
            } else {
                vec![
                    WireBlock::Reasoning { text: thinking },
                    WireBlock::Signature { data: signature },
                ]
            }
        }
        ContentBlock::RedactedThinking { data } => vec![WireBlock::RedactedThinking { data }],
        ContentBlock::ToolUse { id, name, input } => {
            vec![WireBlock::ToolUse { id, name, input }]
        }
        // `tool_result` is lifted out into `WireMessage::Tool`
        // before this point; a stray one here means a bug in
        // `chat_message_to_wire_messages`. Map to a text block
        // with a debug marker so the LLM still sees something
        // (better than silently dropping) and a `tracing::warn!`
        // surfaces the bug.
        ContentBlock::ToolResult { content, .. } => {
            tracing::warn!(
                "content_block_to_wire_block: stray tool_result in assistant role, mapping to text"
            );
            vec![WireBlock::Text {
                text: format!("[stray tool_result: {}]", content),
                cache_control: None,
            }]
        }
    }
}

// ---------------------------------------------------------------------------
// strip_unsupported
// ---------------------------------------------------------------------------

/// Remove blocks the target protocol cannot represent. Pure
/// function; no IO. Called inside `Provider::send` immediately
/// after `chat_request_to_wire` so the wire payload matches the
/// target's actual capabilities.
///
/// The decision matrix (one row per `WireBlock` variant):
///
/// | Variant              | `supports_thinking` | `supports_reasoning_effort` | `supports_thinking_signatures` | Outcome |
/// |----------------------|---------------------|------------------------------|----------------------------------|---------|
/// | `Text`               | *                   | *                            | *                                | keep    |
/// | `ToolUse`            | *                   | *                            | *                                | keep    |
/// | `Reasoning`          | true                | *                            | *                                | keep    |
/// | `Reasoning`          | false               | true                         | *                                | keep (will become reasoning_content on OpenAI) |
/// | `Reasoning`          | false               | false                        | *                                | drop    |
/// | `Signature`          | *                   | *                            | true                             | keep    |
/// | `Signature`          | *                   | *                            | false                            | drop    |
/// | `RedactedThinking`   | *                   | *                            | true                             | keep    |
/// | `RedactedThinking`   | *                   | *                            | false                            | drop    |
///
/// `Tool` messages are passed through unchanged — both protocols
/// support tool results, the wire shape differs (Anthropic
/// `tool_result` block vs OpenAI `role: "tool"`) but the conversion
/// is the provider's job, not the strip pass's.
///
/// `User` messages are passed through unchanged.
pub fn strip_unsupported(
    messages: Vec<WireMessage>,
    target_caps: &WireCapabilities,
) -> Vec<WireMessage> {
    messages
        .into_iter()
        .filter_map(|m| match m {
            WireMessage::User { content } => Some(WireMessage::User { content }),
            WireMessage::UserBlocks { blocks } => {
                // B5 refactor (2026-06-11): `UserBlocks` carries
                // block-level cache_control on text blocks. We
                // keep it intact here — `block_supported` is a
                // no-op for `Text` (see the decision matrix), so
                // the cache marker survives `strip_unsupported`.
                // Anthropic → OpenAI path: the OpenAI adapter
                // drops cache_control at serialization time, so
                // no special handling is needed here.
                Some(WireMessage::UserBlocks { blocks })
            }
            WireMessage::Tool {
                tool_call_id,
                content,
            } => Some(WireMessage::Tool {
                tool_call_id,
                content,
            }),
            WireMessage::Assistant { blocks } => {
                let filtered: Vec<WireBlock> = blocks
                    .into_iter()
                    .filter(|b| block_supported(b, target_caps))
                    .collect();
                // An assistant message that becomes empty after
                // strip is still meaningful: the model saw a
                // pure-reasoning turn. Keep it (with empty
                // blocks); the provider-wire converter will
                // decide whether to send it.
                Some(WireMessage::Assistant { blocks: filtered })
            }
        })
        .collect()
}

fn block_supported(block: &WireBlock, caps: &WireCapabilities) -> bool {
    match block {
        WireBlock::Text { .. } | WireBlock::ToolUse { .. } => true,
        WireBlock::Reasoning { .. } => {
            caps.supports_thinking || caps.supports_reasoning_effort
        }
        WireBlock::Signature { .. } | WireBlock::RedactedThinking { .. } => {
            caps.supports_thinking_signatures
        }
    }
}

// ---------------------------------------------------------------------------
// WireBlock → ChatEvent (streaming side)
// ---------------------------------------------------------------------------

/// Map a single [`WireBlock`] arriving from the provider-wire parser
/// to a [`ChatEvent`] the frontend understands. Returns `None` for
/// blocks that the frontend doesn't care about (e.g. `Signature` —
/// the frontend renders thinking text from `ThinkingDelta` events,
/// and the signature is consumed by the agent loop's
/// `pending_thinking` state, not displayed).
///
/// This function is **independent of protocol**: the provider's
/// wire parser already accumulated the block's full content (e.g.
/// a `WireBlock::ToolUse` has parsed `input` JSON, a `WireBlock::Signature`
/// has the full blob), and this function is a pure mapping.
#[allow(dead_code)] // used by tests; future PRs may call from a unified stream parser
pub fn wire_block_to_chat_event(block: &WireBlock) -> Option<ChatEvent> {
    match block {
        WireBlock::Text { text, .. } => Some(ChatEvent::Delta { text: text.clone() }),
        // `Reasoning` text is emitted as `ThinkingDelta` —
        // ChatEvent is the Anthropic-shaped one and we want the
        // frontend's existing thinking-rendering path to work
        // unchanged for OpenAI reasoning too. The
        // Anthropic-specific `Signature` blob is handled
        // separately by the SSE parser (it's the only path that
        // can deliver it; OpenAI reasoning has no signature).
        WireBlock::Reasoning { text } => {
            Some(ChatEvent::ThinkingDelta { text: text.clone() })
        }
        // The `Signature` blob is consumed at the agent-loop
        // boundary, not in the streaming ChatEvent — but for
        // cross-protocol symmetry we expose it as a
        // `SignatureDelta` when the parser hands us one. The
        // OpenAI parser never produces a `Signature`, so this
        // branch is Anthropic-only in practice.
        WireBlock::Signature { data } => {
            Some(ChatEvent::SignatureDelta { signature: data.clone() })
        }
        WireBlock::RedactedThinking { data } => {
            Some(ChatEvent::RedactedThinkingDelta { data: data.clone() })
        }
        // `ToolUse` is fully assembled by the parser (id / name /
        // parsed input) and maps 1:1 to `ToolCall`.
        WireBlock::ToolUse { id, name, input } => Some(ChatEvent::ToolCall {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        }),
    }
}

// ---------------------------------------------------------------------------
// ToolDef conversion (wire → request-shape, used by the Anthropic adapter
// which still consumes `Vec<ToolDef>` in its `chat_stream_with_tools`)
// ---------------------------------------------------------------------------

/// Re-construct the Anthropic-shaped [`ToolDef`] vector from the
/// wire representation. Used by the Anthropic adapter to keep its
/// `chat_stream_with_tools` signature unchanged (the legacy
/// function takes `Vec<ToolDef>`, not `Vec<WireTool>`). The
/// conversion is a verbatim field copy.
#[allow(dead_code)] // exposed for future protocol adapters; Anthropic adapter currently inlines this
pub fn wire_tools_to_tool_defs(tools: Vec<WireTool>) -> Vec<ToolDef> {
    tools
        .into_iter()
        .map(|t| ToolDef {
            name: t.name,
            description: t.description,
            input_schema: t.input_schema,
        })
        .collect()
}

#[allow(dead_code)]
/// Re-construct the Anthropic-shaped `Vec<ChatMessage>` from the
/// wire representation. The Anthropic adapter uses this to feed
/// its pre-existing `chat_stream_with_tools` function so the
/// PR2 SSE parser (which already speaks the Anthropic wire
/// format) is reused. Pure function; no IO.
pub fn wire_messages_to_chat_messages(messages: Vec<WireMessage>) -> Vec<ChatMessage> {
    messages
        .into_iter()
        .flat_map(wire_message_to_chat_messages)
        .collect()
}

fn wire_message_to_chat_messages(msg: WireMessage) -> Vec<ChatMessage> {
    match msg {
        WireMessage::User { content } => vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text(content),
        }],
        WireMessage::UserBlocks { blocks } => {
            // B5 refactor (2026-06-11): preserve block-level
            // cache_control on text blocks by routing back
            // through `MessageContent::Blocks`. The Anthropic
            // adapter serializes the block array (with
            // cache_control on the relevant block) and the
            // OpenAI adapter flattens the same array to a string
            // (dropping cache_control, which is fine — OpenAI
            // Chat Completions has no prompt-cache marker).
            let merged = wire_blocks_to_content_blocks(blocks);
            vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(merged),
            }]
        }
        WireMessage::Tool {
            tool_call_id,
            content,
        } => {
            // Anthropic's `tool_result` lives inside a
            // `role: "user"` message with content blocks.
            vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: tool_call_id,
                    content,
                    is_error: false,
                }]),
            }]
        }
        WireMessage::Assistant { blocks } => {
            // The forward pass (`content_block_to_wire_block`) splits
            // `Thinking { thinking, signature }` into a consecutive
            // `[Reasoning, Signature]` pair. The inverse recombines
            // a consecutive `Reasoning`+`Signature` pair into a
            // single `Thinking { thinking, signature }` block so the
            // Anthropic round-trip is byte-for-byte identical to
            // the pre-PR3 wire shape (and an Anthropic `thinking`
            // block with `signature: ""` does NOT 400 the next
            // turn).
            let merged = wire_blocks_to_content_blocks(blocks);
            vec![ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(merged),
            }]
        }
    }
}

/// Convert a `Vec<WireBlock>` into a `Vec<ContentBlock>`, fusing
/// a consecutive `Reasoning`+`Signature` pair into a single
/// `ContentBlock::Thinking { thinking, signature }` so the
/// Anthropic round-trip is byte-for-byte identical to the
/// pre-PR3 wire shape.
fn wire_blocks_to_content_blocks(blocks: Vec<WireBlock>) -> Vec<ContentBlock> {
    let mut out: Vec<ContentBlock> = Vec::with_capacity(blocks.len());
    let mut iter = blocks.into_iter();
    while let Some(b) = iter.next() {
        match b {
            WireBlock::Reasoning { text } => {
                // Peek the next block: if it's a `Signature`,
                // fuse them into one `Thinking` block. Otherwise
                // map the `Reasoning` to a `Thinking` with empty
                // signature.
                match iter.next() {
                    Some(WireBlock::Signature { data }) => {
                        // Forward-split case (the common one).
                        out.push(ContentBlock::Thinking {
                            thinking: text,
                            signature: data,
                        });
                    }
                    Some(other) => {
                        // A Reasoning with no following
                        // Signature. Emit the `Thinking` (with
                        // empty signature) and continue with
                        // `other`.
                        out.push(ContentBlock::Thinking {
                            thinking: text,
                            signature: String::new(),
                        });
                        out.push(wire_block_to_content_block(other));
                    }
                    None => {
                        out.push(ContentBlock::Thinking {
                            thinking: text,
                            signature: String::new(),
                        });
                    }
                }
            }
            other => out.push(wire_block_to_content_block(other)),
        }
    }
    out
}

fn wire_block_to_content_block(block: WireBlock) -> ContentBlock {
    match block {
        WireBlock::Text {
            text,
            cache_control,
        } => ContentBlock::Text {
            text,
            cache_control,
        },
        // A standalone `Reasoning` (no following `Signature`) maps
        // to a `Thinking` block with empty signature. This is
        // normally handled inside `wire_blocks_to_content_blocks`
        // (where the merge produces a fused `Thinking` block), but
        // is preserved here for the lone-block call site.
        WireBlock::Reasoning { text } => ContentBlock::Thinking {
            thinking: text,
            signature: String::new(),
        },
        // A standalone `Signature` (no preceding `Reasoning`) maps
        // to a `Thinking` block with empty text + the signature so
        // the round-trip doesn't lose the blob. Unreachable in
        // practice (the merge in `wire_blocks_to_content_blocks`
        // always pairs a `Reasoning` with the following
        // `Signature`); kept defensive.
        WireBlock::Signature { data } => ContentBlock::Thinking {
            thinking: String::new(),
            signature: data,
        },
        WireBlock::RedactedThinking { data } => ContentBlock::RedactedThinking { data },
        WireBlock::ToolUse { id, name, input } => {
            ContentBlock::ToolUse { id, name, input }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ModelRow;

    fn model(
        supports_thinking: bool,
        thinking_effort: Option<&str>,
    ) -> ModelRow {
        ModelRow {
            id: "mid".to_string(),
            provider_id: "pid".to_string(),
            model_name: "m".to_string(),
            display_name: "M".to_string(),
            max_tokens: Some(8192),
            thinking_effort: thinking_effort.map(str::to_string),
            supports_thinking,
            context_window: 200_000,
            created_at: "2026-06-09T00:00:00Z".to_string(),
            updated_at: "2026-06-09T00:00:00Z".to_string(),
        }
    }

    fn anthropic_caps(supports_thinking: bool) -> WireCapabilities {
        WireCapabilities {
            supports_thinking,
            supports_reasoning_effort: supports_thinking,
            supports_thinking_signatures: supports_thinking,
        }
    }

    fn openai_caps(supports_thinking: bool, reasoning: bool) -> WireCapabilities {
        WireCapabilities {
            supports_thinking,
            supports_reasoning_effort: reasoning,
            supports_thinking_signatures: false,
        }
    }

    // ---- WireCapabilities::from_model_row ----

    #[test]
    fn caps_anthropic_with_thinking_signatures_supported() {
        let m = model(true, Some("high"));
        let caps = WireCapabilities::from_model_row(&m, "anthropic");
        assert!(caps.supports_thinking);
        assert!(caps.supports_reasoning_effort);
        assert!(caps.supports_thinking_signatures);
    }

    #[test]
    fn caps_openai_drops_signatures_even_with_effort() {
        let m = model(false, Some("high"));
        let caps = WireCapabilities::from_model_row(&m, "openai");
        assert!(!caps.supports_thinking);
        assert!(caps.supports_reasoning_effort);
        assert!(!caps.supports_thinking_signatures);
    }

    #[test]
    fn caps_no_effort_disables_reasoning_effort() {
        let m = model(false, None);
        let caps = WireCapabilities::from_model_row(&m, "openai");
        assert!(!caps.supports_reasoning_effort);
    }

    // ---- chat_request_to_wire ----

    #[test]
    fn chat_request_to_wire_preserves_system_and_tools() {
        let req = ChatRequest {
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 16384,
            system: Some("You are a coding agent".to_string()),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("hello".to_string()),
            }],
            stream: true,
            tools: vec![ToolDef {
                name: "read_file".to_string(),
                description: Some("read".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, Some("You are a coding agent".to_string()));
        assert_eq!(wire.model, "claude-sonnet-4-5");
        assert_eq!(wire.system.as_deref(), Some("You are a coding agent"));
        assert_eq!(wire.tools.len(), 1);
        assert_eq!(wire.tools[0].name, "read_file");
        assert_eq!(wire.messages.len(), 1);
        assert!(matches!(&wire.messages[0], WireMessage::User { content } if content == "hello"));
    }

    #[test]
    fn chat_request_to_wire_lifts_tool_results_out_of_user_message() {
        let req = ChatRequest {
            model: "m".to_string(),
            max_tokens: 1024,
            system: None,
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text {
                        text: "looking at result:".to_string(),
                        cache_control: None,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "toolu_1".to_string(),
                        content: "127.0.0.1 localhost".to_string(),
                        is_error: false,
                    },
                    ContentBlock::Text {
                        text: "and another:".to_string(),
                        cache_control: None,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "toolu_2".to_string(),
                        content: "ok".to_string(),
                        is_error: false,
                    },
                ]),
            }],
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, None);
        // Expect: [User("looking at result:"), Tool, User("and another:"), Tool]
        assert_eq!(wire.messages.len(), 4);
        assert!(matches!(&wire.messages[0], WireMessage::User { content } if content == "looking at result:"));
        assert!(matches!(&wire.messages[1], WireMessage::Tool { tool_call_id, content }
            if tool_call_id == "toolu_1" && content == "127.0.0.1 localhost"));
        assert!(matches!(&wire.messages[2], WireMessage::User { content } if content == "and another:"));
        assert!(matches!(&wire.messages[3], WireMessage::Tool { tool_call_id, .. }
            if tool_call_id == "toolu_2"));
    }

    #[test]
    fn chat_request_to_wire_thinking_block_splits_reasoning_and_signature() {
        // The Anthropic `thinking` block carries both `thinking` and
        // `signature`; we split them so cross-protocol strip can drop
        // the signature independently of the visible text.
        let req = ChatRequest {
            model: "m".to_string(),
            max_tokens: 1024,
            system: None,
            messages: vec![ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Thinking {
                        thinking: "let me think".to_string(),
                        signature: "sig_abc".to_string(),
                    },
                    ContentBlock::Text {
                        text: "answer".to_string(),
                        cache_control: None,
                    },
                ]),
            }],
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, None);
        assert_eq!(wire.messages.len(), 1);
        let WireMessage::Assistant { blocks } = &wire.messages[0] else {
            panic!("expected Assistant")
        };
        // Thinking → [Reasoning, Signature]; Text → Text. The
        // inverse (`wire_blocks_to_content_blocks`) recombines
        // a consecutive `Reasoning`+`Signature` pair into a
        // single `Thinking { thinking, signature }` block so the
        // Anthropic round-trip is 1:1 with the pre-PR3 shape.
        assert_eq!(blocks.len(), 3);
        assert!(matches!(&blocks[0], WireBlock::Reasoning { text } if text == "let me think"));
        assert!(matches!(&blocks[1], WireBlock::Signature { data } if data == "sig_abc"));
        assert!(matches!(&blocks[2], WireBlock::Text { text, .. } if text == "answer"));
    }

    // ---- strip_unsupported ----

    #[test]
    fn strip_drops_signature_when_target_cant_carry_it() {
        // Anthropic → OpenAI: signature must go.
        let messages = vec![WireMessage::Assistant {
            blocks: vec![
                WireBlock::Reasoning {
                    text: "thought".to_string(),
                },
                WireBlock::Signature {
                    data: "sig_xyz".to_string(),
                },
                WireBlock::Text {
                    text: "answer".to_string(),
                    cache_control: None,
                },
            ],
        }];
        let caps = openai_caps(false, true);
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks } = &stripped[0] else {
            panic!("expected Assistant")
        };
        // Signature dropped, Reasoning kept (reasoning_effort is true),
        // Text kept.
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], WireBlock::Reasoning { text } if text == "thought"));
        assert!(matches!(&blocks[1], WireBlock::Text { text, .. } if text == "answer"));
    }

    #[test]
    fn strip_drops_reasoning_when_target_has_no_thinking_or_reasoning() {
        // OpenAI gpt-4o (no reasoning effort) reading an
        // Anthropic-style thinking block: drop the whole block.
        let messages = vec![WireMessage::Assistant {
            blocks: vec![
                WireBlock::Reasoning {
                    text: "thought".to_string(),
                },
                WireBlock::Text {
                    text: "answer".to_string(),
                    cache_control: None,
                },
            ],
        }];
        let caps = openai_caps(false, false);
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks } = &stripped[0] else {
            panic!("expected Assistant")
        };
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], WireBlock::Text { text, .. } if text == "answer"));
    }

    #[test]
    fn strip_keeps_tool_use_and_text_always() {
        let messages = vec![WireMessage::Assistant {
            blocks: vec![
                WireBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "/etc/hosts"}),
                },
                WireBlock::Text {
                    text: "ok".to_string(),
                    cache_control: None,
                },
            ],
        }];
        // Worst-case caps: nothing supported except text + tool.
        let caps = WireCapabilities {
            supports_thinking: false,
            supports_reasoning_effort: false,
            supports_thinking_signatures: false,
        };
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks } = &stripped[0] else {
            panic!("expected Assistant")
        };
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], WireBlock::ToolUse { .. }));
        assert!(matches!(&blocks[1], WireBlock::Text { .. }));
    }

    #[test]
    fn strip_drops_redacted_thinking_on_cross_protocol() {
        let messages = vec![WireMessage::Assistant {
            blocks: vec![
                WireBlock::RedactedThinking {
                    data: "opaque_blob".to_string(),
                },
                WireBlock::Text {
                    text: "visible".to_string(),
                    cache_control: None,
                },
            ],
        }];
        // OpenAI target: redacted_thinking is opaque to us → drop.
        let caps = openai_caps(true, true);
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks } = &stripped[0] else {
            panic!("expected Assistant")
        };
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], WireBlock::Text { .. }));
    }

    #[test]
    fn strip_preserves_user_and_tool_messages_unchanged() {
        let messages = vec![
            WireMessage::User {
                content: "hi".to_string(),
            },
            WireMessage::Tool {
                tool_call_id: "t1".to_string(),
                content: "result".to_string(),
            },
        ];
        let caps = WireCapabilities {
            supports_thinking: false,
            supports_reasoning_effort: false,
            supports_thinking_signatures: false,
        };
        let stripped = strip_unsupported(messages, &caps);
        assert_eq!(stripped.len(), 2);
        assert!(matches!(&stripped[0], WireMessage::User { content } if content == "hi"));
        assert!(matches!(&stripped[1], WireMessage::Tool { tool_call_id, .. } if tool_call_id == "t1"));
    }

    #[test]
    fn strip_keeps_signature_for_anthropic_target() {
        // Anthropic→Anthropic: signature survives.
        let messages = vec![WireMessage::Assistant {
            blocks: vec![
                WireBlock::Reasoning {
                    text: "thought".to_string(),
                },
                WireBlock::Signature {
                    data: "sig_keep".to_string(),
                },
            ],
        }];
        let caps = anthropic_caps(true);
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks } = &stripped[0] else {
            panic!("expected Assistant")
        };
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], WireBlock::Reasoning { .. }));
        assert!(matches!(&blocks[1], WireBlock::Signature { data } if data == "sig_keep"));
    }

    // ---- wire_block_to_chat_event ----

    #[test]
    fn wire_block_text_to_chat_event_delta() {
        let ev = wire_block_to_chat_event(&WireBlock::Text {
            text: "hi".to_string(),
            cache_control: None,
        })
        .expect("text maps to event");
        assert!(matches!(ev, ChatEvent::Delta { text } if text == "hi"));
    }

    #[test]
    fn wire_block_reasoning_to_chat_event_thinking_delta() {
        let ev = wire_block_to_chat_event(&WireBlock::Reasoning {
            text: "thought".to_string(),
        })
        .expect("reasoning maps to event");
        assert!(matches!(ev, ChatEvent::ThinkingDelta { text } if text == "thought"));
    }

    #[test]
    fn wire_block_tool_use_to_chat_event_tool_call() {
        let ev = wire_block_to_chat_event(&WireBlock::ToolUse {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "/x"}),
        })
        .expect("tool use maps to event");
        match ev {
            ChatEvent::ToolCall { id, name, input } => {
                assert_eq!(id, "t1");
                assert_eq!(name, "read_file");
                assert_eq!(input, serde_json::json!({"path": "/x"}));
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn wire_block_redacted_thinking_to_chat_event_redacted_delta() {
        let ev = wire_block_to_chat_event(&WireBlock::RedactedThinking {
            data: "blob".to_string(),
        })
        .expect("redacted maps to event");
        assert!(matches!(ev, ChatEvent::RedactedThinkingDelta { data } if data == "blob"));
    }

    // ---- round-trip: ChatRequest → Wire → ChatMessage ----
    //
    // These tests lock the PR3 1:1 wire contract for the Anthropic
    // path. Pre-PR3 (PR2), the Anthropic adapter took a `ChatRequest`
    // and posted it verbatim. PR3 routes the request through the
    // wire layer; the inverse (`wire_messages_to_chat_messages`)
    // must reconstruct a `ChatMessage` array that, when
    // re-serialized, is byte-for-byte identical to the pre-PR3
    // request body.

    #[test]
    fn round_trip_preserves_thinking_block_1to1() {
        // A single `Thinking { thinking, signature }` block must
        // round-trip back to a single `Thinking { thinking,
        // signature }` block — NOT two `Thinking` blocks (one
        // with empty signature, which Anthropic would 400 on).
        let original = vec![ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Thinking {
                    thinking: "let me think".to_string(),
                    signature: "sig_abc".to_string(),
                },
                ContentBlock::Text {
                    text: "the answer".to_string(),
                    cache_control: None,
                },
            ]),
        }];
        let req = ChatRequest {
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 16384,
            system: None,
            messages: original.clone(),
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, None);
        let back = wire_messages_to_chat_messages(wire.messages);
        // The 1:1 invariant: the round-tripped assistant message
        // has the same block set as the original.
        assert_eq!(back.len(), 1);
        let ChatMessage { content: MessageContent::Blocks(blocks), .. } = &back[0] else {
            panic!("expected Blocks content");
        };
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            ContentBlock::Thinking { thinking, signature } => {
                assert_eq!(thinking, "let me think");
                assert_eq!(signature, "sig_abc");
            }
            other => panic!("expected Thinking, got {:?}", other),
        }
        assert!(matches!(&blocks[1], ContentBlock::Text { text, .. } if text == "the answer"));
    }

    #[test]
    fn round_trip_preserves_empty_signature_thinking_block() {
        // Defensive: an empty signature stays empty after
        // round-trip (the split helper skips emitting a `Signature`
        // block when the signature is empty, so the inverse just
        // sees a lone `Reasoning`).
        let original = vec![ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::Thinking {
                thinking: "thought".to_string(),
                signature: String::new(),
            }]),
        }];
        let req = ChatRequest {
            model: "m".to_string(),
            max_tokens: 1024,
            system: None,
            messages: original,
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, None);
        let back = wire_messages_to_chat_messages(wire.messages);
        let ChatMessage { content: MessageContent::Blocks(blocks), .. } = &back[0] else {
            panic!("expected Blocks content");
        };
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::Thinking { thinking, signature } => {
                assert_eq!(thinking, "thought");
                assert_eq!(signature, "");
            }
            other => panic!("expected Thinking, got {:?}", other),
        }
    }

    // ---- B5 cache_control preservation ----
    //
    // The synthetic instructions user message carries
    // `cache_control: Some(Ephemeral)` on its first text block so
    // Anthropic can cache the 4 instruction files (CLAUDE.md /
    // AGENTS.md × user / project) on turn 1 and read them from
    // cache on turns 2..MAX_TURNS. These tests lock the wire
    // round-trip preserves the cache marker.

    #[test]
    fn round_trip_preserves_cache_control_on_text_block() {
        // A user message with a cacheable text block + a regular
        // text block: round-trip should preserve cache_control on
        // the first block and produce a `UserBlocks` wire shape
        // (NOT concatenate, which would drop the marker).
        let original = vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "<banner>loaded 4 instructions</banner>".to_string(),
                    cache_control: Some(CacheControl::Ephemeral),
                },
                ContentBlock::Text {
                    text: "<reference>CLAUDE.md body</reference>".to_string(),
                    cache_control: None,
                },
            ]),
        }];
        let req = ChatRequest {
            model: "m".to_string(),
            max_tokens: 1024,
            system: None,
            messages: original,
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, None);
        // Critical: must be UserBlocks (not User { content }),
        // otherwise concatenation drops the cache marker.
        assert_eq!(wire.messages.len(), 1);
        match &wire.messages[0] {
            WireMessage::UserBlocks { blocks } => {
                assert_eq!(blocks.len(), 2);
                match &blocks[0] {
                    WireBlock::Text {
                        text,
                        cache_control,
                    } => {
                        assert_eq!(text, "<banner>loaded 4 instructions</banner>");
                        assert_eq!(*cache_control, Some(CacheControl::Ephemeral));
                    }
                    other => panic!("expected Text, got {:?}", other),
                }
                match &blocks[1] {
                    WireBlock::Text {
                        text,
                        cache_control,
                    } => {
                        assert_eq!(text, "<reference>CLAUDE.md body</reference>");
                        assert_eq!(*cache_control, None);
                    }
                    other => panic!("expected Text, got {:?}", other),
                }
            }
            other => panic!("expected UserBlocks, got {:?}", other),
        }
        // Inverse: round-trip back to ChatMessage, verify
        // cache_control survives the inverse path.
        let back = wire_messages_to_chat_messages(wire.messages);
        assert_eq!(back.len(), 1);
        let ChatMessage { content: MessageContent::Blocks(blocks), .. } = &back[0] else {
            panic!("expected Blocks content");
        };
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            ContentBlock::Text {
                text,
                cache_control,
            } => {
                assert_eq!(text, "<banner>loaded 4 instructions</banner>");
                assert_eq!(*cache_control, Some(CacheControl::Ephemeral));
            }
            other => panic!("expected Text, got {:?}", other),
        }
        match &blocks[1] {
            ContentBlock::Text {
                text,
                cache_control,
            } => {
                assert_eq!(text, "<reference>CLAUDE.md body</reference>");
                assert_eq!(*cache_control, None);
            }
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn user_blocks_with_cache_control_are_not_concatenated() {
        // Two user messages, both with cacheable text blocks —
        // the legacy path would have concatenated them into a
        // single `User { content: String }` (losing both cache
        // markers). With cache_control present, each stays as a
        // separate `UserBlocks` message.
        let req = ChatRequest {
            model: "m".to_string(),
            max_tokens: 1024,
            system: None,
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text {
                        text: "first chunk".to_string(),
                        cache_control: Some(CacheControl::Ephemeral),
                    },
                    ContentBlock::Text {
                        text: "second chunk".to_string(),
                        cache_control: None,
                    },
                ]),
            }],
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, None);
        // 1 UserBlocks message (not 1 User { content } and not 2
        // separate User messages — both blocks belong to the same
        // user message).
        assert_eq!(wire.messages.len(), 1);
        assert!(matches!(&wire.messages[0], WireMessage::UserBlocks { blocks } if blocks.len() == 2));
    }
}
