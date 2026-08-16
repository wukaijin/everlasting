//! wire 模块单元测试(拆分自 wire.rs, 08-07-large-file-splitting)。

#![cfg(test)]

use crate::llm::provider::wire::*;
use crate::llm::types::{
    CacheControl, ChatEvent, ChatMessage, ChatRequest, ContentBlock, MessageContent, Role, ToolDef,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ModelRow;

    fn model(supports_thinking: bool, thinking_effort: Option<&str>) -> ModelRow {
        ModelRow {
            id: "mid".to_string(),
            provider_id: "pid".to_string(),
            model_name: "m".to_string(),
            display_name: "M".to_string(),
            max_tokens: Some(8192),
            thinking_effort: thinking_effort.map(str::to_string),
            supports_thinking,
            supports_images: false,
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

    // ---- orphan_tool_use_ids (Pair Atomicity guard, llm-contract.md §Pair Atomicity) ----

    #[test]
    fn orphan_tool_use_ids_flags_tool_use_without_matching_result() {
        // assistant emitted tool_use, history has no tool_result → orphan
        let msgs = vec![ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "toolu_1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({}),
            }]),
            speaker: None,
        }];
        assert_eq!(orphan_tool_use_ids(&msgs), vec!["toolu_1".to_string()]);
    }

    #[test]
    fn orphan_tool_use_ids_empty_when_every_use_has_result() {
        let msgs = vec![
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                }]),
                speaker: None,
            },
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_1".to_string(),
                    content: "ok".to_string(),
                    is_error: false,
                }]),
                speaker: None,
            },
        ];
        assert!(orphan_tool_use_ids(&msgs).is_empty());
    }

    #[test]
    fn orphan_tool_use_ids_flags_partial_results() {
        // assistant emits 2 tool_use, only 1 result returned → 1 orphan.
        // This is the cancel-during-tool-execution partial-result shape.
        let msgs = vec![
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    ContentBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "toolu_2".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({}),
                    },
                ]),
                speaker: None,
            },
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_1".to_string(),
                    content: "ok".to_string(),
                    is_error: false,
                }]),
                speaker: None,
            },
        ];
        assert_eq!(orphan_tool_use_ids(&msgs), vec!["toolu_2".to_string()]);
    }

    // ---- chat_request_to_wire orphan self-heal (08-06 group-chat fix) ----

    /// 08-06: an orphan tool_use (assistant emitted tool_use with no
    /// matching tool_result) would 400 upstream. `chat_request_to_wire`
    /// must self-heal by appending a synthetic tool_result for each
    /// orphan id so the request satisfies Pair Atomicity. This is the
    /// defensive fix for the group-chat death-loop (session 4a9d3566):
    /// once a single orphan landed in the DB, every subsequent request
    /// 400'd → `[生成出错中断]` → moderator retried → more orphans.
    #[test]
    fn chat_request_to_wire_heals_orphan_tool_use_with_synthetic_result() {
        let req = ChatRequest {
            model: "test".to_string(),
            max_tokens: 100,
            messages: vec![ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "orphan_1".to_string(),
                    name: "nominate_speaker".to_string(),
                    input: serde_json::json!({}),
                }]),
                speaker: None,
            }],
            system: None,
            stream: false,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, None);
        // The last wire message must be a Tool carrying the synthetic
        // result for the orphan id.
        let last = wire.messages.last().expect("wire must have the heal msg");
        match last {
            WireMessage::Tool {
                tool_call_id,
                content,
            } => {
                assert_eq!(tool_call_id, "orphan_1");
                assert!(
                    content.contains("synthesized by wire layer"),
                    "synthetic result must mark itself: {content:?}"
                );
            }
            other => panic!("expected WireMessage::Tool, got {other:?}"),
        }
    }

    /// 08-06: when there are NO orphans, `chat_request_to_wire` must not
    /// inject anything (byte-identical to pre-fix for clean history).
    #[test]
    fn chat_request_to_wire_no_heal_when_history_is_clean() {
        let req = ChatRequest {
            model: "test".to_string(),
            max_tokens: 100,
            messages: vec![
                ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                        id: "ok_1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({}),
                    }]),
                    speaker: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: "ok_1".to_string(),
                        content: "data".to_string(),
                        is_error: false,
                    }]),
                    speaker: None,
                },
            ],
            system: None,
            stream: false,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, None);
        // No synthetic Tool appended — the wire has exactly the
        // assistant + tool pair, nothing more.
        assert_eq!(
            wire.messages.len(),
            2,
            "clean history must not get a heal injection: {:?}",
            wire.messages
        );
    }

    #[test]
    fn orphan_tool_call_order_empty_when_assistant_directly_followed_by_tool() {
        // assistant(tool_use) → Tool: the canonical correct shape.
        let messages = vec![
            WireMessage::Assistant {
                blocks: vec![WireBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                }],
                speaker: None,
            },
            WireMessage::Tool {
                tool_call_id: "toolu_1".to_string(),
                content: "ok".to_string(),
            },
            WireMessage::User {
                content: "thanks".to_string(),
                speaker: None,
            },
        ];
        assert!(
            orphan_tool_call_order(&messages).is_empty(),
            "no violation: assistant(tool_use) is immediately followed by Tool"
        );
    }

    #[test]
    fn orphan_tool_call_order_empty_for_two_tool_uses_followed_by_two_tools() {
        // assistant emits 2 tool_use; both are answered back-to-back
        // BEFORE any user/assistant message appears.
        let messages = vec![
            WireMessage::Assistant {
                blocks: vec![
                    WireBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({}),
                    },
                    WireBlock::ToolUse {
                        id: "toolu_2".to_string(),
                        name: "grep".to_string(),
                        input: serde_json::json!({}),
                    },
                ],
                speaker: None,
            },
            WireMessage::Tool {
                tool_call_id: "toolu_1".to_string(),
                content: "ok".to_string(),
            },
            WireMessage::Tool {
                tool_call_id: "toolu_2".to_string(),
                content: "ok".to_string(),
            },
            WireMessage::User {
                content: "next".to_string(),
                speaker: None,
            },
        ];
        assert!(
            orphan_tool_call_order(&messages).is_empty(),
            "no violation: both tool_call_ids satisfied by back-to-back Tool messages"
        );
    }

    #[test]
    fn orphan_tool_call_order_flags_user_text_between_assistant_and_tool() {
        // THE BUG: a loop-detection hint Text block sits at the head
        // of the user(tool_results) message → wire fan-out produces
        // `assistant(tool_use) → user(text) → tool` and OpenAI 400s.
        let messages = vec![
            WireMessage::Assistant {
                blocks: vec![WireBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                }],
                speaker: None,
            },
            WireMessage::User {
                content: "⚠️  loop detected ...".to_string(),
                speaker: None,
            },
            WireMessage::Tool {
                tool_call_id: "toolu_1".to_string(),
                content: "ok".to_string(),
            },
        ];
        let violations = orphan_tool_call_order(&messages);
        assert_eq!(
            violations.len(),
            1,
            "exactly one violation (the interleaved user text)"
        );
        assert!(
            violations[0].contains("toolu_1"),
            "violation names the tool_use_id: {}",
            violations[0]
        );
        assert!(
            violations[0].contains("User("),
            "violation names the offending message kind: {}",
            violations[0]
        );
        assert!(
            violations[0].contains("index 1"),
            "violation names the offending wire message index: {}",
            violations[0]
        );
        assert!(
            violations[0].contains("immediately"),
            "first interleaving message is flagged as the immediate-follow break: {}",
            violations[0]
        );
    }

    #[test]
    fn orphan_tool_call_order_flags_userblocks_between_assistant_and_tool() {
        // The B5 memory UserBlocks path (multi-block user message)
        // interleaved between assistant(tool_use) and its Tool result.
        let messages = vec![
            WireMessage::Assistant {
                blocks: vec![WireBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                }],
                speaker: None,
            },
            WireMessage::UserBlocks {
                blocks: vec![WireBlock::Text {
                    text: "banner".to_string(),
                    cache_control: None,
                }],
            },
            WireMessage::Tool {
                tool_call_id: "toolu_1".to_string(),
                content: "ok".to_string(),
            },
        ];
        let violations = orphan_tool_call_order(&messages);
        assert_eq!(violations.len(), 1);
        assert!(
            violations[0].contains("UserBlocks("),
            "UserBlocks kind surfaced: {}",
            violations[0]
        );
    }

    #[test]
    fn orphan_tool_call_order_flags_second_assistant_before_tool_complete() {
        // assistant(tool_use A) → assistant(...) → tool(A): the second
        // assistant is interleaved before A's result is satisfied.
        let messages = vec![
            WireMessage::Assistant {
                blocks: vec![WireBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                }],
                speaker: None,
            },
            WireMessage::Assistant {
                blocks: vec![WireBlock::Text {
                    text: "thinking...".to_string(),
                    cache_control: None,
                }],
                speaker: None,
            },
            WireMessage::Tool {
                tool_call_id: "toolu_1".to_string(),
                content: "ok".to_string(),
            },
        ];
        let violations = orphan_tool_call_order(&messages);
        assert_eq!(violations.len(), 1);
        assert!(
            violations[0].contains("Assistant("),
            "Assistant kind surfaced: {}",
            violations[0]
        );
    }

    #[test]
    fn orphan_tool_call_order_no_violation_when_no_tool_uses() {
        // Pure text conversation — nothing to check.
        let messages = vec![
            WireMessage::User {
                content: "hi".to_string(),
                speaker: None,
            },
            WireMessage::Assistant {
                blocks: vec![WireBlock::Text {
                    text: "hello".to_string(),
                    cache_control: None,
                }],
                speaker: None,
            },
        ];
        assert!(orphan_tool_call_order(&messages).is_empty());
    }

    #[test]
    fn orphan_tool_call_order_truncates_long_user_content_in_diagnostic() {
        // Diagnostic must not blow up the log on a huge user message —
        // `truncate(content, 40)` caps the displayed prefix.
        let long = "x".repeat(500);
        let messages = vec![
            WireMessage::Assistant {
                blocks: vec![WireBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                }],
                speaker: None,
            },
            WireMessage::User {
                content: long.clone(),
                speaker: None,
            },
            WireMessage::Tool {
                tool_call_id: "toolu_1".to_string(),
                content: "ok".to_string(),
            },
        ];
        let violations = orphan_tool_call_order(&messages);
        assert_eq!(violations.len(), 1);
        // The diagnostic should contain the ellipsis marker, NOT the
        // full 500-char string.
        assert!(
            violations[0].contains("…"),
            "long content is truncated: {}",
            violations[0]
        );
        assert!(
            violations[0].len() < long.len() + 200,
            "diagnostic is bounded: {}",
            violations[0].len()
        );
    }

    #[test]
    fn orphan_tool_call_order_flags_partial_tools_then_interleave() {
        // assistant emits 2 tool_use; first Tool answers toolu_1, then
        // a User message interleaves before toolu_2 is answered.
        // The User at index 3 should be flagged (not immediately the
        // first following message, but before toolu_2 is satisfied).
        let messages = vec![
            WireMessage::Assistant {
                blocks: vec![
                    WireBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({}),
                    },
                    WireBlock::ToolUse {
                        id: "toolu_2".to_string(),
                        name: "grep".to_string(),
                        input: serde_json::json!({}),
                    },
                ],
                speaker: None,
            },
            WireMessage::Tool {
                tool_call_id: "toolu_1".to_string(),
                content: "ok1".to_string(),
            },
            WireMessage::User {
                content: "interleaved".to_string(),
                speaker: None,
            },
            WireMessage::Tool {
                tool_call_id: "toolu_2".to_string(),
                content: "ok2".to_string(),
            },
        ];
        let violations = orphan_tool_call_order(&messages);
        assert_eq!(
            violations.len(),
            1,
            "the User at index 2 is the only violation: {}",
            violations.to_vec().join("\n")
        );
        assert!(
            violations[0].contains("toolu_2"),
            "the unsatisfied id (toolu_2) appears in missing list: {}",
            violations[0]
        );
        assert!(
            violations[0].contains("before all tool_call_ids were satisfied"),
            "non-immediate violation tagged correctly: {}",
            violations[0]
        );
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
                speaker: None,
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
        assert!(
            matches!(&wire.messages[0], WireMessage::User { content, .. } if content == "hello")
        );
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
                speaker: None,
            }],
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, None);
        // Expect: [User("looking at result:"), Tool, User("and another:"), Tool]
        assert_eq!(wire.messages.len(), 4);
        assert!(
            matches!(&wire.messages[0], WireMessage::User { content, .. } if content == "looking at result:")
        );
        assert!(
            matches!(&wire.messages[1], WireMessage::Tool { tool_call_id, content }
            if tool_call_id == "toolu_1" && content == "127.0.0.1 localhost")
        );
        assert!(
            matches!(&wire.messages[2], WireMessage::User { content, .. } if content == "and another:")
        );
        assert!(
            matches!(&wire.messages[3], WireMessage::Tool { tool_call_id, .. }
            if tool_call_id == "toolu_2")
        );
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
                speaker: None,
            }],
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, None);
        assert_eq!(wire.messages.len(), 1);
        let WireMessage::Assistant { blocks, .. } = &wire.messages[0] else {
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
            speaker: None,
        }];
        let caps = openai_caps(false, true);
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks, .. } = &stripped[0] else {
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
            speaker: None,
        }];
        let caps = openai_caps(false, false);
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks, .. } = &stripped[0] else {
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
            speaker: None,
        }];
        // Worst-case caps: nothing supported except text + tool.
        let caps = WireCapabilities {
            supports_thinking: false,
            supports_reasoning_effort: false,
            supports_thinking_signatures: false,
        };
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks, .. } = &stripped[0] else {
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
            speaker: None,
        }];
        // OpenAI target: redacted_thinking is opaque to us → drop.
        let caps = openai_caps(true, true);
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks, .. } = &stripped[0] else {
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
                speaker: None,
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
        assert!(matches!(&stripped[0], WireMessage::User { content, .. } if content == "hi"));
        assert!(
            matches!(&stripped[1], WireMessage::Tool { tool_call_id, .. } if tool_call_id == "t1")
        );
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
            speaker: None,
        }];
        let caps = anthropic_caps(true);
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks, .. } = &stripped[0] else {
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
            speaker: None,
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
        let ChatMessage {
            content: MessageContent::Blocks(blocks),
            ..
        } = &back[0]
        else {
            panic!("expected Blocks content");
        };
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
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
            speaker: None,
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
        let ChatMessage {
            content: MessageContent::Blocks(blocks),
            ..
        } = &back[0]
        else {
            panic!("expected Blocks content");
        };
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
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
            speaker: None,
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
        let ChatMessage {
            content: MessageContent::Blocks(blocks),
            ..
        } = &back[0]
        else {
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
                speaker: None,
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
        assert!(
            matches!(&wire.messages[0], WireMessage::UserBlocks { blocks } if blocks.len() == 2)
        );
    }
}
