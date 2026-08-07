//! OpenAI 流式 tool-call 装配辅助(拆分自 openai.rs, 08-07-large-file-splitting)。
//!
//! `ToolCallBuf` 按 delta.index 累积分片,`build_tool_call_event` 在
//! 参数完整时产出 `ChatEvent`;`parse_openai_usage` 解析 usage 块。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::llm::types::{ChatEvent, TokenUsage};

// ---------------------------------------------------------------------------
// Tool call accumulation helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub(crate) struct ToolCallBuf {
    id: String,
    name: String,
    args_buf: String,
}

/// Accumulate one OpenAI `tool_calls` delta (`tc`) into the
/// per-index assembly map. OpenAI streams a tool call as a
/// sequence of deltas all keyed by the same `index`; we merge the
/// `id` / `function.name` / `function.arguments` fragments into a
/// single [`ToolCallBuf`] per index.
///
/// RULE-D-007 (2026-06-25): the official OpenAI API always emits
/// `index` on every tool_call delta. Some third-party
/// OpenAI-compatible proxies omit it — previously we fell back to
/// `0`, which made two index-less tool calls collide on key `0`
/// (the second overwrote the first's id/name and concatenated
/// arguments onto its `args_buf`). Now an index-less delta is
/// warned + skipped: the official API is unaffected, and a
/// misbehaving proxy drops the call rather than corrupting another.
pub(crate) fn accumulate_tool_call_delta(state: &mut HashMap<u32, ToolCallBuf>, tc: &Value) {
    let Some(idx) = tc.get("index").and_then(|i| i.as_u64()) else {
        tracing::warn!(
            tc = %serde_json::to_string(tc).unwrap_or_default(),
            "openai: tool_call delta missing `index`, skipping (third-party proxy?)"
        );
        return;
    };
    let idx = idx as u32;
    let entry = state.entry(idx).or_default();
    if let Some(id) = tc.get("id").and_then(|s| s.as_str()) {
        if !id.is_empty() {
            entry.id = id.to_string();
        }
    }
    if let Some(name) = tc
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(|s| s.as_str())
        .or_else(|| tc.get("name").and_then(|s| s.as_str()))
    {
        if !name.is_empty() {
            entry.name = name.to_string();
        }
    }
    if let Some(args) = tc
        .get("function")
        .and_then(|f| f.get("arguments"))
        .and_then(|s| s.as_str())
    {
        entry.args_buf.push_str(args);
    }
}

/// Build the `ChatEvent::ToolCall` for one fully-assembled tool
/// call buffer. Returns `None` if the buffer has no name (the
/// stream never delivered the `function.name` field — defensive).
pub(crate) fn build_tool_call_event(buf: &ToolCallBuf, _idx: u32) -> Option<ChatEvent> {
    if buf.name.is_empty() {
        tracing::warn!(
            args_buf = %buf.args_buf,
            "openai: tool_call buffer has no name; skipping emit"
        );
        return None;
    }
    let input: Value = if buf.args_buf.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&buf.args_buf).unwrap_or_else(|e| {
            tracing::warn!(
                args_buf = %buf.args_buf,
                error = %e,
                "openai: failed to parse tool_call arguments JSON, using empty object"
            );
            json!({})
        })
    };
    Some(ChatEvent::ToolCall {
        id: buf.id.clone(),
        name: buf.name.clone(),
        input,
    })
}

// ---------------------------------------------------------------------------
// A4: parse_openai_usage — normalize OpenAI's `usage` chunk into
// the protocol-agnostic `TokenUsage` schema.
// ---------------------------------------------------------------------------

/// Parse OpenAI's `usage` payload into a protocol-agnostic
/// [`TokenUsage`]. Schema mapping (per
/// `backend/llm-contract.md` "Scenario: Token Usage Tracking"
/// §3 "OpenAI normalization"):
///
/// - `prompt_tokens` → `input_tokens`
/// - `completion_tokens` → `output_tokens`
/// - `prompt_tokens_details.cached_tokens` → `cache_read_input_tokens`
/// - `cache_creation_input_tokens` → 0 (no OpenAI equivalent
///   today; the field is documented but rarely populated)
///
/// Defensive: any field may be missing (older API versions /
/// proxies omit the cached_tokens sub-object). Missing fields
/// default to 0. Returns `None` if no recognizable integer fields
/// were present (e.g. a chunk with no `usage` key, which is the
/// common case on every non-final chunk).
pub(crate) fn parse_openai_usage(v: &Value) -> Option<TokenUsage> {
    let usage = v.get("usage")?;
    let input = usage
        .get("prompt_tokens")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let output = usage
        .get("completion_tokens")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let cache_read = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    if input == 0 && output == 0 && cache_read == 0 {
        // Same all-zero contract as Anthropic: a real
        // OpenAI turn with 0 prompt + 0 completion is not
        // realistic, so an all-zero payload is treated as
        // "no usage" and the agent loop's SQL write is
        // skipped.
        return None;
    }
    Some(TokenUsage {
        input_tokens: input.min(u32::MAX as u64) as u32,
        output_tokens: output.min(u32::MAX as u64) as u32,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read.min(u32::MAX as u64) as u32,
        // 2026-06-26 snapshot fix: cross-provider normalized
        // "total input for this request". OpenAI's
        // `prompt_tokens` is ALREADY inclusive of
        // `cached_tokens` (it's the full prompt length), so the
        // context footprint is just `input`. Do NOT add
        // `cache_read` here — that would double-count.
        context_input_tokens: input.min(u32::MAX as u64) as u32,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- A4: parse_openai_usage ----

    #[test]
    fn parse_openai_usage_full_payload() {
        // Standard OpenAI cumulative usage chunk.
        let v = serde_json::json!({
            "usage": {
                "prompt_tokens": 200,
                "completion_tokens": 30,
                "total_tokens": 230,
                "prompt_tokens_details": { "cached_tokens": 50 }
            }
        });
        let u = parse_openai_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 200);
        assert_eq!(u.output_tokens, 30);
        // OpenAI has no cache_creation field today; the
        // normalized schema still requires a value (0).
        assert_eq!(u.cache_creation_input_tokens, 0);
        assert_eq!(u.cache_read_input_tokens, 50);
        // 2026-06-26 snapshot fix: context_input_tokens = prompt_tokens
        // (= input). OpenAI's prompt_tokens is ALREADY inclusive of
        // cached_tokens, so adding cache_read here would double-count.
        // Verified: 200 (NOT 200 + 50 = 250).
        assert_eq!(u.context_input_tokens, 200);
    }

    #[test]
    fn parse_openai_usage_minimal_payload() {
        // Older API version / non-caching model: no
        // `prompt_tokens_details` field.
        let v = serde_json::json!({
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 10,
                "total_tokens": 60
            }
        });
        let u = parse_openai_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 50);
        assert_eq!(u.output_tokens, 10);
        assert_eq!(u.cache_read_input_tokens, 0);
        assert_eq!(u.context_input_tokens, 50);
    }

    #[test]
    fn parse_openai_usage_no_usage_key_returns_none() {
        // The common case on every non-final chunk: no
        // `usage` field at all. The agent loop's per-turn
        // accumulation must NOT fire on these chunks.
        let v = serde_json::json!({
            "choices": [{
                "delta": { "content": "hello" }
            }]
        });
        assert!(parse_openai_usage(&v).is_none());
    }

    #[test]
    fn parse_openai_usage_zero_returns_none() {
        // All-zero usage → "no usage", same contract as
        // Anthropic. (See parse_anthropic_usage's docstring
        // for the rationale.)
        let v = serde_json::json!({
            "usage": {
                "prompt_tokens": 0,
                "completion_tokens": 0,
                "total_tokens": 0
            }
        });
        assert!(parse_openai_usage(&v).is_none());
    }

    #[test]
    fn parse_openai_usage_empty_prompt_tokens_details() {
        // Defensive: `prompt_tokens_details: {}` is valid
        // OpenAI; we must not crash on it.
        let v = serde_json::json!({
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "total_tokens": 120,
                "prompt_tokens_details": {}
            }
        });
        let u = parse_openai_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 20);
        assert_eq!(u.cache_read_input_tokens, 0);
    }

    // ---- cross-protocol strip behavior (integration with wire) ----

    #[test]
    fn build_tool_call_event_parses_accumulated_arguments_json() {
        let buf = ToolCallBuf {
            id: "call_42".to_string(),
            name: "read_file".to_string(),
            args_buf: r#"{"path":"/etc/hosts"}"#.to_string(),
        };
        let ev = build_tool_call_event(&buf, 0).expect("name is set");
        match ev {
            ChatEvent::ToolCall { id, name, input } => {
                assert_eq!(id, "call_42");
                assert_eq!(name, "read_file");
                assert_eq!(input, serde_json::json!({"path":"/etc/hosts"}));
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn build_tool_call_event_handles_partial_arguments() {
        // OpenAI streams `function.arguments` as fragments
        // that may not be valid JSON until the final chunk.
        // We tolerate partial JSON by buffering and parsing
        // at emit time. Here we verify the buffer path with
        // a complete JSON after concatenation. (The
        // backslash-escaped JSON below avoids the
        // `r#"..."#` raw-string closing-delimiter collision
        // — the JSON itself contains a trailing `"` that
        // would be mistaken for the end of the raw string.)
        let mut buf = ToolCallBuf {
            id: "call_1".to_string(),
            name: "shell".to_string(),
            args_buf: "{\"cmd\":\"".to_string(),
        };
        buf.args_buf.push_str("ls\"}");
        let ev = build_tool_call_event(&buf, 0).expect("name is set");
        match ev {
            ChatEvent::ToolCall { name, input, .. } => {
                assert_eq!(name, "shell");
                assert_eq!(input, serde_json::json!({"cmd": "ls"}));
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn build_tool_call_event_returns_none_without_name() {
        // Defensive: an OpenAI delta never carried a name
        // for this index. Drop the event rather than emit
        // an incomplete ToolCall.
        let buf = ToolCallBuf {
            id: "call_x".to_string(),
            name: String::new(),
            args_buf: "{}".to_string(),
        };
        let ev = build_tool_call_event(&buf, 0);
        assert!(ev.is_none());
    }

    #[test]
    fn build_tool_call_event_empty_args_buf_yields_empty_object() {
        // Defensive: no arguments at all → empty object,
        // not a parse failure.
        let buf = ToolCallBuf {
            id: "call_x".to_string(),
            name: "ping".to_string(),
            args_buf: String::new(),
        };
        let ev = build_tool_call_event(&buf, 0).expect("name is set");
        match ev {
            ChatEvent::ToolCall { input, .. } => {
                assert_eq!(input, serde_json::json!({}));
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    // ---- accumulate_tool_call_delta (RULE-D-007) ----

    #[test]
    fn accumulate_tool_call_delta_skips_delta_missing_index() {
        // RULE-D-007: a tool_call delta without `index` is skipped
        // rather than falling back to key 0 (which would collide
        // with a real index-0 tool call and corrupt it).
        let mut state: HashMap<u32, ToolCallBuf> = HashMap::new();
        // first delta carries index 0
        accumulate_tool_call_delta(
            &mut state,
            &serde_json::json!({"index":0,"id":"call_a","function":{"name":"read_file","arguments":"{\"path\":"}}),
        );
        // second delta omits index (the bug surface)
        accumulate_tool_call_delta(
            &mut state,
            &serde_json::json!({"id":"call_b","function":{"name":"write_file","arguments":"\"x\""}}),
        );
        // only idx 0 present; second delta dropped, not collided
        assert_eq!(state.len(), 1, "index-less delta must not create an entry");
        let buf = &state[&0];
        assert_eq!(buf.id, "call_a");
        assert_eq!(buf.name, "read_file");
        assert_eq!(buf.args_buf, "{\"path\":");
    }

    #[test]
    fn accumulate_tool_call_delta_merges_same_index_fragments() {
        // Regression guard: the normal OpenAI contract — many deltas
        // sharing one `index` — still merges into one buffer.
        let mut state: HashMap<u32, ToolCallBuf> = HashMap::new();
        accumulate_tool_call_delta(
            &mut state,
            &serde_json::json!({"index":1,"function":{"name":"grep","arguments":"{\"a\":"}}),
        );
        accumulate_tool_call_delta(
            &mut state,
            &serde_json::json!({"index":1,"id":"call_1","function":{"arguments":"\"b\"}"}}),
        );
        let buf = &state[&1];
        assert_eq!(buf.id, "call_1");
        assert_eq!(buf.name, "grep");
        assert_eq!(buf.args_buf, "{\"a\":\"b\"}");
    }
}
