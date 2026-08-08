//! Anthropic provider 单元测试(迁移自 anthropic.rs 内联 mod tests,
//! 08-08-a-class-anthropic-split)。

#![cfg(test)]
// 顶部 import 供 `mod tests` 的 `use super::*` 使用,lib 构建下视为未用
#![allow(unused_imports)]
use super::anthropic::*;
use super::{build_provider, Provider, ProviderProtocol};
use crate::db;
use crate::llm::types::{ChatEvent, ChatMessage, ChatRequest, ThinkingConfig, TokenUsage};

mod tests {
    use super::*;
    use crate::llm::types::ChatRequest;

    #[test]
    fn default_max_tokens_is_16384_not_1024() {
        // Extended thinking tokens count against max_tokens; 1024 was
        // bumped to 16384 in step 6 to cover a typical thinking + reply
        // turn without truncation.
        assert_eq!(DEFAULT_MAX_TOKENS, 16384);
    }

    #[test]
    fn thinking_config_is_adaptive_summarized_with_configured_effort() {
        let config = LlmConfig {
            base_url: "https://example.com".to_string(),
            model: "claude-opus-4-7".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            thinking_effort: "xhigh".to_string(),
        };
        let tc = config.thinking_config();
        match tc {
            ThinkingConfig::Adaptive { display, effort } => {
                assert_eq!(display, "summarized");
                assert_eq!(effort, "xhigh");
            }
        }
    }

    /// Step 4 follow-up Bug 3: when the agent loop builds a system
    /// prompt for the current session, that string must make it into
    /// the request body's top-level `system` field (Anthropic's
    /// schema). Verified by serializing a `ChatRequest` with the
    /// `system` field populated and checking the wire shape.
    #[test]
    fn chat_request_system_field_serializes_when_some() {
        let req = ChatRequest {
            model: "test".to_string(),
            max_tokens: 100,
            messages: vec![],
            system: Some("You are a coding agent in worktree /foo".to_string()),
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.get("system").and_then(|s| s.as_str()),
            Some("You are a coding agent in worktree /foo")
        );
    }

    /// The `AnthropicProvider` reports Anthropic as its protocol and
    /// supports the three capabilities the chat command cares about
    /// (system prompt, tools, streaming).
    #[test]
    fn anthropic_provider_reports_capabilities_and_protocol() {
        let p = AnthropicProvider::new(LlmConfig {
            base_url: "https://api.anthropic.com".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            thinking_effort: "high".to_string(),
        });
        assert_eq!(p.protocol(), ProviderProtocol::Anthropic);
        let caps = p.capabilities();
        assert!(caps.supports_system_prompt);
        assert!(caps.supports_tools);
        assert!(caps.supports_streaming);
    }

    /// Two `AnthropicProvider`s built from the same `LlmConfig` are
    /// interchangeable — the chat command could in principle clone
    /// the provider for the 20-turn loop, but in practice we just
    /// call `send` on the same instance. The relevant invariant:
    /// `Send + Sync` (the trait's super-trait) is satisfied, so the
    /// chat command's `Box<dyn Provider>` can move into a
    /// `tauri::async_runtime::spawn` task.
    #[test]
    fn anthropic_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AnthropicProvider>();
    }

    /// Sanity: the factory in `mod.rs` constructs an
    /// `AnthropicProvider` whose internal `LlmConfig` is wired from
    /// the catalog rows. We re-check the protocol + capabilities
    /// here (the catalog-driven path), distinct from the
    /// hand-built `AnthropicProvider::new` test above.
    #[test]
    fn factory_built_provider_reports_anthropic_capabilities() {
        let p = crate::db::ProviderRow {
            id: "pid-1".to_string(),
            protocol: "anthropic".to_string(),
            display_name: "Anthropic 官方".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: "sk-test".to_string(),
            has_key: true,
            created_at: "2026-06-09T00:00:00Z".to_string(),
            updated_at: "2026-06-09T00:00:00Z".to_string(),
        };
        let m = db::ModelRow {
            id: "mid-1".to_string(),
            provider_id: "pid-1".to_string(),
            model_name: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            max_tokens: Some(8192),
            thinking_effort: Some("high".to_string()),
            supports_thinking: true,
            context_window: 200_000,
            created_at: "2026-06-09T00:00:00Z".to_string(),
            updated_at: "2026-06-09T00:00:00Z".to_string(),
        };
        let provider = super::build_provider(&p, &m).expect("anthropic is implemented");
        assert_eq!(provider.protocol(), ProviderProtocol::Anthropic);
        let caps = provider.capabilities();
        assert!(caps.supports_system_prompt);
        assert!(caps.supports_tools);
        assert!(caps.supports_streaming);
    }

    // ---- A4: parse_anthropic_usage ----

    #[test]
    fn parse_anthropic_usage_full_payload() {
        // Anthropic's `message_delta.usage` (cumulative per-turn).
        let v = serde_json::json!({
            "input_tokens": 1234,
            "output_tokens": 56,
            "cache_creation_input_tokens": 100,
            "cache_read_input_tokens": 200,
        });
        let u = parse_anthropic_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 1234);
        assert_eq!(u.output_tokens, 56);
        assert_eq!(u.cache_creation_input_tokens, 100);
        assert_eq!(u.cache_read_input_tokens, 200);
        // 2026-06-26 snapshot fix: context_input_tokens = input +
        // cache_creation + cache_read (Anthropic's input_tokens
        // EXCLUDES cache reads/creations; the true context footprint
        // is the sum). 1234 + 100 + 200 = 1534.
        assert_eq!(u.context_input_tokens, 1534);
    }

    #[test]
    fn parse_anthropic_usage_minimal_payload() {
        // Pre-caching Anthropic / older proxy / non-thinking
        // call: only the two core fields are present. Defaults
        // fill the cache fields to 0.
        let v = serde_json::json!({
            "input_tokens": 42,
            "output_tokens": 7,
        });
        let u = parse_anthropic_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 42);
        assert_eq!(u.output_tokens, 7);
        assert_eq!(u.cache_creation_input_tokens, 0);
        assert_eq!(u.cache_read_input_tokens, 0);
        // 42 + 0 + 0 = 42.
        assert_eq!(u.context_input_tokens, 42);
    }

    #[test]
    fn parse_anthropic_usage_zero_returns_none() {
        // An all-zero payload is treated as "no usage
        // information" so the agent loop's
        // `if let Some(t) = usage { ... }` path correctly skips
        // the SQL write. See the function's docstring for the
        // rationale.
        let v = serde_json::json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0,
        });
        assert!(parse_anthropic_usage(&v).is_none());
    }

    #[test]
    fn parse_anthropic_usage_empty_object_returns_none() {
        // A `usage: {}` event (defensive — Anthropic doesn't
        // emit this, but a proxy might) is treated as
        // "no usage".
        let v = serde_json::json!({});
        assert!(parse_anthropic_usage(&v).is_none());
    }

    // -----------------------------------------------------------------
    // DeepSeek-Via-Anthropic-Relay reasoning_content fix
    // (task 06-20-deepseek-reasoner-reasoning-content-400 +
    // follow-up 06-21-fix-deepseek-relay-thinking-block-drop-causing-turn-2-400)
    //
    // These tests pin the contract of `apply_deepseek_reasoning_fix`:
    //
    //   (A) For assistant messages with at least one thinking block
    //       whose `thinking` text is non-empty, add a top-level
    //       `reasoning_content` field whose value is the concatenation
    //       of ALL thinking blocks' `thinking` text (joined by `\n`).
    //       Empty-signature blocks contribute their text too.
    //
    // The fix does NOT drop any thinking blocks. The previous
    // 06-20 implementation had a (B) "drop empty-signature thinking
    // blocks" step; that was based on an unverified attribution
    // ("empty sig inflates the relay's accumulated-state count")
    // that turned out to be WRONG on real-relay probing — the
    // wukaijin relay requires `content[].thinking` blocks AND a
    // top-level `reasoning_content` field TOGETHER (signatures not
    // verified), so dropping blocks triggered a new turn-2 400
    // (`content[].thinking must be passed back`). See
    // `deepseek_relay_contract_v1_v2_v3` for the pinned contract.
    //
    // User messages and tool result messages are NOT touched. The
    // top-level `thinking: adaptive` field on the request body is NOT
    // touched (Claude extended thinking depends on it). Messages with
    // no thinking blocks (pure text + tool_use) do NOT gain a
    // `reasoning_content` field (the collected buffer is empty and
    // an empty `reasoning_content: ""` would mismatch the relay's
    // content-shape contract, so the field is omitted entirely).
    //
    // See `.trellis/tasks/06-21-fix-deepseek-relay-thinking-block-drop-causing-turn-2-400/prd.md`
    // for the V1/V2/V3 probe evidence and the corrected rationale.
    // -----------------------------------------------------------------

    /// Helper: build a `ChatRequest` from a list of message JSON
    /// values, so the test bodies can focus on the message shape and
    /// not on constructing `ChatMessage` / `ContentBlock` by hand for
    /// every case. The `model` / `max_tokens` / `system` / `tools`
    /// fields are fixed at benign values; the fix doesn't touch any
    /// of them.
    fn chat_request_with_messages(messages: Vec<serde_json::Value>) -> ChatRequest {
        let parsed: Vec<ChatMessage> = messages
            .into_iter()
            .map(|m| serde_json::from_value(m).expect("message JSON parses"))
            .collect();
        ChatRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: 16384,
            messages: parsed,
            system: None,
            stream: true,
            tools: vec![],
            thinking: Some(ThinkingConfig::Adaptive {
                display: "summarized".to_string(),
                effort: "high".to_string(),
            }),
        }
    }

    #[test]
    fn deepseek_reasoning_fix_keeps_empty_sig_and_lifts_reasoning_content() {
        // An assistant message with both an empty-signature thinking
        // block and a non-empty-signature thinking block must KEEP
        // BOTH blocks verbatim (the relay does not verify signatures)
        // AND lift a top-level `reasoning_content` whose value is the
        // `\n`-join of ALL thinking blocks' text.
        //
        // This is the corrected contract: the previous 06-20 fix
        // DROPPED empty-signature blocks, which the wukaijin relay
        // rejects as `content[].thinking must be passed back` on the
        // next turn. Empty signatures are produced by the relay
        // itself in streaming mode (it does not emit
        // `signature_delta`), so persistence will land empty
        // signatures and the fix must round-trip them intact.
        let req = chat_request_with_messages(vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "empty sig thinking", "signature": ""},
                {"type": "thinking", "thinking": "uuid sig thinking", "signature": "uuid-sig-abc"},
                {"type": "text", "text": "visible answer"}
            ]
        })]);
        let body = apply_deepseek_reasoning_fix(&req);
        let content = body["messages"][0]["content"]
            .as_array()
            .expect("content array");
        // ALL 3 blocks survive (2 thinking + 1 text). No drops.
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], "");
        assert_eq!(content[0]["thinking"], "empty sig thinking");
        assert_eq!(content[1]["type"], "thinking");
        assert_eq!(content[1]["signature"], "uuid-sig-abc");
        assert_eq!(content[1]["thinking"], "uuid sig thinking");
        assert_eq!(content[2]["type"], "text");
        assert_eq!(content[2]["text"], "visible answer");
        // reasoning_content carries the text of ALL thinking blocks
        // (empty-sig block included), joined by `\n`.
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("empty sig thinking\nuuid sig thinking".to_string())
        );
    }

    #[test]
    fn deepseek_reasoning_fix_keeps_all_empty_sig_and_lifts_reasoning_content() {
        // An assistant message whose thinking blocks ALL have empty
        // signatures must STILL keep them all and STILL lift a
        // top-level `reasoning_content` whose value is the `\n`-join
        // of all their text. The previous 06-20 behavior (drop empty
        // blocks, omit `reasoning_content`) was wrong: the relay
        // requires the blocks AND the field together, and accepts
        // empty signatures without verification.
        let req = chat_request_with_messages(vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "empty 1", "signature": ""},
                {"type": "thinking", "thinking": "empty 2", "signature": ""},
                {"type": "text", "text": "answer"}
            ]
        })]);
        let body = apply_deepseek_reasoning_fix(&req);
        let content = body["messages"][0]["content"]
            .as_array()
            .expect("content array");
        // ALL 3 blocks survive — empty signatures are NOT a drop signal.
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], "");
        assert_eq!(content[0]["thinking"], "empty 1");
        assert_eq!(content[1]["type"], "thinking");
        assert_eq!(content[1]["signature"], "");
        assert_eq!(content[1]["thinking"], "empty 2");
        assert_eq!(content[2]["type"], "text");
        // reasoning_content = "\n"-join of all thinking blocks' text.
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("empty 1\nempty 2".to_string())
        );
    }

    #[test]
    fn deepseek_relay_contract_v1_v2_v3() {
        // PIN TEST — this test exists specifically to prevent a
        // future regression to "drop empty-signature thinking blocks"
        // (the original 06-20 fix that caused the turn-2 400). The
        // wukaijin.com relay's thinking-mode contract was verified
        // against the real relay via V1/V2/V3 probe experiments
        // (scripts `/tmp/ds_probe/v{1,2,3}*.json` in the task prd):
        //
        //   V1: drop `content[].thinking` blocks
        //       → 400 "content[].thinking must be passed back"
        //   V2: keep `content[].thinking` blocks + add `reasoning_content`
        //       → 200 ✅
        //   V3: keep `content[].thinking` blocks + NO `reasoning_content`
        //       → 400 "reasoning_content must be passed back"
        //
        // Conclusion: the relay requires blocks AND `reasoning_content`
        // TOGETHER, and does NOT cryptographically verify the
        // `signature` field (empty signatures are accepted). The
        // correct `apply_deepseek_reasoning_fix` output for any input
        // containing thinking blocks is V2.
        //
        // See `.trellis/tasks/06-21-fix-deepseek-relay-thinking-block-drop-causing-turn-2-400/prd.md`
        // for the V1/V2/V3 table and the DB evidence (session
        // `863fda30-66a1-421d-bd91-0c3a6bb9b342` seq=1 assistant
        // has `"signature": ""`).

        // Turn-2 assistant shape (DeepSeek-via-relay, empty signatures
        // because the relay's streaming mode doesn't emit
        // `signature_delta` — this is the realistic input shape).
        let turn2_assistant = serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "first reasoning", "signature": ""},
                {"type": "text", "text": "answer"}
            ]
        });

        // Sanity check what each V variant looks like relative to the
        // input, then assert the fix produces V2.
        let input = chat_request_with_messages(vec![turn2_assistant]);
        let body = apply_deepseek_reasoning_fix(&input);
        let content = body["messages"][0]["content"]
            .as_array()
            .expect("content array");

        // V1 invariant: NOT this — would be `content.len() == 1` with
        // only the text block. We assert against it explicitly.
        assert_ne!(
            content.len(),
            1,
            "V1 (drop thinking blocks) must NOT happen — relay 400s with 'content[].thinking must be passed back'"
        );

        // V2 (the contract): both thinking blocks AND
        // `reasoning_content` present.
        assert_eq!(
            content.len(),
            2,
            "V2: all content blocks preserved (thinking + text)"
        );
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], ""); // empty sig kept as-is
        assert_eq!(content[1]["type"], "text");
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("first reasoning".to_string()),
            "V2: reasoning_content must be present with the lifted thinking text"
        );

        // V3 invariant: NOT this — would be `content[].thinking` blocks
        // present but no top-level `reasoning_content`. We assert the
        // field is present (covered above) and assert a non-null value
        // shape explicitly so a future edit that nulls it out trips
        // here.
        assert!(
            body["messages"][0].get("reasoning_content").is_some(),
            "V3 (blocks kept, no reasoning_content) must NOT happen — relay 400s with 'reasoning_content must be passed back'"
        );
        assert!(
            body["messages"][0]["reasoning_content"].is_string(),
            "reasoning_content must be a non-null string, not null/sentinel"
        );
    }

    #[test]
    fn deepseek_reasoning_fix_keeps_nonempty_sig_and_adds_reasoning_content() {
        // Single non-empty-signature thinking block: step (B) keeps
        // it verbatim; step (A) lifts its `thinking` text to the
        // top-level `reasoning_content` field.
        let req = chat_request_with_messages(vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "let me think", "signature": "uuid-xyz"},
                {"type": "text", "text": "ok"}
            ]
        })]);
        let body = apply_deepseek_reasoning_fix(&req);
        let content = body["messages"][0]["content"]
            .as_array()
            .expect("content array");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], "uuid-xyz");
        assert_eq!(content[0]["thinking"], "let me think");
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("let me think".to_string())
        );
    }

    #[test]
    fn deepseek_reasoning_fix_concatenates_multiple_nonempty_blocks() {
        // Multiple non-empty-signature thinking blocks (a model can
        // emit more than one per turn). They are all preserved in
        // `content[]` AND their `thinking` text is joined with `\n`
        // into the `reasoning_content` field.
        let req = chat_request_with_messages(vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "step 1", "signature": "sig-1"},
                {"type": "thinking", "thinking": "step 2", "signature": "sig-2"},
                {"type": "text", "text": "done"}
            ]
        })]);
        let body = apply_deepseek_reasoning_fix(&req);
        let content = body["messages"][0]["content"]
            .as_array()
            .expect("content array");
        // Both thinking blocks preserved.
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["signature"], "sig-1");
        assert_eq!(content[1]["signature"], "sig-2");
        assert_eq!(content[2]["type"], "text");
        // reasoning_content = "step 1\nstep 2" (joined by \n).
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("step 1\nstep 2".to_string())
        );
    }

    #[test]
    fn deepseek_reasoning_fix_skips_user_messages() {
        // (R4 contract.) User-role messages must be entirely
        // untouched — content[] unchanged, no reasoning_content
        // field added, no other mutations. The fix is an
        // assistant-message-only patch.
        let req = chat_request_with_messages(vec![
            serde_json::json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "what is X?"},
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok", "is_error": false}
                ]
            }),
            serde_json::json!({
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "thinking", "signature": "sig-1"},
                    {"type": "text", "text": "X is ..."}
                ]
            }),
        ]);
        let body = apply_deepseek_reasoning_fix(&req);
        // User message: untouched.
        let user = &body["messages"][0];
        assert_eq!(user["role"], "user");
        assert_eq!(user["content"].as_array().unwrap().len(), 2);
        assert!(user.get("reasoning_content").is_none());
        // Assistant message: gets the reasoning_content field.
        let asst = &body["messages"][1];
        assert_eq!(
            asst["reasoning_content"],
            serde_json::Value::String("thinking".to_string())
        );
    }

    #[test]
    fn deepseek_reasoning_fix_no_thinking_blocks_no_reasoning_content() {
        // An assistant message with NO thinking blocks (pure text +
        // tool_use) must not gain a reasoning_content field. The fix
        // is a no-op for such messages.
        let req = chat_request_with_messages(vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "sure, let me read"},
                {"type": "tool_use", "id": "toolu_42", "name": "read_file", "input": {"path": "/etc/hosts"}}
            ]
        })]);
        let body = apply_deepseek_reasoning_fix(&req);
        let content = body["messages"][0]["content"]
            .as_array()
            .expect("content array");
        // Unchanged: text + tool_use, no thinking blocks.
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "tool_use");
        assert!(
            body["messages"][0].get("reasoning_content").is_none(),
            "reasoning_content must NOT appear on a no-thinking assistant message: {:?}",
            body["messages"][0]
        );
    }

    #[test]
    fn deepseek_reasoning_fix_preserves_top_level_thinking_field() {
        // The top-level `thinking: adaptive` field on the request
        // body must be preserved verbatim — Claude extended thinking
        // depends on it. The fix only mutates assistant messages'
        // `content[]` and (conditionally) adds `reasoning_content`.
        let req = ChatRequest {
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 16384,
            messages: vec![serde_json::from_value(serde_json::json!({
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "deep", "signature": "sig-abc"},
                    {"type": "text", "text": "answer"}
                ]
            }))
            .unwrap()],
            system: Some("You are a coding agent".to_string()),
            stream: true,
            tools: vec![],
            thinking: Some(ThinkingConfig::Adaptive {
                display: "summarized".to_string(),
                effort: "high".to_string(),
            }),
        };
        let body = apply_deepseek_reasoning_fix(&req);
        // Top-level thinking field preserved verbatim.
        let thinking = body.get("thinking").expect("thinking field present");
        assert_eq!(thinking["type"], "adaptive");
        assert_eq!(thinking["display"], "summarized");
        assert_eq!(thinking["effort"], "high");
        // Sanity: the other top-level fields are untouched too.
        assert_eq!(body["model"], "claude-sonnet-4-5");
        assert_eq!(body["max_tokens"], 16384);
        assert_eq!(body["system"], "You are a coding agent");
        assert_eq!(body["stream"], true);
        // reasoning_content still attached to the assistant message.
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            serde_json::Value::String("deep".to_string())
        );
    }

    // -----------------------------------------------------------------------
    // 事件 handler 单元测试(08-08-a-class-anthropic-split 提取后新增,AC7)
    // -----------------------------------------------------------------------

    #[test]
    fn handle_content_block_start_dispatches_block_types() {
        // tool_use → ToolUse(id/name 提取)
        let mut st = BlockState::Idle;
        handle_content_block_start(
            r#"{"content_block": {"type": "tool_use", "id": "tu_1", "name": "read_file"}}"#,
            &mut st,
        );
        match st {
            BlockState::ToolUse { id, name, .. } => {
                assert_eq!(id, "tu_1");
                assert_eq!(name, "read_file");
            }
            _ => panic!("expected ToolUse"),
        }
        // thinking → Thinking
        let mut st = BlockState::Idle;
        handle_content_block_start(r#"{"content_block": {"type": "thinking"}}"#, &mut st);
        assert!(matches!(st, BlockState::Thinking { .. }));
        // redacted_thinking → RedactedThinking(携带 data)
        let mut st = BlockState::Idle;
        handle_content_block_start(
            r#"{"content_block": {"type": "redacted_thinking", "data": "abc"}}"#,
            &mut st,
        );
        assert!(matches!(st, BlockState::RedactedThinking { data_buf } if data_buf == "abc"));
        // 未知/默认类型 → Text
        let mut st = BlockState::Idle;
        handle_content_block_start(r#"{"content_block": {"type": "text"}}"#, &mut st);
        assert!(matches!(st, BlockState::Text));
        // JSON 解析失败 → 静默跳过(状态不变)
        let mut st = BlockState::Idle;
        handle_content_block_start("not-json", &mut st);
        assert!(matches!(st, BlockState::Idle));
    }

    #[test]
    fn handle_content_block_stop_tool_use_emits_tool_call_and_resets_state() {
        let mut st = BlockState::ToolUse {
            id: "tu_1".to_string(),
            name: "read_file".to_string(),
            json_buf: r#"{"path": "a.rs"}"#.to_string(),
        };
        let ev = handle_content_block_stop(&mut st).expect("ToolCall event");
        match ev {
            ChatEvent::ToolCall { id, name, input } => {
                assert_eq!(id, "tu_1");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "a.rs");
            }
            _ => panic!("expected ToolCall"),
        }
        assert!(matches!(st, BlockState::Idle), "state must reset to Idle");
    }

    #[test]
    fn handle_content_block_stop_empty_tool_json_defaults_to_empty_object() {
        let mut st = BlockState::ToolUse {
            id: "tu_2".to_string(),
            name: "grep".to_string(),
            json_buf: String::new(),
        };
        let ev = handle_content_block_stop(&mut st).expect("ToolCall event");
        match ev {
            ChatEvent::ToolCall { input, .. } => {
                assert!(input.as_object().is_some_and(|o| o.is_empty()))
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn handle_content_block_stop_thinking_signature_emits_only_when_nonempty() {
        // 空签名 → None(无 SignatureDelta)
        let mut st = BlockState::Thinking {
            thinking_buf: "think".to_string(),
            signature_buf: String::new(),
        };
        assert!(handle_content_block_stop(&mut st).is_none());
        // 非空 → 单次 SignatureDelta
        let mut st = BlockState::Thinking {
            thinking_buf: "think".to_string(),
            signature_buf: "sig123".to_string(),
        };
        match handle_content_block_stop(&mut st) {
            Some(ChatEvent::SignatureDelta { signature }) => assert_eq!(signature, "sig123"),
            _ => panic!("expected SignatureDelta"),
        }
    }

    #[test]
    fn handle_message_delta_usage_overwrites_and_none_preserves() {
        let mut stop_reason: Option<String> = None;
        let mut usage: Option<TokenUsage> = None;
        // 带 usage 的 message_delta → 覆盖
        handle_message_delta(
            r#"{"delta": {"stop_reason": "end_turn"}, "usage": {"input_tokens": 1, "output_tokens": 2}}"#,
            &mut stop_reason,
            &mut usage,
        );
        assert_eq!(stop_reason.as_deref(), Some("end_turn"));
        assert!(usage.is_some(), "usage must be set from message_delta");
        // 无 usage 字段 → 保留原值(不当 None 覆盖)
        handle_message_delta(r#"{"delta": {}}"#, &mut stop_reason, &mut usage);
        assert!(
            usage.is_some(),
            "existing usage must be preserved when delta has none"
        );
    }
}
