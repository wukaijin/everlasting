//! OpenAI provider 单元测试(拆分自 openai.rs, 08-07-large-file-splitting)。

#![cfg(test)]

// 顶部 import 供 `mod tests` 的 `use super::*` 使用,lib 构建下视为未用
#[allow(unused_imports)]
use super::openai::{
    assistant_blocks_to_openai, is_o1_family, openai_caps, OpenAIConfig, OpenAIProvider,
};
#[allow(unused_imports)]
use super::streaming::*;
#[allow(unused_imports)]
use crate::llm::error::{classify_error_response, LlmError};
#[allow(unused_imports)]
use crate::llm::provider::wire::*;
#[allow(unused_imports)]
use crate::llm::provider::{Provider, ProviderCapabilities, ProviderProtocol};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::error::LlmError;
    use crate::llm::provider::wire::{
        chat_request_to_wire, strip_unsupported, wire_block_to_chat_event, WireBlock,
        WireCapabilities, WireMessage, WireRequest, WireTool,
    };
    use crate::llm::types::{
        ChatEvent, ChatMessage, ChatRequest, ContentBlock, MessageContent, Role,
    };

    fn cfg() -> OpenAIConfig {
        OpenAIConfig {
            base_url: "https://api.openai.com".to_string(),
            model: "gpt-4o".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            reasoning_effort: None,
        }
    }

    /// DeepSeek-v4 config for RULE-D-006 tests. A reasoning-capable
    /// model (reasoning_effort set) so the `reasoning_content` field
    /// gate in `build_http_body` is open and the DeepSeek contract
    /// pin applies. Matches the prod deepseek-v4-flash OpenAI route.
    fn deepseek_cfg() -> OpenAIConfig {
        OpenAIConfig {
            base_url: "https://api.wukaijin.com".to_string(),
            model: "deepseek-v4-flash".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            reasoning_effort: Some("high".to_string()),
        }
    }

    // ---- openai_caps (RULE-D-005) ----

    #[test]
    fn openai_caps_derives_reasoning_effort_from_config() {
        // A model that opted into reasoning effort (o1/o3 with
        // thinking_effort set) keeps the capability.
        let caps = openai_caps(Some("high"));
        assert!(!caps.supports_thinking);
        assert!(caps.supports_reasoning_effort);
        assert!(!caps.supports_thinking_signatures);

        // A non-reasoning model (gpt-4o, no thinking_effort) must
        // NOT claim reasoning support — otherwise strip_unsupported
        // keeps historical Reasoning blocks and pollutes context.
        let caps = openai_caps(None);
        assert!(!caps.supports_reasoning_effort);
    }

    #[test]
    fn openai_caps_strip_drops_reasoning_for_non_reasoning_model() {
        // End-to-end of RULE-D-005: a gpt-4o provider (no
        // reasoning_effort) must drop Reasoning blocks during strip,
        // not keep them as the old hardcoded-true caps did.
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
        let caps = openai_caps(None);
        let stripped = strip_unsupported(messages, &caps);
        let WireMessage::Assistant { blocks, .. } = &stripped[0] else {
            panic!("expected Assistant");
        };
        // Reasoning dropped (non-reasoning model), only Text remains.
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], WireBlock::Text { text, .. } if text == "answer"));
    }

    // ---- endpoint() ----

    #[test]
    fn endpoint_trims_trailing_slash() {
        let c = OpenAIConfig {
            base_url: "https://api.openai.com/v1/".to_string(),
            ..cfg()
        };
        // The base_url already includes `/v1`; the helper only
        // appends `/chat/completions` (no leading `/v1/`).
        assert_eq!(c.endpoint(), "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn endpoint_uses_provided_base_url() {
        let c = OpenAIConfig {
            base_url: "https://proxy.example.com/openai/v1".to_string(),
            ..cfg()
        };
        assert_eq!(
            c.endpoint(),
            "https://proxy.example.com/openai/v1/chat/completions"
        );
    }

    // BUG FIX (06-09-fix-session): real OpenAI-compatible
    // providers (the seed's `https://api.openai.com/v1` and any
    // user-added proxy like `https://hub.example.com/v1`)
    // already include the `/v1` version in `base_url`. The
    // endpoint helper must NOT add another `/v1/`, otherwise
    // the upstream 404s with `path not found: /v1/v1/chat/completions`
    // and the SSE parser never sees a stream — which is the
    // root cause of the "新 session 发送消息，闪一下变空"
    // regression. The pre-fix tests above would have caught this
    // if the seed base_url had been passed in (they hard-coded
    // `https://api.openai.com/...` without the version suffix);
    // the regression test below covers the realistic base_url
    // shape.
    #[test]
    fn endpoint_does_not_double_prefix_v1_when_base_url_includes_v1() {
        let c = OpenAIConfig {
            base_url: "https://api.openai.com/v1".to_string(),
            ..cfg()
        };
        assert_eq!(c.endpoint(), "https://api.openai.com/v1/chat/completions");

        let c = OpenAIConfig {
            base_url: "https://api.deepseek.com/v1".to_string(),
            ..cfg()
        };
        assert_eq!(c.endpoint(), "https://api.deepseek.com/v1/chat/completions");
    }

    // ---- protocol() and capabilities() ----

    #[test]
    fn openai_provider_reports_openai_capabilities_and_protocol() {
        let p = OpenAIProvider::new(cfg());
        assert_eq!(p.protocol(), ProviderProtocol::Openai);
        let caps = p.capabilities();
        assert!(caps.supports_system_prompt);
        assert!(caps.supports_tools);
        assert!(caps.supports_streaming);
    }

    #[test]
    fn openai_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenAIProvider>();
    }

    // ---- build_http_body ----

    #[test]
    fn build_http_body_system_prompt_becomes_first_message() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: Some("You are a coding agent".to_string()),
            messages: vec![WireMessage::User {
                content: "hello".to_string(),
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are a coding agent");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "hello");
    }

    #[test]
    fn build_http_body_no_system_prompt_omits_system_message() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "hi".to_string(),
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn build_http_body_tools_wrapped_in_function_envelope() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "x".to_string(),
                speaker: None,
            }],
            tools: vec![WireTool {
                name: "read_file".to_string(),
                description: Some("read".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            }],
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let tools = body.get("tools").and_then(|t| t.as_array()).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "read_file");
        assert_eq!(tools[0]["function"]["description"], "read");
        assert!(tools[0]["function"]["parameters"].is_object());
    }

    #[test]
    fn build_http_body_tool_results_become_role_tool_messages() {
        // The wire layer lifts `tool_result` blocks out of
        // user messages into `WireMessage::Tool`; OpenAI emits
        // each as a `role: "tool"` message with `tool_call_id`.
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![
                WireMessage::User {
                    content: "looking:".to_string(),
                    speaker: None,
                },
                WireMessage::Tool {
                    tool_call_id: "call_1".to_string(),
                    content: "127.0.0.1 localhost".to_string(),
                },
            ],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], "call_1");
        assert_eq!(msgs[1]["content"], "127.0.0.1 localhost");
    }

    #[test]
    fn build_http_body_assistant_message_carries_text_and_tool_calls() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![
                    WireBlock::Text {
                        text: "let me read".to_string(),
                        cache_control: None,
                    },
                    WireBlock::ToolUse {
                        id: "call_42".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "/etc/hosts"}),
                    },
                ],
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(msgs.len(), 1);
        let m0 = &msgs[0];
        assert_eq!(m0["role"], "assistant");
        assert_eq!(m0["content"], "let me read");
        let tcs = m0["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "call_42");
        assert_eq!(tcs[0]["function"]["name"], "read_file");
        // `arguments` is a JSON string in OpenAI's wire format.
        let args = tcs[0]["function"]["arguments"].as_str().unwrap();
        assert_eq!(args, "{\"path\":\"/etc/hosts\"}");
        // RULE-D-006a regression guard: cfg() is gpt-4o with
        // reasoning_effort=None → a NON-reasoning model. The
        // `reasoning_content` field MUST be absent (not "none",
        // not "" — the field is entirely omitted to keep the
        // vanilla OpenAI shape). See `build_http_body` RULE-D-006a.
        assert!(
            m0.get("reasoning_content").is_none(),
            "gpt-4o (non-reasoning) must not carry reasoning_content: {m0}"
        );
    }

    #[test]
    fn build_http_body_omits_tools_field_when_empty() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "x".to_string(),
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        // `tools` should be absent (not present-but-empty) so
        // the upstream doesn't get an empty `tools: []` and
        // refuse the call.
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn build_http_body_sets_model_and_max_tokens_from_config() {
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(8192),
            system: None,
            messages: vec![WireMessage::User {
                content: "x".to_string(),
                speaker: None,
            }],
            tools: vec![],
        };
        let c = OpenAIConfig {
            model: "gpt-4.1".to_string(),
            max_tokens: 8192,
            ..cfg()
        };
        let body = OpenAIProvider::build_http_body(&wire, &c);
        assert_eq!(body["model"], "gpt-4.1");
        assert_eq!(body["max_tokens"], 8192);
        assert_eq!(body["stream"], true);
        // RULE-D-002 regression guard: non-o1 models must NOT emit
        // the o1-only key.
        assert!(
            body.get("max_completion_tokens").is_none(),
            "non-o1 model must not emit max_completion_tokens: {body}"
        );
    }

    // ---- RULE-D-002: o1+ family uses max_completion_tokens ----

    #[test]
    fn is_o1_family_matches_reasoning_models() {
        // o1 line: o1 / o1-mini / o1-preview / o1-pro
        assert!(is_o1_family("o1"));
        assert!(is_o1_family("o1-mini"));
        assert!(is_o1_family("o1-preview"));
        assert!(is_o1_family("o1-pro"));
        // o3 line: o3 / o3-mini / o3-pro
        assert!(is_o1_family("o3"));
        assert!(is_o1_family("o3-mini"));
        assert!(is_o1_family("o3-pro"));
        // o4 line
        assert!(is_o1_family("o4-mini"));
        // case-insensitive (third-party gateways may emit caps)
        assert!(is_o1_family("O1-MINI"));
        assert!(is_o1_family("  o3-mini  ")); // trims whitespace
    }

    #[test]
    fn is_o1_family_rejects_non_reasoning_models() {
        assert!(!is_o1_family("gpt-4o"));
        assert!(!is_o1_family("gpt-4o-mini"));
        assert!(!is_o1_family("gpt-4.1"));
        assert!(!is_o1_family("chatgpt-4o-latest"));
        assert!(!is_o1_family("glm-4.7"));
    }

    #[test]
    fn build_http_body_o1_family_uses_max_completion_tokens() {
        let wire = WireRequest {
            model: "o1-mini".to_string(),
            max_tokens: Some(8192),
            system: None,
            messages: vec![WireMessage::User {
                content: "x".to_string(),
                speaker: None,
            }],
            tools: vec![],
        };
        let c = OpenAIConfig {
            model: "o3-mini".to_string(),
            max_tokens: 8192,
            ..cfg()
        };
        let body = OpenAIProvider::build_http_body(&wire, &c);
        // o1+ family MUST use max_completion_tokens ...
        assert_eq!(body["max_completion_tokens"], 8192);
        // ... and MUST NOT carry max_tokens (the server 400s on it).
        assert!(
            body.get("max_tokens").is_none(),
            "o1 family must not emit max_tokens (server 400s): {body}"
        );
    }

    // ---- A4: stream_options.include_usage ----

    #[test]
    fn build_http_body_includes_stream_options_for_usage() {
        // A4 (Token Usage Tracking): the request body must
        // include `stream_options: { include_usage: true }`
        // so OpenAI sends a final `usage` chunk in the SSE
        // stream. Without this, `parse_openai_usage` never
        // sees a payload and the agent loop's per-turn
        // accumulation is skipped.
        let wire = WireRequest {
            model: "gpt-4o".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "hi".to_string(),
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let so = body
            .get("stream_options")
            .expect("stream_options key present");
        assert_eq!(so["include_usage"], true);
    }

    // ---- A4: parse_openai_usage ----

    #[test]
    fn openai_strip_drops_thinking_signature_from_anthropic_history() {
        // Simulate a session that was started on a Claude
        // model (history has Thinking+Signature blocks) and
        // the user switched to gpt-4o. The OpenAI send path
        // should drop the signature.
        let req = ChatRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 16384,
            system: Some("hi".to_string()),
            messages: vec![ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Thinking {
                        thinking: "let me think".to_string(),
                        signature: "sig_xyz".to_string(),
                    },
                    ContentBlock::Text {
                        text: "the answer".to_string(),
                        cache_control: None,
                    },
                ]),
                speaker: None,
            }],
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, Some("hi".to_string()));
        let caps = WireCapabilities {
            supports_thinking: false,
            supports_reasoning_effort: true,
            supports_thinking_signatures: false,
        };
        let stripped = strip_unsupported(wire.messages, &caps);
        // The signature-bearing Anthropic session has its
        // signature stripped. The visible text remains, and
        // (post-PR1, RULE-D-006) the surviving `Reasoning` block
        // is lifted into the assistant message's top-level
        // `reasoning_content` field by `build_http_body` — see
        // `deepseek_reasoning_content_round_trip_*` tests below.
        assert_eq!(stripped.len(), 1);
        let WireMessage::Assistant { blocks, .. } = &stripped[0] else {
            panic!("expected Assistant")
        };
        // Reasoning kept (reasoning_effort=true), Signature dropped.
        assert!(blocks
            .iter()
            .any(|b| matches!(b, WireBlock::Reasoning { .. })));
        assert!(!blocks
            .iter()
            .any(|b| matches!(b, WireBlock::Signature { .. })));
        assert!(blocks.iter().any(|b| matches!(b, WireBlock::Text { .. })));
    }

    // ---- error classification on OpenAI-shaped bodies ----

    #[test]
    fn openai_401_classified_as_auth() {
        // OpenAI uses `code` (not `type`) in the error body.
        // The wire shape: { error: { message, type, code, param } }.
        let body = r#"{"error":{"message":"Incorrect API key provided","type":"error","code":"invalid_api_key"}}"#;
        let err = classify_error_response(401, body, None);
        assert!(matches!(err, LlmError::Auth(_)));
    }

    #[test]
    fn openai_429_classified_as_rate_limit() {
        let body = r#"{"error":{"message":"Rate limit reached","type":"error","code":"rate_limit_exceeded"}}"#;
        let err = classify_error_response(429, body, None);
        assert!(matches!(err, LlmError::RateLimit { .. }));
    }

    #[test]
    fn openai_400_with_invalid_request_code_is_invalid() {
        let body = r#"{"error":{"message":"Invalid tool definition","type":"invalid_request_error","code":"invalid_request_error"}}"#;
        let err = classify_error_response(400, body, None);
        assert!(matches!(err, LlmError::InvalidRequest(_)));
    }

    #[test]
    fn openai_500_classified_as_server() {
        let body = r#"{"error":{"message":"Internal server error","type":"server_error","code":"server_error"}}"#;
        let err = classify_error_response(500, body, None);
        assert!(matches!(err, LlmError::Server { status: 500, .. }));
    }

    // ---- tool call accumulator (offline) ----

    // ---- wire_block_to_chat_event path coverage ----

    #[test]
    fn wire_block_to_chat_event_text_path() {
        let ev = wire_block_to_chat_event(&WireBlock::Text {
            text: "hi".to_string(),
            cache_control: None,
        })
        .unwrap();
        assert!(matches!(ev, ChatEvent::Delta { text } if text == "hi"));
    }

    #[test]
    fn wire_block_to_chat_event_reasoning_path() {
        let ev = wire_block_to_chat_event(&WireBlock::Reasoning {
            text: "thinking...".to_string(),
        })
        .unwrap();
        assert!(matches!(ev, ChatEvent::ThinkingDelta { text } if text == "thinking..."));
    }

    // ---- RULE-D-006 (2026-06-21): DeepSeek reasoning_content round-trip ----
    //
    // PR1 of `06-21-route-deepseek-via-openai-protocol-for-native-reasoning-content`.
    // Pre-PR1 the OpenAI adapter prepended `Reasoning` text into the content
    // string as `format!("[reasoning] {}", text)` (a hidden-comment marker).
    // That polluted the visible answer every turn and didn't satisfy
    // DeepSeek v4's `reasoning_content` field contract. PR1 lifts the
    // reasoning text into a dedicated top-level `reasoning_content` field
    // on the assistant message, sibling of `content`. Pure-text assistant
    // turns (worker memory acks, plain replies) get `reasoning_content:"none"`
    // (literal non-empty string — AstrBot PR 7823's choice for DeepSeek v4
    // strictness; harmless on real OpenAI o1/o3 which ignore unknown fields).

    #[test]
    fn reasoning_block_becomes_reasoning_content_field() {
        // The core PR1 invariant: a Reasoning block is lifted to the
        // top-level `reasoning_content` field, NOT prepended into content.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![
                    WireBlock::Reasoning {
                        text: "step 1: analyze".to_string(),
                    },
                    WireBlock::Text {
                        text: "the answer".to_string(),
                        cache_control: None,
                    },
                ],
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(msgs.len(), 1);
        let m0 = &msgs[0];
        assert_eq!(m0["role"], "assistant");
        // content carries ONLY the visible text — no `[reasoning]` marker.
        assert_eq!(m0["content"], "the answer");
        // reasoning_content carries the reasoning text verbatim.
        assert_eq!(m0["reasoning_content"], "step 1: analyze");
        // Negative regression guard: content must NOT contain the marker.
        let content_str = m0["content"].as_str().unwrap();
        assert!(
            !content_str.contains("[reasoning]"),
            "content must not carry the pre-PR1 marker: {content_str}"
        );
    }

    #[test]
    fn text_only_assistant_gets_none_reasoning_content() {
        // Pure-text assistant (worker memory ack, plain reply): no
        // reasoning block to lift. DeepSeek v4 still wants a non-empty
        // `reasoning_content` field (AstrBot PR 7823 contract), so emit
        // the literal string `"none"` — never `""`, never absent.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![WireBlock::Text {
                    text: "Understood.".to_string(),
                    cache_control: None,
                }],
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["content"], "Understood.");
        assert_eq!(m0["reasoning_content"], "none");
    }

    #[test]
    fn multiple_reasoning_blocks_joined_with_newline() {
        // The wire layer splits an Anthropic `Thinking` block into
        // `Reasoning` + `Signature`. After strip drops the signature,
        // multiple surviving `Reasoning` blocks (rare but possible when
        // an assistant turn had several Thinking blocks) are joined with
        // `\n` — matches the AstrBot PR 7823 convention and the live T4
        // probe (multi-line reasoning_content → 200).
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![
                    WireBlock::Reasoning {
                        text: "first thought".to_string(),
                    },
                    WireBlock::Reasoning {
                        text: "second thought".to_string(),
                    },
                    WireBlock::Text {
                        text: "final".to_string(),
                        cache_control: None,
                    },
                ],
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["reasoning_content"], "first thought\nsecond thought");
        assert_eq!(m0["content"], "final");
    }

    #[test]
    fn user_message_does_not_get_reasoning_content_field() {
        // Only assistant messages carry reasoning_content. A user
        // message must NOT get the field — it would be semantically
        // wrong (user messages have no reasoning) and could confuse
        // strict upstream validators.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "hi there".to_string(),
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["role"], "user");
        assert!(
            m0.get("reasoning_content").is_none(),
            "user message must not carry reasoning_content: {m0}"
        );
    }

    #[test]
    fn tool_message_does_not_get_reasoning_content_field() {
        // Same invariant for `role: "tool"` results.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Tool {
                tool_call_id: "call_1".to_string(),
                content: "result body".to_string(),
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["role"], "tool");
        assert!(
            m0.get("reasoning_content").is_none(),
            "tool message must not carry reasoning_content: {m0}"
        );
    }

    #[test]
    fn assistant_with_tool_use_only_gets_none_reasoning_content() {
        // An assistant turn that issued a tool call but produced no
        // reasoning (e.g. a deterministic tool dispatch) still needs
        // a non-empty `reasoning_content` for DeepSeek v4 — `"none"`.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![
                    WireBlock::Text {
                        text: "reading file".to_string(),
                        cache_control: None,
                    },
                    WireBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "/x"}),
                    },
                ],
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["role"], "assistant");
        assert_eq!(m0["content"], "reading file");
        assert_eq!(m0["reasoning_content"], "none");
        // tool_calls still emitted alongside reasoning_content.
        assert_eq!(m0["tool_calls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn gpt_4o_does_not_get_reasoning_content_field_even_with_reasoning_block() {
        // RULE-D-006a regression guard: the `reasoning_content` field
        // gate is based on the MODEL config (reasoning_effort /
        // is_o1_family), NOT on whether the wire payload happens to
        // carry a Reasoning block. On a non-reasoning OpenAI model
        // (gpt-4o with no reasoning_effort) the field is NEVER injected,
        // even if a Reasoning block survived into `build_http_body`
        // (e.g. a future caller forgets to strip). This keeps the
        // vanilla OpenAI request shape clean — `reasoning_content` is
        // a provider-specific extension, not a documented OpenAI field,
        // and carrying it on a gpt-4o request is a latent compat bug
        // against proxies that reserve the name.
        //
        // (In normal operation `strip_unsupported` already drops the
        // Reasoning block before this point for gpt-4o — see
        // `openai_caps_strip_drops_reasoning_for_non_reasoning_model`.
        // This test defends the build_http_body gate independently.)
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(), // wire.model is ignored by build_http_body
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![
                    WireBlock::Reasoning {
                        text: "sneaky reasoning that should not lift".to_string(),
                    },
                    WireBlock::Text {
                        text: "answer".to_string(),
                        cache_control: None,
                    },
                ],
                speaker: None,
            }],
            tools: vec![],
        };
        // cfg() = gpt-4o, reasoning_effort=None → non-reasoning model.
        let body = OpenAIProvider::build_http_body(&wire, &cfg());
        let m0 = &body["messages"].as_array().unwrap()[0];
        assert_eq!(m0["role"], "assistant");
        // The Reasoning text MUST NOT leak into content either (the
        // pre-PR1 `[reasoning]` marker is gone, and the field is gated
        // off so nothing carries the text).
        assert_eq!(m0["content"], "answer");
        let content_str = m0["content"].as_str().unwrap();
        assert!(
            !content_str.contains("sneaky reasoning"),
            "gpt-4o content must not carry reasoning text: {content_str}"
        );
        // The field is entirely absent — not "none", not "".
        assert!(
            m0.get("reasoning_content").is_none(),
            "gpt-4o (non-reasoning) must not carry reasoning_content even with a Reasoning block: {m0}"
        );
    }

    #[test]
    fn o1_family_gets_reasoning_content_field_without_explicit_effort() {
        // RULE-D-006a: an o1-family model is reasoning-capable even
        // without an explicit reasoning_effort set (the family itself
        // is the opt-in signal). The field gate must open for it so
        // o1/o3 history round-trips carry `reasoning_content`.
        let o1_cfg = OpenAIConfig {
            base_url: "https://api.openai.com".to_string(),
            model: "o1-mini".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            reasoning_effort: None, // o1 family is the signal, not effort
        };
        let wire = WireRequest {
            model: "o1-mini".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Assistant {
                blocks: vec![WireBlock::Text {
                    text: "ok".to_string(),
                    cache_control: None,
                }],
                speaker: None,
            }],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &o1_cfg);
        let m0 = &body["messages"].as_array().unwrap()[0];
        // No reasoning block → "none" (o1 family is reasoning-capable).
        assert_eq!(m0["reasoning_content"], "none");
    }

    // ---- DeepSeek v4 reasoning_content contract pin ----
    //
    // This is the PR1 acceptance contract: every assistant message in
    // the history that goes on the wire to a DeepSeek-v4 model MUST
    // carry a non-empty `reasoning_content` field. Verified live
    // (2026-06-21, wukaijin OpenAI endpoint, deepseek-v4-flash):
    //
    //   T1  no field            → 200 (lenient today; AstrBot says strict)
    //   T2  `"none"`            → 200
    //   T3  `""`                → 200 (today; AstrBot says this is rejected)
    //   T4  multi-line non-empty → 200
    //
    // We pin the AstrBot/stricter shape: every assistant has a
    // non-empty `reasoning_content`. If a future change regresses to
    // empty/missing, this test fails before the user sees a 400.

    #[test]
    fn deepseek_reasoning_content_contract_pin_mixed_history() {
        // Construct a realistic multi-turn DeepSeek history:
        //   turn 1 user      — greeting
        //   turn 2 assistant — pure text ack (worker memory ack)
        //   turn 3 user      — real question
        //   turn 4 assistant — full reasoning + answer
        //   turn 5 user      — follow-up
        // The contract: BOTH assistant turns (2 and 4) must carry
        // non-empty `reasoning_content`. Turn 2 has no reasoning block
        // → `"none"`. Turn 4 has reasoning → the joined text.
        let wire = WireRequest {
            model: "deepseek-v4-flash".to_string(),
            max_tokens: Some(16384),
            system: Some("You are a coding agent.".to_string()),
            messages: vec![
                WireMessage::User {
                    content: "remember: project uses pnpm".to_string(),
                    speaker: None,
                },
                WireMessage::Assistant {
                    blocks: vec![WireBlock::Text {
                        text: "Understood.".to_string(),
                        cache_control: None,
                    }],
                    speaker: None,
                },
                WireMessage::User {
                    content: "how do I run tests?".to_string(),
                    speaker: None,
                },
                WireMessage::Assistant {
                    blocks: vec![
                        WireBlock::Reasoning {
                            text: "user asked about tests; project uses pnpm".to_string(),
                        },
                        WireBlock::Text {
                            text: "Run `pnpm test`.".to_string(),
                            cache_control: None,
                        },
                    ],
                    speaker: None,
                },
                WireMessage::User {
                    content: "thanks".to_string(),
                    speaker: None,
                },
            ],
            tools: vec![],
        };
        let body = OpenAIProvider::build_http_body(&wire, &deepseek_cfg());
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        // system + 5 user/assistant turns = 6 total.
        assert_eq!(msgs.len(), 6);
        // System message: no reasoning_content.
        assert_eq!(msgs[0]["role"], "system");
        assert!(msgs[0].get("reasoning_content").is_none());

        // Walk every message and assert the contract.
        for (i, m) in msgs.iter().enumerate() {
            let role = m["role"].as_str().unwrap();
            match role {
                "assistant" => {
                    let rc = m
                        .get("reasoning_content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_else(|| panic!("msg {i}: assistant missing reasoning_content"));
                    assert!(
                        !rc.is_empty(),
                        "msg {i}: assistant reasoning_content must be non-empty (got \"\")"
                    );
                }
                "user" | "system" | "tool" => {
                    assert!(
                        m.get("reasoning_content").is_none(),
                        "msg {i}: {role} message must not carry reasoning_content: {m}"
                    );
                }
                other => panic!("msg {i}: unexpected role {other}"),
            }
        }
        // Spot-check turn 2 (the ack) is `"none"`...
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[2]["reasoning_content"], "none");
        // ...and turn 4 (the real reasoning) is the joined text.
        assert_eq!(msgs[4]["role"], "assistant");
        assert_eq!(
            msgs[4]["reasoning_content"],
            "user asked about tests; project uses pnpm"
        );
    }

    // ---- live integration test (env-gated, no hardcoded endpoint) ----

    /// Live integration smoke test against any OpenAI-compatible endpoint.
    /// Off by default. Opt in by setting all of:
    /// - `EVERLASTING_RUN_LIVE_OPENAI_TEST=1` (master switch)
    /// - `EVERLASTING_LIVE_OPENAI_BASE_URL` (e.g. `https://api.openai.com/v1`)
    /// - `EVERLASTING_LIVE_OPENAI_API_KEY`
    /// Optional: `EVERLASTING_LIVE_OPENAI_MODEL` (default `"test-model"`).
    /// Missing any of those prints a one-line notice and returns success
    /// — keeps CI fast, offline-safe, and free of committed
    /// secrets/endpoints.
    #[tokio::test]
    async fn live_openai_compat_smoke_test() {
        if std::env::var("EVERLASTING_RUN_LIVE_OPENAI_TEST").is_err() {
            eprintln!(
                "skipping live test (set EVERLASTING_RUN_LIVE_OPENAI_TEST=1 plus \
                 EVERLASTING_LIVE_OPENAI_BASE_URL / EVERLASTING_LIVE_OPENAI_API_KEY / \
                 EVERLASTING_LIVE_OPENAI_MODEL to run)"
            );
            return;
        }
        let base_url = match std::env::var("EVERLASTING_LIVE_OPENAI_BASE_URL") {
            Ok(v) if !v.is_empty() => v,
            _ => {
                eprintln!("skipping live test (EVERLASTING_LIVE_OPENAI_BASE_URL not set or empty)");
                return;
            }
        };
        let api_key = match std::env::var("EVERLASTING_LIVE_OPENAI_API_KEY") {
            Ok(v) if !v.is_empty() => v,
            _ => {
                eprintln!("skipping live test (EVERLASTING_LIVE_OPENAI_API_KEY not set or empty)");
                return;
            }
        };
        let model = std::env::var("EVERLASTING_LIVE_OPENAI_MODEL")
            .unwrap_or_else(|_| "test-model".to_string());
        use futures_util::StreamExt;
        let c = OpenAIConfig {
            base_url,
            model,
            api_key,
            max_tokens: 65536,
            reasoning_effort: None,
        };
        let p = OpenAIProvider::new(c);
        let mut s = p.send(
            Some("You are a coding agent.".to_string()),
            vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("吃了吗".to_string()),
                speaker: None,
            }],
            vec![],
        );
        let mut events = Vec::new();
        while let Some(ev) = s.next().await {
            events.push(ev);
        }
        eprintln!("=== events from live send: {} total ===", events.len());
        for (i, e) in events.iter().enumerate() {
            eprintln!("  [{}] {:?}", i, e);
        }
        // Assertions
        let mut saw_start = false;
        let mut accumulated = String::new();
        let mut saw_done = false;
        for e in &events {
            match e {
                Ok(ChatEvent::Start) => saw_start = true,
                Ok(ChatEvent::Delta { text }) => accumulated.push_str(text),
                Ok(ChatEvent::Done { .. }) => saw_done = true,
                Err(e) => panic!("got error event: {:?}", e),
                _ => {}
            }
        }
        assert!(saw_start, "expected Start event");
        assert!(saw_done, "expected Done event");
        assert!(
            !accumulated.is_empty(),
            "expected non-empty text, got: {:?}",
            accumulated
        );
    }
}
