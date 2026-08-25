#![cfg(test)]

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

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

    // Wrap the run in a task-local 50ms ask-timeout scope so the
    // worker's ASK_TIMEOUT arm fires in milliseconds (not the
    // production 120s). task-local (not thread-local) because this
    // test runs under `flavor = "multi_thread"` and the worker's
    // `ask_path` may resume on a different OS thread after an
    // `.await`; the worker runs in-task (dispatch.rs uses `Box::pin`,
    // not `tokio::spawn`), so the parent task's scope is visible all
    // the way down. See `with_ask_timeout_for_test` in `ask.rs`.
    //
    // The outer `tokio::time::timeout(2s)` is a hang backstop — ample
    // headroom over the 50ms inner timeout while still failing fast if
    // the worker's ask round-trip regresses into a real hang (the
    // pre-PR2b symptom). Was 130s under the real-timeout design.
    let run_result = crate::agent::permissions::ask::with_ask_timeout_for_test(
        std::time::Duration::from_millis(50),
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run_chat_loop(
                vec![],
                mock.clone(),
                200_000,
                None,
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
                std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
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
                None, // project_main_override (2026-07-29)
                // L3b (2026-06-27): thread the test harness's app_data_dir.
                h.app_data_dir.clone(),
                None,
                // 2026-06-30 (ask_user_question task): per-test QuestionStore.
                h.question_store.clone(),
                // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
                // workflow_ctx = None (tests don't exercise the workflow
                // breadcrumb injection seam; that lives in separate
                // `agent::workflow::inject` tests).
                None,
                // group_chat_state = None (tests don't exercise group chat).
                None,
                None,
                // D (2026-08-14): stub loaded-set registry(测试默认
                // 开关 off 路径不 stub;interception 测试用 harness 的)。
                h.stub_loaded.clone(),
                // F1 queue driver (2026-08-25): single-shot call site —
                // guard-owned cleanup (not a continuation round).
                false,
            ),
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
