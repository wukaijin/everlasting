#![cfg(test)]

use std::sync::Arc;

use super::run_loop;
use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

/// L3a AC1 + AC6: parent emits a pure batch of 3 dispatch_subagent
/// tool_uses → 3 workers run concurrently → 3 tool_results return in
/// tool_use order → parent turn 2 emits final text. The MockProvider
/// script slots 1-3 are 3 identical worker single-turn summaries;
/// the concurrent branch consumes them in any order (the result is
/// the same regardless of which worker gets which slot because the
/// slots are identical).
#[tokio::test(flavor = "multi_thread")]
async fn l3a_pure_batch_of_three_dispatches_runs_concurrently() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: 3 dispatch_subagent tool_uses in ONE batch.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "research topic A"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "research topic B"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_c".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "research topic C"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot 1, 2, 3 — identical single-turn summaries.
        // Order of consumption is non-deterministic under concurrency
        // but each produces a distinct summary so we can verify all
        // 3 landed (without depending on which worker got which slot).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "worker result #1".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "worker result #2".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "worker result #3".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2: final text.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "synthesized all 3 reports".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        "rid-l3a-three",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 5 sends: parent_t1 + 3 workers + parent_t2.
    assert_eq!(
        mock.call_count(),
        5,
        "3 concurrent workers each consume one send slot"
    );

    // 3 tool_results, all completed. The `emit_tool_result` IPC
    // events fire as each task completes (streaming, mirroring the
    // L2 parallel path) so the emitter's snapshot order reflects
    // COMPLETION order, not tool_use order. The actual LLM context
    // order is determined by `result_slots[i]` which writes each
    // block at its OWN index — verified below via the persisted
    // tool_result message in the DB.
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 3, "3 dispatch_subagent → 3 tool_results");
    let mut tool_use_ids: Vec<String> = results.iter().map(|r| r.tool_use_id.clone()).collect();
    tool_use_ids.sort();
    assert_eq!(
        tool_use_ids,
        vec![
            "toolu_dispatch_a".to_string(),
            "toolu_dispatch_b".to_string(),
            "toolu_dispatch_c".to_string(),
        ],
        "all 3 tool_use ids present (order is completion-driven, not tool_use)"
    );
    for r in &results {
        assert!(!r.is_error, "completed worker → is_error=false");
        assert!(
            r.content.contains("[status: completed]"),
            "tool_result must carry status=completed, got: {}",
            r.content
        );
    }
    // All 3 worker summaries landed across the 3 tool_results.
    let combined: String = results.iter().map(|r| r.content.as_str()).collect();
    for marker in &["worker result #1", "worker result #2", "worker result #3"] {
        assert!(
            combined.contains(marker),
            "combined tool_results must contain '{}', got: {}",
            marker,
            combined
        );
    }

    // Verify the LLM-context order: the persisted tool_result
    // message (the user-role turn after the parent's assistant
    // turn with the 3 tool_uses) must contain the tool_result
    // blocks in tool_use order (result_slots[i] preserves the
    // index regardless of completion order). This is the real
    // invariant the concurrent branch guarantees.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    // Find the tool_result user turn (the one whose content JSON
    // contains "tool_result" blocks) and extract the tool_use_ids
    // in their serialized order.
    let mut found_order: Vec<String> = Vec::new();
    for m in &loaded.messages {
        let text = serde_json::to_string(&m.content).unwrap_or_default();
        if !text.contains(r#""type":"tool_result""#) {
            continue;
        }
        // Parse the JSON to extract tool_use_ids in order.
        if let Ok(arr) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(blocks) = arr.as_array() {
                for b in blocks {
                    if b.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                        if let Some(id) = b.get("tool_use_id").and_then(|v| v.as_str()) {
                            found_order.push(id.to_string());
                        }
                    }
                }
            }
        }
        break;
    }
    assert_eq!(
        found_order,
        vec![
            "toolu_dispatch_a".to_string(),
            "toolu_dispatch_b".to_string(),
            "toolu_dispatch_c".to_string(),
        ],
        "persisted tool_result blocks must be in tool_use order (result_slots[i] invariant)"
    );

    // 3 subagent_runs rows persisted (one per worker, all completed).
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 3, "3 worker runs persisted");
    for run in &runs {
        assert_eq!(run.status, "completed", "each worker run completed");
        assert!(run.finished_at.is_some(), "finished_at set");
    }
}

/// L3a AC3: a pure batch of (default limit + 1) dispatch_subagent
/// tool_uses → all hard-rejected with tool_error. No worker runs.
/// The MockProvider script has only parent_t1 + parent_t2 (no worker
/// slots) because no worker should be spawned. N is derived from
/// [`DEFAULT_DELEGATION_MAX_CONCURRENT_CHILDREN`] so the test tracks
/// the limit automatically (the default rose 3→10 on 2026-06-29).
#[tokio::test]
async fn l3a_pure_batch_over_limit_hard_rejects_all() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // Parent turn 1: N dispatch_subagent tool_uses, N = default limit
    // + 1, so the pure batch is OVER the limit → OverLimit branch →
    // all hard-rejected, no workers spawned. Deriving N from the
    // default const keeps this test correct if the default changes.
    let n_over = crate::agent::chat_loop::DEFAULT_DELEGATION_MAX_CONCURRENT_CHILDREN + 1;
    let mut turn1_events: Vec<std::result::Result<ChatEvent, crate::llm::error::LlmError>> =
        vec![Ok(ChatEvent::Start)];
    for i in 1..=n_over {
        turn1_events.push(Ok(ChatEvent::ToolCall {
            id: format!("toolu_over_{i}"),
            name: "dispatch_subagent".into(),
            input: serde_json::json!({ "subagent": "researcher", "task": format!("t{i}") }),
        }));
    }
    turn1_events.push(Ok(ChatEvent::Done {
        stop_reason: Some("tool_use".into()),
        usage: Some(TokenUsage::default()),
    }));

    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(turn1_events),
        // Parent turn 2: final text — no worker slots because all
        // 4 dispatches are hard-rejected (no run_subagent calls).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ok will reduce concurrency".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        "rid-l3a-over",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // Only 2 sends: parent_t1 + parent_t2 (no workers spawned).
    assert_eq!(
        mock.call_count(),
        2,
        "over-limit batch must NOT spawn any workers"
    );

    // N tool_results, all is_error=true, in tool_use order.
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), n_over, "N dispatches → N tool_results");
    for r in &results {
        assert!(r.is_error, "over-limit reject → is_error=true");
        assert!(
            r.content.contains("Concurrent dispatch limit reached"),
            "tool_result must explain the limit, got: {}",
            r.content
        );
    }

    // No subagent_runs rows persisted.
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert!(runs.is_empty(), "over-limit batch must persist 0 runs");
}

/// L3a AC4: parent cancel mid-batch propagates to all concurrent
/// workers. Script: parent_t1 emits 3 dispatches; the 3 worker
/// slots are `HangingThenCancel` (never produce an event, wait for
/// the cancel arm). The cancel side-channel fires the parent token
/// once all 3 workers have entered their `send`. The child_token
/// relationship propagates the cancel to all 3 workers; each
/// worker's select! cancel arm wins, each exits Done{cancelled},
/// `run_subagent` formats each tool_result with
/// `[status: cancelled]`. The parent loop's `cancel_parent`
/// aggregation (any worker cancelled → parent cancelled) flips the
/// parent's `cancelled` flag → parent loop drives its own cancel
/// path → terminal Done{cancelled}.
#[tokio::test(flavor = "multi_thread")]
async fn l3a_concurrent_cancel_propagates_to_all_workers() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: 3 dispatches.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_cancel_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher", "task": "hang A"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_cancel_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher", "task": "hang B"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_cancel_c".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher", "task": "hang C"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slots 1-3: HangingThenCancel (wait for cancel).
        MockResponse::HangingThenCancel,
        MockResponse::HangingThenCancel,
        MockResponse::HangingThenCancel,
        // Parent turn 2 (only reached if cancel fails to propagate).
        MockResponse::HangingThenCancel,
    ]));

    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let call_handle = mock.call_count_handle();
    let cancel_task = tokio::spawn(async move {
        // Wait until all 3 workers have entered their send (call_count >= 4:
        // parent_t1 + 3 workers).
        loop {
            if call_handle.load(std::sync::atomic::Ordering::SeqCst) >= 4 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        // Give the workers a moment to settle into their hung select!
        // state, then cancel the parent token.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        cancel_for_task.cancel();
    });

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        "rid-l3a-cancel",
        test_messages(),
        cancel_token,
    )
    .await;
    let _ = cancel_task.await;

    // 3 tool_results, all cancelled.
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 3, "3 dispatches → 3 cancelled tool_results");
    for r in &results {
        assert!(r.is_error, "cancelled worker → is_error=true");
        assert!(
            r.content.contains("[status: cancelled]"),
            "tool_result must carry status=cancelled, got: {}",
            r.content
        );
    }

    // Parent loop emits its own terminal Done{cancelled} (cancel_parent
    // aggregation flipped the parent's cancelled flag).
    assert_eq!(
        emitter.cancel_done_count(),
        1,
        "parent loop emits Done{{cancelled}} after all-worker cancel"
    );

    // 3 subagent_runs rows persisted, all cancelled.
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 3, "3 worker runs persisted");
    let cancelled_count = runs.iter().filter(|r| r.status == "cancelled").count();
    assert_eq!(cancelled_count, 3, "all 3 runs cancelled");
}

/// L3a worker token isolation (2026-06-26 reversal of RULE-A-015/PR2a):
/// 3 concurrent workers' token usage does NOT fold into the parent
/// session's `last_*` snapshot. The snapshot fix gates
/// `update_last_turn_usage` back under `!skip_persist`, so worker
/// turns don't touch the parent's snapshot. Worker usage stays in
/// each worker's `subagent_runs.token_usage_json`.
#[tokio::test(flavor = "multi_thread")]
async fn l3a_concurrent_token_usage_does_not_fold_into_parent() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let worker_usage = TokenUsage {
        input_tokens: 50,
        output_tokens: 25,
        cache_creation_input_tokens: 3,
        cache_read_input_tokens: 7,
        context_input_tokens: 60,
    };
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: 3 dispatches.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_usage_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher", "task": "compute"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_usage_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher", "task": "compute"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_usage_c".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher", "task": "compute"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    context_input_tokens: 10,
                }),
            }),
        ]),
        // 3 worker slots, each with identical non-zero usage.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "w1".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(worker_usage),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "w2".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(worker_usage),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "w3".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(worker_usage),
            }),
        ]),
        // Parent turn 2 — the LAST parent turn. Its usage is what
        // the parent's `last_*` snapshot should carry.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ack".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage {
                    input_tokens: 20,
                    output_tokens: 8,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    context_input_tokens: 20,
                }),
            }),
        ]),
    ]));

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        "rid-l3a-usage",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // Parent's snapshot should reflect ONLY parent_t2 (the last
    // parent turn). The 3 concurrent worker turns (each in=50)
    // MUST NOT land here — worker isolation per the 2026-06-26
    // snapshot fix.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    let s = &loaded.session;
    assert_eq!(
        s.last_context_input_tokens,
        Some(20),
        "parent snapshot reflects only the last parent turn, got: {:?}",
        s.last_context_input_tokens
    );
    assert_eq!(s.last_input_tokens, Some(20));
    assert_eq!(s.last_output_tokens, Some(8));
    assert_eq!(s.last_cache_creation, Some(0));
    assert_eq!(s.last_cache_read, Some(0));

    // Each of the 3 worker runs should carry its own usage in
    // `subagent_runs.token_usage_json`.
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 3, "3 worker runs persisted");
    for run in &runs {
        let usage_json = run
            .token_usage_json
            .as_ref()
            .expect("token_usage_json populated");
        let v: serde_json::Value = serde_json::from_str(usage_json).expect("valid JSON");
        assert_eq!(v.get("input_tokens").and_then(|x| x.as_i64()), Some(50));
        assert_eq!(v.get("output_tokens").and_then(|x| x.as_i64()), Some(25));
    }
}

/// L3a AC7 + single-dispatch regression: a mixed batch
/// (dispatch_subagent + read_file) falls through to the regular
/// serial path. The dispatch executes serially (single worker),
/// and the read_file executes serially too. Neither tool is run
/// concurrently. Verifies the classifier's `Serial` branch is
/// reached and the existing serial `for` loop runs unchanged.
#[tokio::test]
async fn l3a_mixed_batch_falls_through_to_serial_path() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: 1 dispatch + 1 read_file (mixed).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_mixed_dispatch".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher", "task": "mixed batch test"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_mixed_read".into(),
                name: "read_file".into(),
                input: serde_json::json!({ "path": "README.md" }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot (single, serial).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "mixed-batch worker result".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ack mixed".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        "rid-l3a-mixed",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 3 sends: parent_t1 + 1 worker (serial) + parent_t2.
    assert_eq!(
        mock.call_count(),
        3,
        "mixed batch runs the dispatch serially (1 worker)"
    );

    // 2 tool_results (1 dispatch + 1 read_file). The serial path
    // processes the for-loop in tool_use order, so the emitter
    // snapshot order matches tool_use order (no concurrency).
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_use_id, "toolu_mixed_dispatch");
    assert_eq!(results[1].tool_use_id, "toolu_mixed_read");
    // The dispatch tool_result is completed.
    assert!(!results[0].is_error);
    assert!(results[0].content.contains("[status: completed]"));

    // Exactly 1 subagent_run persisted (the single serial dispatch).
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 1, "mixed batch → 1 serial dispatch → 1 run");
}

/// L3a single-dispatch regression: a batch with exactly 1
/// dispatch_subagent (no other tools) classifies as `Serial` and
/// runs through the existing serial path unchanged. This is the
/// critical regression guard for the B6 single-dispatch tests
/// above (their behavior must NOT change under L3a).
#[tokio::test]
async fn l3a_single_dispatch_runs_serial_path_unchanged() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: single dispatch.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_single_dispatch".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher", "task": "single"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "single-dispatch worker result".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ack single".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        "rid-l3a-single",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 3 sends: parent_t1 + 1 worker (serial) + parent_t2.
    assert_eq!(
        mock.call_count(),
        3,
        "single dispatch runs serially (1 worker, no concurrent branch)"
    );

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1);
    assert!(!results[0].is_error);
    assert!(results[0].content.contains("[status: completed]"));

    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 1, "single dispatch → 1 run");
}
