//! SubagentBufferSink 单元测试(迁移自 sink.rs 内联 mod tests,
//! 08-08-a-class-sink-split)。

#![cfg(test)]

// 顶部 import 供 `mod tests` 的 `use super::*` 使用,lib 构建下视为未用
#[allow(unused_imports)]
use super::sink::*;
#[allow(unused_imports)]
use super::transcript::{TranscriptEntry, TranscriptKind};
#[allow(unused_imports)]
use crate::agent::permissions::PermissionAskPayload;
#[allow(unused_imports)]
use crate::llm::types::{ChatEvent, TokenUsage};
#[allow(unused_imports)]
use crate::state::{ChatEventPayload, ToolCallPayload, ToolResultPayload};
#[allow(unused_imports)]
use std::sync::Arc;
#[allow(unused_imports)]
use std::sync::Mutex as StdMutex;

mod tests {
    use super::*;
    use crate::state::ChatEventSink;

    // ---- helpers ----

    fn done_with_usage(input: u32, output: u32) -> ChatEventPayload {
        ChatEventPayload {
            request_id: "rid-u".to_string(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("end_turn".to_string()),
                usage: Some(TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    context_input_tokens: input,
                }),
            },
        }
    }

    fn sink_with_resolved(rid: &str, outcome: &str) -> Vec<TranscriptEntry> {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_permission_ask_resolved(rid, outcome);
        sink.transcript_snapshot()
    }

    // ---- basic sink behavior ----

    #[test]
    fn buffer_sink_accumulates_text_deltas() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        let rid = "rid-test".to_string();
        for t in ["hello", " ", "world"] {
            sink.emit_chat_event(&ChatEventPayload {
                request_id: rid.clone(),
                session_id: "sess-test".into(),
                event: ChatEvent::Delta {
                    text: t.to_string(),
                },
            });
        }
        assert_eq!(sink.final_text(), "hello world");
    }

    #[test]
    fn buffer_sink_tracks_cancelled_done() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        let rid = "rid-cancel".to_string();
        sink.emit_chat_event(&ChatEventPayload {
            request_id: rid.clone(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        assert!(sink.was_cancelled());
        assert!(!sink.had_error());
    }

    #[test]
    fn buffer_sink_tracks_error_event() {
        use crate::llm::LlmErrorCategory;
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        let rid = "rid-err".to_string();
        sink.emit_chat_event(&ChatEventPayload {
            request_id: rid.clone(),
            session_id: "sess-test".into(),
            event: ChatEvent::Error {
                message: "boom".to_string(),
                category: LlmErrorCategory::Server,
            },
        });
        assert!(sink.had_error());
        assert!(!sink.was_cancelled());
    }

    #[test]
    fn buffer_sink_records_transcript_entries() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        let rid = "rid-transcript".to_string();
        sink.emit_chat_event(&ChatEventPayload {
            request_id: rid.clone(),
            session_id: "sess-test".into(),
            event: ChatEvent::Start,
        });
        sink.emit_tool_call(&ToolCallPayload {
            request_id: rid.clone(),
            session_id: "sess-test".into(),
            id: "toolu_1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "/x"}),
        });
        sink.emit_tool_result(&ToolResultPayload {
            request_id: rid,
            session_id: "sess-test".into(),
            tool_use_id: "toolu_1".to_string(),
            content: "ok".to_string(),
            is_error: false,
            images: None,
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 3);
        assert_eq!(transcript[0].kind, TranscriptKind::ChatEvent);
        assert_eq!(transcript[1].kind, TranscriptKind::ToolCall);
        assert_eq!(transcript[2].kind, TranscriptKind::ToolResult);
    }

    // ---- token usage accumulation (B6 PR2) ----

    /// 08-20-turn-usage-event-quota-view WP1:`TurnUsage` 是 trace 观测
    /// 非对话事件 —— worker transcript 与 `subagent:event` 均不记录
    /// (持久记录走 turn_trace run 行的 Done 臂 upsert,不经本 sink)。
    /// 锁住早退契约,防回归成"每轮一条无人渲染的 transcript 行"
    /// (会破 `persists_subagent_run` 的 transcript 计数断言)。
    #[test]
    fn buffer_sink_skips_turn_usage_transcript_record() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid-u".into(),
            session_id: "sess-test".into(),
            event: ChatEvent::TurnUsage {
                request_id: "rid-u".into(),
                seq: 1,
                run_id: "run-1".into(),
                usage: TokenUsage::default(),
                tools_token: None,
                memory_token: None,
                images_token: None,
                at_files_token: None,
                system_token: None,
                context_window: 200_000,
            },
        });
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid-u".into(),
            session_id: "sess-test".into(),
            event: ChatEvent::Start,
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(
            transcript.len(),
            1,
            "TurnUsage must not enter the worker transcript"
        );
        assert_eq!(transcript[0].kind, TranscriptKind::ChatEvent);
    }

    #[test]
    fn buffer_sink_accumulates_token_usage_per_turn() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&done_with_usage(50, 10));
        let total = sink.cumulative_usage();
        assert_eq!(total.input_tokens, 350);
        assert_eq!(total.output_tokens, 90);
    }

    #[test]
    fn buffer_sink_drain_per_turn_usage_clears_buffer() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(10, 5));
        let drained = sink.drain_per_turn_usage();
        assert_eq!(drained.input_tokens, 10);
        assert_eq!(drained.output_tokens, 5);
        // After drain, the cumulative is zero.
        let after = sink.cumulative_usage();
        assert_eq!(after.input_tokens, 0);
        assert_eq!(after.output_tokens, 0);
    }

    #[test]
    fn buffer_sink_done_without_usage_does_not_accumulate() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        let total = sink.cumulative_usage();
        assert_eq!(total.input_tokens, 0);
        assert_eq!(total.output_tokens, 0);
    }

    // ---- R3 (2026-06-21) max_turns terminal-patch regression tests ----

    /// R3 regression: the synthetic terminal `Done{max_turns, usage:
    /// last_usage}` must NOT double-count the last turn (the guard
    /// skips the push for synthetic terminals).
    #[test]
    fn buffer_sink_max_turns_terminal_does_not_double_count_last_turn() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&done_with_usage(50, 10));
        let t_last = TokenUsage {
            input_tokens: 50,
            output_tokens: 10,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            context_input_tokens: 50,
        };
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("max_turns".to_string()),
                usage: Some(t_last),
            },
        });
        let total = sink.cumulative_usage();
        assert_eq!(
            total.input_tokens, 350,
            "cumulative input = 100+200+50 (synthetic terminal must not double-count)"
        );
        assert_eq!(
            total.output_tokens, 90,
            "cumulative output = 50+30+10 (synthetic terminal must not double-count)"
        );
    }

    /// R3 mirror: cancelled synthetic terminal must NOT affect
    /// cumulative_usage().
    #[test]
    fn buffer_sink_cancelled_terminal_does_not_affect_cumulative_usage() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        let total = sink.cumulative_usage();
        assert_eq!(total.input_tokens, 300);
        assert_eq!(total.output_tokens, 80);
    }

    /// RULE-FrontSubagent-004: `turns_completed()` increments once
    /// per REAL per-turn Done (synthetic terminals do NOT bump).
    #[test]
    fn buffer_sink_turns_completed_tracks_real_per_turn_dones() {
        // (a) Clean end_turn: 3 per-turn Dones → turns_completed == 3.
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&done_with_usage(50, 10));
        assert_eq!(
            sink.turns_completed(),
            3,
            "3 real per-turn Dones → counter == 3"
        );

        // (b) Cancelled: 2 per-turn Dones + 1 synthetic cancelled.
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        assert_eq!(
            sink.turns_completed(),
            2,
            "cancelled synthetic terminal must NOT increment"
        );

        // (c) max_turns: 200 per-turn Dones + 1 synthetic max_turns.
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        for _ in 0..200 {
            sink.emit_chat_event(&done_with_usage(100, 50));
        }
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("max_turns".to_string()),
                usage: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    context_input_tokens: 100,
                }),
            },
        });
        assert_eq!(
            sink.turns_completed(),
            200,
            "max_turns synthetic terminal must NOT increment (counter == real turn budget)"
        );
    }

    /// RULE-FrontSubagent-004: turns_completed() and per_turn_usage
    /// stay 1:1 (same discriminator guards both).
    #[test]
    fn buffer_sink_turns_completed_equals_per_turn_usage_len() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&done_with_usage(50, 10));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        assert_eq!(sink.turns_completed(), 3);
        let total = sink.cumulative_usage();
        assert_eq!(total.input_tokens, 350);
        assert_eq!(total.output_tokens, 90);
    }

    /// R3 was_incomplete: set on synthetic `Done{max_turns}`.
    #[test]
    fn buffer_sink_max_turns_terminal_sets_was_incomplete() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("max_turns".to_string()),
                usage: None,
            },
        });
        assert!(
            sink.was_incomplete(),
            "max_turns terminal must set was_incomplete=true"
        );
        assert!(
            !sink.was_cancelled(),
            "max_turns must NOT also set was_cancelled"
        );
        assert!(!sink.had_error(), "max_turns must NOT also set had_error");
    }

    /// R3 was_cancelled: set on synthetic `Done{cancelled}`.
    #[test]
    fn buffer_sink_cancelled_terminal_sets_was_cancelled_only() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        assert!(
            sink.was_cancelled(),
            "cancelled terminal must set was_cancelled=true"
        );
        assert!(
            !sink.was_incomplete(),
            "cancelled must NOT also set was_incomplete"
        );
    }

    /// R3: clean `end_turn` exit sets neither flag.
    #[test]
    fn buffer_sink_end_turn_terminal_does_not_set_incomplete_or_cancelled() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            session_id: "sess-test".into(),
            event: ChatEvent::Done {
                stop_reason: Some("end_turn".to_string()),
                usage: None,
            },
        });
        assert!(
            !sink.was_incomplete(),
            "end_turn terminal must NOT set was_incomplete"
        );
        assert!(
            !sink.was_cancelled(),
            "end_turn terminal must NOT set was_cancelled"
        );
    }

    // ---- PR2 hotfix: subagent:event IPC payload ----

    /// Each `emit_*` appends a transcript entry AND (when armed via
    /// `new_with_collector`) the matching IPC payload.
    #[test]
    fn subagent_buffer_sink_emits_ipc_event_per_emit() {
        crate::agent::subagent::clear_test_collector();
        let collector: Arc<StdMutex<Vec<serde_json::Value>>> = Arc::new(StdMutex::new(Vec::new()));
        let sink = SubagentBufferSink::new_with_collector(
            "rid-pr2".into(),
            "sid-pr2".into(),
            collector.clone(),
        );

        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid-pr2".into(),
            session_id: "sess-test".into(),
            event: ChatEvent::Start,
        });
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid-pr2".into(),
            session_id: "sess-test".into(),
            id: "toolu_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/x"}),
        });
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid-pr2".into(),
            session_id: "sess-test".into(),
            tool_use_id: "toolu_1".into(),
            content: "ok".into(),
            is_error: false,
            images: None,
        });
        sink.emit_permission_ask(crate::agent::permissions::PermissionAskPayload {
            rid: "ask-rid".into(),
            session_id: "sid-pr2".into(),
            tool_use_id: "toolu_1".into(),
            tool_name: "shell".into(),
            tool_input: serde_json::json!({"command": "rm -rf /"}),
            risk: crate::agent::permissions::Risk::High,
            reason: Some("dangerous".into()),
            path: None,
            worker_run_id: None,
        });

        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 4);
        assert_eq!(transcript[0].kind, TranscriptKind::ChatEvent);
        assert_eq!(transcript[1].kind, TranscriptKind::ToolCall);
        assert_eq!(transcript[2].kind, TranscriptKind::ToolResult);
        assert_eq!(transcript[3].kind, TranscriptKind::PermissionAsk);

        let collected = collector.lock().unwrap().clone();
        assert_eq!(collected.len(), 4, "every emit must produce 1 IPC payload");
        assert_eq!(collected[0]["kind"], "chat_event");
        assert_eq!(collected[1]["kind"], "tool_call");
        assert_eq!(collected[2]["kind"], "tool_result");
        assert_eq!(collected[3]["kind"], "permission_ask");
        for (i, p) in collected.iter().enumerate() {
            assert_eq!(p["runId"], "rid-pr2", "payload #{i} runId");
            assert_eq!(p["sessionId"], "sid-pr2", "payload #{i} sessionId");
            assert!(
                p["payload"].is_object() || p["payload"].is_null(),
                "payload #{i} shape"
            );
            assert!(
                p["timestamp"].as_str().unwrap().contains('T'),
                "payload #{i} timestamp is RFC 3339"
            );
        }

        crate::agent::subagent::clear_test_collector();
    }

    /// `new_without_app_handle` does NOT emit IPC events.
    #[test]
    fn subagent_buffer_sink_without_app_handle_does_not_emit_ipc() {
        crate::agent::subagent::clear_test_collector();
        let sink = SubagentBufferSink::new_without_app_handle("rid-noop".into(), "sid-noop".into());
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid-noop".into(),
            session_id: "sess-test".into(),
            event: ChatEvent::Start,
        });
        assert_eq!(sink.transcript_snapshot().len(), 1);
        TEST_COLLECTOR.with(|c| {
            assert!(
                c.borrow().is_none(),
                "no collector armed → no IPC attempted"
            );
        });
    }

    // ---- B6 PR3 redesign: tool_use_id + duration_ms payload fields ----

    #[test]
    fn tool_call_payload_json_includes_tool_use_id() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid".into(),
            session_id: "sess-test".into(),
            id: "toolu_42".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/foo"}),
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 1);
        let entry = &transcript[0];
        assert_eq!(entry.kind, TranscriptKind::ToolCall);
        let pj = entry
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("tool_use_id").and_then(|v| v.as_str()),
            Some("toolu_42"),
            "tool_call payload_json must carry top-level tool_use_id"
        );
        assert_eq!(
            pj.get("id").and_then(|v| v.as_str()),
            Some("toolu_42"),
            "original `id` field preserved"
        );
        assert_eq!(pj.get("name").and_then(|v| v.as_str()), Some("read_file"));
        assert!(pj.get("input").is_some(), "input preserved");
    }

    #[test]
    fn tool_result_payload_json_includes_duration_ms() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid".into(),
            session_id: "sess-test".into(),
            id: "toolu_p".into(),
            name: "shell".into(),
            input: serde_json::json!({"command": "ls"}),
        });
        std::thread::sleep(std::time::Duration::from_millis(5));
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid".into(),
            session_id: "sess-test".into(),
            tool_use_id: "toolu_p".into(),
            content: "ok".into(),
            is_error: false,
            images: None,
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 2);
        let result_entry = &transcript[1];
        assert_eq!(result_entry.kind, TranscriptKind::ToolResult);
        let pj = result_entry
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("tool_use_id").and_then(|v| v.as_str()),
            Some("toolu_p"),
            "tool_result payload_json carries top-level tool_use_id"
        );
        let duration = pj
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .expect("duration_ms is u64");
        assert!(
            duration >= 4,
            "duration_ms must reflect wall-clock gap, got {duration}"
        );
        assert!(
            duration < 5_000,
            "duration_ms unreasonably large: {duration}"
        );
        assert_eq!(pj.get("content").and_then(|v| v.as_str()), Some("ok"));
        assert_eq!(pj.get("is_error").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn orphan_tool_result_gets_duration_ms_zero() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid".into(),
            session_id: "sess-test".into(),
            tool_use_id: "toolu_orphan".into(),
            content: "partial".into(),
            is_error: false,
            images: None,
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 1);
        let pj = transcript[0]
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("tool_use_id").and_then(|v| v.as_str()),
            Some("toolu_orphan"),
        );
        assert_eq!(
            pj.get("duration_ms").and_then(|v| v.as_u64()),
            Some(0),
            "orphan tool_result must have duration_ms=0"
        );
    }

    #[test]
    fn consecutive_pairs_get_independent_durations() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid".into(),
            session_id: "sess-test".into(),
            id: "toolu_a".into(),
            name: "read_file".into(),
            input: serde_json::json!({}),
        });
        std::thread::sleep(std::time::Duration::from_millis(2));
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid".into(),
            session_id: "sess-test".into(),
            tool_use_id: "toolu_a".into(),
            content: "a".into(),
            is_error: false,
            images: None,
        });
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid".into(),
            session_id: "sess-test".into(),
            id: "toolu_b".into(),
            name: "read_file".into(),
            input: serde_json::json!({}),
        });
        std::thread::sleep(std::time::Duration::from_millis(8));
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid".into(),
            session_id: "sess-test".into(),
            tool_use_id: "toolu_b".into(),
            content: "b".into(),
            is_error: false,
            images: None,
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 4);
        let dur_a = transcript[1]
            .payload_json
            .as_object()
            .unwrap()
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap();
        let dur_b = transcript[3]
            .payload_json
            .as_object()
            .unwrap()
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert!(
            dur_b >= dur_a,
            "second pair ({dur_b}ms) should be at least as long as first ({dur_a}ms)"
        );
        assert!(dur_a >= 1, "dur_a < 1ms is implausible, got {dur_a}");
        assert!(
            dur_b >= 4,
            "dur_b < 4ms is implausible (we slept 8ms), got {dur_b}"
        );
    }

    // ---- emit_permission_ask (RULE-FrontSubagent-003) ----

    /// PR1.5: `emit_permission_ask` produces a PermissionAsk
    /// transcript entry whose payload carries the PARENT session id.
    #[test]
    fn emit_permission_ask_populates_transcript_with_parent_session_id() {
        let sink = SubagentBufferSink::new_without_app_handle(
            "worker-rid-1".into(),
            "parent-sess-1".into(),
        );
        sink.emit_permission_ask(crate::agent::permissions::PermissionAskPayload {
            rid: "ask-rid-1".into(),
            session_id: "parent-sess-1".into(),
            tool_use_id: "toolu_w1".into(),
            tool_name: "write_file".into(),
            tool_input: serde_json::json!({"path": "/repo/outside/foo.rs"}),
            risk: crate::agent::permissions::Risk::High,
            reason: Some("requires confirmation".into()),
            path: Some("/repo/outside/foo.rs".into()),
            worker_run_id: Some("worker-run-1".into()),
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(
            transcript.len(),
            1,
            "emit_permission_ask must produce exactly 1 transcript entry"
        );
        let entry = &transcript[0];
        assert_eq!(entry.kind, TranscriptKind::PermissionAsk);
        let pj = entry
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("sessionId").and_then(|v| v.as_str()),
            Some("parent-sess-1"),
            "transcript payload must carry parent session_id (PR1.5 cross-layer fix)"
        );
        assert_eq!(
            pj.get("workerRunId").and_then(|v| v.as_str()),
            Some("worker-run-1"),
            "transcript payload must carry workerRunId camelCase"
        );
        assert_eq!(pj.get("rid").and_then(|v| v.as_str()), Some("ask-rid-1"),);
        assert_eq!(
            pj.get("toolName").and_then(|v| v.as_str()),
            Some("write_file"),
        );
        assert_eq!(
            pj.get("toolUseId").and_then(|v| v.as_str()),
            Some("toolu_w1"),
        );
    }

    // ---- emit_permission_ask_resolved (RULE-WorkerAsk-001) ----

    #[test]
    fn emit_permission_ask_resolved_allow_records_entry() {
        let transcript = sink_with_resolved("ask-rid-allow", "allow");
        assert_eq!(
            transcript.len(),
            1,
            "emit_permission_ask_resolved must produce exactly 1 transcript entry"
        );
        let entry = &transcript[0];
        assert_eq!(
            entry.kind,
            TranscriptKind::PermissionAskResolved,
            "kind must be PermissionAskResolved"
        );
        let pj = entry
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("rid").and_then(|v| v.as_str()),
            Some("ask-rid-allow"),
            "rid must match the input"
        );
        assert_eq!(
            pj.get("outcome").and_then(|v| v.as_str()),
            Some("allow"),
            "outcome must be 'allow' for AllowOnce/AllowAlways arm"
        );
    }

    #[test]
    fn emit_permission_ask_resolved_deny_records_entry() {
        let transcript = sink_with_resolved("ask-rid-deny", "deny");
        assert_eq!(transcript.len(), 1);
        let entry = &transcript[0];
        assert_eq!(entry.kind, TranscriptKind::PermissionAskResolved);
        let pj = entry
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(pj.get("rid").and_then(|v| v.as_str()), Some("ask-rid-deny"));
        assert_eq!(
            pj.get("outcome").and_then(|v| v.as_str()),
            Some("deny"),
            "outcome must be 'deny' for user-initiated Deny arm"
        );
    }

    #[test]
    fn emit_permission_ask_resolved_timeout_records_entry() {
        let transcript = sink_with_resolved("ask-rid-timeout", "timeout");
        assert_eq!(transcript.len(), 1);
        let entry = &transcript[0];
        assert_eq!(entry.kind, TranscriptKind::PermissionAskResolved);
        let pj = entry
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("rid").and_then(|v| v.as_str()),
            Some("ask-rid-timeout"),
        );
        assert_eq!(
            pj.get("outcome").and_then(|v| v.as_str()),
            Some("timeout"),
            "outcome must be 'timeout' for the 120s ASK_TIMEOUT arm"
        );
    }

    #[test]
    fn emit_permission_ask_resolved_cancel_records_entry() {
        let transcript = sink_with_resolved("ask-rid-cancel", "cancel");
        assert_eq!(transcript.len(), 1);
        let entry = &transcript[0];
        assert_eq!(entry.kind, TranscriptKind::PermissionAskResolved);
        let pj = entry
            .payload_json
            .as_object()
            .expect("payload_json is object");
        assert_eq!(
            pj.get("rid").and_then(|v| v.as_str()),
            Some("ask-rid-cancel"),
        );
        assert_eq!(
            pj.get("outcome").and_then(|v| v.as_str()),
            Some("cancel"),
            "outcome must be 'cancel' for parent-token cancel arm"
        );
    }

    /// The trait default is a no-op for sinks that do NOT override it.
    #[test]
    fn emit_permission_ask_resolved_default_is_noop_on_non_buffer_sink() {
        struct NoopSink;
        impl crate::state::ChatEventSink for NoopSink {
            fn emit_chat_event(&self, _: &ChatEventPayload) {}
            fn emit_tool_call(&self, _: &ToolCallPayload) {}
            fn emit_tool_result(&self, _: &ToolResultPayload) {}
            fn emit_permission_ask(&self, _: PermissionAskPayload) {}
            // emit_permission_ask_resolved: default no-op.
        }
        let sink = NoopSink;
        // Must not panic.
        sink.emit_permission_ask_resolved("rid", "allow");
        sink.emit_permission_ask_resolved("rid", "deny");
        sink.emit_permission_ask_resolved("rid", "timeout");
        sink.emit_permission_ask_resolved("rid", "cancel");
    }

    /// Multiple outcomes for the same rid produce multiple entries
    /// (the sink does NOT deduplicate by rid).
    #[test]
    fn emit_permission_ask_resolved_multiple_outcomes_for_same_rid() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_permission_ask_resolved("same-rid", "allow");
        sink.emit_permission_ask_resolved("same-rid", "deny");
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].kind, TranscriptKind::PermissionAskResolved);
        assert_eq!(transcript[1].kind, TranscriptKind::PermissionAskResolved);
        assert_eq!(
            transcript[0]
                .payload_json
                .get("outcome")
                .and_then(|v| v.as_str()),
            Some("allow"),
        );
        assert_eq!(
            transcript[1]
                .payload_json
                .get("outcome")
                .and_then(|v| v.as_str()),
            Some("deny"),
        );
    }

    /// 07-06 (am-observability-panel, R2b / AC7): the worker's
    /// `SubagentBufferSink` does NOT forward `ChatEvent::Recall`
    /// to the main chat IPC channel. The worker's own
    /// `build_recall_text_with_rows` / `recall_pitfall_with_hits`
    /// still runs (the worker can recall autonomously), but
    /// the recall event stays inside the worker's transcript
    /// — the main chat's `lastRecallHits` chip is unaffected.
    ///
    /// The test pins two related structural properties:
    ///
    /// (a) **No `app_handle` use in `emit_chat_event`**: the
    ///     worker's sink impl is in `sink/events.rs::emit_chat_event`; that
    ///     block does not reference `self.app_handle` at all
    ///     (a `grep` of the impl confirms it). The chat-event
    ///     IPC forwarding is the `AppHandleSink`'s
    ///     responsibility, NOT the worker's. The `Recall` event
    ///     is recorded into the worker's transcript (per
    ///     line 528-529 in `emit_chat_event`); that's the
    ///     intended scope.
    /// (b) **Constructed without an `app_handle`**: the test
    ///     constructor `new_without_app_handle` (line 198)
    ///     sets `app_handle: None`. The worker's nested
    ///     `run_chat_loop` (production path) constructs
    ///     `SubagentBufferSink::new_without_app_handle` too —
    ///     the production worker has no `app_handle` to forward
    ///     to. So even if a future refactor accidentally added
    ///     `self.app_handle.as_ref().map(|h| h.emit(...))` to
    ///     the chat-event path, the `None` case would silently
    ///     no-op (no `app` to emit on).
    ///
    /// Net effect: the worker's `Recall` events land in the
    /// worker's transcript; the main chat's IPC is structurally
    /// unreachable from a worker's `SubagentBufferSink`. The
    /// main chat's `lastRecallHits` chip never sees a worker
    /// recall (AC7).
    #[test]
    fn worker_sink_does_not_forward_recall_to_main_chat() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        // (a) The worker's sink constructor exposes NO way to
        // forward chat events to the main chat IPC. `app_handle`
        // is `None`; even if a future refactor added
        // `self.app_handle.as_ref().map(|h| h.emit("chat-event", ...))`,
        // it would be a `None`-no-op (no `app` to emit on).
        assert!(
            sink.app_handle.is_none(),
            "worker sink must be constructed without an app_handle (no IPC forward path)"
        );
        // (b) The worker's emit_chat_event must NOT panic on
        // the new `Recall` variant. The match has a wildcard
        // arm that drops it into the transcript record (no
        // IPC, just local buffer).
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid-recall".into(),
            session_id: "sess-test".into(),
            event: ChatEvent::Recall {
                hits: vec![crate::llm::types::RecallHit {
                    memory_id: "m1".into(),
                    title: "t".into(),
                    kind: "fact".into(),
                    source: "fts".into(),
                }],
            },
        });
        // The sink's `transcript_snapshot` is the worker-side
        // audit surface — the Recall event IS recorded there
        // (for historical-replay rendering in the drawer), but
        // it does NOT reach the main chat IPC. The test above
        // pins the structural property; the practical effect
        // (no main-chat IPC emit) is the absence of an
        // `app_handle` to call.
    }
}
