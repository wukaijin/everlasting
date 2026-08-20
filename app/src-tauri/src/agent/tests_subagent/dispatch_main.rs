#![cfg(test)]

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

#[tokio::test]
async fn agent_loop_dispatch_subagent_completes_and_returns_summary() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent tool_use.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_1".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "Find all .rs files under src/."
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1 (script slot 1): single-turn summary.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "found 3 files".into(),
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
                text: "ok based on the worker's report".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-dispatch".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (200) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 PR1b: production-style caller → skip_session_active=false.
        false,
        // B6 PR1b: production-style caller → skip_persist=false
        // (persist every turn normally; worker skip is gated by the
        // dispatch_subagent interceptor at chat_loop.rs:1380, not
        // by the test harness).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): tests pass
        // `None` (production-style caller — not a worker, so
        // the PermissionContext.worker_run_id is unused by the
        // ask_path parent branch). The worker nested call in
        // `run_subagent` passes `Some(worker_run_id_opt)`.
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
        None,
        // 2026-06-30 (ask_user_question task): per-test QuestionStore
        h.question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
        // workflow_ctx = None (tests don't exercise the workflow
        // breadcrumb injection seam; that lives in separate
        // `agent::workflow::inject` tests).
        None,
        None,
        None,
        h.stub_loaded.clone(),
    )
    .await;

    // Parent turn count: parent_t1 + worker_t1 + parent_t2 = 3 sends.
    assert_eq!(
        mock.call_count(),
        3,
        "expected 3 send calls (parent_t1 + worker_t1 + parent_t2)"
    );

    // The dispatch_subagent tool_result carries the worker's summary
    // + the status prefix.
    let results = emitter.tool_results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "exactly one dispatch_subagent tool_result"
    );
    assert!(
        !results[0].is_error,
        "completed worker → is_error=false, got: {}",
        results[0].content
    );
    assert!(
        results[0].content.contains("[status: completed]"),
        "tool_result must carry status=completed prefix, got: {}",
        results[0].content
    );
    assert!(
        results[0].content.contains("found 3 files"),
        "tool_result must carry the worker's summary, got: {}",
        results[0].content
    );

    // Parent messages contain the dispatch_subagent tool_call +
    // tool_result, but NO worker text ("found 3 files") outside the
    // tool_result envelope. The worker's stream is isolated.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    let mut dispatch_tool_call_seen = false;
    let mut dispatch_tool_result_seen = false;
    let mut phantom_worker_text = 0;
    for m in &loaded.messages {
        let text = serde_json::to_string(&m.content).unwrap_or_default();
        if text.contains(r#""name":"dispatch_subagent""#) {
            dispatch_tool_call_seen = true;
        }
        // The tool_result content envelope echoes "found 3 files";
        // count only NON-tool_result rows that contain the worker's
        // text (those would be phantom worker leaks).
        if !text.contains(r#""type":"tool_result""#) && text.contains("found 3 files") {
            phantom_worker_text += 1;
        }
        if text.contains(r#""type":"tool_result""#) && text.contains("found 3 files") {
            dispatch_tool_result_seen = true;
        }
    }
    assert!(dispatch_tool_call_seen, "parent must persist the tool_call");
    assert!(
        dispatch_tool_result_seen,
        "parent must persist the dispatch tool_result"
    );
    assert_eq!(
        phantom_worker_text, 0,
        "worker intermediate text must NOT leak into parent messages"
    );

    // L3d (2026-06-25): worker nesting prevention regression guard.
    // The per-turn tool list rebuild (`chat_loop.rs` ~line 990)
    // appends `dispatch_subagent` via `definition_with_cache` —
    // WITHOUT the `effective_is_worker` gate this would re-expose
    // `dispatch_subagent` to a worker LLM even though
    // `filter_tools_for_subagent` stripped it from the seed list
    // (the seed is filtered once in `dispatch/prepare.rs::prepare_worker`, but the
    // per-turn append happens inside the nested `run_chat_loop`
    // body that the worker also reaches). This assertion locks
    // the no-nesting invariant: the worker turn (send slot 1,
    // index 1) MUST NOT see `dispatch_subagent` in its tool list.
    //
    // Slot 0 = parent_t1 (dispatch_subagent IS visible — parent
    //          needs to be able to dispatch).
    // Slot 1 = worker_t1 (dispatch_subagent MUST NOT be visible).
    // Slot 2 = parent_t2 (dispatch_subagent IS visible again).
    let sent_tools = mock.sent_tools();
    assert_eq!(
        sent_tools.len(),
        3,
        "expected 3 send calls captured (parent_t1 + worker_t1 + parent_t2)"
    );
    let parent_t1_names: Vec<&str> = sent_tools[0].iter().map(|t| t.name.as_str()).collect();
    let worker_t1_names: Vec<&str> = sent_tools[1].iter().map(|t| t.name.as_str()).collect();
    let parent_t2_names: Vec<&str> = sent_tools[2].iter().map(|t| t.name.as_str()).collect();
    assert!(
        parent_t1_names.contains(&"dispatch_subagent"),
        "parent_t1 MUST see dispatch_subagent (so it can dispatch): {:?}",
        parent_t1_names
    );
    assert!(
        !worker_t1_names.contains(&"dispatch_subagent"),
        "worker_t1 MUST NOT see dispatch_subagent (no nesting): {:?}",
        worker_t1_names
    );
    assert!(
        parent_t2_names.contains(&"dispatch_subagent"),
        "parent_t2 MUST see dispatch_subagent again: {:?}",
        parent_t2_names
    );
}

/// Worker cancel: the parent's cancellation token fires mid-worker.
/// The worker's child_token inherits the cancel; its stream loop's
/// `select!` cancel arm wins, the worker emits Done{cancelled}, and
/// run_subagent formats the tool_result with `[status: cancelled]` +
/// the CANCELLED_MARKER.
///
/// Script: parent_t1 dispatches; worker_t1 is HangingThenCancel
/// (worker's select! never produces an event, the cancel arm wins).
/// The cancel side-channel cancels the parent token once call_count
/// >= 2 (worker's send has been called).
#[tokio::test]
async fn agent_loop_dispatch_subagent_cancel_propagates_to_worker() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent tool_use.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_cancel".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "search forever"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: HangingThenCancel — never produces events.
        MockResponse::HangingThenCancel,
        // Parent turn 2 sentinel (only consumed if cancel fails).
        MockResponse::HangingThenCancel,
    ]));

    let call_handle = mock.call_count_handle();
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let cancel_handle = tokio::spawn(async move {
        // Wait until the worker's send has started (call_count >= 2),
        // then cancel the parent token. The child_token relationship
        // propagates the cancel to the worker.
        loop {
            if call_handle.load(Ordering::SeqCst) >= 2 {
                cancel_for_task.cancel();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-dispatch-cancel".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        cancel_token,
        None,
        h.background_shells.clone(),
        None,
        false,
        // B6 PR1b: production-style caller → skip_persist=false
        // (persist every turn normally; worker skip is gated by the
        // dispatch_subagent interceptor at chat_loop.rs:1380).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): tests pass
        // `None` (production-style caller — not a worker, so
        // the PermissionContext.worker_run_id is unused by the
        // ask_path parent branch). The worker nested call in
        // `run_subagent` passes `Some(worker_run_id_opt)`.
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
        None,
        // 2026-06-30 (ask_user_question task): per-test QuestionStore
        h.question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
        // workflow_ctx = None (tests don't exercise the workflow
        // breadcrumb injection seam; that lives in separate
        // `agent::workflow::inject` tests).
        None,
        None,
        None,
        h.stub_loaded.clone(),
    )
    .await;
    cancel_handle.await.unwrap();

    // The dispatch_subagent tool_result carries the cancelled prefix.
    let results = emitter.tool_results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "exactly one tool_result (cancel still pairs)"
    );
    assert!(results[0].is_error, "cancelled worker → is_error=true");
    assert!(
        results[0].content.contains("[status: cancelled]"),
        "tool_result must carry status=cancelled prefix, got: {}",
        results[0].content
    );
    assert!(
        results[0]
            .content
            .contains(crate::agent::helpers::CANCELLED_MARKER),
        "tool_result must carry CANCELLED_MARKER, got: {}",
        results[0].content
    );

    // Parent loop then emits its own terminal Done{cancelled} (the
    // cancel_parent flag flipped the parent's cancelled branch).
    assert_eq!(
        emitter.cancel_done_count(),
        1,
        "parent loop emits Done{{cancelled}} after worker cancel"
    );
}

/// Worker error: the worker's stream emits an Error event. The
/// worker's error path runs (per RULE-A-007), the worker exits, and
/// run_subagent formats the tool_result with `[status: error]`.
///
/// Script: parent_t1 dispatches; worker_t1 is a MockResponse::Events
/// with Delta + Err (the LlmError variant). The worker's had_error
/// flag flips → SubagentStatus::Error → format_dispatch_result
/// prefixes `[status: error]`.
#[tokio::test]
async fn agent_loop_dispatch_subagent_error_returns_status_error() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent tool_use.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_err".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "do something that will error",
                    // L3b (2026-06-27): this test exercises the
                    // worker's error path, NOT isolation. Force
                    // non-isolated so the worker doesn't try to
                    // create a worktree against the non-git test
                    // fixture (which would fail dispatch).
                    "isolation": false
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: stream errors mid-turn.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "starting work".into(),
            }),
            Err(LlmError::Server {
                status: 503,
                message: "worker upstream failed".into(),
                retry_after: None,
            }),
        ]),
        // Parent turn 2: final text (worker exited with error →
        // tool_result → parent turn 2).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ok noting the worker errored".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-dispatch-err".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        // B6 PR1b: production-style caller → skip_persist=false
        // (persist every turn normally; worker skip is gated by the
        // dispatch_subagent interceptor at chat_loop.rs:1380).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): tests pass
        // `None` (production-style caller — not a worker, so
        // the PermissionContext.worker_run_id is unused by the
        // ask_path parent branch). The worker nested call in
        // `run_subagent` passes `Some(worker_run_id_opt)`.
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
        None,
        // 2026-06-30 (ask_user_question task): per-test QuestionStore
        h.question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
        // workflow_ctx = None (tests don't exercise the workflow
        // breadcrumb injection seam; that lives in separate
        // `agent::workflow::inject` tests).
        None,
        None,
        None,
        h.stub_loaded.clone(),
    )
    .await;

    // 3 sends: parent_t1 + worker_t1 (errored) + parent_t2.
    assert_eq!(
        mock.call_count(),
        3,
        "worker error → tool_result → parent turn 2"
    );

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1, "exactly one tool_result");
    assert!(results[0].is_error, "errored worker → is_error=true");
    assert!(
        results[0].content.contains("[status: error]"),
        "tool_result must carry status=error prefix, got: {}",
        results[0].content
    );

    // Parent loop does NOT abort — the worker's error is contained
    // inside the tool_result. The parent continues to turn 2.
    let done_events: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect();
    assert!(
        done_events.iter().any(|s| s == "end_turn"),
        "parent loop completes normally after worker error, got stops: {:?}",
        done_events
    );
}

/// RULE-BackSubagent-001 (PR2): when a worker errors AFTER executing
/// some tool_calls, the parent's `dispatch_subagent` tool_result must
/// carry a `Worker partial actions:` summary so the parent LLM can do
/// compensatory repair (see that `read_file` already ran before
/// deciding what to retry / skip).
///
/// Mock script:
/// - Parent turn 1: dispatch_subagent tool_use.
/// - Worker turn 1: read_file tool_use → loop executes it, landing a
///   tool_call + tool_result in the worker's SubagentBufferSink
///   transcript.
/// - Worker turn 2: stream errors mid-turn → worker exits Error.
/// - Parent turn 2: final text.
///
/// The worker transcript now has one tool_call + paired tool_result, so
/// `summarize_worker_tool_actions` produces a non-empty summary and
/// `format_dispatch_result` appends the `Worker partial actions:`
/// section to the parent's tool_result content.
#[tokio::test(flavor = "multi_thread")]
async fn agent_loop_dispatch_subagent_error_includes_partial_transcript_summary() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent tool_use.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_partial".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "read a file, then the upstream will error",
                    // L3b (2026-06-27): force non-isolated (test
                    // exercises the worker's partial-transcript
                    // summary path, not isolation).
                    "isolation": false
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: read_file tool_use. The loop executes it,
        // emitting a tool_call + tool_result into the worker transcript.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_worker_read".into(),
                name: "read_file".into(),
                input: serde_json::json!({ "path": "nonexistent-worker-file.rs" }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 2: stream errors mid-turn → worker exits Error.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "analyzing".into(),
            }),
            Err(LlmError::Server {
                status: 503,
                message: "worker upstream failed".into(),
                retry_after: None,
            }),
        ]),
        // Parent turn 2: final text.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ok noting the worker did some work before erroring".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-dispatch-partial".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        Some(false),
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        None,
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
        None,
        // 2026-06-30 (ask_user_question task): per-test QuestionStore
        h.question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
        // workflow_ctx = None (tests don't exercise the workflow
        // breadcrumb injection seam; that lives in separate
        // `agent::workflow::inject` tests).
        None,
        None,
        None,
        h.stub_loaded.clone(),
    )
    .await;

    // 4 sends: parent_t1 + worker_t1 (tool_use) + worker_t2 (errored) + parent_t2.
    assert_eq!(
        mock.call_count(),
        4,
        "worker ran a tool turn before erroring"
    );

    let results = emitter.tool_results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "exactly one tool_result (dispatch_subagent)"
    );
    assert!(results[0].is_error, "errored worker → is_error=true");
    assert!(
        results[0].content.contains("[status: error]"),
        "tool_result must carry status=error prefix, got: {}",
        results[0].content
    );
    // RULE-BackSubagent-001: the parent must see the worker's executed
    // tool_call in the partial-actions summary section.
    assert!(
        results[0].content.contains("Worker partial actions:"),
        "tool_result must carry partial actions section, got: {}",
        results[0].content
    );
    assert!(
        results[0].content.contains("read_file("),
        "summary must list the executed read_file call, got: {}",
        results[0].content
    );
}

/// Worker guard does NOT evict the parent's session_active_request
/// entry. This is the PR1a `skip_session_active` regression guard
/// called out in the PR1b task brief.
///
/// Setup: pre-populate session_active_request[parent_session_id] =
/// parent_rid (what `chat.rs::chat` would do on spawn). Run the
/// parent loop with a dispatch_subagent tool_use. After the loop
/// exits (parent CancellationGuard Drop runs), the
/// session_active_request must be EMPTY (parent's own Drop cleared
/// it) — but DURING the loop, while the worker's CancellationGuard
/// drops, the entry must STILL contain parent_rid (the worker's
/// skip_session_active=true guard left it alone).
///
/// The cleanest way to test this is to check post-loop: parent's
/// guard clears the entry on Drop, so the map is empty. But if the
/// worker's guard had ALSO cleared it (the bug we're guarding
/// against), the parent's loop would see the entry gone MID-loop
/// — that wouldn't surface as a post-loop failure. So we ALSO
/// inspect mid-loop via a side-channel: register a separate rid
/// in cancellations before the loop and verify the worker's rid
/// appears there during the worker's run.
///
/// Simplification: the most direct invariant is "the worker rid
/// appears in `cancellations` during the worker's run and is
/// cleaned up by the worker's guard Drop, while the parent rid
/// remains registered for the parent's lifetime." We assert:
///   1. Post-loop: `cancellations` is empty (both rids cleaned up).
///   2. Post-loop: `session_active_request[parent_session_id]` is
///      gone (parent's Drop cleared it; the worker's Drop did NOT
///      clear it mid-loop, which would have left the entry gone
///      BEFORE the parent's Drop — observable via mid-loop cancel).
///
/// The cleanest behavioral test: trigger a dispatch, then mid-loop
/// inspect the maps. We do that via the MockProvider's call_count
/// signal + a short-lived snapshot task.
#[tokio::test(flavor = "multi_thread")]
async fn agent_loop_dispatch_subagent_guard_does_not_evict_parent_session_active() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_guard".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "noop"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: HANG. The worker stays in its select!
        // loop until the parent cancels the parent_token (which
        // fires the worker's child_token). This keeps the
        // worker "in flight" long enough for the snapshot task
        // below to read `cancellations` and
        // `session_active_request` while the worker is still
        // running — the worker's CancellationGuard has NOT yet
        // dropped, so the worker rid is still in cancellations
        // and the parent session_active_request entry is
        // untouched.
        MockResponse::HangingThenCancel,
        // Parent turn 2: final (only consumed after the cancel
        // propagates back through the worker, then through
        // `run_subagent`'s `cancel_parent` flag).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    // Pre-populate the parent's session_active_request entry, mirroring
    // what chat.rs::chat does on spawn. This lets us assert the
    // worker's guard Drop leaves it intact.
    let parent_rid = "rid-guard-test".to_string();
    {
        let mut map = h.session_active_request.lock().await;
        map.insert(h.session_id.clone(), parent_rid.clone());
    }
    // Also register the parent token in cancellations, mirroring
    // chat.rs::chat.
    let parent_token = CancellationToken::new();
    {
        let mut map = h.cancellations.lock().await;
        map.insert(parent_rid.clone(), parent_token.clone());
    }

    // Snapshot task: race the loop, snapshot the maps once the
    // worker's send has been called (call_count >= 2). At that
    // point the worker is mid-run (hung on its HangingThenCancel
    // stream); the parent's session_active_request entry must
    // STILL be intact, AND the worker rid must be in
    // `cancellations` (the worker registered itself in
    // `run_subagent` before the nested `run_chat_loop` call).
    let session_active_clone = h.session_active_request.clone();
    let cancellations_clone = h.cancellations.clone();
    let session_id_clone = h.session_id.clone();
    let call_handle = mock.call_count_handle();
    // Clone the parent_rid for the snapshot closure; the original
    // stays for the run_chat_loop call below.
    let parent_rid_for_snapshot = parent_rid.clone();
    let snapshot_handle: tokio::task::JoinHandle<
        Option<(bool, bool)>, // (parent_session_active_present, worker_rid_present)
    > = tokio::spawn(async move {
        // Wait until the worker has been dispatched (call_count >= 2).
        for _ in 0..1000 {
            if call_handle.load(Ordering::SeqCst) >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        if call_handle.load(Ordering::SeqCst) < 2 {
            return None; // worker never ran
        }
        // Give the worker a moment to register its rid AND settle
        // into its hung select! state. The worker is HUNG (Hanging
        // ThenCancel stream) so its CancellationGuard is held
        // open — the worker rid will remain in `cancellations`
        // and the parent session_active_request entry will
        // remain untouched until we cancel below.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let parent_present = {
            let map = session_active_clone.lock().await;
            map.get(&session_id_clone).map(|s| s.to_string())
                == Some(parent_rid_for_snapshot.clone())
        };
        // The worker's rid must be present in cancellations (it
        // registered itself). Its key is `<parent_rid>-sub-<toolu_id>`.
        let worker_rid_suffix = format!("{}-sub-toolu_dispatch_guard", parent_rid_for_snapshot);
        let worker_present = {
            let map = cancellations_clone.lock().await;
            map.contains_key(&worker_rid_suffix)
        };
        Some((parent_present, worker_present))
    });

    // Cancel task: once the snapshot has had its chance to read
    // the maps, cancel the parent token. The child_token
    // relationship propagates the cancel to the worker, the
    // worker's select! cancel arm wins, the worker exits with
    // Done{cancelled}, run_subagent detects the cancel_parent
    // flag, the parent loop flips its `cancelled` and drives
    // its own cancel path (Done{cancelled} to the parent
    // sink). The parent_token was pre-inserted in cancellations
    // (we mock what `chat.rs::chat` does on spawn).
    let cancel_for_task = parent_token.clone();
    let cancel_handle = tokio::spawn(async move {
        // Wait until the snapshot has had time to read the maps
        // AND take its snapshot. The snapshot polls for up to
        // ~2000ms after spawn; we give it a comfortable 500ms
        // margin so the cancel propagates AFTER the snapshot,
        // not before. The parent token is pre-inserted in
        // cancellations (mirroring `chat.rs::chat`); cancelling
        // it before the parent dispatches the worker would
        // short-circuit the parent's tool execution, and
        // `run_subagent` would never run (the worker is never
        // dispatched). 500ms is enough for the parent's user-
        // message persist + first `provider.send` + tool
        // dispatch.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        cancel_for_task.cancel();
    });

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        parent_rid.clone(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        parent_token,
        None,
        h.background_shells.clone(),
        None,
        false,
        // B6 PR1b: production-style caller → skip_persist=false
        // (persist every turn normally; worker skip is gated by the
        // dispatch_subagent interceptor at chat_loop.rs:1380).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): tests pass
        // `None` (production-style caller — not a worker, so
        // the PermissionContext.worker_run_id is unused by the
        // ask_path parent branch). The worker nested call in
        // `run_subagent` passes `Some(worker_run_id_opt)`.
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
        None,
        // 2026-06-30 (ask_user_question task): per-test QuestionStore
        h.question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
        // workflow_ctx = None (tests don't exercise the workflow
        // breadcrumb injection seam; that lives in separate
        // `agent::workflow::inject` tests).
        None,
        None,
        None,
        h.stub_loaded.clone(),
    )
    .await;
    cancel_handle.await.unwrap();

    let snapshot = snapshot_handle.await.expect("snapshot task not panic");
    let (parent_present, worker_present) = snapshot.expect("snapshot captured");

    // Mid-loop invariants:
    //   1. Parent's session_active_request entry is STILL the parent
    //      rid (worker's skip_session_active=true Drop has not
    //      evicted it; if it had, the entry would be gone OR the
    //      parent's cancel_inflight_for_session would have lost its
    //      target).
    //   2. Worker rid is present in cancellations (the worker
    //      registered itself).
    assert!(
        parent_present,
        "parent's session_active_request entry must survive the worker's guard Drop          (skip_session_active=true)"
    );
    assert!(
        worker_present,
        "worker rid must be registered in cancellations during the worker's run"
    );
}

/// 08-20-worker-turn-trace-persist AC2 + AC3: researcher dispatch 多轮
/// → worker 的每个真实 LLM turn 落 (父 sid, run UUID, seq 递增) 行,
/// 且 worker 行不污染主行 / 不写父 sessions.last_*。
///
/// Script: parent_t1(dispatch) → worker_t1(read_file tool_use) →
/// worker_t2(final text) → parent_t2(final text)。三方 usage 值互不
/// 相同(parent_t1=100 / worker=700,800 / parent_t2=111),使"谁的
/// 数字落在哪"可判。
///
/// AC2 断言(run 行):每轮 token_usage_json 非空、tools_token 非空
/// (worker 工具集过滤后仍非空 → cl100k > 0)、context_window 落值
/// (worker 继承父 200_000)、memory/images/at_files 列 NULL(worker
/// 语义:prompt.rs 注入 / 无附件 / 无 @注入)。
/// AC3 断言(主行隔离):run_id='' 行只有父自己的 2 轮(usage 值
/// 100/111,worker 的 700/800 不出现);list_turn_traces 不含 worker
/// 行;sessions.last_input_tokens = 111(父 t2 的值,worker 覆盖会
/// 变成 700/800 —— RULE-A-015 reversal 回归锁)。
#[tokio::test(flavor = "multi_thread")]
async fn agent_loop_dispatch_subagent_worker_turn_trace_rows_persisted() {
    let h = make_harness().await;
    // worker_t1 要真读一个文件(read-only 工具走完 execute → tool_result
    // 回喂 → worker 进入第 2 轮)。
    std::fs::write(h.project_path.join("worker_note.txt"), "data").unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch researcher.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_trace".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "read the note and summarize"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage {
                    input_tokens: 100,
                    ..Default::default()
                }),
            }),
        ]),
        // Worker turn 1: read_file tool_use (real turn #1).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_worker_read".into(),
                name: "read_file".into(),
                input: serde_json::json!({ "path": "worker_note.txt" }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage {
                    input_tokens: 700,
                    output_tokens: 70,
                    cache_creation_input_tokens: 7,
                    cache_read_input_tokens: 77,
                    context_input_tokens: 784,
                }),
            }),
        ]),
        // Worker turn 2: final text (real turn #2).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "the note says data".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage {
                    input_tokens: 800,
                    output_tokens: 80,
                    cache_creation_input_tokens: 8,
                    cache_read_input_tokens: 88,
                    context_input_tokens: 896,
                }),
            }),
        ]),
        // Parent turn 2: final text.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ok based on the worker's report".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage {
                    input_tokens: 111,
                    ..Default::default()
                }),
            }),
        ]),
    ]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-dispatch-trace".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        // B6 PR1b: production-style caller → skip_persist=false
        // (worker 的 skip 在 run_subagent 的嵌套调用里)。
        false,
        Some(false),
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        None,
        None,
        h.subagent_cache.clone(),
        None,
        None,
        None, // project_main_override (2026-07-29)
        h.app_data_dir.clone(),
        None,
        h.question_store.clone(),
        None,
        None,
        None,
        h.stub_loaded.clone(),
    )
    .await;

    // 4 sends: parent_t1 + worker_t1 + worker_t2 + parent_t2.
    assert_eq!(
        mock.call_count(),
        4,
        "expected 4 sends (parent_t1 + worker_t1 + worker_t2 + parent_t2)"
    );

    // The run row exists (insert_run succeeded → worker_run_id_opt=Some).
    let runs = db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list subagent runs");
    assert_eq!(runs.len(), 1, "exactly one worker run");
    let run_id = runs[0].id.clone();

    // ---- AC2: worker per-turn rows ----
    let worker_rows = db::trace::list_worker_turn_traces(&h.db, &run_id)
        .await
        .expect("list worker turn traces");
    assert_eq!(
        worker_rows.len(),
        2,
        "each real worker LLM turn must leave exactly one run row"
    );
    // (父 sid, run UUID, seq 递增):worker seq 是 worker loop 自己的
    // 游标(从父 DB messages max+1 起),只断言严格递增 + 归属。
    assert!(
        worker_rows[0].seq < worker_rows[1].seq,
        "worker row seqs must be strictly increasing: {:?}",
        worker_rows.iter().map(|r| r.seq).collect::<Vec<_>>()
    );
    for (i, row) in worker_rows.iter().enumerate() {
        assert_eq!(row.session_id, h.session_id, "row {i}: parent session id");
        assert_eq!(row.run_id, run_id, "row {i}: run id routing");
        assert!(
            row.token_usage_json
                .as_deref()
                .is_some_and(|j| !j.is_empty()),
            "row {i}: usage JSON must be non-empty"
        );
        assert!(
            row.tools_token.is_some_and(|t| t > 0),
            "row {i}: worker toolset estimate must land (got {:?})",
            row.tools_token
        );
        assert_eq!(
            row.context_window,
            Some(200_000),
            "row {i}: worker inherits the parent context window"
        );
        // worker 切片语义(prd R2):memory 走 subagent/prompt.rs、无
        // 附件、无 @注入 → 三列 NULL,不是 0。
        assert_eq!(row.memory_token, None, "row {i}: memory slice NULL");
        assert_eq!(row.images_token, None, "row {i}: images slice NULL");
        assert_eq!(row.at_files_token, None, "row {i}: at_files slice NULL");
    }
    // 逐轮 usage 区分:worker_t1=700 / worker_t2=800(非零且互不相同,
    // 证明两行是各自 turn 的快照而非同一行被覆写)。
    let w0_in = serde_json::from_str::<serde_json::Value>(
        worker_rows[0].token_usage_json.as_deref().unwrap(),
    )
    .unwrap()["input_tokens"]
        .clone();
    let w1_in = serde_json::from_str::<serde_json::Value>(
        worker_rows[1].token_usage_json.as_deref().unwrap(),
    )
    .unwrap()["input_tokens"]
        .clone();
    assert_eq!(w0_in, serde_json::json!(700));
    assert_eq!(w1_in, serde_json::json!(800));

    // ---- AC3: 主行隔离 ----
    // run_id='' 的行只有父自己的两轮,usage 值 100 / 111(worker 的
    // 700 / 800 若混入即 fail)。
    let main_rows = db::trace::list_turn_traces(&h.db, &h.session_id)
        .await
        .expect("list main turn traces");
    assert_eq!(main_rows.len(), 2, "main rows = parent_t1 + parent_t2 only");
    let mut main_inputs: Vec<i64> = Vec::new();
    for row in &main_rows {
        assert_eq!(row.run_id, "", "main rows must carry the '' sentinel");
        let v: serde_json::Value =
            serde_json::from_str(row.token_usage_json.as_deref().unwrap()).unwrap();
        main_inputs.push(v["input_tokens"].as_i64().unwrap());
    }
    main_inputs.sort();
    assert_eq!(
        main_inputs,
        vec![100, 111],
        "main rows must carry ONLY the parent's usages (worker 700/800 leaked?)"
    );

    // worker 不写父 sessions.last_*:快照必须停留在父 t2 的 111
    // (RULE-A-015 reversal 回归锁;worker 的 700/800 任一次覆盖都会
    // 改变此值)。
    let last_input: Option<i64> =
        sqlx::query_scalar("SELECT last_input_tokens FROM sessions WHERE id = ?")
            .bind(&h.session_id)
            .fetch_one(&h.db)
            .await
            .expect("read sessions.last_input_tokens");
    assert_eq!(
        last_input,
        Some(111),
        "sessions.last_* must hold the PARENT's last turn (111), not a worker turn's"
    );
}
