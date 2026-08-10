//! Chat types → wire conversion (outbound direction).
//!
//! Request building + orphan guard + `strip_unsupported` silent degradation pass.

use super::types::{WireBlock, WireCapabilities, WireMessage, WireRequest, WireTool};
use crate::llm::types::{ChatMessage, ChatRequest, ContentBlock, MessageContent, Role};
use std::collections::HashSet;

pub fn chat_request_to_wire(req: ChatRequest, system: Option<String>) -> WireRequest {
    // Orphan guard (llm-contract.md §Pair Atomicity tool_use↔tool_result Pair
    // Atomicity): scan the Anthropic-shaped messages BEFORE fan-out
    // for any assistant `tool_use` whose `tool_use_id` has no matching
    // `tool_result` anywhere in history. Such an orphan makes the very
    // request we're about to build fail upstream — OpenAI 400
    // "insufficient tool messages following tool_calls" / Anthropic
    // 2013.
    //
    // 08-06 fix (group-chat speaker-desync): previously this was pure
    // diagnostics — it logged the orphan but still sent the broken
    // request, which 400'd every subsequent turn in a group chat once a
    // single orphan landed in the DB (root cause: an intercepted
    // arbitration tool whose error tool_result wasn't persisted on a
    // max_turns exit; see session 4a9d3566 seq 29). Now we SELF-HEAL:
    // append a synthetic `tool_result` (role: user) for each orphan id
    // so the request satisfies Pair Atomicity and the conversation can
    // continue. The synthetic result marks itself as an error so the
    // model knows the tool call didn't complete normally.
    let mut messages = req.messages;
    let orphans = orphan_tool_use_ids(&messages);
    if !orphans.is_empty() {
        tracing::warn!(
            orphan_count = orphans.len(),
            orphan_tool_use_ids = ?orphans,
            "wire: orphan tool_use detected — injecting synthetic tool_result(s) \
             to satisfy Pair Atomicity (llm-contract.md §Pair Atomicity). Root cause is \
             upstream (a tool_result that should have been persisted); this is \
             a defensive heal so the request doesn't 400."
        );
        let synthetic_results: Vec<ContentBlock> = orphans
            .iter()
            .map(|id| ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: "[tool result missing from history — synthesized by \
                          wire layer to keep the conversation going]"
                    .to_string(),
                is_error: true,
            })
            .collect();
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(synthetic_results),
            speaker: None,
        });
    }
    let messages = messages
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
    }
}

/// Collect every `assistant(tool_use)` id in `messages` that has no
/// matching `user(tool_result)` id anywhere in the same history.
///
/// Non-empty return = the request is about to violate the
/// tool_use↔tool_result Pair Atomicity invariant (llm-contract.md §Pair Atomicity)
/// and the upstream provider will reject it. Pure read; no mutation.
/// Used by [`chat_request_to_wire`] as a defensive diagnostic so the
/// next orphan failure is grep-able instead of requiring fresh RCA.
pub(crate) fn orphan_tool_use_ids(messages: &[ChatMessage]) -> Vec<String> {
    let mut uses: Vec<String> = Vec::new();
    let mut results: HashSet<String> = HashSet::new();
    for m in messages {
        let MessageContent::Blocks(blocks) = &m.content else {
            continue;
        };
        match m.role {
            Role::Assistant => {
                for b in blocks {
                    if let ContentBlock::ToolUse { id, .. } = b {
                        uses.push(id.clone());
                    }
                }
            }
            Role::User => {
                for b in blocks {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                        results.insert(tool_use_id.clone());
                    }
                }
            }
        }
    }
    uses.into_iter()
        .filter(|id| !results.contains(id))
        .collect()
}

/// Wire-layer **order** guard for the OpenAI "tool_calls must be
/// followed by tool messages" hard constraint.
///
/// OpenAI Chat Completions enforces a stricter contract than Anthropic:
/// an assistant message carrying `tool_calls[]` MUST be immediately
/// followed by `role: "tool"` messages — one per `tool_call_id`, with
/// no `role: "user"` / `role: "assistant"` message interleaved between
/// the assistant(tool_calls) and its tool messages. Anthropic's same
/// Pair Atomicity rule (llm-contract.md §Pair Atomicity) tolerates the
/// `tool_result` blocks living inside a single `role: "user"` message
/// regardless of any interleaved text block; OpenAI does NOT, and
/// rejects with HTTP 400 "An assistant message with 'tool_calls' must
/// be followed by tool messages responding to each 'tool_call_id'".
///
/// This function walks the wire messages and, for every
/// `WireMessage::Assistant { blocks }` that emits ≥1 `ToolUse`,
/// verifies that the **immediately following** wire messages cover
/// every `ToolUse` id with a `WireMessage::Tool` — and that the first
/// such following message is itself a `Tool` (not a `User` /
/// `UserBlocks` / another `Assistant`). Any interleaving non-Tool
/// message before all the assistant's tool_use ids are satisfied is a
/// violation.
///
/// Returns one human-readable description string per violation (so the
/// caller can `tracing::error!` a single line per problem). Empty Vec
/// = the order is valid. Pure read; no mutation. Complements the
/// count-based [`orphan_tool_use_ids`] check (which catches missing
/// tool_results entirely; this one catches the order being wrong even
/// when every id has a result somewhere).
pub(crate) fn orphan_tool_call_order(messages: &[WireMessage]) -> Vec<String> {
    let mut violations: Vec<String> = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        // Collect tool_use ids from this assistant message (in order).
        if let WireMessage::Assistant { blocks, .. } = &messages[i] {
            let tool_uses: Vec<String> = blocks
                .iter()
                .filter_map(|b| {
                    if let WireBlock::ToolUse { id, .. } = b {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect();
            if !tool_uses.is_empty() {
                let mut remaining: HashSet<String> = tool_uses.iter().cloned().collect();
                let mut j = i + 1;
                let mut first = true;
                while j < messages.len() && !remaining.is_empty() {
                    match &messages[j] {
                        WireMessage::Tool { tool_call_id, .. } => {
                            remaining.remove(tool_call_id);
                        }
                        WireMessage::User { content, .. } => {
                            let kind = format!("User({:?})", truncate(content, 40));
                            let missing: Vec<String> = remaining.iter().cloned().collect();
                            let which = if first {
                                "immediately"
                            } else {
                                "before all tool_call_ids were satisfied"
                            };
                            violations.push(format!(
                                "assistant at index {} emitted tool_use_ids {:?}; a non-Tool wire message ({}) appears at index {} {} — OpenAI requires the assistant(tool_calls) be followed by role:tool messages with no interleaving (missing: {:?})",
                                i, tool_uses, kind, j, which, missing
                            ));
                            break;
                        }
                        WireMessage::UserBlocks { .. } => {
                            let kind = "UserBlocks(...)".to_string();
                            let missing: Vec<String> = remaining.iter().cloned().collect();
                            let which = if first {
                                "immediately"
                            } else {
                                "before all tool_call_ids were satisfied"
                            };
                            violations.push(format!(
                                "assistant at index {} emitted tool_use_ids {:?}; a non-Tool wire message ({}) appears at index {} {} — OpenAI requires the assistant(tool_calls) be followed by role:tool messages with no interleaving (missing: {:?})",
                                i, tool_uses, kind, j, which, missing
                            ));
                            break;
                        }
                        WireMessage::Assistant { .. } => {
                            let kind = "Assistant(...)".to_string();
                            let missing: Vec<String> = remaining.iter().cloned().collect();
                            let which = if first {
                                "immediately"
                            } else {
                                "before all tool_call_ids were satisfied"
                            };
                            violations.push(format!(
                                "assistant at index {} emitted tool_use_ids {:?}; a non-Tool wire message ({}) appears at index {} {} — OpenAI requires the assistant(tool_calls) be followed by role:tool messages with no interleaving (missing: {:?})",
                                i, tool_uses, kind, j, which, missing
                            ));
                            break;
                        }
                    }
                    first = false;
                    j += 1;
                }
            }
        }
        i += 1;
    }
    violations
}

/// Truncate a string for diagnostic display. Returns the original if
/// `max` ≥ length, otherwise the first `max` chars + `"…"`.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{}…", head)
    }
}

/// Convert one [`ChatMessage`] into zero, one, or more
/// [`WireMessage`]s. A `role: "user"` message containing a
/// `tool_result` block fans out into separate `Tool` messages
/// (one per block) so the OpenAI side can emit one `role: "tool"`
/// message per `tool_call_id`. A `role: "user"` message containing
/// only text stays a single `User` message.
fn chat_message_to_wire_messages(msg: ChatMessage) -> Vec<WireMessage> {
    // Group chat (07-29-group-chat): hoist speaker out before the
    // move-heavy match below so every User/Assistant variant can
    // thread it. Classic-chat messages have `speaker == None`, so
    // this is a cheap clone of None for the common path.
    let speaker = msg.speaker.clone();
    match msg.role {
        Role::User => match msg.content {
            MessageContent::Text(s) => vec![WireMessage::User {
                content: s,
                speaker,
            }],
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
                                        speaker: speaker.clone(),
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
                            speaker,
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
            vec![WireMessage::Assistant { blocks, speaker }]
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
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
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
        .map(|m| match m {
            WireMessage::User { content, speaker } => WireMessage::User { content, speaker },
            WireMessage::UserBlocks { blocks } => {
                // B5 refactor (2026-06-11): `UserBlocks` carries
                // block-level cache_control on text blocks. We
                // keep it intact here — `block_supported` is a
                // no-op for `Text` (see the decision matrix), so
                // the cache marker survives `strip_unsupported`.
                // Anthropic → OpenAI path: the OpenAI adapter
                // drops cache_control at serialization time, so
                // no special handling is needed here.
                WireMessage::UserBlocks { blocks }
            }
            WireMessage::Tool {
                tool_call_id,
                content,
            } => WireMessage::Tool {
                tool_call_id,
                content,
            },
            WireMessage::Assistant { blocks, speaker } => {
                let filtered: Vec<WireBlock> = blocks
                    .into_iter()
                    .filter(|b| block_supported(b, target_caps))
                    .collect();
                // An assistant message that becomes empty after
                // strip is still meaningful: the model saw a
                // pure-reasoning turn. Keep it (with empty
                // blocks); the provider-wire converter will
                // decide whether to send it.
                WireMessage::Assistant {
                    blocks: filtered,
                    speaker,
                }
            }
        })
        .collect()
}

fn block_supported(block: &WireBlock, caps: &WireCapabilities) -> bool {
    match block {
        WireBlock::Text { .. } | WireBlock::ToolUse { .. } => true,
        WireBlock::Reasoning { .. } => caps.supports_thinking || caps.supports_reasoning_effort,
        WireBlock::Signature { .. } | WireBlock::RedactedThinking { .. } => {
            caps.supports_thinking_signatures
        }
    }
}

// ---------------------------------------------------------------------------
