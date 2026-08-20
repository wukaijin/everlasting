#![cfg(test)]

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

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
        None,
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
        None,
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
        None,
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
        None,
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
        v.get("cache_creation_input_tokens")
            .and_then(|x| x.as_i64()),
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
