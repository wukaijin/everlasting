#![cfg(test)]

use crate::llm::types::*;

#[test]
fn message_content_serialize_text_as_string() {
    let mc = MessageContent::Text("hello".to_string());
    let json = serde_json::to_string(&mc).unwrap();
    assert_eq!(json, "\"hello\"");
}

#[test]
fn message_content_deserialize_string() {
    let mc: MessageContent = serde_json::from_str("\"hello\"").unwrap();
    assert_eq!(mc, MessageContent::Text("hello".to_string()));
}

#[test]
fn message_content_serialize_blocks_as_array() {
    let blocks = vec![ContentBlock::Text {
        text: "hi".to_string(),
        cache_control: None,
    }];
    let mc = MessageContent::Blocks(blocks);
    let json = serde_json::to_string(&mc).unwrap();
    assert!(json.starts_with('['));
    assert!(json.contains("\"type\":\"text\""));
}

#[test]
fn message_content_deserialize_blocks() {
    let json = r#"[{"type":"text","text":"hello"}]"#;
    let mc: MessageContent = serde_json::from_str(json).unwrap();
    match mc {
        MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 1);
            assert_eq!(
                blocks[0],
                ContentBlock::Text {
                    text: "hello".to_string(),
                    cache_control: None,
                }
            );
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn chat_message_backward_compat() {
    // Step 1 frontend sends {"role":"user","content":"hi"}
    let msg: ChatMessage = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
    assert_eq!(msg.role, Role::User);
    assert_eq!(msg.content, MessageContent::Text("hi".to_string()));

    // Round-trip: serializes back as plain string
    let json = serde_json::to_string(&msg).unwrap();
    assert_eq!(json, r#"{"role":"user","content":"hi"}"#);
}

#[test]
fn chat_message_with_tool_use() {
    let json = r#"{"role":"assistant","content":[
        {"type":"text","text":"let me read that"},
        {"type":"tool_use","id":"toolu_123","name":"read_file","input":{"path":"/etc/hosts"}}
    ]}"#;
    let msg: ChatMessage = serde_json::from_str(json).unwrap();
    match &msg.content {
        MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 2);
            assert!(
                matches!(&blocks[0], ContentBlock::Text { text, .. } if text == "let me read that")
            );
            assert!(
                matches!(&blocks[1], ContentBlock::ToolUse { name, .. } if name == "read_file")
            );
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn chat_message_with_tool_result() {
    let json = r#"{"role":"user","content":[
        {"type":"tool_result","tool_use_id":"toolu_123","content":"127.0.0.1 localhost"}
    ]}"#;
    let msg: ChatMessage = serde_json::from_str(json).unwrap();
    match &msg.content {
        MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 1);
            assert!(
                matches!(&blocks[0], ContentBlock::ToolResult { content, is_error, .. }
                if content == "127.0.0.1 localhost" && !is_error)
            );
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn chat_request_tools_omitted_when_empty() {
    let req = ChatRequest {
        model: "test".to_string(),
        max_tokens: 100,
        messages: vec![],
        system: None,
        stream: true,
        tools: vec![],
        thinking: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(!json.contains("tools"));
    assert!(!json.contains("thinking"));
}

#[test]
fn chat_request_tools_present_when_nonempty() {
    let req = ChatRequest {
        model: "test".to_string(),
        max_tokens: 100,
        messages: vec![],
        system: None,
        stream: true,
        tools: vec![ToolDef {
            name: "read_file".to_string(),
            description: Some("read a file".to_string()),
            input_schema: serde_json::json!({"type": "object"}),
        }],
        thinking: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"tools\""));
    assert!(json.contains("\"read_file\""));
}

#[test]
fn chat_request_thinking_omitted_when_none() {
    let req = ChatRequest {
        model: "claude-opus-4-7".to_string(),
        max_tokens: 16384,
        messages: vec![],
        system: None,
        stream: true,
        tools: vec![],
        thinking: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(!json.contains("thinking"));
}

#[test]
fn chat_request_thinking_adaptive_serializes_correctly() {
    let req = ChatRequest {
        model: "claude-opus-4-7".to_string(),
        max_tokens: 16384,
        messages: vec![],
        system: None,
        stream: true,
        tools: vec![],
        thinking: Some(ThinkingConfig::Adaptive {
            display: "summarized".to_string(),
            effort: "high".to_string(),
        }),
    };
    let json = serde_json::to_string(&req).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let t = v.get("thinking").expect("thinking key present");
    assert_eq!(t.get("type").and_then(|s| s.as_str()), Some("adaptive"));
    assert_eq!(
        t.get("display").and_then(|s| s.as_str()),
        Some("summarized")
    );
    assert_eq!(t.get("effort").and_then(|s| s.as_str()), Some("high"));
}

#[test]
fn message_content_to_text() {
    let blocks = vec![
        ContentBlock::Text {
            text: "hello ".to_string(),
            cache_control: None,
        },
        ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({}),
        },
        ContentBlock::Text {
            text: "world".to_string(),
            cache_control: None,
        },
    ];
    let mc = MessageContent::Blocks(blocks);
    assert_eq!(mc.to_text(), "hello world");
}

// -----------------------------------------------------------------------
// Thinking block round-trips
// -----------------------------------------------------------------------

#[test]
fn thinking_block_serializes_to_anthropic_schema() {
    let block = ContentBlock::Thinking {
        thinking: "let me think...".to_string(),
        signature: "EqQBCgIYAhIM1gbcDa...".to_string(),
    };
    let json = serde_json::to_string(&block).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v.get("type").and_then(|s| s.as_str()), Some("thinking"));
    assert_eq!(
        v.get("thinking").and_then(|s| s.as_str()),
        Some("let me think...")
    );
    assert_eq!(
        v.get("signature").and_then(|s| s.as_str()),
        Some("EqQBCgIYAhIM1gbcDa...")
    );
}

#[test]
fn thinking_block_deserializes_from_anthropic_schema() {
    let json = r#"{"type":"thinking","thinking":"analyze GCD","signature":"abc123"}"#;
    let block: ContentBlock = serde_json::from_str(json).unwrap();
    assert_eq!(
        block,
        ContentBlock::Thinking {
            thinking: "analyze GCD".to_string(),
            signature: "abc123".to_string(),
        }
    );
}

#[test]
fn redacted_thinking_block_serializes_to_anthropic_schema() {
    let block = ContentBlock::RedactedThinking {
        data: "EmwKAhIM1gbcDa9GJwZA".to_string(),
    };
    let json = serde_json::to_string(&block).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v.get("type").and_then(|s| s.as_str()),
        Some("redacted_thinking")
    );
    assert_eq!(
        v.get("data").and_then(|s| s.as_str()),
        Some("EmwKAhIM1gbcDa9GJwZA")
    );
}

#[test]
fn redacted_thinking_block_deserializes_from_anthropic_schema() {
    let json = r#"{"type":"redacted_thinking","data":"EmwKAhIM1gbcDa9GJwZA"}"#;
    let block: ContentBlock = serde_json::from_str(json).unwrap();
    assert_eq!(
        block,
        ContentBlock::RedactedThinking {
            data: "EmwKAhIM1gbcDa9GJwZA".to_string(),
        }
    );
}

#[test]
fn chat_message_round_trip_with_thinking_blocks() {
    // The full assistant turn: text + thinking + tool_use. Must round-trip
    // losslessly so the LLM gets the exact signature back on the next
    // turn (otherwise it 400s).
    let json = r#"{"role":"assistant","content":[
        {"type":"thinking","thinking":"need to read the file","signature":"sig_abc"},
        {"type":"text","text":"OK, reading now"},
        {"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"/etc/hosts"}}
    ]}"#;
    let msg: ChatMessage = serde_json::from_str(json).unwrap();
    // Re-serialize and re-parse: must produce the same blocks.
    let re = serde_json::to_string(&msg).unwrap();
    let msg2: ChatMessage = serde_json::from_str(&re).unwrap();
    assert_eq!(msg, msg2);

    match &msg2.content {
        MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 3);
            assert!(
                matches!(&blocks[0], ContentBlock::Thinking { thinking, signature }
                if thinking == "need to read the file" && signature == "sig_abc")
            );
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn chat_message_round_trip_with_redacted_thinking() {
    let json = r#"{"role":"assistant","content":[
        {"type":"redacted_thinking","data":"EmwKAhIM1gbcDa9GJwZA"},
        {"type":"text","text":"answer"}
    ]}"#;
    let msg: ChatMessage = serde_json::from_str(json).unwrap();
    let re = serde_json::to_string(&msg).unwrap();
    let msg2: ChatMessage = serde_json::from_str(&re).unwrap();
    assert_eq!(msg, msg2);
}

#[test]
fn message_content_to_text_excludes_thinking() {
    // Thinking text must NOT leak into the denormalized `text` column
    // (DB text is used for sidebar previews / search).
    let blocks = vec![
        ContentBlock::Thinking {
            thinking: "secret thought".to_string(),
            signature: "sig".to_string(),
        },
        ContentBlock::Text {
            text: "visible answer".to_string(),
            cache_control: None,
        },
        ContentBlock::RedactedThinking {
            data: "redacted".to_string(),
        },
    ];
    let mc = MessageContent::Blocks(blocks);
    assert_eq!(mc.to_text(), "visible answer");
}

#[test]
fn chat_event_thinking_delta_serializes_with_snake_case_kind() {
    let ev = ChatEvent::ThinkingDelta {
        text: "analyzing".to_string(),
    };
    let json = serde_json::to_string(&ev).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v.get("kind").and_then(|s| s.as_str()),
        Some("thinking_delta")
    );
    assert_eq!(v.get("text").and_then(|s| s.as_str()), Some("analyzing"));
}

#[test]
fn chat_event_signature_delta_serializes_with_snake_case_kind() {
    let ev = ChatEvent::SignatureDelta {
        signature: "sig_xyz".to_string(),
    };
    let json = serde_json::to_string(&ev).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v.get("kind").and_then(|s| s.as_str()),
        Some("signature_delta")
    );
    assert_eq!(v.get("signature").and_then(|s| s.as_str()), Some("sig_xyz"));
}

#[test]
fn chat_event_redacted_thinking_delta_serializes_with_snake_case_kind() {
    let ev = ChatEvent::RedactedThinkingDelta {
        data: "redacted_blob".to_string(),
    };
    let json = serde_json::to_string(&ev).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v.get("kind").and_then(|s| s.as_str()),
        Some("redacted_thinking_delta")
    );
    assert_eq!(
        v.get("data").and_then(|s| s.as_str()),
        Some("redacted_blob")
    );
}

// -----------------------------------------------------------------------
// A4 — TokenUsage
// -----------------------------------------------------------------------

#[test]
fn token_usage_serializes_with_snake_case_fields() {
    // The IPC payload crosses the Tauri boundary camelCase by
    // default, but the inner JSON object keeps snake_case for
    // these field names (the outer `kind` discriminator and the
    // inner `stop_reason` are both snake_case in the existing
    // `ChatEvent::Done` shape — see backend/llm-contract.md
    // §"Scenario: Token Usage Tracking" §3).
    let u = TokenUsage {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_input_tokens: 10,
        cache_read_input_tokens: 20,
        context_input_tokens: 130,
    };
    let json = serde_json::to_string(&u).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v.get("input_tokens"), Some(&serde_json::json!(100)));
    assert_eq!(v.get("output_tokens"), Some(&serde_json::json!(50)));
    assert_eq!(
        v.get("cache_creation_input_tokens"),
        Some(&serde_json::json!(10))
    );
    assert_eq!(
        v.get("cache_read_input_tokens"),
        Some(&serde_json::json!(20))
    );
    // 2026-06-26 snapshot fix: the new `context_input_tokens`
    // field MUST serialize as snake_case (no rename attribute
    // on the struct, but lock the contract explicitly since
    // the field is the canonical frontend "% of context_window"
    // numerator — a rename here would silently break the UI).
    assert_eq!(v.get("context_input_tokens"), Some(&serde_json::json!(130)));
}

#[test]
fn token_usage_default_is_all_zero() {
    // The DB's `UPDATE col = col + ?` path doesn't see a
    // default-zero, but the `Done { usage: None }` -> no-op
    // path means the chat command never constructs a
    // default-zero usage. This test just locks the contract
    // that `Default::default() == TokenUsage { 0, 0, 0, 0 }`.
    let u = TokenUsage::default();
    assert_eq!(u.input_tokens, 0);
    assert_eq!(u.output_tokens, 0);
    assert_eq!(u.cache_creation_input_tokens, 0);
    assert_eq!(u.cache_read_input_tokens, 0);
    assert_eq!(u.context_input_tokens, 0);
}

#[test]
fn token_usage_deserializes_legacy_4_field_json_with_default_context() {
    // 2026-06-26 snapshot fix (PRD decision D6): legacy
    // `subagent_runs.token_usage_json` rows written before the
    // `context_input_tokens` field existed carry only the four
    // original fields. The `#[serde(default)]` attribute on
    // `context_input_tokens` MUST make this deserialize cleanly
    // (defaulting to 0) rather than erroring — otherwise a
    // single pre-snapshot worker row would break
    // `SubagentDrawer`'s expand UI on every page load.
    let legacy_json = r#"{
        "input_tokens": 100,
        "output_tokens": 50,
        "cache_creation_input_tokens": 10,
        "cache_read_input_tokens": 20
    }"#;
    let u: TokenUsage = serde_json::from_str(legacy_json)
        .expect("legacy 4-field JSON must deserialize (#[serde(default)] on context_input_tokens)");
    assert_eq!(u.input_tokens, 100);
    assert_eq!(u.output_tokens, 50);
    assert_eq!(u.cache_creation_input_tokens, 10);
    assert_eq!(u.cache_read_input_tokens, 20);
    assert_eq!(
        u.context_input_tokens, 0,
        "missing field defaults to 0 (not an error)"
    );
}

#[test]
fn chat_event_done_carries_usage_payload() {
    // The A4 wire shape: `Done { stop_reason, usage }`. The
    // `usage` field is serialized with the inner `kind` tag
    // already supplied by the outer enum, so the payload
    // looks like:
    //
    //   { "kind": "done", "stop_reason": "end_turn",
    //     "usage": { "input_tokens": 100, ... } }
    //
    // when `Some`, and `usage: null` (or absent — we use
    // `skip_serializing_if` below for compact payloads) when
    // `None`. The agent loop checks `Some(t) => accumulate`,
    // `None => skip`.
    let ev = ChatEvent::Done {
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 25,
            context_input_tokens: 125,
        }),
    };
    let json = serde_json::to_string(&ev).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v.get("kind").and_then(|s| s.as_str()), Some("done"));
    assert_eq!(
        v.get("stop_reason").and_then(|s| s.as_str()),
        Some("end_turn")
    );
    let usage = v.get("usage").expect("usage key present");
    assert_eq!(usage.get("input_tokens"), Some(&serde_json::json!(100)));
    assert_eq!(
        usage.get("cache_read_input_tokens"),
        Some(&serde_json::json!(25))
    );
}

#[test]
fn chat_event_done_with_none_usage_emits_null() {
    // Cancel / error / network drop path: usage is None.
    // The agent loop's `if let Some(t) = event.usage` check
    // skips accumulation, so the None case must be
    // distinguishable from `Some(TokenUsage::default())`
    // (which would otherwise be a no-op write, but it's
    // wasteful — we should be able to skip the SQL
    // round-trip).
    let ev = ChatEvent::Done {
        stop_reason: Some("cancelled".to_string()),
        usage: None,
    };
    let json = serde_json::to_string(&ev).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v.get("kind").and_then(|s| s.as_str()), Some("done"));
    // `usage` is present as JSON null (not absent) so the
    // frontend's TypeScript side can rely on the key being
    // there. (`serde(tag = "kind")` does not skip None
    // fields by default.)
    assert!(v.get("usage").map(|x| x.is_null()).unwrap_or(false));
}

// -----------------------------------------------------------------------
// B2 PR3: InjectionRecord / ChatEvent::FileInjections wire shape
// -----------------------------------------------------------------------

/// Verify `InjectionRecord` serializes with the exact shape the
/// frontend's `InjectionEntry` discriminated union expects:
/// `{ path: string, action: { kind: 'injected'|'degraded'|'skipped', ... } }`.
/// The metadata-persist path serializes via `serde_json::Value`
/// and the rehydrate path decodes back into `InjectionRecord` —
/// round-trip through both `String` and `Value` is verified.
#[test]
fn b2_pr3_injection_record_wire_shape() {
    use crate::agent::at_file::{FileKind, InjectionAction, InjectionRecord, SkipReason};
    let records = vec![
        InjectionRecord {
            path: "src/foo.ts".to_string(),
            action: InjectionAction::Injected { lines: 48 },
        },
        InjectionRecord {
            path: "bar.png".to_string(),
            action: InjectionAction::Degraded {
                file_kind: FileKind::Image,
            },
        },
        InjectionRecord {
            path: "doc.pdf".to_string(),
            action: InjectionAction::Degraded {
                file_kind: FileKind::Pdf,
            },
        },
        InjectionRecord {
            path: "doc.docx".to_string(),
            action: InjectionAction::Degraded {
                file_kind: FileKind::Office,
            },
        },
        InjectionRecord {
            path: "x.zip".to_string(),
            action: InjectionAction::Degraded {
                file_kind: FileKind::Binary,
            },
        },
        InjectionRecord {
            path: "missing.txt".to_string(),
            action: InjectionAction::Skipped {
                reason: SkipReason::Missing,
            },
        },
        InjectionRecord {
            path: "../../etc/passwd".to_string(),
            action: InjectionAction::Skipped {
                reason: SkipReason::OutOfRoot,
            },
        },
        InjectionRecord {
            path: "/etc/shadow".to_string(),
            action: InjectionAction::Skipped {
                reason: SkipReason::Unreadable,
            },
        },
    ];
    let json = serde_json::to_string(&records).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let arr = v.as_array().unwrap();
    // Injected: kind=injected, has `lines`.
    assert_eq!(arr[0]["path"], "src/foo.ts");
    assert_eq!(arr[0]["action"]["kind"], "injected");
    assert_eq!(arr[0]["action"]["lines"], 48);
    // Degraded: kind=degraded, has `file_kind` (snake_case enum).
    assert_eq!(arr[1]["action"]["kind"], "degraded");
    assert_eq!(arr[1]["action"]["file_kind"], "image");
    assert_eq!(arr[2]["action"]["file_kind"], "pdf");
    assert_eq!(arr[3]["action"]["file_kind"], "office");
    assert_eq!(arr[4]["action"]["file_kind"], "binary");
    // Skipped: kind=skipped, has `reason` (snake_case enum).
    assert_eq!(arr[5]["action"]["kind"], "skipped");
    assert_eq!(arr[5]["action"]["reason"], "missing");
    assert_eq!(arr[6]["action"]["reason"], "out_of_root");
    assert_eq!(arr[7]["action"]["reason"], "unreadable");
    // Round-trip via `String` (the IPC JSON path).
    let decoded: Vec<InjectionRecord> = serde_json::from_str(&json).unwrap();
    assert_eq!(records, decoded);
    // Round-trip via `serde_json::Value` (the
    // `update_message_metadata` persist path: `to_value`
    // → `Value::String` for the SQL column → `from_str` on
    // reload).
    let meta = serde_json::to_value(&records).unwrap();
    let meta_back: Vec<InjectionRecord> = serde_json::from_value(meta).unwrap();
    assert_eq!(records, meta_back);
}

/// Verify `ChatEvent::FileInjections` wire shape — the frontend
/// `case "file_injections"` arm reads `event.message_seq` and
/// `event.injections` off the IPC payload, then `msgs.find`
/// patches the user message's `injections` array.
#[test]
fn b2_pr3_chat_event_file_injections_wire_shape() {
    use crate::agent::at_file::{InjectionAction, InjectionRecord};
    let ev = ChatEvent::FileInjections {
        request_id: "rid123".to_string(),
        message_seq: 42,
        injections: vec![InjectionRecord {
            path: "foo.txt".to_string(),
            action: InjectionAction::Injected { lines: 12 },
        }],
    };
    let json = serde_json::to_string(&ev).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    // The `kind` discriminator is snake_case from the enum tag.
    assert_eq!(v["kind"], "file_injections");
    // The other 3 fields are top-level on the JSON object.
    assert_eq!(v["request_id"], "rid123");
    assert_eq!(v["message_seq"], 42);
    assert_eq!(v["injections"][0]["path"], "foo.txt");
    assert_eq!(v["injections"][0]["action"]["kind"], "injected");
    assert_eq!(v["injections"][0]["action"]["lines"], 12);
}

/// A5+ (2026-07-04, R8): verify `ChatEvent::Retrying` wire shape.
/// The frontend `streamController` `case 'retrying'` arm reads
/// `attempt` / `max_attempts` / `wait_ms` / `reason` off the IPC
/// payload. The `kind` discriminator is snake_case (`retrying`)
/// from the enum-level `#[serde(rename_all = "snake_case")]` tag.
#[test]
fn a5plus_chat_event_retrying_wire_shape() {
    let ev = ChatEvent::Retrying {
        attempt: 2,
        max_attempts: 3,
        wait_ms: 1500,
        reason: "服务器错误 (HTTP 503)".to_string(),
    };
    let json = serde_json::to_string(&ev).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["kind"], "retrying");
    assert_eq!(v["attempt"], 2);
    assert_eq!(v["max_attempts"], 3);
    assert_eq!(v["wait_ms"], 1500);
    assert_eq!(v["reason"], "服务器错误 (HTTP 503)");
}

// ---------------------------------------------------------------------------
// 08-21-b1-image-followups R4: ToolResult serde 三分支(手动 serde 的
// 硬闸 —— 无图路径必须与历史 derive 输出逐字节一致)。
// ---------------------------------------------------------------------------

#[test]
fn tool_result_plain_serializes_byte_identical() {
    let block = ContentBlock::ToolResult {
        tool_use_id: "toolu_123".to_string(),
        content: "127.0.0.1 localhost".to_string(),
        is_error: false,
        images: None,
        resolved: None,
    };
    assert_eq!(
        serde_json::to_string(&block).unwrap(),
        r#"{"type":"tool_result","tool_use_id":"toolu_123","content":"127.0.0.1 localhost"}"#
    );
    // is_error: true 必须出现(skip 过滤只跳 false)。
    let err_block = ContentBlock::ToolResult {
        tool_use_id: "t".to_string(),
        content: "boom".to_string(),
        is_error: true,
        images: None,
        resolved: None,
    };
    let s = serde_json::to_string(&err_block).unwrap();
    assert!(s.contains(r#""is_error":true"#), "{s}");
}

#[test]
fn tool_result_images_refs_serialize_without_base64() {
    let block = ContentBlock::ToolResult {
        tool_use_id: "t".to_string(),
        content: "[image: shot.png]".to_string(),
        is_error: false,
        images: Some(vec![crate::llm::types::AttachmentRef {
            file: "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6.png".to_string(),
            media_type: "image/png".to_string(),
            source: "read_file".to_string(),
            tokens_est: Some(640),
        }]),
        resolved: None,
    };
    let s = serde_json::to_string(&block).unwrap();
    assert!(
        s.contains(r#""images":[{"file":"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6.png""#),
        "{s}"
    );
    assert!(s.contains(r#""tokens_est":640"#), "{s}");
    assert!(!s.contains("base64"), "DB 形态永不携带 base64: {s}");
    // 旧 DB 行(无 images 字段)反序列化兼容。
    let old: ContentBlock =
        serde_json::from_str(r#"{"type":"tool_result","tool_use_id":"t","content":"ok"}"#).unwrap();
    assert!(matches!(
        &old,
        ContentBlock::ToolResult {
            images: None,
            resolved: None,
            ..
        }
    ));
    // 带 images 的行 round-trip。
    let rt: ContentBlock = serde_json::from_str(&s).unwrap();
    assert_eq!(rt, block);
}

#[test]
fn tool_result_resolved_serializes_content_block_array() {
    let block = ContentBlock::ToolResult {
        tool_use_id: "t".to_string(),
        content: "[image: shot.png — sent]".to_string(),
        is_error: false,
        images: None,
        resolved: Some(vec![crate::llm::types::ImageSource {
            source_type: "base64".to_string(),
            media_type: "image/png".to_string(),
            data: "aGVsbG8=".to_string(),
        }]),
    };
    let v: serde_json::Value = serde_json::to_value(&block).unwrap();
    // content 是 block array(image 在前、text 在后),且不出现 images 字段。
    // (serde_json Map 键序不定,断言走结构不走走 substring。)
    let arr = v["content"].as_array().expect("content must be an array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["type"], "image");
    assert_eq!(arr[0]["source"]["type"], "base64");
    assert_eq!(arr[0]["source"]["data"], "aGVsbG8=");
    assert_eq!(arr[1]["type"], "text");
    assert_eq!(arr[1]["text"], "[image: shot.png — sent]");
    assert!(v.get("images").is_none(), "HTTP 形态不得输出 refs 字段");
    let s = serde_json::to_string(&block).unwrap();
    // array 形态反序列化对称(仅测试摄入)。
    let rt: ContentBlock = serde_json::from_str(&s).unwrap();
    match rt {
        ContentBlock::ToolResult {
            resolved,
            images,
            content,
            ..
        } => {
            assert_eq!(images, None);
            assert_eq!(content, "[image: shot.png — sent]");
            let imgs = resolved.expect("array form must parse into resolved");
            assert_eq!(imgs.len(), 1);
            assert_eq!(imgs[0].data, "aGVsbG8=");
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn content_block_other_variants_round_trip_after_manual_serde() {
    // 手动 serde 改造的回归锁:非 ToolResult 变体的 JSON 形状不变。
    let cases = vec![
        (
            ContentBlock::Text {
                text: "hi".into(),
                cache_control: None,
            },
            r#"{"type":"text","text":"hi"}"#,
        ),
        (
            ContentBlock::Text {
                text: "hi".into(),
                cache_control: Some(crate::llm::types::CacheControl::Ephemeral),
            },
            r#"{"type":"text","text":"hi","cache_control":{"type":"ephemeral"}}"#,
        ),
        (
            ContentBlock::ImageRef {
                file: "a.png".into(),
                media_type: "image/png".into(),
            },
            r#"{"type":"image_ref","file":"a.png","media_type":"image/png"}"#,
        ),
    ];
    for (block, expect) in cases {
        assert_eq!(serde_json::to_string(&block).unwrap(), expect);
        let rt: ContentBlock = serde_json::from_str(expect).unwrap();
        assert_eq!(rt, block);
    }
}
