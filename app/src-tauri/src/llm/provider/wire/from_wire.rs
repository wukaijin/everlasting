//! Wire → chat types conversion (inbound direction).
//!
//! Stream blocks → `ChatEvent`, wire messages → `ChatMessage`.

use super::types::{WireBlock, WireMessage, WireTool};
use crate::llm::types::{ChatEvent, ChatMessage, ContentBlock, MessageContent, Role, ToolDef};

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
        WireBlock::Reasoning { text } => Some(ChatEvent::ThinkingDelta { text: text.clone() }),
        // The `Signature` blob is consumed at the agent-loop
        // boundary, not in the streaming ChatEvent — but for
        // cross-protocol symmetry we expose it as a
        // `SignatureDelta` when the parser hands us one. The
        // OpenAI parser never produces a `Signature`, so this
        // branch is Anthropic-only in practice.
        WireBlock::Signature { data } => Some(ChatEvent::SignatureDelta {
            signature: data.clone(),
        }),
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
        // B1: images are request-side only — a provider stream never
        // emits one, so there is no ChatEvent to map to.
        WireBlock::Image { .. } => None,
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
    fuse_adjacent_tool_results(
        messages
            .into_iter()
            .flat_map(wire_message_to_chat_messages)
            .collect(),
    )
}

/// Fuse runs of consecutive user messages whose content is
/// exclusively `tool_result` blocks back into ONE user message
/// carrying all the blocks (09-11-deepseek-tool-result-split-400).
///
/// Why this exists: the forward pass (`chat_message_to_wire_messages`)
/// lifts each `ToolResult` block of a persisted user row into its own
/// `WireMessage::Tool`, and this file's inverse maps each back to a
/// separate `role: "user"` message — so a row `[tool_result × N]`
/// round-trips into N consecutive single-result user messages. Native
/// Anthropic merges consecutive user messages before validating, but
/// strict Anthropic-schema relays validate per-message: the
/// wukaijin.com deepseek channel rejects the split shape with
/// `400 messages.N: tool_use ids were found without tool_result
/// blocks immediately after` when one assistant message carried ≥2
/// tool_use blocks (incident 2026-09-10 18:37 UTC, group-chat session
/// caa5020a: deepseek-flash three-struck while glm channels on the
/// same relay accepted the identical shape). Fusing restores the
/// pre-PR2 wire shape the provider-trait spec froze as 1:1.
///
/// Scope rules (deliberate — see task design.md):
/// - Only runs of ≥2 fuse; a lone tool-result message is untouched.
/// - A following plain-text user message (e.g. the loop-detection
///   hint that tools.rs appends as a trailing Text block) is NOT
///   folded into the fused result message: the wire layer has lost
///   the original row boundary, and in group chat the next user row
///   may be another speaker's rewritten text — gluing it onto the
///   results would corrupt speaker attribution. Consecutive user
///   messages are wire-legal for both protocols; the pairing check
///   only requires the results to sit in the message immediately
///   after the assistant's tool_use.
pub(crate) fn fuse_adjacent_tool_results(msgs: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::with_capacity(msgs.len());
    let mut run: Vec<ChatMessage> = Vec::new();
    for m in msgs {
        if is_pure_tool_result_user(&m) {
            run.push(m);
        } else {
            flush_tool_result_run(&mut out, &mut run);
            out.push(m);
        }
    }
    flush_tool_result_run(&mut out, &mut run);
    out
}

/// Whether the message is a `role: user` row whose content blocks
/// are all `tool_result` (the shape the `WireMessage::Tool` arm
/// produces, plus any hypothetical multi-block pure-result row).
fn is_pure_tool_result_user(m: &ChatMessage) -> bool {
    m.role == Role::User
        && match &m.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .all(|b| matches!(b, ContentBlock::ToolResult { .. })),
            MessageContent::Text(_) => false,
        }
}

/// Merge a buffered run of pure tool-result user messages into `out`.
/// Runs of length ≤1 push through untouched (original message
/// identity preserved); longer runs collapse into one user message
/// with the blocks concatenated in arrival order. The fused message
/// takes the first message's `speaker`/`attachments` (both `None`
/// for every producer of this shape today).
fn flush_tool_result_run(out: &mut Vec<ChatMessage>, run: &mut Vec<ChatMessage>) {
    if run.len() <= 1 {
        out.append(run);
        return;
    }
    let mut iter = run.drain(..);
    let first = iter
        .next()
        .expect("flush_tool_result_run: run.len() >= 2 guarantees a first element");
    let mut blocks: Vec<ContentBlock> = Vec::new();
    if let MessageContent::Blocks(bs) = &first.content {
        blocks.extend(bs.iter().cloned());
    }
    for m in iter {
        if let MessageContent::Blocks(bs) = &m.content {
            blocks.extend(bs.iter().cloned());
        }
    }
    out.push(ChatMessage {
        role: Role::User,
        content: MessageContent::Blocks(blocks),
        speaker: first.speaker,
        attachments: first.attachments,
    });
}

fn wire_message_to_chat_messages(msg: WireMessage) -> Vec<ChatMessage> {
    match msg {
        WireMessage::User { content, speaker } => vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text(content),
            speaker,
            attachments: None,
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
                speaker: None,
                attachments: None,
            }]
        }
        WireMessage::Tool {
            tool_call_id,
            content,
            images,
        } => {
            // Anthropic's `tool_result` lives inside a
            // `role: "user"` message with content blocks.
            // R4 (08-21-b1-image-followups): tool images ride
            // `resolved` — the manual `ContentBlock` Serialize then
            // emits the Anthropic-documented tool_result content
            // **block array** (image blocks + text block) instead of
            // a plain string. Strip pass guarantees `images` is empty
            // on no-vision targets, so this arm only fires for
            // vision-capable models.
            vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: tool_call_id,
                    content,
                    is_error: false,
                    images: None,
                    resolved: if images.is_empty() {
                        None
                    } else {
                        Some(
                            images
                                .into_iter()
                                .map(|img| crate::llm::types::ImageSource {
                                    source_type: "base64".to_string(),
                                    media_type: img.media_type,
                                    data: img.data,
                                })
                                .collect(),
                        )
                    },
                }]),
                speaker: None,
                attachments: None,
            }]
        }
        WireMessage::Assistant { blocks, speaker } => {
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
                speaker,
                attachments: None,
            }]
        }
    }
}

/// Convert a `Vec<WireBlock>` into a `Vec<ContentBlock>`, fusing
/// a consecutive `Reasoning`+`Signature` pair into a single
/// `ContentBlock::Thinking { thinking, signature }` so the
/// Anthropic round-trip is byte-for-byte identical to the
/// pre-PR3 wire shape.
pub(crate) fn wire_blocks_to_content_blocks(blocks: Vec<WireBlock>) -> Vec<ContentBlock> {
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
        WireBlock::ToolUse { id, name, input } => ContentBlock::ToolUse { id, name, input },
        // B1 (2026-08-16): a user-message image survives strip (or was
        // already placeholder-replaced); map back to the resolved
        // `ContentBlock::Image` so the Anthropic adapter's serde
        // serialization emits the native image block verbatim.
        WireBlock::Image { media_type, data } => ContentBlock::Image {
            source: crate::llm::types::ImageSource {
                source_type: "base64".to_string(),
                media_type,
                data,
            },
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
