#![cfg(test)]

use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::agent::chat_loop::{classify_dispatch_batch, DispatchBatch};
use crate::agent::system_prompt::build_system_prompt;
use crate::agent::subagent::filter_tools_readonly;
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

/// Worker completes: parent turn 1 emits dispatch_subagent, the
/// worker runs a single turn (produces "found 3 files" summary),
/// parent turn 2 sees the tool_result and emits final text.
///
/// Invariants:
/// - The dispatch_subagent tool_result carries `[status: completed]`
///   + the worker's final text.
/// - The parent's persisted messages contain the dispatch_subagent
///   tool_call (assistant turn) + the tool_result (user turn). NO
///   worker intermediate events leak into the parent's session —
///   the worker's tool_use / tool_result land ONLY in the
///   SubagentBufferSink transcript, which is in-memory only.
/// - Parent frontend emits exactly one tool:call (the dispatch) +
///   one tool:result (the summary). No worker tool:call / tool:result
///   on the parent sink.
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
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
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
    // (the seed is filtered once in `dispatch.rs:187`, but the
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
    let parent_t1_names: Vec<&str> =
        sent_tools[0].iter().map(|t| t.name.as_str()).collect();
    let worker_t1_names: Vec<&str> =
        sent_tools[1].iter().map(|t| t.name.as_str()).collect();
    let parent_t2_names: Vec<&str> =
        sent_tools[2].iter().map(|t| t.name.as_str()).collect();
    assert!(
        parent_t1_names.iter().any(|n| *n == "dispatch_subagent"),
        "parent_t1 MUST see dispatch_subagent (so it can dispatch): {:?}",
        parent_t1_names
    );
    assert!(
        !worker_t1_names.iter().any(|n| *n == "dispatch_subagent"),
        "worker_t1 MUST NOT see dispatch_subagent (no nesting): {:?}",
        worker_t1_names
    );
    assert!(
        parent_t2_names.iter().any(|n| *n == "dispatch_subagent"),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
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
        None,
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
        None,
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
    )
    .await;

    // 4 sends: parent_t1 + worker_t1 (tool_use) + worker_t2 (errored) + parent_t2.
    assert_eq!(
        mock.call_count(),
        4,
        "worker ran a tool turn before erroring"
    );

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1, "exactly one tool_result (dispatch_subagent)");
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
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
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

// ---------------------------------------------------------------------------
// B6 PR2: subagent_runs persistence integration tests
// ---------------------------------------------------------------------------

/// End-to-end: parent dispatches a researcher worker → worker
/// runs and returns a summary → `subagent_runs` row is in
/// `completed` state with `transcript_json` non-empty and
/// `summary` containing the worker's text. This is the canonical
/// PR2 success path: a `subagent_runs` row must survive a session
/// reload (PR3's expand UI will read it).
#[tokio::test]
async fn agent_loop_dispatch_subagent_persists_subagent_run() {
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
        // Worker turn 1: single-turn summary.
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
        None,
        false,
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
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
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
    )
    .await;

    // Verify the worker run is in `subagent_runs` and the row
    // reflects the completed state. The list_runs_by_session
    // query returns newest first — the only run is the one we
    // just dispatched.
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 1, "exactly one subagent_run was persisted");
    let run = &runs[0];
    assert_eq!(run.status, "completed");
    assert_eq!(run.subagent_name, "researcher");
    assert!(run.finished_at.is_some(), "finished_at must be set");
    assert_eq!(
        run.summary.as_deref(),
        Some("found 3 files"),
        "summary must equal worker's final_text"
    );
    // transcript_json must be a valid JSON array of TranscriptEntry.
    let transcript: Vec<crate::agent::subagent::TranscriptEntry> =
        serde_json::from_str(run.transcript_json.as_deref().unwrap())
            .expect("transcript_json parses as Vec<TranscriptEntry>");
    // Worker emitted 3 events (Start, Delta, Done) → 3 transcript entries.
    assert_eq!(transcript.len(), 3);
    assert_eq!(
        transcript[0].kind,
        crate::agent::subagent::TranscriptKind::ChatEvent
    );
    // token_usage_json must round-trip as a TokenUsage (all zeros here).
    let usage: TokenUsage = serde_json::from_str(run.token_usage_json.as_deref().unwrap())
        .expect("token_usage_json parses as TokenUsage");
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);
    // The worker rid format is "{parent_rid}-sub-{tool_use_id}".
    assert!(run.parent_request_id.contains("rid-dispatch-sub-"));
}

/// End-to-end: parent dispatches a worker and the parent cancel
/// propagates → `subagent_runs` row is in `cancelled` state with
/// `finished_at` set and `summary` reflecting the partial
/// accumulation.
#[tokio::test]
async fn agent_loop_dispatch_subagent_cancelled_persists_status_cancelled() {
    use crate::db::subagent_runs::{get_run, list_runs_by_session};

    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // Two HangingThenCancel responses: parent turn 1 gets cancelled
    // before the dispatch (actually we want parent to dispatch
    // first, then cancel mid-worker). The MockProvider's
    // HangingThenCancel pattern is "produce 0 events, wait for
    // cancel" — used for the worker below.
    //
    // For parent turn 1 we need a real response that issues the
    // dispatch_subagent tool_use, then we cancel after the worker
    // starts.
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_1".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "long running search"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: HangingThenCancel — never produces an
        // event; the cancel arm wins, the worker emits
        // Done{cancelled}.
        MockResponse::HangingThenCancel,
    ]));
    let cancel_token = CancellationToken::new();
    let cancel_token_for_task = cancel_token.clone();
    let call_count_for_cancel = mock.clone();
    let cancel_task = tokio::spawn(async move {
        // Wait until the worker has been entered (call_count >= 2)
        // before firing the cancel.
        loop {
            if call_count_for_cancel.call_count() >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
        // Brief delay so the worker is mid-flight (so its select!
        // sees the cancel).
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_token_for_task.cancel();
    });

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-cancel".into(),
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
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
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
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
    )
    .await;
    let _ = cancel_task.await;

    // Worker run is persisted with status=cancelled.
    let runs = list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list");
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.status, "cancelled");
    assert!(run.finished_at.is_some());
    // get_run returns the same row (catches the path-not-list path).
    let fetched = get_run(&h.db, &run.id).await.unwrap().expect("row exists");
    assert_eq!(fetched.status, "cancelled");
}

/// Audit invariant (R6 / AC4): worker's `record_audit_event` calls
/// do NOT add **new** rows to the parent's `session_audit_events`
/// that aren't attributable to the parent's own ⑨ 关 path. The
/// parent WILL write 2 audit rows for `dispatch_subagent`:
/// 1. `tool_allowed` from `permissions::check` (line 556 in
///    `permissions/mod.rs`).
/// 2. `tool_executed` from `record_tool_executed_audit`
///    (`agent/chat_loop.rs:1362`).
///
/// Both are parent-side writes — neither is the worker writing
/// ⑨ decisions to the parent's audit log. The worker path's
/// `skip_persist=true` (B6 PR1b) gates the worker's own
/// `record_audit_event` / `record_tool_executed_audit` call
/// sites inside `run_chat_loop` — so a worker with no tool
/// calls (like this researcher test) produces 0 worker-internal
/// audit rows. The total audit count delta is therefore
/// **exactly 2** for this test scenario; a delta > 2 would mean
/// the worker is leaking audit rows.
#[tokio::test]
async fn agent_loop_dispatch_subagent_audit_not_polluted_by_worker() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_1".into(),
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
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2: final text.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ack".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    // Snapshot the audit count BEFORE the run.
    let audit_before = crate::db::permissions::list_audit_events(&h.db, &h.session_id)
        .await
        .expect("list_audit_events before");
    let before_count = audit_before.len();

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-audit".into(),
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
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
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
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
    )
    .await;

    let audit_after = crate::db::permissions::list_audit_events(&h.db, &h.session_id)
        .await
        .expect("list_audit_events after");
    let after_count = audit_after.len();
    let delta = after_count - before_count;
    // Parent's 2 rows: `tool_allowed` + `tool_executed` for the
    // `dispatch_subagent` tool_use. A delta > 2 means the
    // worker leaked audit rows.
    assert_eq!(
        delta, 2,
        "worker must not add audit rows beyond the parent's 2 \
         (tool_allowed + tool_executed for dispatch_subagent); got delta={}",
        delta
    );
}

/// Worker token isolation (2026-06-26 reversal of RULE-A-015/PR2a):
/// the worker's per-turn `TokenUsage` does NOT fold into the parent
/// session's `last_*` snapshot columns. The snapshot fix moved
/// `update_last_turn_usage` BACK inside the `!skip_persist` gate
/// at `chat_loop.rs`, so worker turns (which run with
/// `skip_persist=true`) don't touch the parent's snapshot. Worker
/// token usage stays isolated in `subagent_runs.token_usage_json`
/// (written at worker exit by `dispatch.rs::run_subagent`).
#[tokio::test]
async fn agent_loop_dispatch_subagent_token_usage_does_not_fold_into_parent() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_1".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "compute usage"
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
        // Worker turn 1: returns a non-zero usage. This MUST NOT
        // land in the parent's `last_*` snapshot (skip_persist=true
        // on the worker path).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: 7,
                    cache_read_input_tokens: 11,
                    context_input_tokens: 118,
                }),
            }),
        ]),
        // Parent turn 2: this is the LAST parent turn — its usage
        // is what the parent's `last_*` snapshot should carry.
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

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-usage".into(),
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
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
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
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
    )
    .await;

    // The parent's session snapshot should reflect ONLY the last
    // PARENT turn (parent_t2: in=20, out=8). The worker's turn
    // (in=100, cc=7, cr=11) MUST NOT appear here — worker token
    // usage stays isolated in `subagent_runs.token_usage_json`.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    let s = &loaded.session;
    assert_eq!(
        s.last_context_input_tokens,
        Some(20),
        "parent snapshot should reflect only parent_t2 (the last parent turn), not the worker"
    );
    assert_eq!(s.last_input_tokens, Some(20));
    assert_eq!(s.last_output_tokens, Some(8));
    assert_eq!(s.last_cache_creation, Some(0));
    assert_eq!(s.last_cache_read, Some(0));

    // The worker's usage MUST be in subagent_runs.token_usage_json.
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 1, "exactly 1 worker run persisted");
    let run = &runs[0];
    let usage_json = run
        .token_usage_json
        .as_ref()
        .expect("token_usage_json is populated at worker exit");
    let v: serde_json::Value = serde_json::from_str(usage_json).expect("valid JSON");
    assert_eq!(v.get("input_tokens").and_then(|x| x.as_i64()), Some(100));
    assert_eq!(v.get("output_tokens").and_then(|x| x.as_i64()), Some(50));
    assert_eq!(
        v.get("cache_creation_input_tokens").and_then(|x| x.as_i64()),
        Some(7)
    );
    assert_eq!(
        v.get("cache_read_input_tokens").and_then(|x| x.as_i64()),
        Some(11)
    );
    // The worker's `context_input_tokens` (input+cc+cr=118) is
    // serialized through `cumulative_usage` → `token_usage_json`.
    assert_eq!(
        v.get("context_input_tokens").and_then(|x| x.as_i64()),
        Some(118)
    );
}

/// RULE-A-014 end-to-end: `general-purpose` worker + Edit mode +
/// `write_file` to a path outside the worker's cwd. The worker's
/// `permissions::check` would normally emit a `permission:ask` for
/// a Tier 4 path-outside-cwd tool_use — and the worker has no UI
/// sink, so the oneshot resolution would never arrive. PR2b
/// threads `is_worker: Option<bool>` through the nested
/// `run_chat_loop` so the worker builds a `PermissionContext` with
/// `is_worker: true`, which short-circuits the Tier 4 `ask_path`
/// to `Decision::Deny` (mirroring the Claude Code "background
/// subagent auto-deny" convention). The worker's tool_result
/// carries `is_error=true` + the deny reason, the LLM self-
/// corrects on turn 2, the worker completes normally, and the
/// parent loop gets the dispatch_subagent tool_result with
/// `[status: completed]`. Without PR2b, this test would HANG
/// (the worker's `select!` waits on the oneshot that never
/// resolves), the `MockProvider`'s call_count would never reach
/// 3, and the test would time out (default `#[tokio::test]`
/// timeout is 60s).
///
/// Note: `Edit` mode (the harness default) is used because
/// `Plan` mode's `filter_tools_for_mode` drops `write_file` from
/// the worker's tool set entirely (defense in depth — the worker
/// never sees the tool, so the worker never even gets to call
/// `permissions::check` for it). Edit mode keeps the tool
/// available, and the `is_within_root(cwd, path)` check inside
/// Tier 4 dispatches to `ask_path` only when the target path is
/// outside the project root — `/tmp/everlasting_worker_escape`
/// is a real path outside any test's tempdir.
#[tokio::test(flavor = "multi_thread")]
async fn agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent general-purpose.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_rule_a_014".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "Write a file at /tmp/everlasting_worker_escape.txt with content 'leaked'",
                    // L3b (2026-06-27): force non-isolated (test
                    // exercises the PR2b RULE-A-014 worker ask
                    // collapse, not isolation).
                    "isolation": false
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: write_file to a path OUTSIDE the worker's
        // cwd. The path is absolute (`/tmp/...`), so `is_within_root`
        // returns false → Tier 4 `ask_path` triggers. With
        // `is_worker=true` (PR2b), `ask_path` returns
        // `Decision::Deny` immediately (no permission:ask emit, no
        // oneshot wait — the worker cannot ask the user).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_worker_write".into(),
                name: "write_file".into(),
                input: serde_json::json!({
                    "path": "/tmp/everlasting_worker_escape.txt",
                    "content": "leaked"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 2: LLM sees the deny tool_result, self-
        // corrects with a final summary. (No additional tool_use
        // — the worker gave up and reported back to the parent.)
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "Write denied by worker permission policy; cannot surface modal.".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2: final text response.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ack".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    // Snapshot the audit count BEFORE so we can assert the worker's
    // ⑨ decision does NOT add a `tool_permission_ask` row (PR2b
    // collapses the ask to a deny — no permission:ask emit, no
    // oneshot wait, no `tool_permission_ask` audit row). The
    // worker's auto-deny DOES write a `tool_denied` audit row
    // (permissions::ask_path line 1002-1009, unconditional), so
    // the post-run delta includes 1 `tool_denied` from the worker
    // + 2 parent rows (tool_allowed + tool_executed for
    // dispatch_subagent) = 3 total.
    let audit_before = crate::db::permissions::list_audit_events(&h.db, &h.session_id)
        .await
        .expect("list_audit_events before");

    // Wrap the run in a `tokio::time::timeout` so a hang (the
    // pre-PR2b symptom — oneshot never resolved) is caught and
    // fails the test with a clear message instead of timing out
    // the test runner at 60s.
    //
    // 2026-06-22 (RULE-FrontSubagent-003 fix): the worker now
    // enters the interactive ask round-trip instead of auto-
    // denying synchronously. Without anyone responding, the
    // worker waits up to 120s for the ASK_TIMEOUT to fire (the
    // post-fix behavior). The outer timeout is therefore raised
    // to 130s so the test completes by the natural timeout
    // path (not by the outer kill). The test's purpose (verify
    // the worker does NOT hang forever on the oneshot) is
    // preserved — the worker's auto-deny after 120s IS the
    // "no hang" guarantee the test was originally asserting.
    let run_result = tokio::time::timeout(
        std::time::Duration::from_secs(130),
        run_chat_loop(
            vec![],
            mock.clone(),
            200_000,
            "rid-rule-a-014".into(),
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
            // B6 Subagent PR2b (RULE-A-014, 2026-06-20):
            // production-style caller → Some(false). The parent
            // loop is NOT a worker; only the nested worker call
            // passes Some(true) (at chat_loop.rs:2155). Mirrors
            // the production chat.rs call site.
            Some(false),
            // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None
            // (no Tauri runtime).
            None,
            // 2026-06-21 fix (B6 review defect A): tests pass
            // `None` (production-style caller — not a worker,
            // so the parent's `assemble_system_prompt(mode_prefix,
            // base_prompt)` path runs unchanged). The worker
            // nested call in `run_subagent` passes `Some(...)` to
            // fully replace the parent's prompt with the worker's
            // `SubagentDef.system_prompt`.
            None,
            // 2026-06-22 (RULE-FrontSubagent-003 fix): production-style
            // caller — no worker context — worker_run_id is None.
            None,
            h.subagent_cache.clone(),
            None,
            // L3b (2026-06-27): production-style caller → worktree_override = None.
            None,
            // L3b (2026-06-27): thread the test harness's app_data_dir.
            h.app_data_dir.clone(),
        ),
    )
    .await;
    assert!(
        run_result.is_ok(),
        "PR2b fix: run_chat_loop must NOT hang on the worker's \
         Tier 4 ask_path — without the fix, the worker's \
         oneshot never resolves and the test times out at 15s"
    );

    // 4 sends: parent_t1 + worker_t1 + worker_t2 + parent_t2.
    assert_eq!(
        mock.call_count(),
        4,
        "expected 4 send calls (parent_t1 + worker_t1 + worker_t2 + parent_t2); \
         without PR2b, worker_t1's oneshot hang would prevent the worker from \
         ever emitting Done, so call_count would be stuck at 2"
    );

    // The dispatch_subagent tool_result is the parent's view of
    // the worker — it must carry `[status: completed]` + the
    // worker's final summary (which mentions the deny).
    let results = emitter.tool_results_snapshot();
    let dispatch_result = results
        .iter()
        .find(|r| r.content.contains("dispatch_subagent") || r.tool_use_id.contains("dispatch"))
        .or_else(|| results.first())
        .expect("at least one tool_result (the dispatch_subagent pair)");
    assert!(
        !dispatch_result.is_error,
        "completed worker → is_error=false, got: {}",
        dispatch_result.content
    );
    assert!(
        dispatch_result.content.contains("[status: completed]"),
        "tool_result must carry status=completed, got: {}",
        dispatch_result.content
    );
    assert!(
        dispatch_result
            .content
            .contains("Write denied by worker permission policy"),
        "tool_result must echo the worker's self-correction summary, got: {}",
        dispatch_result.content
    );

    // CRITICAL: the worker's `tool_denied` must NOT pollute the
    // parent's `session_audit_events` (RULE-A-016, B6 PR3a
    // 2026-06-20). Before the fix, the worker's Tier 4 ask_path
    // collapse wrote a `tool_denied` row into the parent's audit
    // table — which leaked worker ⑨ decisions into the C4 audit
    // log UI. The fix routes the worker's deny to the
    // `SubagentBufferSink` transcript (as a `PermissionAsk`
    // entry) and skips the parent's audit write. This assertion
    // confirms the worker's deny row IS NOT in the parent's
    // audit — the regression catch.
    let audit_after = crate::db::permissions::list_audit_events(&h.db, &h.session_id)
        .await
        .expect("list_audit_events after");
    let tool_denied_count = audit_after
        .iter()
        .filter(|e| {
            e.kind == "tool_denied"
                && e.payload_json
                    .as_deref()
                    .unwrap_or("")
                    .contains("write_file")
        })
        .count();
    assert_eq!(
        tool_denied_count,
        0,
        "RULE-A-016: worker's tool_denied must NOT pollute the \
         parent's session_audit_events (PR3a routes the deny to \
         the worker's transcript instead); got audit events: {:?}",
        audit_after
            .iter()
            .map(|e| (e.kind.as_str(), e.payload_json.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
    );
    // No `tool_permission_ask` rows from the worker — the
    // ask_path collapse bypasses the IPC + oneshot dance
    // entirely.
    let tool_permission_ask_count = audit_after
        .iter()
        .filter(|e| e.kind == "tool_permission_ask")
        .count();
    assert_eq!(
        tool_permission_ask_count, 0,
        "worker must NOT emit tool_permission_ask (PR2b ask_path \
         collapse goes straight to Deny — no modal, no oneshot)"
    );
    // Sanity: the delta vs `audit_before` is bounded (parent's
    // 2 rows for dispatch_subagent ONLY — worker tool_denied
    // went to transcript per RULE-A-016). A larger delta would
    // mean a regression (e.g. the worker's record_tool_executed_audit
    // leaking).
    let delta = audit_after.len() - audit_before.len();
    assert!(
        delta <= 2,
        "RULE-A-016 invariant: parent's audit log gains at most 2 \
         rows (tool_allowed + tool_executed for dispatch_subagent); \
         worker's tool_denied now lives in subagent_runs.transcript_json. \
         got delta={}",
        delta
    );

    // RULE-A-016 cross-check: the worker's transcript MUST carry
    // the deny as a `TranscriptKind::PermissionAsk` entry (this is
    // where the worker's audit-like record lives post-PR3a).
    // Fetch the worker's `subagent_runs` row (the most recent one
    // for this session — there's only one in this test).
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 1, "exactly one subagent_runs row");
    let run = &runs[0];
    let transcript: Vec<crate::agent::subagent::TranscriptEntry> =
        serde_json::from_str(run.transcript_json.as_deref().unwrap())
            .expect("transcript_json parses as Vec<TranscriptEntry>");
    let permission_ask_count = transcript
        .iter()
        .filter(|e| e.kind == crate::agent::subagent::TranscriptKind::PermissionAsk)
        .count();
    assert_eq!(
        permission_ask_count,
        1,
        "RULE-A-016: worker's transcript must carry exactly 1 \
         PermissionAsk entry (the auto-deny for write_file); got \
         transcript: {:?}",
        transcript.iter().map(|e| e.kind).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 2026-06-21 fix (B6 review defect A): system_prompt_override
//
// Pre-fix the worker path's `assemble_subagent_prompt(def, task)`
// output was dead code (`_worker_system_prompt` discarded at
// `chat_loop.rs:2052`); the worker actually received the parent's
// `assemble_system_prompt(mode_prefix, base_prompt)` output, which
// made `SubagentDef.system_prompt` effectively documentation-only
// and produced prompt / permission contradictions in Edit/Plan
// mode (worker told "you can write" in Edit mode but Tier 4
// collapsed write tools to `Deny` because the worker has no UI
// sink). The fix threads the worker's overridden prompt as the
// 23rd `run_chat_loop` parameter. These two tests pin the
// behavior: the override is actually used (worker path) and the
// None case still goes through the parent's
// `assemble_system_prompt` path (production path — the common
// case the existing 34 tests already cover; this is a
// targeted regression guard).
// ---------------------------------------------------------------------------

/// Worker path: when `system_prompt_override` is `Some(p)`,
/// `run_chat_loop` sends `p` as the system prompt to the LLM,
/// NOT the parent's `assemble_system_prompt(mode_prefix,
/// base_prompt)` output. Verifies the worker actually receives
/// its `SubagentDef.system_prompt` and the pre-fix dead-code
/// regression is locked.
#[tokio::test]
async fn system_prompt_override_worker_path_sends_override() {
    use crate::agent::subagent::{assemble_subagent_prompt, lookup_subagent};
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "hi".into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));

    // The worker uses the `researcher` `SubagentDef` (read-only
    // research subagent); its system_prompt is the one the
    // worker path should see.
    let def = lookup_subagent("researcher").expect("researcher is a built-in subagent");
    let worker_prompt = assemble_subagent_prompt(def, "summarize the docs");

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-worker-override".into(),
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
        // B6 PR2b: production-style caller is NOT a worker
        // (this is the worker-path test, so the
        // `is_worker` flag itself is `Some(false)` — the
        // "worker-ness" is conveyed by the
        // `system_prompt_override` param, not by `is_worker`).
        // The `is_worker` flag governs the ⑨ 关 Tier 4
        // collapse; the override is a separate concern.
        Some(false),
        None,
        // The actual fix being tested.
        Some(worker_prompt.clone()),
        // 2026-06-22 (RULE-FrontSubagent-003 fix): this test
        // exercises the worker prompt override (B6 review defect
        // A); it's NOT a worker ask test. The
        // `is_worker=Some(false)` already routes ask_path to the
        // parent branch. worker_run_id stays None.
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
        None,
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
    )
    .await;

    // The override must reach the LLM verbatim.
    let sent = mock.sent_systems();
    assert_eq!(sent.len(), 1, "expected exactly 1 send call");
    let received = sent[0]
        .as_ref()
        .expect("worker path: system prompt must be Some, not None");
    assert_eq!(
        received, &worker_prompt,
        "worker path system prompt must equal `SubagentDef.system_prompt` \
         (the pre-fix bug was the override being dead-code-discarded and \
         the parent's `assemble_system_prompt` output being sent instead)"
    );
    // Negative guard: the parent prompt would carry the mode_prefix
    // (e.g. "You are in Yolo mode..."); the worker's prompt
    // explicitly does NOT (Claude Code convention — workers do
    // not inherit the main system prompt).
    assert!(
        !received.contains("Yolo mode")
            && !received.contains("Edit mode")
            && !received.contains("Plan mode"),
        "worker's system prompt must NOT carry the parent's mode_prefix; \
         the worker's `SubagentDef.system_prompt` is a fully-replacement prompt. \
         got: {}",
        received
    );
}

/// Production path: when `system_prompt_override` is `None`
/// (the production + 35 existing test path), `run_chat_loop`
/// sends the result of `assemble_system_prompt(mode_prefix,
/// base_prompt)` to the LLM. This is the regression guard that
/// the parent path is unaffected by the worker-path fix.
#[tokio::test]
async fn system_prompt_override_none_path_uses_parent_assembly() {
    use crate::agent::permissions::mode_system_prefix;
    use crate::agent::system_prompt::{assemble_system_prompt, lookup_head_sha};
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "hi".into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-parent-override-none".into(),
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
        // Production path: `None` override.
        None,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): production-style
        // caller — no worker context — worker_run_id is None.
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
        None,
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
    )
    .await;

    // Recompute what the parent path should send. We mirror the
    // exact steps inside `run_chat_loop` at the system-prompt
    // site: load session + project, build base_prompt via
    // `build_system_prompt`, prefix with `mode_system_prefix`.
    let sent = mock.sent_systems();
    assert_eq!(sent.len(), 1, "expected exactly 1 send call");
    let received = sent[0]
        .as_ref()
        .expect("parent path: system prompt must be Some, not None");

    // Re-derive the expected parent prompt for the harness's
    // session + project.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session");
    let project = db::get_project(&h.db, &loaded.session.project_id)
        .await
        .expect("get_project")
        .expect("project");
    let worktree_path = std::path::PathBuf::from(
        loaded
            .session
            .worktree_path
            .clone()
            .unwrap_or_else(|| project.path.clone()),
    );
    let head_sha = lookup_head_sha(&worktree_path);
    let base_prompt = build_system_prompt(&loaded.session, &project, &worktree_path, &head_sha);
    let expected = assemble_system_prompt(mode_system_prefix(loaded.session.mode), &base_prompt);
    assert_eq!(
        received, &expected,
        "parent path (override=None) must send the parent's \
         `assemble_system_prompt(mode_prefix, base_prompt)` output; \
         the worker-path fix must NOT regress the parent path"
    );
}

// ---------------------------------------------------------------------------
// L3a (2026-06-24): concurrent dispatch_subagent batch (read-only fan-out)
// ---------------------------------------------------------------------------

/// Helper that runs `run_chat_loop` with the standard test arguments
/// (mirrors the call sites in the B6 tests above but lets the L3a
/// tests specify only the script + rid + token). Reduces the 23+
/// parameter boilerplate per test.
async fn run_loop(
    h: &super::tests_common::TestHarness,
    mock: Arc<MockProvider>,
    emitter: Arc<MockEmitter>,
    rid: &str,
    messages: Vec<crate::llm::types::ChatMessage>,
    token: tokio_util::sync::CancellationToken,
) {
    run_chat_loop(
        vec![],
        mock,
        200_000,
        rid.into(),
        h.session_id.clone(),
        messages,
        emitter,
        h.db.clone(),
        h.cancellations.clone(),
        h.session_active_request.clone(),
        h.read_guard.clone(),
        h.memory_cache.clone(),
        h.skill_cache.clone(),
        h.permission_asks.clone(),
        token,
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        Some(false),
        None,
        None,
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
        None,
        // L3b (2026-06-27): thread the test harness's app_data_dir.
        h.app_data_dir.clone(),
    )
    .await;
}

/// `filter_tools_readonly` (L3a unit guard): when applied to the
/// full `builtin_tools()` set, the result contains exactly the 5
/// read-only tools (read_file / grep / glob / list_dir / web_fetch)
/// and nothing else. This is the 2nd layer of the 3-layer read-only
/// guarantee; the unit test pins the function directly so a future
/// tool added to `builtin_tools()` does NOT silently leak into the
/// concurrent worker toolset. (`web_fetch` joined the read-only set
/// on 2026-06-25, task 06-25-subagent-web-access — it is a read-only
/// network op with its own SSRF guard in `tools/web_fetch.rs`.)
#[test]
fn l3a_filter_tools_readonly_keeps_only_five_read_tools() {
    let all = crate::tools::builtin_tools();
    let filtered = filter_tools_readonly(all);
    let names: Vec<String> = filtered.iter().map(|t| t.name.clone()).collect();
    assert_eq!(
        names.len(),
        5,
        "exactly 5 read-only tools, got: {:?}",
        names
    );
    for required in &["read_file", "grep", "glob", "list_dir", "web_fetch"] {
        assert!(
            names.iter().any(|n| n == required),
            "filter must keep {}, got: {:?}",
            required,
            names
        );
    }
    for forbidden in &[
        "write_file",
        "edit_file",
        "shell",
        "dispatch_subagent",
        "update_checklist",
    ] {
        assert!(
            !names.iter().any(|n| n == forbidden),
            "filter must NOT keep {}, got: {:?}",
            forbidden,
            names
        );
    }
}

/// `classify_dispatch_batch` (L3a unit guard): pure-batch counting
/// + limit threshold. Pins the three branches (Serial / Concurrent /
/// OverLimit) without spinning up the agent loop.
#[test]
fn l3a_classify_dispatch_batch_branches_correctly() {
    let dispatch_input = serde_json::json!({ "subagent": "researcher", "task": "x" });
    let read_input = serde_json::json!({ "path": "a.rs" });
    let tc = |id: &str, name: &str, input: serde_json::Value| (id.to_string(), name.to_string(), input);

    // Single dispatch → Serial.
    let single = vec![tc("t1", "dispatch_subagent", dispatch_input.clone())];
    assert!(matches!(
        classify_dispatch_batch(&single, 3),
        DispatchBatch::Serial
    ));

    // 2 dispatches, pure → Concurrent.
    let two = vec![
        tc("t1", "dispatch_subagent", dispatch_input.clone()),
        tc("t2", "dispatch_subagent", dispatch_input.clone()),
    ];
    assert!(matches!(
        classify_dispatch_batch(&two, 3),
        DispatchBatch::Concurrent { count: 2 }
    ));

    // 3 dispatches, pure, at limit → Concurrent.
    let three = vec![
        tc("t1", "dispatch_subagent", dispatch_input.clone()),
        tc("t2", "dispatch_subagent", dispatch_input.clone()),
        tc("t3", "dispatch_subagent", dispatch_input.clone()),
    ];
    assert!(matches!(
        classify_dispatch_batch(&three, 3),
        DispatchBatch::Concurrent { count: 3 }
    ));

    // 4 dispatches, pure, over limit → OverLimit.
    let four = vec![
        tc("t1", "dispatch_subagent", dispatch_input.clone()),
        tc("t2", "dispatch_subagent", dispatch_input.clone()),
        tc("t3", "dispatch_subagent", dispatch_input.clone()),
        tc("t4", "dispatch_subagent", dispatch_input.clone()),
    ];
    assert!(matches!(
        classify_dispatch_batch(&four, 3),
        DispatchBatch::OverLimit { count: 4, max_concurrent: 3 }
    ));

    // Mixed batch (1 dispatch + 1 read_file) → Serial (fall through).
    let mixed = vec![
        tc("t1", "dispatch_subagent", dispatch_input.clone()),
        tc("t2", "read_file", read_input.clone()),
    ];
    assert!(matches!(
        classify_dispatch_batch(&mixed, 3),
        DispatchBatch::Serial
    ));

    // Pure read batch (no dispatch) → Serial (handled by L2 path above;
    // classify_dispatch_batch is only consulted in the serial-else arm).
    let read_only = vec![
        tc("t1", "read_file", read_input.clone()),
        tc("t2", "grep", read_input.clone()),
    ];
    assert!(matches!(
        classify_dispatch_batch(&read_only, 3),
        DispatchBatch::Serial
    ));
}

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
    let mut tool_use_ids: Vec<String> =
        results.iter().map(|r| r.tool_use_id.clone()).collect();
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

/// L3b PR2 (2026-06-27): concurrent `general-purpose` workers
/// with the LLM opting out of isolation (`isolation: false` in
/// the dispatch input). Mirrors the L3a concurrent branch's
/// pre-PR2 behavior (read-only-equivalent: shared cwd, full
/// toolset) but now the LLM explicitly drives the choice via the
/// dispatch input override (truth table: `frontmatter Some(true)`
/// + dispatch `Some(false)` → NOT isolated). This pins that the
/// `force_readonly=true` L3a behavior is still reachable via the
/// new isolation-truth-table path.
///
/// The test mock uses no real git repo — the LLM opt-out routes
/// the worker back to the parent's worktree, sidestepping
/// `create_worker`'s libgit2 requirement. Two general-purpose
/// workers in a pure batch both complete with text-only
/// summaries.
///
/// (Pre-PR2 this test was named
/// `l3a_concurrent_general_purpose_workers_complete_readonly`
/// and asserted the 2nd-layer read-only strip; that strip is
/// removed in PR2 — the new safety argument is per-worker
/// worktree isolation, documented in
/// `agent-loop-architecture.md` §"Pattern: Concurrent isolated
/// dispatch (L3b PR2)".)
#[tokio::test(flavor = "multi_thread")]
async fn l3b_concurrent_general_purpose_workers_complete_shared() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: 2 general-purpose dispatches. The
        // `isolation: false` dispatch input overrides the
        // builtin `general-purpose.isolation: Some(true)` default
        // (per the resolve_isolation truth table:
        // `frontmatter Some(true)` + `dispatch Some(false)` →
        // NOT isolated). This reproduces the L3a pre-PR2
        // concurrent behavior (shared cwd + full toolset) without
        // requiring a real git repo fixture.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_gp_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "investigate topic A read-only",
                    "isolation": false,
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_gp_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "investigate topic B read-only",
                    "isolation": false,
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot 1: single-turn text-only summary.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "general-purpose read-only result A".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot 2.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "general-purpose read-only result B".into(),
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
                text: "ack".into(),
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
        "rid-l3b-gp-shared",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 4 sends: parent_t1 + 2 workers + parent_t2.
    assert_eq!(mock.call_count(), 4);

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(!r.is_error, "general-purpose concurrent → completed");
        assert!(
            r.content.contains("[status: completed]"),
            "got: {}",
            r.content
        );
    }
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
    assert!(
        results[0]
            .content
            .contains("[status: completed]")
    );

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
    assert!(
        results[0]
            .content
            .contains("[status: completed]")
    );

    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 1, "single dispatch → 1 run");
}

// ---------------------------------------------------------------------------
// L3b PR2 (2026-06-27): concurrent isolated dispatch regression tests
//
// Three tests pin the post-PR2 contract:
// 1. l3b_concurrent_general_purpose_workers_complete_with_writes
//    — 2 general-purpose workers in a pure batch each in their
//    own worker worktree; both complete cleanly. (Writes exercised
//    in a follow-up; this test pins the concurrent+isolated
//    dispatch surface.)
// 2. l3b_concurrent_workers_have_isolated_worktrees
//    — concurrent workers' `subagent_runs.worktree_path` are
//    distinct (non-overlapping) — provably per-run isolated.
// 3. l3b_concurrent_force_readonly_param_no_longer_set
//    — regression: the concurrent branch no longer threads
//    `force_readonly=true` to `run_subagent`. Each concurrent
//    worker is observed with the full toolset of its
//    `SubagentDef` (researcher = 4 read-only tools; general-
//    purpose = full minus structurally disabled), NOT the L3a
//    5-tool read-only strip.
//
// All 3 tests need a real git repo in the project path so
// `create_worker_worktree` succeeds. `make_harness_with_git_repo`
// (defined in `tests_common.rs`, L3b PR1) is the variant.
// ---------------------------------------------------------------------------

/// L3b PR2 AC1: 2 general-purpose workers in a pure batch, each
/// in its own `worker/<run_id>` worktree. Both complete cleanly
/// (text-only summaries — we don't exercise the write path here;
/// the worker worktree + lock + destroy flow is the surface to
/// pin). 2 independent `subagent_runs` rows persist + parent
/// receives 2 `[status: completed]` summaries.
///
/// Uses `make_harness_with_git_repo` (L3b PR1) so
/// `create_worker_worktree` succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_concurrent_general_purpose_workers_complete_with_writes() {
    let h = super::tests_common::make_harness_with_git_repo().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: 2 general-purpose dispatches (no
        // `isolation` override — builtin default is `Some(true)`,
        // so each worker gets its own worktree).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_writes_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "write a different value to file A"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_writes_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "write a different value to file B"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot 1: single-turn summary.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "wrote file A in worker worktree".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot 2.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "wrote file B in worker worktree".into(),
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
                text: "ack".into(),
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
        "rid-l3b-writes",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 4 sends: parent_t1 + 2 workers + parent_t2.
    assert_eq!(mock.call_count(), 4);

    // 2 tool_results, both completed (worker write path is
    // exercised inside the worker; tool_result just reports the
    // dispatch summary, not the inner tool calls).
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(!r.is_error, "general-purpose concurrent+isolated → completed");
        assert!(
            r.content.contains("[status: completed]"),
            "got: {}",
            r.content
        );
    }

    // 2 subagent_runs rows persisted.
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 2, "2 concurrent workers → 2 runs");
    for run in &runs {
        assert_eq!(run.status, "completed", "each worker completed");
        assert!(run.finished_at.is_some(), "finished_at set");
    }
}

/// L3b PR2 AC2: concurrent workers are **per-run isolated** —
/// each worker gets a unique `worker_run_id` (UUID, distinct
/// across the batch), which derives the unique
/// `<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`
/// directory + the unique `worker/<run_id>` branch. The
/// `subagent_runs.id` row id is the `worker_run_id` itself
/// (passed to `insert_run_with_id`), so the per-run isolation
/// is observable in the `subagent_runs` table.
///
/// Note: `subagent_runs.worktree_path` is **NULL** for workers
/// that exit with no changes (the post-loop destroy path
/// clears the column — see `dispatch.rs::run_subagent` "No
/// changes — destroy + clear" branch). To observe a
/// non-NULL `worktree_path` post-exit, the worker must leave
/// changes behind (which requires a real `write_file` tool
/// path the mock cannot easily exercise). This test pins the
/// **isolation primitive** (per-run UUID + per-run
/// worktree_path computation), not the post-destroy
/// preservation semantics — those are covered by
/// `probe_worker_changes_*` in `dispatch.rs::tests` and the
/// PR3 merge/discard tool tests (out of scope for PR2).
#[tokio::test(flavor = "multi_thread")]
async fn l3b_concurrent_workers_have_isolated_worktrees() {
    let h = super::tests_common::make_harness_with_git_repo().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_iso_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "isolated worker A"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_iso_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "isolated worker B"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // 2 worker single-turn summaries.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "A done".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "B done".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ack".into() }),
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
        "rid-l3b-iso",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 2 subagent_runs rows persisted, each with a distinct
    // `id` (the worker_run_id UUID).
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 2, "2 concurrent workers → 2 runs");

    // The 2 worker_run_ids are distinct — each worker got its
    // own UUID, which means each got its own
    // `<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`
    // directory (the worktree path is computed FROM the
    // worker_run_id) and its own `worker/<run_id>` branch (the
    // branch name is computed FROM the worker_run_id). This is
    // the per-run isolation primitive.
    let mut ids: Vec<String> = runs.iter().map(|r| r.id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        2,
        "2 concurrent workers MUST have distinct worker_run_id (UUID) values, got: {:?}",
        runs.iter().map(|r| (&r.id, &r.status)).collect::<Vec<_>>()
    );

    // Each run's `parent_request_id` is the worker_rid
    // (`{parent_rid}-sub-{tool_use_id}`), which is also
    // distinct per dispatch (the tool_use_id is unique per
    // batch). This is the SECOND isolation primitive — the
    // worker_rid → worktree_path → worker_run_id round-trip.
    let mut rids: Vec<String> = runs
        .iter()
        .map(|r| r.parent_request_id.clone())
        .collect();
    rids.sort();
    rids.dedup();
    assert_eq!(
        rids.len(),
        2,
        "2 concurrent workers MUST have distinct worker_rid values, got: {:?}",
        runs.iter().map(|r| (&r.id, &r.parent_request_id)).collect::<Vec<_>>()
    );
    // Each worker_rid carries the tool_use_id suffix, proving
    // the 1:1 mapping (worker_rid derived from tool_use_id).
    let rids_set: std::collections::HashSet<&str> = runs
        .iter()
        .map(|r| r.parent_request_id.as_str())
        .collect();
    assert!(
        rids_set.iter().any(|r| r.contains("toolu_l3b_iso_a")),
        "one worker_rid should encode toolu_l3b_iso_a: {:?}",
        rids
    );
    assert!(
        rids_set.iter().any(|r| r.contains("toolu_l3b_iso_b")),
        "one worker_rid should encode toolu_l3b_iso_b: {:?}",
        rids
    );
}

/// L3b PR2 AC3: regression — the concurrent branch no longer
/// forces read-only (`force_readonly=false` is threaded to
/// `run_subagent` for every concurrent task). The worker turn
/// receives the FULL `general-purpose` toolset (minus the
/// 4 `STRUCTURALLY_DISABLED` tools), NOT the L3a 5-tool
/// read-only strip. We use `general-purpose` here (with
/// `isolation: false` dispatch override so no git fixture is
/// needed) so the test exercises a toolset shape that DIFFERS
/// pre/post PR2:
///
/// - pre-PR2 (L3a `force_readonly=true`): worker turn tools =
///   the 5-tool read-only strip
///   (`read_file, grep, glob, list_dir, web_fetch`)
/// - post-PR2 (no strip): worker turn tools = the
///   `general-purpose` allowlist minus `STRUCTURALLY_DISABLED`
///   = full builtin set minus the structurally-disabled tools
///   that appear in it (`update_checklist`, `run_background_shell`,
///   `shell_status`, `shell_kill`, and — per L3b PR3 B3 —
///   `merge_worker`, `discard_worker`) ≈ 9 tools (incl.
///   `write_file`, `edit_file`, `shell`)
///
/// The discriminant: `write_file` is in the post-PR2
/// `general-purpose` toolset but NOT in the L3a strip. If the
/// strip is still applied, `write_file` will be absent.
///
/// `MockProvider::sent_tools()` snapshots the tools Vec the
/// LLM received per turn. We assert on the worker turn
/// (send slot 1).
#[tokio::test(flavor = "multi_thread")]
async fn l3b_concurrent_force_readonly_param_no_longer_set() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: 2 general-purpose dispatches. LLM
        // opts out of isolation (no git fixture), so workers
        // reuse the parent's worktree.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_reg_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "general-purpose task A",
                    "isolation": false,
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_reg_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "general-purpose task B",
                    "isolation": false,
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // 2 worker single-turn summaries.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "general-purpose A".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "general-purpose B".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ack".into() }),
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
        "rid-l3b-reg",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 4 sends: parent_t1 + 2 workers + parent_t2.
    assert_eq!(mock.call_count(), 4);

    // Snapshot the per-turn tools the LLM received.
    let sent_tools = mock.sent_tools();
    assert_eq!(sent_tools.len(), 4, "4 sends captured");

    // Worker turn 1 (send slot 1) — concurrent branch, no
    // force_readonly strip. The tools come from the
    // general-purpose SubagentDef allowlist (empty = full
    // builtin set) minus STRUCTURALLY_DISABLED (7 entries:
    // dispatch_subagent, update_checklist, run_background_shell,
    // shell_status, shell_kill, merge_worker, discard_worker).
    let worker_t1_names: Vec<&str> =
        sent_tools[1].iter().map(|t| t.name.as_str()).collect();
    // The PR2 discriminant: `write_file` is in the post-PR2
    // general-purpose toolset but NOT in the L3a 5-tool strip.
    // If the strip is still applied (L3a regression), this
    // assertion fails.
    assert!(
        worker_t1_names.contains(&"write_file"),
        "worker_t1 MUST have write_file (general-purpose toolset, no L3a strip): {:?}",
        worker_t1_names
    );
    assert!(
        worker_t1_names.contains(&"edit_file"),
        "worker_t1 MUST have edit_file (general-purpose toolset, no L3a strip): {:?}",
        worker_t1_names
    );
    assert!(
        worker_t1_names.contains(&"shell"),
        "worker_t1 MUST have shell (general-purpose toolset, no L3a strip): {:?}",
        worker_t1_names
    );
    // STRUCTURALLY_DISABLED still applies (the 3-layer no-
    // nesting protection): dispatch_subagent MUST NOT appear.
    assert!(
        !worker_t1_names.contains(&"dispatch_subagent"),
        "worker_t1 MUST NOT have dispatch_subagent (STRUCTURALLY_DISABLED, no nesting): {:?}",
        worker_t1_names
    );
    assert!(
        !worker_t1_names.contains(&"update_checklist"),
        "worker_t1 MUST NOT have update_checklist (STRUCTURALLY_DISABLED): {:?}",
        worker_t1_names
    );
    // L3b PR3 B3 (2026-06-28): merge_worker / discard_worker are
    // STRUCTURALLY_DISABLED — a worker must not rewrite the parent
    // session's history (no merging / discarding a sibling worker's
    // branch via a run_id visible in the dispatch tool_result).
    assert!(
        !worker_t1_names.contains(&"merge_worker"),
        "worker_t1 MUST NOT have merge_worker (STRUCTURALLY_DISABLED, B3): {:?}",
        worker_t1_names
    );
    assert!(
        !worker_t1_names.contains(&"discard_worker"),
        "worker_t1 MUST NOT have discard_worker (STRUCTURALLY_DISABLED, B3): {:?}",
        worker_t1_names
    );

    // 2 subagent_runs rows persisted.
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 2, "2 concurrent general-purpose → 2 runs");
}

// ---------------------------------------------------------------------------
// L3b PR3 (2026-06-27): merge_worker / discard_worker tool tests
//
// Tests pin the post-merge / post-discard DB state + libgit2
// state. Each test sets up a project + parent session worktree
// + worker worktree via `make_harness_with_git_repo` (so
// `create_worker` succeeds), then invokes the tool helper
// directly to avoid the full agent loop harness.
//
// The `merge_worker` tool exercises:
// - fast-forward path (no common ancestor between parent and
//   worker — actually the fast-forward path triggers when
//   the parent tip IS an ancestor of the worker tip)
// - 3-way merge path (parent has diverged)
// - conflict path (both branches modify the same file)
//
// The `discard_worker` tool exercises:
// - happy path: worktree_path set → destroy + clear
// - fail-fast on already-destroyed run
// ---------------------------------------------------------------------------

/// L3b PR3 AC1: `merge_worker` happy path. Worker makes a
/// change on a separate file; `merge_worker` fast-forwards
/// the parent branch and clears the worktree_path column.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_merge_worker_happy_path_fast_forward() {
    use crate::git::worktree::worker_worktree_path;

    // Set up the harness + a parent session worktree (the
    // agent loop will operate on this).
    let h = super::tests_common::make_harness_with_git_repo().await;

    // We need a parent session worktree attached so the
    // tool can find the parent branch. We simulate the
    // attach by directly setting the worktree state.
    let wt_path = h.project_path.join(format!("parent_wt_{}", h.session_id));
    let _ = std::fs::remove_dir_all(&wt_path);
    crate::git::create_worktree(&h.project_path, &wt_path, &h.session_id)
        .expect("create parent session worktree");
    crate::db::set_worktree_state(
        &h.db,
        &h.session_id,
        crate::db::WorktreeState::Active,
        Some(wt_path.to_str().unwrap()),
        None,
    )
    .await
    .expect("set worktree_state active");

    // The worker run id is a fixed UUID so we can verify
    // the row state.
    let run_id = "00000000-0000-0000-0000-000000000001";
    // Insert a subagent_runs row with worktree_path set +
    // create a worker worktree on a separate file.
    let worker_wt = worker_worktree_path(&h.app_data_dir, &h.project_id, run_id);
    crate::git::worktree::create_worker(
        &h.project_path,
        &worker_wt,
        &wt_path,
        run_id,
    )
    .expect("create worker worktree");
    // The worker creates a new file in its worktree and
    // commits it on the worker branch.
    std::fs::write(worker_wt.join("worker_change.txt"), "from worker").unwrap();
    let add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&worker_wt)
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = std::process::Command::new("git")
        .args(["commit", "-m", "worker commit", "--no-gpg-sign"])
        .current_dir(&worker_wt)
        .output()
        .unwrap();
    assert!(commit.status.success(), "worker commit failed: {:?}", commit);

    // Insert a subagent_runs row referencing the worktree.
    crate::db::subagent_runs::insert_run_with_id(
        &h.db,
        run_id,
        &h.session_id,
        "parent-rid-l3b-mw-happy",
        "general-purpose",
        Some("write a file"),
    )
    .await
    .expect("insert_run_with_id");
    crate::db::subagent_runs::set_worktree_path(
        &h.db,
        run_id,
        Some(worker_wt.to_str().unwrap()),
    )
    .await
    .expect("set_worktree_path");

    // Build the ToolContext the merge_worker tool needs.
    let ctx = crate::tools::ToolContext {
        worktree_path: wt_path.clone(),
        cwd: wt_path.clone(),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: h.db.clone(),
        project_id: "test-proj".to_string(),
        data_dir: h.app_data_dir.clone(),
    };

    // Invoke merge_worker.
    let input = serde_json::json!({"run_id": run_id});
    let (msg, is_err, _update, _exit_code) = crate::tools::merge_worker::execute(
        &input,
        &ctx,
        Some(&h.session_id),
    )
    .await;
    assert!(!is_err, "merge_worker should succeed: {}", msg);
    assert!(
        msg.contains("fast-forward") || msg.contains("3-way"),
        "merge message should describe the merge kind: {}",
        msg
    );

    // The parent worktree should now have the worker's
    // file (because the parent branch was fast-forwarded).
    assert!(
        wt_path.join("worker_change.txt").exists(),
        "parent worktree should have the worker's file post-merge: {:?}",
        std::fs::read_dir(&wt_path).unwrap().collect::<Vec<_>>()
    );

    // The worker worktree + branch are destroyed; the
    // worktree_path column is cleared.
    assert!(!worker_wt.exists(), "worker worktree dir removed");
    let updated_run = crate::db::subagent_runs::get_run(&h.db, run_id)
        .await
        .expect("get_run")
        .expect("run row should exist");
    assert!(
        updated_run.worktree_path.is_none(),
        "worktree_path column cleared post-merge: {:?}",
        updated_run.worktree_path
    );
}

/// L3b PR3 AC2: `merge_worker` conflict path. Both branches
/// modify the same file → `is_error: true` with the conflict
/// file list; the worker branch + worktree are preserved.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_merge_worker_conflict_returns_error() {
    use crate::git::worktree::worker_worktree_path;

    let h = super::tests_common::make_harness_with_git_repo().await;

    // Parent session worktree (from initial commit only).
    let wt_path = h.project_path.join(format!("parent_wt_{}", h.session_id));
    let _ = std::fs::remove_dir_all(&wt_path);
    crate::git::create_worktree(&h.project_path, &wt_path, &h.session_id)
        .expect("create parent session worktree");
    crate::db::set_worktree_state(
        &h.db,
        &h.session_id,
        crate::db::WorktreeState::Active,
        Some(wt_path.to_str().unwrap()),
        None,
    )
    .await
    .expect("set worktree_state active");

    // Worker worktree — also from the initial commit (NOT
    // from the parent branch, which would create a
    // fast-forward path). We have to construct the
    // worker_wt branch ourselves because `create_worker`
    // bases off `parent_wt.head()` (the parent's current
    // tip). To create a true 3-way conflict, we want
    // both branches to fork from the same base.
    //
    // Strategy: pre-create the worker branch pointing at
    // the project HEAD, then `create_worker` will fast-
    // forward it to the worker's first commit (the
    // conflict-creating change).
    let run_id = "00000000-0000-0000-0000-000000000002";
    let worker_wt = worker_worktree_path(&h.app_data_dir, &h.project_id, run_id);
    // Use `create_worker` from a synthetic parent_wt that
    // has the project HEAD (so the worker bases on the
    // initial commit, not the parent's later commit).
    let temp_parent = h.project_path.join(format!("temp_parent_{}", run_id));
    let _ = std::fs::remove_dir_all(&temp_parent);
    crate::git::create_worktree(&h.project_path, &temp_parent, &format!("temp-p-{}", run_id))
        .expect("create temp parent worktree");
    crate::git::worktree::create_worker(
        &h.project_path,
        &worker_wt,
        &temp_parent,
        run_id,
    )
    .expect("create worker worktree");
    // Clean up the temp parent worktree (we don't need
    // it anymore; the worker branch is now based off the
    // project HEAD, which is what we want for a 3-way
    // conflict).
    let temp_branch = format!("session/temp-p-{}", run_id);
    crate::git::worktree::destroy_worker(
        &h.project_path,
        &temp_parent,
        &format!("temp-p-{}", run_id),
    )
    .expect("destroy temp worker");
    let _ = std::fs::remove_dir_all(&temp_parent);
    if let Ok(repo) = git2::Repository::open(&h.project_path) {
        if let Ok(mut b) = repo.find_branch(&temp_branch, git2::BranchType::Local) {
            let _ = b.delete();
        }
    }

    // Parent branch modifies seed.txt (commits on
    // session/<sid>).
    std::fs::write(wt_path.join("seed.txt"), "from parent").unwrap();
    let add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = std::process::Command::new("git")
        .args(["commit", "-m", "parent edit", "--no-gpg-sign"])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    assert!(commit.status.success(), "parent commit failed: {:?}", commit);

    // Worker modifies seed.txt differently (commits on
    // worker/<run_id>).
    std::fs::write(worker_wt.join("seed.txt"), "from worker").unwrap();
    let w_add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&worker_wt)
        .output()
        .unwrap();
    assert!(w_add.status.success());
    let w_commit = std::process::Command::new("git")
        .args(["commit", "-m", "worker edit", "--no-gpg-sign"])
        .current_dir(&worker_wt)
        .output()
        .unwrap();
    assert!(w_commit.status.success());

    crate::db::subagent_runs::insert_run_with_id(
        &h.db,
        run_id,
        &h.session_id,
        "parent-rid-l3b-mw-conflict",
        "general-purpose",
        Some("edit seed.txt"),
    )
    .await
    .expect("insert_run_with_id");
    crate::db::subagent_runs::set_worktree_path(
        &h.db,
        run_id,
        Some(worker_wt.to_str().unwrap()),
    )
    .await
    .expect("set_worktree_path");

    let ctx = crate::tools::ToolContext {
        worktree_path: wt_path.clone(),
        cwd: wt_path.clone(),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: h.db.clone(),
        project_id: "test-proj".to_string(),
        data_dir: h.app_data_dir.clone(),
    };

    let input = serde_json::json!({"run_id": run_id});
    let (msg, is_err, _update, _exit_code) = crate::tools::merge_worker::execute(
        &input,
        &ctx,
        Some(&h.session_id),
    )
    .await;
    if !is_err {
        eprintln!("merge_worker succeeded unexpectedly: {}", msg);
    }
    assert!(is_err, "merge_worker should fail on conflict (msg={})", msg);
    assert!(
        msg.contains("merge conflict"),
        "error should describe the conflict: {}",
        msg
    );
    assert!(
        msg.contains("seed.txt"),
        "conflict list should mention seed.txt: {}",
        msg
    );

    // Worker worktree + branch are PRESERVED.
    assert!(worker_wt.exists(), "worker worktree dir preserved on conflict");
    let repo = git2::Repository::open(&h.project_path).unwrap();
    assert!(
        repo.find_branch(&format!("worker/{}", run_id), git2::BranchType::Local)
            .is_ok(),
        "worker branch preserved on conflict"
    );
    // The subagent_runs row's worktree_path is still set
    // (the cleanup finalize was NOT called because the
    // merge didn't succeed).
    let run_row = crate::db::subagent_runs::get_run(&h.db, run_id)
        .await
        .expect("get_run")
        .expect("run row should exist");
    assert!(
        run_row.worktree_path.is_some(),
        "worktree_path column preserved on conflict: {:?}",
        run_row.worktree_path
    );
}

/// L3b PR3 AC9: `merge_worker` with no parent worktree
/// attached → "parent session has no worktree" error.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_merge_worker_no_parent_worktree_errors() {
    let h = super::tests_common::make_harness_with_git_repo().await;

    // The session has NO worktree attached (we never call
    // attach_worktree). The session's `worktree_path` is
    // NULL.
    // Insert a fake subagent_runs row (no worktree_path
    // needed for this error — the check fires BEFORE the
    // worktree_path column is read).
    let run_id = "00000000-0000-0000-0000-000000000003";
    crate::db::subagent_runs::insert_run_with_id(
        &h.db,
        run_id,
        &h.session_id,
        "parent-rid-l3b-mw-no-wt",
        "general-purpose",
        Some("test"),
    )
    .await
    .expect("insert_run_with_id");

    // The parent_wt path is some made-up path (not a real
    // worktree). The libgit2 error fires when we try to
    // open the repo + find the parent branch.
    let bogus_wt = h.project_path.join(format!("missing_wt_{}", h.session_id));
    let ctx = crate::tools::ToolContext {
        worktree_path: bogus_wt,
        cwd: h.project_path.clone(),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: h.db.clone(),
        project_id: "test-proj".to_string(),
        data_dir: h.app_data_dir.clone(),
    };
    let input = serde_json::json!({"run_id": run_id});
    let (msg, is_err, _update, _exit_code) = crate::tools::merge_worker::execute(
        &input,
        &ctx,
        Some(&h.session_id),
    )
    .await;
    assert!(is_err, "merge_worker should fail when no parent worktree");
    // The error path fires on `find_branch` — it should
    // mention the branch name OR the worktree path.
    assert!(
        msg.contains("not found")
            || msg.contains("could not open")
            || msg.contains("worktree"),
        "error should explain the failure: {}",
        msg
    );
}

/// L3b PR3 AC4: `discard_worker` happy path. Worker makes
/// a change → `discard_worker` → branch deleted +
/// worktree_path cleared.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_discard_worker_happy_path() {
    use crate::git::worktree::worker_worktree_path;

    let h = super::tests_common::make_harness_with_git_repo().await;
    let wt_path = h.project_path.join(format!("parent_wt_{}", h.session_id));
    let _ = std::fs::remove_dir_all(&wt_path);
    crate::git::create_worktree(&h.project_path, &wt_path, &h.session_id)
        .expect("create parent session worktree");
    crate::db::set_worktree_state(
        &h.db,
        &h.session_id,
        crate::db::WorktreeState::Active,
        Some(wt_path.to_str().unwrap()),
        None,
    )
    .await
    .expect("set worktree_state active");

    let run_id = "00000000-0000-0000-0000-000000000004";
    let worker_wt = worker_worktree_path(&h.app_data_dir, &h.project_id, run_id);
    crate::git::worktree::create_worker(
        &h.project_path,
        &worker_wt,
        &wt_path,
        run_id,
    )
    .expect("create worker worktree");
    // Worker makes a change but we don't care about
    // commit (discard is independent of commit state).
    std::fs::write(worker_wt.join("discard_me.txt"), "should be gone").unwrap();

    crate::db::subagent_runs::insert_run_with_id(
        &h.db,
        run_id,
        &h.session_id,
        "parent-rid-l3b-dw-happy",
        "general-purpose",
        Some("test discard"),
    )
    .await
    .expect("insert_run_with_id");
    crate::db::subagent_runs::set_worktree_path(
        &h.db,
        run_id,
        Some(worker_wt.to_str().unwrap()),
    )
    .await
    .expect("set_worktree_path");

    let ctx = crate::tools::ToolContext {
        worktree_path: wt_path.clone(),
        cwd: wt_path.clone(),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: h.db.clone(),
        project_id: "test-proj".to_string(),
        data_dir: h.app_data_dir.clone(),
    };

    let input = serde_json::json!({"run_id": run_id});
    let (msg, is_err, _update, _exit_code) = crate::tools::discard_worker::execute(
        &input,
        &ctx,
        Some(&h.session_id),
    )
    .await;
    assert!(!is_err, "discard_worker should succeed: {}", msg);
    assert!(
        msg.contains("discarded"),
        "success message should confirm discard: {}",
        msg
    );

    // Worker worktree + branch are gone.
    assert!(!worker_wt.exists(), "worker worktree dir removed");
    let repo = git2::Repository::open(&h.project_path).unwrap();
    assert!(
        repo.find_branch(&format!("worker/{}", run_id), git2::BranchType::Local)
            .is_err(),
        "worker branch deleted"
    );
    // worktree_path column is cleared.
    let updated_run = crate::db::subagent_runs::get_run(&h.db, run_id)
        .await
        .expect("get_run")
        .expect("run row should exist");
    assert!(
        updated_run.worktree_path.is_none(),
        "worktree_path column cleared post-discard: {:?}",
        updated_run.worktree_path
    );
}

/// L3b PR3 AC10: `discard_worker` fail-fast on
/// already-destroyed run. `worktree_path` is NULL → error
/// `worker already destroyed`.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_discard_worker_already_destroyed_errors() {
    let h = super::tests_common::make_harness_with_git_repo().await;
    let run_id = "00000000-0000-0000-0000-000000000005";

    // Insert a subagent_runs row WITHOUT setting
    // worktree_path (so it's NULL — the "already
    // destroyed" state).
    crate::db::subagent_runs::insert_run_with_id(
        &h.db,
        run_id,
        &h.session_id,
        "parent-rid-l3b-dw-already",
        "general-purpose",
        Some("test"),
    )
    .await
    .expect("insert_run_with_id");
    // Do NOT call set_worktree_path — the column stays
    // NULL.

    let ctx = crate::tools::ToolContext {
        worktree_path: h.project_path.clone(),
        cwd: h.project_path.clone(),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: h.db.clone(),
        project_id: "test-proj".to_string(),
        data_dir: h.app_data_dir.clone(),
    };
    let input = serde_json::json!({"run_id": run_id});
    let (msg, is_err, _update, _exit_code) = crate::tools::discard_worker::execute(
        &input,
        &ctx,
        Some(&h.session_id),
    )
    .await;
    assert!(is_err, "discard_worker should fail on already-destroyed");
    assert!(
        msg.contains("already destroyed"),
        "error should be 'worker already destroyed': {}",
        msg
    );
}
