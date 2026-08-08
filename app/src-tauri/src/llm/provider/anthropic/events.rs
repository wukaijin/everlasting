//! Anthropic SSE 事件状态机(拆分自 anthropic.rs,08-08-a-class-anthropic-split)。
//!
//! `BlockState` + 5 个事件 handler:无 yield 纯函数,由 `chat_stream_with_tools`
//! 宏体事件分发调用,返回待 yield 的 `Option<ChatEvent>`(或 `()`)。

#![allow(unused_imports)]
use crate::llm::types::{ChatEvent, TokenUsage};

use super::parse_anthropic_usage;

/// State machine for the current content block being received from the SSE
/// stream. Used to know how to interpret `content_block_delta` events and
/// to assemble the right payload on `content_block_stop`.
#[derive(Debug)]
pub(crate) enum BlockState {
    /// Not inside any content block.
    Idle,
    /// Inside a text block.
    Text,
    /// Inside a tool_use block — accumulate JSON fragments.
    ToolUse {
        id: String,
        name: String,
        json_buf: String,
    },
    /// Inside a thinking block — accumulate thinking text and the opaque
    /// signature blob (delivered via `signature_delta` just before stop).
    Thinking {
        thinking_buf: String,
        signature_buf: String,
    },
    /// Inside a redacted_thinking block. The block carries only an opaque
    /// `data` payload (no streaming deltas); we treat the buffer as the
    /// fully-assembled payload once `content_block_stop` fires.
    RedactedThinking { data_buf: String },
}

/// 处理 `content_block_start` SSE 事件:按块类型转换 `BlockState`
/// (tool_use / thinking / redacted_thinking / 默认 Text)。
/// 无即时事件(不 yield);JSON 解析失败时静默跳过(原代码行为,无 else 分支)。
/// 提取自 chat_stream_with_tools 事件分发,08-08-a-class-anthropic-split。
pub(crate) fn handle_content_block_start(data: &str, block_state: &mut BlockState) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
        if let Some(cb) = v.get("content_block") {
            match cb.get("type").and_then(|t| t.as_str()) {
                Some("tool_use") => {
                    let id = cb
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let name = cb
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    tracing::debug!(id = %id, name = %name, "▶ tool_use block start");
                    *block_state = BlockState::ToolUse {
                        id,
                        name,
                        json_buf: String::new(),
                    };
                }
                Some("thinking") => {
                    // The initial signature is usually
                    // an empty string in the start
                    // event; it gets filled in by the
                    // `signature_delta` event just
                    // before stop. We don't need to
                    // seed the buf from `content_block.signature`
                    // — Anthropic guarantees the
                    // signature is fully delivered via
                    // the delta. (Defensive seed
                    // preserved in case the schema
                    // ever ships the whole thing up
                    // front.)
                    let initial_sig = cb
                        .get("signature")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    let initial_thinking = cb
                        .get("thinking")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    tracing::debug!("▶ thinking block start");
                    *block_state = BlockState::Thinking {
                        thinking_buf: initial_thinking,
                        signature_buf: initial_sig,
                    };
                }
                Some("redacted_thinking") => {
                    // The `data` field is the full
                    // opaque payload (no streaming
                    // deltas for this block type).
                    let data = cb
                        .get("data")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    tracing::debug!("▶ redacted_thinking block start");
                    *block_state = BlockState::RedactedThinking { data_buf: data };
                }
                _ => {
                    *block_state = BlockState::Text;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 处理 `content_block_delta` SSE 事件:按 delta 类型累积状态并返回待 yield 的
/// 事件(text_delta → `Delta`;thinking_delta → `ThinkingDelta`;input_json /
/// signature_delta 仅累积,返回 `None`)。无 yield 的纯函数(提取自
/// chat_stream_with_tools 事件分发,08-08-a-class-anthropic-split)。
pub(crate) fn handle_content_block_delta(
    data: &str,
    block_state: &mut BlockState,
) -> Option<ChatEvent> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
        if let Some(delta) = v.get("delta") {
            match delta.get("type").and_then(|t| t.as_str()) {
                Some("text_delta") => {
                    if let Some(s) = delta.get("text").and_then(|t| t.as_str()) {
                        return Some(ChatEvent::Delta {
                            text: s.to_string(),
                        });
                    }
                }
                Some("input_json_delta") => {
                    if let Some(partial) = delta.get("partial_json").and_then(|p| p.as_str()) {
                        if let BlockState::ToolUse { json_buf, .. } = block_state {
                            json_buf.push_str(partial);
                        }
                    }
                }
                Some("thinking_delta") => {
                    if let Some(s) = delta.get("thinking").and_then(|t| t.as_str()) {
                        if let BlockState::Thinking { thinking_buf, .. } = block_state {
                            thinking_buf.push_str(s);
                        }
                        return Some(ChatEvent::ThinkingDelta {
                            text: s.to_string(),
                        });
                    }
                }
                Some("signature_delta") => {
                    // Buffer only — emit the
                    // assembled `SignatureDelta` once
                    // on `content_block_stop`. This
                    // protects the frontend's
                    // `currentThinkingBlock` invariant
                    // ("one signature per block")
                    // even if the server ever splits
                    // the signature across multiple
                    // events. (Today Anthropic sends
                    // exactly one `signature_delta`
                    // per thinking block — see
                    // research/anthropic-thinking-api.md
                    // §6 — but we don't want to depend
                    // on that.)
                    if let Some(s) = delta.get("signature").and_then(|t| t.as_str()) {
                        if let BlockState::Thinking { signature_buf, .. } = block_state {
                            signature_buf.push_str(s);
                        }
                    }
                }
                other => {
                    tracing::debug!("▶ content_block_delta with unknown delta type: {:?}", other);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 处理 `content_block_stop` SSE 事件:`mem::replace` 终结当前块并返回待 yield 的
/// 事件(ToolUse → `ToolCall`(JSON 解析容错,空/坏 buf 默认 `{}`);Thinking →
/// 签名非空 `SignatureDelta`;RedactedThinking → 数据非空 `RedactedThinkingDelta`;
/// Text/Idle → `None`)。无 yield 的纯函数(提取自 chat_stream_with_tools
/// 事件分发,08-08-a-class-anthropic-split)。
pub(crate) fn handle_content_block_stop(block_state: &mut BlockState) -> Option<ChatEvent> {
    match std::mem::replace(block_state, BlockState::Idle) {
        BlockState::ToolUse { id, name, json_buf } => {
            // Parse accumulated JSON; default to {} if empty or broken.
            let input: serde_json::Value = if json_buf.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&json_buf).unwrap_or_else(|e| {
                    tracing::warn!(
                        json_buf = %json_buf,
                        error = %e,
                        "failed to parse tool_use input JSON, using empty object"
                    );
                    serde_json::json!({})
                })
            };
            tracing::debug!(id = %id, name = %name, "▶ tool_use block complete");
            Some(ChatEvent::ToolCall { id, name, input })
        }
        BlockState::Thinking { signature_buf, .. } => {
            // Emit the fully-assembled signature as a
            // single `SignatureDelta` event — the
            // frontend's `currentThinkingBlock` and
            // the agent loop's `pending_thinking`
            // both rely on the invariant that there's
            // at most one `SignatureDelta` per
            // thinking block, otherwise the frontend
            // would open a fresh (corrupted) block on
            // each subsequent chunk and the agent
            // loop's `pending_thinking` would never
            // see the full signature in one event.
            //
            // `thinking_delta` events were already
            // streamed as they arrived; the frontend
            // appends them to the in-flight thinking
            // block's `text` directly.
            tracing::debug!(
                signature_len = signature_buf.len(),
                "▶ thinking block complete"
            );
            if !signature_buf.is_empty() {
                Some(ChatEvent::SignatureDelta {
                    signature: signature_buf,
                })
            } else {
                None
            }
        }
        BlockState::RedactedThinking { data_buf } => {
            // Emit the full opaque payload as a single
            // event so the frontend (and persistence)
            // can record it. The data is not
            // displayable; the agent loop stores it
            // verbatim for round-trip back to the
            // LLM.
            tracing::debug!(
                data_len = data_buf.len(),
                "▶ redacted_thinking block complete"
            );
            if !data_buf.is_empty() {
                Some(ChatEvent::RedactedThinkingDelta { data: data_buf })
            } else {
                None
            }
        }
        BlockState::Text | BlockState::Idle => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 处理 `message_start` SSE 事件:写入 usage 基线(仅 `usage.is_none()` 时;
/// 后续 `message_delta.usage` 为权威累计值并覆盖)。无即时事件(不 yield)。
/// 提取自 chat_stream_with_tools 事件分发,08-08-a-class-anthropic-split。
pub(crate) fn handle_message_start(data: &str, usage: &mut Option<TokenUsage>) {
    // Some proxies (and the Anthropic SDK's
    // pre-stream `message_start`) attach
    // `usage: { ... }` at the top level of
    // `message_start`. We treat it as the
    // initial baseline; the subsequent
    // `message_delta.usage` (above) is the
    // authoritative cumulative payload and
    // overwrites this. Without this, a
    // connection that errored out before the
    // first `message_delta` would never get a
    // `usage` report.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
        if let Some(usage_value) = v.get("message").and_then(|m| m.get("usage")) {
            if let Some(u) = parse_anthropic_usage(usage_value) {
                if usage.is_none() {
                    *usage = Some(u);
                }
            }
        } else if let Some(usage_value) = v.get("usage") {
            if let Some(u) = parse_anthropic_usage(usage_value) {
                if usage.is_none() {
                    *usage = Some(u);
                }
            }
        }
    }
    // We already emitted Start; log for debugging.
    tracing::debug!("▶ message_start");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 处理 `message_delta` SSE 事件:提取 stop_reason + 累积 usage。
/// `parse_anthropic_usage` 结果若 `Some(u)` 则覆盖 `usage`(None 时保留原值)。
/// 无即时事件(不 yield)。提取自 chat_stream_with_tools 事件分发,
/// 08-08-a-class-anthropic-split。
pub(crate) fn handle_message_delta(
    data: &str,
    stop_reason: &mut Option<String>,
    usage: &mut Option<TokenUsage>,
) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
        if let Some(delta) = v.get("delta") {
            if let Some(sr) = delta.get("stop_reason").and_then(|r| r.as_str()) {
                tracing::debug!(stop_reason = %sr, "▶ message_delta");
                *stop_reason = Some(sr.to_string());
            }
        }
        // A4: usage is at the top level of
        // `message_delta`, not under `delta`.
        // Anthropic schema (cumulative per-turn):
        //   { "type": "message_delta",
        //     "delta": { "stop_reason": "..." },
        //     "usage": { "input_tokens": N,
        //                "output_tokens": N,
        //                "cache_creation_input_tokens": N,
        //                "cache_read_input_tokens": N } }
        // The first `message_delta` event for a
        // turn typically reports
        // `output_tokens: 1`; later ones carry
        // the cumulative value. We keep the
        // last seen non-null payload (defensive
        // — a per-event accumulator would
        // also work, but the chat command
        // only writes on `Done` anyway).
        if let Some(usage_value) = v.get("usage") {
            if let Some(u) = parse_anthropic_usage(usage_value) {
                *usage = Some(u);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
