//! OpenAI Responses provider 单元测试(09-18-openai-responses-provider PR1)。
//! 样例驱动风格参照 tests_openai.rs:请求构造断言落在 `build_http_body`
//! 的输出 body 上(AC4 断言对象,review.md 修正 1),事件机组喂合成 SSE
//! 行序列驱动 `ResponsesStreamState`(不发真实 HTTP)。

#![cfg(test)]

// 顶部 import 供 `mod tests` 的 `use super::*` 使用,lib 构建下视为未用
#[allow(unused_imports)]
use super::responses::{
    normalize_responses_effort, responses_caps, ResponsesConfig, ResponsesProvider,
    ResponsesStreamState,
};
#[allow(unused_imports)]
use crate::llm::error::LlmError;
#[allow(unused_imports)]
use crate::llm::provider::streaming::parse_responses_usage;
#[allow(unused_imports)]
use crate::llm::provider::wire::*;
#[allow(unused_imports)]
use crate::llm::provider::{build_provider, Provider, ProviderCapabilities, ProviderProtocol};
#[allow(unused_imports)]
use crate::llm::sse::SseParser;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::wire::{
        chat_request_to_wire, strip_unsupported, WireBlock, WireMessage, WireRequest, WireTool,
    };
    use crate::llm::types::{
        ChatEvent, ChatMessage, ChatRequest, ContentBlock, MessageContent, Role,
    };
    use serde_json::{json, Value};

    fn cfg() -> ResponsesConfig {
        ResponsesConfig {
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5".to_string(),
            api_key: "sk-test".to_string(),
            max_tokens: 16384,
            reasoning_effort: None,
            supports_images: true,
        }
    }

    fn cfg_effort(effort: Option<&str>) -> ResponsesConfig {
        ResponsesConfig {
            reasoning_effort: effort.map(str::to_string),
            ..cfg()
        }
    }

    // -----------------------------------------------------------------
    // SSE 事件机 helper:合成 `event:` + `data:` 行序列,喂 SseParser,
    // 每个事件再交 `ResponsesStreamState::handle_event`(与 send 循环
    // 同构,只是把 HTTP 层换成合成序列)。
    // -----------------------------------------------------------------

    fn sse_line(t: &str, data: &Value) -> String {
        format!(
            "event: {}\ndata: {}\n\n",
            t,
            serde_json::to_string(data).unwrap()
        )
    }

    fn run_sse(
        state: &mut ResponsesStreamState,
        chunks: &[String],
    ) -> Vec<Result<ChatEvent, LlmError>> {
        let mut parser = SseParser::new();
        let mut out = Vec::new();
        for event in parser.feed(&chunks.join("")) {
            let v: Value =
                serde_json::from_str(&event.data).expect("synthetic event data must be JSON");
            out.extend(state.handle_event(&event.event, &v));
        }
        out
    }

    fn typed(t: &str, mut extra: Value) -> String {
        if let Some(obj) = extra.as_object_mut() {
            obj.insert("type".to_string(), json!(t));
        }
        sse_line(t, &extra)
    }

    // ---- endpoint() ----

    #[test]
    fn endpoint_appends_responses_only_no_double_v1() {
        // base_url 含 /v1 的约定与 openai 条目同源;06-09 的 /v1/v1
        // 双拼 bug 不许在第三协议复发。
        let c = ResponsesConfig {
            base_url: "https://api.openai.com/v1".to_string(),
            ..cfg()
        };
        assert_eq!(c.endpoint(), "https://api.openai.com/v1/responses");

        let c = ResponsesConfig {
            base_url: "https://hub.example.com/v1/".to_string(),
            ..cfg()
        };
        assert_eq!(c.endpoint(), "https://hub.example.com/v1/responses");
    }

    // ---- protocol() / capabilities() / Send+Sync ----

    #[test]
    fn responses_provider_reports_capabilities_and_protocol() {
        let p = ResponsesProvider::new(cfg());
        assert_eq!(p.protocol(), ProviderProtocol::OpenaiResponses);
        let caps = p.capabilities();
        assert!(caps.supports_system_prompt);
        assert!(caps.supports_tools);
        assert!(caps.supports_streaming);
    }

    #[test]
    fn responses_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ResponsesProvider>();
    }

    // ---- build_http_body 形状组(design §4.1) ----

    #[test]
    fn build_http_body_basic_shape() {
        // 基本形状:instructions / input item 数组 / max_output_tokens /
        // store:false / stream:true / tools 扁平 + 显式 strict:false。
        // cfg() 无 effort → 无 reasoning 对象。
        let wire = WireRequest {
            model: "gpt-5".to_string(),
            max_tokens: Some(16384),
            system: Some("You are a coding agent".to_string()),
            messages: vec![WireMessage::User {
                content: "hello".to_string(),
                speaker: None,
            }],
            tools: vec![WireTool {
                name: "read_file".to_string(),
                description: Some("read".to_string()),
                input_schema: json!({"type": "object"}),
            }],
        };
        let body = ResponsesProvider::build_http_body(&wire, &cfg());
        assert_eq!(body["model"], "gpt-5");
        assert_eq!(body["instructions"], "You are a coding agent");
        assert_eq!(body["max_output_tokens"], 16384);
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "hello");
        // 扁平 function tool(无 `function: {…}` 信封)+ 显式 strict:false
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "read_file");
        assert_eq!(tools[0]["description"], "read");
        assert!(tools[0]["parameters"].is_object());
        assert_eq!(tools[0]["strict"], false);
        assert!(tools[0].get("function").is_none(), "no CC-style envelope");
        // 无 effort → 不发 reasoning 对象
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn build_http_body_no_system_omits_instructions() {
        let wire = WireRequest {
            model: "gpt-5".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::User {
                content: "hi".to_string(),
                speaker: None,
            }],
            tools: vec![],
        };
        let body = ResponsesProvider::build_http_body(&wire, &cfg());
        assert!(body.get("instructions").is_none());
        assert!(body.get("tools").is_none(), "empty tools → key absent");
    }

    #[test]
    fn build_http_body_speaker_prefix_inline_user_and_assistant() {
        // 群聊 speaker 内联:格式与 anthropic.rs apply_speaker_prefix
        // 同款 `@name: `(review.md 修正 3,不是裸 `Name: `)。
        let wire = WireRequest {
            model: "gpt-5".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![
                WireMessage::User {
                    content: "hi there".to_string(),
                    speaker: Some("Alice".to_string()),
                },
                WireMessage::Assistant {
                    blocks: vec![WireBlock::Text {
                        text: "hello back".to_string(),
                        cache_control: None,
                    }],
                    speaker: Some("Bob".to_string()),
                },
            ],
            tools: vec![],
        };
        let body = ResponsesProvider::build_http_body(&wire, &cfg());
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "@Alice: hi there");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"], "@Bob: hello back");
    }

    #[test]
    fn build_http_body_assistant_text_then_function_call_items_order() {
        // Assistant [Text, ToolUse×2] → assistant message item +
        // 两个独立 function_call item,保序:text 先、call 后
        // (Responses 的工具调用是独立 item,不是 message 上的数组)。
        let wire = WireRequest {
            model: "gpt-5".to_string(),
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
                        input: json!({"path": "/etc/hosts"}),
                    },
                    WireBlock::ToolUse {
                        id: "call_43".to_string(),
                        name: "list_dir".to_string(),
                        input: json!({"path": "/etc"}),
                    },
                ],
                speaker: None,
            }],
            tools: vec![],
        };
        let body = ResponsesProvider::build_http_body(&wire, &cfg());
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[0]["content"], "let me read");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_42");
        assert_eq!(input[1]["name"], "read_file");
        assert_eq!(input[1]["arguments"], "{\"path\":\"/etc/hosts\"}");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_43");
        // 纯 tool-use 轮(无 Text 块)→ message item 空串 content,
        // item 顺序仍成立。
        let wire = WireRequest {
            messages: vec![WireMessage::Assistant {
                blocks: vec![WireBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "ping".to_string(),
                    input: json!({}),
                }],
                speaker: None,
            }],
            ..wire
        };
        let body = ResponsesProvider::build_http_body(&wire, &cfg());
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[0]["content"], "");
        assert_eq!(input[1]["type"], "function_call");
    }

    #[test]
    fn build_http_body_tool_message_becomes_function_call_output() {
        // Tool 结果是独立 function_call_output item,用 call_id(不是
        // item id)与 function_call 关联。
        let wire = WireRequest {
            model: "gpt-5".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::Tool {
                tool_call_id: "call_42".to_string(),
                content: "127.0.0.1 localhost".to_string(),
                images: Vec::new(),
            }],
            tools: vec![],
        };
        let body = ResponsesProvider::build_http_body(&wire, &cfg());
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[0]["call_id"], "call_42");
        assert_eq!(input[0]["output"], "127.0.0.1 localhost");
    }

    #[test]
    fn build_http_body_image_block_becomes_input_image_part() {
        // UserBlocks 里的图片 → input_image part(data URL + detail:auto);
        // 文本 → input_text part。Responses 原生接受 parts 数组,不像
        // CC 那样无图时降级回 string。
        let wire = WireRequest {
            model: "gpt-5".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![WireMessage::UserBlocks {
                blocks: vec![
                    WireBlock::Text {
                        text: "what is this?".to_string(),
                        cache_control: None,
                    },
                    WireBlock::Image {
                        media_type: "image/png".to_string(),
                        data: "aGVsbG8=".to_string(),
                    },
                ],
            }],
            tools: vec![],
        };
        let body = ResponsesProvider::build_http_body(&wire, &cfg());
        let input = body["input"].as_array().unwrap();
        let parts = input[0]["content"].as_array().expect("content is parts");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[0]["text"], "what is this?");
        assert_eq!(parts[1]["type"], "input_image");
        assert_eq!(parts[1]["image_url"], "data:image/png;base64,aGVsbG8=");
        assert_eq!(parts[1]["detail"], "auto");
    }

    #[test]
    fn build_http_body_effort_vocabulary_normalization() {
        // effort 词表归一(design §2.3):词表内原样;xhigh/max → high;
        // 词表外 → 不发 reasoning 对象(不是发出去 400)。
        for (configured, expected) in [
            ("high", Some("high")),
            ("minimal", Some("minimal")),
            ("low", Some("low")),
            ("medium", Some("medium")),
            // Anthropic/DeepSeek 词表存量 → 归一到 high + warn
            ("xhigh", Some("high")),
            ("max", Some("high")),
        ] {
            let wire = WireRequest {
                model: "gpt-5".to_string(),
                max_tokens: Some(16384),
                system: None,
                messages: vec![],
                tools: vec![],
            };
            let body = ResponsesProvider::build_http_body(&wire, &cfg_effort(Some(configured)));
            assert_eq!(
                body["reasoning"]["effort"],
                expected.unwrap(),
                "configured={configured}"
            );
            // summary:auto 是 reasoning_summary_text 事件流的前提
            assert_eq!(body["reasoning"]["summary"], "auto");
        }
        // 词表外 → 无 reasoning 对象
        let wire = WireRequest {
            model: "gpt-5".to_string(),
            max_tokens: Some(16384),
            system: None,
            messages: vec![],
            tools: vec![],
        };
        let body = ResponsesProvider::build_http_body(&wire, &cfg_effort(Some("bogus")));
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn build_http_body_anthropic_history_blocks_absent_in_body_ac4() {
        // AC4(review.md 修正 1):Anthropic 会话切到 Responses 模型,
        // thinking/signature/redacted 块不进 wire body,请求不 400。
        // 关键:effort 已设时 strip 的 block_supported 是 OR 语义,
        // Reasoning 块会幸存 strip——真正的丢弃点在 build 层,所以
        // 断言对象必须是 build_http_body 的输出 body(不能断言
        // post-strip wire)。
        let req = ChatRequest {
            model: "gpt-5".to_string(),
            max_tokens: 16384,
            system: Some("hi".to_string()),
            messages: vec![ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Thinking {
                        thinking: "let me think".to_string(),
                        signature: "sig_xyz".to_string(),
                    },
                    ContentBlock::RedactedThinking {
                        data: "opaque_blob".to_string(),
                    },
                    ContentBlock::Text {
                        text: "the answer".to_string(),
                        cache_control: None,
                    },
                ]),
                speaker: None,
                attachments: None,
            }],
            stream: true,
            tools: vec![],
            thinking: None,
        };
        let wire = chat_request_to_wire(req, Some("hi".to_string()));
        // 推理模型档(effort 已设):Reasoning 幸存 strip——这正是
        // 评审指出的 OR 语义,前置断言钉住这个前提。
        let caps = responses_caps(Some("high"), true);
        assert!(caps.supports_reasoning_effort);
        let stripped = strip_unsupported(wire.messages, &caps);
        assert_eq!(stripped.len(), 1);
        let WireMessage::Assistant { blocks, .. } = &stripped[0] else {
            panic!("expected Assistant")
        };
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, WireBlock::Reasoning { .. })),
            "precondition: Reasoning survives strip under OR semantics"
        );
        assert!(!blocks
            .iter()
            .any(|b| matches!(b, WireBlock::Signature { .. })));
        assert!(!blocks
            .iter()
            .any(|b| matches!(b, WireBlock::RedactedThinking { .. })));

        // AC4 断言对象 = body:Reasoning 文本被 build 层丢弃,
        // signature / redacted blob 全 body 无痕迹。
        let wire = WireRequest {
            messages: stripped,
            ..wire
        };
        let body = ResponsesProvider::build_http_body(&wire, &cfg_effort(Some("high")));
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[0]["content"], "the answer");
        let body_str = serde_json::to_string(&body).unwrap();
        assert!(!body_str.contains("let me think"), "{body_str}");
        assert!(!body_str.contains("sig_xyz"), "{body_str}");
        assert!(!body_str.contains("opaque_blob"), "{body_str}");
        // 推理档的 reasoning 对象照发
        assert_eq!(body["reasoning"]["effort"], "high");
    }

    // ---- 事件机组(design §4.2):喂合成 SSE 行序列 ----

    #[test]
    fn event_stream_plain_text_end_turn_with_usage() {
        let mut state = ResponsesStreamState::new();
        let events = run_sse(
            &mut state,
            &[
                typed("response.created", json!({"response": {"id": "resp_1"}})),
                typed(
                    "response.output_text.delta",
                    json!({"output_index": 1, "delta": "Hello"}),
                ),
                typed(
                    "response.output_text.delta",
                    json!({"output_index": 1, "delta": ","}),
                ),
                typed(
                    "response.output_text.delta",
                    json!({"output_index": 1, "delta": "world"}),
                ),
                typed(
                    "response.completed",
                    json!({"response": {
                        "id": "resp_1",
                        "output": [{"type": "message", "role": "assistant",
                                    "content": [{"type": "output_text", "text": "Hello,world"}]}],
                        "usage": {
                            "input_tokens": 120,
                            "input_tokens_details": {"cached_tokens": 64},
                            "output_tokens": 30,
                            "output_tokens_details": {"reasoning_tokens": 10},
                            "total_tokens": 150
                        }
                    }}),
                ),
            ],
        );
        assert_eq!(events.len(), 5, "got: {events:?}");
        assert!(matches!(events[0], Ok(ChatEvent::Start)));
        assert!(matches!(&events[1], Ok(ChatEvent::Delta { text }) if text == "Hello"));
        assert!(matches!(&events[2], Ok(ChatEvent::Delta { text }) if text == ","));
        assert!(matches!(&events[3], Ok(ChatEvent::Delta { text }) if text == "world"));
        match &events[4] {
            Ok(ChatEvent::Done { stop_reason, usage }) => {
                assert_eq!(stop_reason.as_deref(), Some("end_turn"));
                let u = usage.expect("usage present");
                assert_eq!(u.input_tokens, 120);
                assert_eq!(u.output_tokens, 30);
                assert_eq!(u.cache_read_input_tokens, 64);
                // reasoning_tokens 是 output 子集,不重复计
                assert_eq!(u.context_input_tokens, 120);
            }
            other => panic!("expected Done, got {other:?}"),
        }
        assert!(state.is_finished());
    }

    #[test]
    fn event_stream_tool_call_assembled_from_deltas() {
        let mut state = ResponsesStreamState::new();
        let events = run_sse(
            &mut state,
            &[
                typed("response.created", json!({"response": {"id": "resp_1"}})),
                typed(
                    "response.output_item.added",
                    json!({"output_index": 1, "item": {"type": "function_call",
                           "id": "fc_1", "call_id": "call_abc", "name": "read_file",
                           "arguments": ""}}),
                ),
                typed(
                    "response.function_call_arguments.delta",
                    json!({"output_index": 1, "delta": "{\"path\":"}),
                ),
                typed(
                    "response.function_call_arguments.delta",
                    json!({"output_index": 1, "delta": "\"/etc/hosts\"}"}),
                ),
                // arguments.done 是冗余事件,不是 flush 点——不得双发
                typed(
                    "response.function_call_arguments.done",
                    json!({"output_index": 1, "arguments": "{\"path\":\"/etc/hosts\"}"}),
                ),
                typed(
                    "response.output_item.done",
                    json!({"output_index": 1, "item": {"type": "function_call",
                           "id": "fc_1", "call_id": "call_abc", "name": "read_file",
                           "arguments": "{\"path\":\"/etc/hosts\"}"}}),
                ),
                typed(
                    "response.completed",
                    json!({"response": {"id": "resp_1",
                           "output": [{"type": "function_call", "call_id": "call_abc",
                                       "name": "read_file", "arguments": "{\"path\":\"/etc/hosts\"}"}],
                           "usage": {"input_tokens": 90, "output_tokens": 21}}}),
                ),
            ],
        );
        // Start + ToolCall + Done(arguments.done 不产出)
        assert_eq!(events.len(), 3, "got: {events:?}");
        assert!(matches!(events[0], Ok(ChatEvent::Start)));
        match &events[1] {
            Ok(ChatEvent::ToolCall { id, name, input }) => {
                assert_eq!(id, "call_abc", "关联键是 call_id");
                assert_eq!(name, "read_file");
                assert_eq!(*input, json!({"path": "/etc/hosts"}));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        // completed 的 output 含 function_call → tool_use
        match &events[2] {
            Ok(ChatEvent::Done { stop_reason, .. }) => {
                assert_eq!(stop_reason.as_deref(), Some("tool_use"));
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn event_stream_parallel_tool_calls_interleaved_by_output_index() {
        // 两个 function_call(不同 output_index)的 delta 交错到达,
        // 各自聚合互不串线;flush 顺序按 output_item.done 到达顺序。
        let mut state = ResponsesStreamState::new();
        let events = run_sse(
            &mut state,
            &[
                typed("response.created", json!({"response": {"id": "resp_1"}})),
                typed(
                    "response.output_item.added",
                    json!({"output_index": 1, "item": {"type": "function_call",
                           "call_id": "call_1", "name": "read_file", "arguments": ""}}),
                ),
                typed(
                    "response.output_item.added",
                    json!({"output_index": 2, "item": {"type": "function_call",
                           "call_id": "call_2", "name": "list_dir", "arguments": ""}}),
                ),
                typed(
                    "response.function_call_arguments.delta",
                    json!({"output_index": 1, "delta": "{\"path\":"}),
                ),
                typed(
                    "response.function_call_arguments.delta",
                    json!({"output_index": 2, "delta": "{\"path\":\"/etc\"}"}),
                ),
                typed(
                    "response.function_call_arguments.delta",
                    json!({"output_index": 1, "delta": "\"/etc/hosts\"}"}),
                ),
                typed(
                    "response.output_item.done",
                    json!({"output_index": 1, "item": {"type": "function_call",
                           "call_id": "call_1", "name": "read_file",
                           "arguments": "{\"path\":\"/etc/hosts\"}"}}),
                ),
                typed(
                    "response.output_item.done",
                    json!({"output_index": 2, "item": {"type": "function_call",
                           "call_id": "call_2", "name": "list_dir",
                           "arguments": "{\"path\":\"/etc\"}"}}),
                ),
                typed(
                    "response.completed",
                    json!({"response": {"id": "resp_1",
                           "output": [{"type": "function_call"}, {"type": "function_call"}],
                           "usage": {"input_tokens": 90, "output_tokens": 40}}}),
                ),
            ],
        );
        let tool_calls: Vec<&ChatEvent> = events
            .iter()
            .filter_map(|e| e.as_ref().ok())
            .filter(|e| matches!(e, ChatEvent::ToolCall { .. }))
            .collect();
        assert_eq!(tool_calls.len(), 2, "got: {events:?}");
        match tool_calls[0] {
            ChatEvent::ToolCall { id, input, .. } => {
                assert_eq!(id, "call_1");
                assert_eq!(*input, json!({"path": "/etc/hosts"}));
            }
            _ => unreachable!(),
        }
        match tool_calls[1] {
            ChatEvent::ToolCall { id, input, .. } => {
                assert_eq!(id, "call_2");
                assert_eq!(*input, json!({"path": "/etc"}));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn event_stream_incomplete_max_tokens() {
        let mut state = ResponsesStreamState::new();
        let events = run_sse(
            &mut state,
            &[
                typed("response.created", json!({"response": {"id": "resp_2"}})),
                typed(
                    "response.output_text.delta",
                    json!({"output_index": 1, "delta": "partial"}),
                ),
                typed(
                    "response.incomplete",
                    json!({"response": {"id": "resp_2",
                           "incomplete_details": {"reason": "max_output_tokens"},
                           "usage": {"input_tokens": 50, "output_tokens": 16}}}),
                ),
            ],
        );
        assert_eq!(events.len(), 3, "got: {events:?}");
        match &events[2] {
            Ok(ChatEvent::Done { stop_reason, usage }) => {
                assert_eq!(stop_reason.as_deref(), Some("max_tokens"));
                assert_eq!(usage.map(|u| u.output_tokens), Some(16));
            }
            other => panic!("expected Done, got {other:?}"),
        }
        assert!(state.is_finished());
    }

    #[test]
    fn event_stream_truncated_arguments_degrade_to_raw_string() {
        // review.md 修正 6:max_output_tokens 截断 arguments →
        // output_item.done 带不完整 JSON → input 降级为
        // Value::String(原始串),不 panic 不丢调用;随后 incomplete
        // → max_tokens 衔接语义。
        let mut state = ResponsesStreamState::new();
        let raw = "{\"path\": \"/etc/ho";
        let events = run_sse(
            &mut state,
            &[
                typed("response.created", json!({"response": {"id": "resp_3"}})),
                typed(
                    "response.output_item.added",
                    json!({"output_index": 0, "item": {"type": "function_call",
                           "call_id": "call_trunc", "name": "read_file", "arguments": ""}}),
                ),
                typed(
                    "response.function_call_arguments.delta",
                    json!({"output_index": 0, "delta": raw}),
                ),
                typed(
                    "response.output_item.done",
                    json!({"output_index": 0, "item": {"type": "function_call",
                           "call_id": "call_trunc", "name": "read_file",
                           "arguments": raw}}),
                ),
                typed(
                    "response.incomplete",
                    json!({"response": {"id": "resp_3",
                           "incomplete_details": {"reason": "max_output_tokens"},
                           "usage": {"input_tokens": 70, "output_tokens": 16}}}),
                ),
            ],
        );
        assert_eq!(events.len(), 3, "got: {events:?}");
        match &events[1] {
            Ok(ChatEvent::ToolCall { id, input, .. }) => {
                assert_eq!(id, "call_trunc");
                assert_eq!(*input, Value::String(raw.to_string()));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        match &events[2] {
            Ok(ChatEvent::Done { stop_reason, .. }) => {
                assert_eq!(stop_reason.as_deref(), Some("max_tokens"));
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn event_stream_flush_prefers_delta_buffer_over_empty_done_item() {
        // 官方形状:output_index 在事件 envelope 层(item 上没有),
        // flush 必须以 envelope 键查聚合 buffer——done item 省略
        // arguments 的网关场景下,delta 累积串是唯一数据源,不能退化
        // 成 {}(flush 先 buffer、done item 逐字段兜底的次序钉死)。
        let mut state = ResponsesStreamState::new();
        let events = run_sse(
            &mut state,
            &[
                typed("response.created", json!({"response": {"id": "resp_8"}})),
                typed(
                    "response.output_item.added",
                    json!({"output_index": 0, "item": {"type": "function_call",
                           "call_id": "call_buf", "name": "read_file", "arguments": ""}}),
                ),
                typed(
                    "response.function_call_arguments.delta",
                    json!({"output_index": 0, "delta": "{\"path\":\"/etc/passwd\"}"}),
                ),
                typed(
                    "response.output_item.done",
                    json!({"output_index": 0, "item": {"type": "function_call",
                           "call_id": "call_buf", "name": "read_file", "arguments": ""}}),
                ),
                typed(
                    "response.completed",
                    json!({"response": {"id": "resp_8",
                           "output": [{"type": "function_call"}],
                           "usage": {"input_tokens": 33, "output_tokens": 9}}}),
                ),
            ],
        );
        assert_eq!(events.len(), 3, "got: {events:?}");
        match &events[1] {
            Ok(ChatEvent::ToolCall { id, input, .. }) => {
                assert_eq!(id, "call_buf");
                assert_eq!(*input, json!({"path": "/etc/passwd"}));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        assert!(matches!(&events[2], Ok(ChatEvent::Done { stop_reason, .. })
            if stop_reason.as_deref() == Some("tool_use")));
    }

    #[test]
    fn event_stream_refusal_part_becomes_delta() {
        // review.md 修正 5:message item 的 refusal content part →
        // 可见 Delta(不是 LlmError),completed 正常收尾——拒绝不得
        // 产出零文本空气泡。
        let mut state = ResponsesStreamState::new();
        let events = run_sse(
            &mut state,
            &[
                typed("response.created", json!({"response": {"id": "resp_4"}})),
                typed(
                    "response.output_item.done",
                    json!({"output_index": 0, "item": {"type": "message",
                           "role": "assistant",
                           "content": [{"type": "refusal",
                                        "refusal": "I cannot help with that."}]}}),
                ),
                typed(
                    "response.completed",
                    json!({"response": {"id": "resp_4",
                           "output": [{"type": "message"}],
                           "usage": {"input_tokens": 40, "output_tokens": 12}}}),
                ),
            ],
        );
        assert_eq!(events.len(), 3, "got: {events:?}");
        assert!(matches!(events[0], Ok(ChatEvent::Start)));
        assert!(matches!(&events[1], Ok(ChatEvent::Delta { text })
            if text == "I cannot help with that."));
        match &events[2] {
            Ok(ChatEvent::Done { stop_reason, .. }) => {
                assert_eq!(stop_reason.as_deref(), Some("end_turn"));
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn event_stream_failed_yields_llm_error() {
        let mut state = ResponsesStreamState::new();
        let events = run_sse(
            &mut state,
            &[
                typed("response.created", json!({"response": {"id": "resp_5"}})),
                typed(
                    "response.failed",
                    json!({"response": {"id": "resp_5",
                           "error": {"code": "server_error",
                                     "message": "The model is overloaded"}}}),
                ),
            ],
        );
        assert_eq!(events.len(), 2, "got: {events:?}");
        assert!(matches!(events[0], Ok(ChatEvent::Start)));
        match &events[1] {
            Err(LlmError::Server { message, .. }) => {
                assert!(message.contains("overloaded"), "{message}");
            }
            other => panic!("expected LlmError, got {other:?}"),
        }
        assert!(state.is_finished());
    }

    #[test]
    fn event_stream_unknown_events_ignored_no_panic() {
        // 未上心事件(web_search 系 / queued / in_progress)穿插 →
        // 无 panic、无事件,且后续正常事件照常处理。
        let mut state = ResponsesStreamState::new();
        let events = run_sse(
            &mut state,
            &[
                typed("response.created", json!({"response": {"id": "resp_6"}})),
                typed("response.queued", json!({"response": {"id": "resp_6"}})),
                typed(
                    "response.in_progress",
                    json!({"response": {"id": "resp_6"}}),
                ),
                typed(
                    "response.web_search_call.in_progress",
                    json!({"output_index": 0, "item": {"type": "web_search_call"}}),
                ),
                typed(
                    "response.web_search_call.completed",
                    json!({"output_index": 0, "item": {"type": "web_search_call"}}),
                ),
                typed(
                    "response.output_text.delta",
                    json!({"output_index": 1, "delta": "ok"}),
                ),
                typed(
                    "response.completed",
                    json!({"response": {"id": "resp_6",
                           "output": [{"type": "message"}],
                           "usage": {"input_tokens": 10, "output_tokens": 2}}}),
                ),
            ],
        );
        assert_eq!(events.len(), 3, "got: {events:?}");
        assert!(matches!(events[0], Ok(ChatEvent::Start)));
        assert!(matches!(&events[1], Ok(ChatEvent::Delta { text }) if text == "ok"));
        assert!(matches!(&events[2], Ok(ChatEvent::Done { .. })));
    }

    #[test]
    fn event_stream_duplicate_created_emits_start_once() {
        // 防御:网关重放 created 不得重发 Start。
        let mut state = ResponsesStreamState::new();
        let events = run_sse(
            &mut state,
            &[
                typed("response.created", json!({"response": {"id": "resp_7"}})),
                typed("response.created", json!({"response": {"id": "resp_7"}})),
            ],
        );
        assert_eq!(events.len(), 1, "got: {events:?}");
        assert!(matches!(events[0], Ok(ChatEvent::Start)));
    }

    // ---- usage 组(design §4.3):parse_responses_usage 三态 ----

    #[test]
    fn parse_responses_usage_full_payload() {
        // usage 嵌在 response 对象内(官方流式 completed 形状)。
        let v = json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1",
                "usage": {
                    "input_tokens": 200,
                    "input_tokens_details": {"cached_tokens": 50},
                    "output_tokens": 30,
                    "output_tokens_details": {"reasoning_tokens": 12},
                    "total_tokens": 230
                }
            }
        });
        let u = parse_responses_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 200);
        assert_eq!(u.output_tokens, 30);
        assert_eq!(u.cache_read_input_tokens, 50);
        // Responses 的 input_tokens 已含 cached_tokens(同 CC
        // prompt_tokens 语义):context = input,加 cache_read 会重复计。
        assert_eq!(u.context_input_tokens, 200);
        assert_eq!(u.cache_creation_input_tokens, 0);
    }

    #[test]
    fn parse_responses_usage_missing_details_and_top_level_fallback() {
        // 老 API / 非缓存模型:无 input_tokens_details 子对象。
        let v = json!({
            "response": {"usage": {"input_tokens": 50, "output_tokens": 10}}
        });
        let u = parse_responses_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 50);
        assert_eq!(u.output_tokens, 10);
        assert_eq!(u.cache_read_input_tokens, 0);
        assert_eq!(u.context_input_tokens, 50);

        // 防御 fallback:部分网关把 usage 平铺在事件顶层。
        let v = json!({"usage": {"input_tokens": 7, "output_tokens": 3}});
        let u = parse_responses_usage(&v).expect("non-zero usage");
        assert_eq!(u.input_tokens, 7);
        assert_eq!(u.output_tokens, 3);
    }

    #[test]
    fn parse_responses_usage_zero_or_missing_returns_none() {
        // 全零 → None(照 parse_openai_usage 惯例,agent loop 跳过 SQL 写)。
        let v = json!({
            "response": {"usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}}
        });
        assert!(parse_responses_usage(&v).is_none());
        // 无 usage 键 → None。
        let v = json!({"response": {"id": "resp_1"}});
        assert!(parse_responses_usage(&v).is_none());
    }

    // ---- responses_caps(RULE-D-005 结构兄弟) ----

    #[test]
    fn responses_caps_derive_from_config() {
        // 推理模型档(effort 已设):仅 reasoning_effort 开,
        // thinking / signatures 永远关。
        let caps = responses_caps(Some("high"), true);
        assert!(!caps.supports_thinking);
        assert!(caps.supports_reasoning_effort);
        assert!(!caps.supports_thinking_signatures);
        assert!(caps.supports_images);

        // 非推理档:Reasoning 块在 strip 层就被丢弃。
        let caps = responses_caps(None, false);
        assert!(!caps.supports_reasoning_effort);
        assert!(!caps.supports_images);
    }

    #[test]
    fn normalize_effort_empty_string_is_silent_none() {
        // 空串 = 未配置(与 openai.rs 的 effort 空串守卫同款),静默不发。
        assert_eq!(normalize_responses_effort(Some("")), None);
        assert_eq!(normalize_responses_effort(None), None);
    }

    // ---- 工厂分支与协议枚举 ----

    #[test]
    fn build_provider_openai_responses_returns_responses_provider() {
        let p = crate::db::ProviderRow {
            id: "pid-1".to_string(),
            protocol: "openai_responses".to_string(),
            display_name: "OpenAI Responses".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            has_key: true,
            disabled: false,
            created_at: "2026-09-18T00:00:00Z".to_string(),
            updated_at: "2026-09-18T00:00:00Z".to_string(),
        };
        let m = crate::db::ModelRow {
            id: "mid-1".to_string(),
            provider_id: "pid-1".to_string(),
            model_name: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            max_tokens: None,
            thinking_effort: Some("xhigh".to_string()),
            supports_thinking: false,
            supports_images: false,
            context_window: 400_000,
            disabled: false,
            created_at: "2026-09-18T00:00:00Z".to_string(),
            updated_at: "2026-09-18T00:00:00Z".to_string(),
        };
        let provider = build_provider(&p, &m).expect("openai_responses is implemented");
        assert_eq!(provider.protocol(), ProviderProtocol::OpenaiResponses);
        let caps = provider.capabilities();
        assert!(caps.supports_system_prompt);
        assert!(caps.supports_tools);
        assert!(caps.supports_streaming);
    }

    #[test]
    fn provider_protocol_openai_responses_three_spellings_agree() {
        // serde / as_str / from_str_opt 三面同词(rename_all=lowercase
        // 会把变体名折成 "openairesponses",显式 rename 钉住一致性)。
        let v = ProviderProtocol::OpenaiResponses;
        assert_eq!(v.as_str(), "openai_responses");
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"openai_responses\"");
        let parsed: ProviderProtocol = serde_json::from_str("\"openai_responses\"").unwrap();
        assert_eq!(parsed, ProviderProtocol::OpenaiResponses);
        // from_str_opt 分支
        assert_eq!(
            crate::db::ProviderProtocol::from_str_opt("openai_responses"),
            ProviderProtocol::OpenaiResponses
        );
        // 未知值兜底 Anthropic 的既有行为不受影响
        assert_eq!(
            crate::db::ProviderProtocol::from_str_opt("mystery"),
            ProviderProtocol::Anthropic
        );
    }
}
