#![cfg(test)]

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
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
/// explicit-agent-dispatch (2026-06-30): a forced `@@` dispatch
/// bypasses the parent LLM entirely. The loop short-circuits at
/// turn 1 — it never calls `provider.stream` for the parent; it
/// dispatches the named worker directly via the same `run_subagent`
/// path the LLM-driven interceptor uses, then emits the worker's
/// summary as the turn's assistant text and exits.
///
/// Invariants locked:
/// - `mock.call_count() == 1` — ONLY the worker's single turn. The
///   parent contributed zero LLM calls (the short-circuit). This is
///   the core "explicit dispatch" guarantee.
/// - One dispatch_subagent tool:result carrying `[status: completed]`
///   + the worker's summary.
/// - The assistant turn is persisted carrying the summary text (so a
///   reload shows the result in the main conversation).
#[tokio::test]
async fn agent_loop_forced_dispatch_runs_worker_without_llm() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // Only the worker's response is scripted — the parent never
    // calls the provider (the forced short-circuit skips provider.stream).
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: "found 3 files".into(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        None,
        "rid-forced".into(),
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
        None, // resend_seq
        h.background_shells.clone(),
        None,                                                                 // max_turns
        false,                                                                // skip_session_active
        false,                                                                // skip_persist
        Some(false),                                                          // is_worker
        None,                                                                 // worker_catalog
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        None, // system_prompt_override
        None, // worker_run_id
        h.subagent_cache.clone(),
        None,
        None, // worktree_override
        None, // project_main_override (2026-07-29)
        h.app_data_dir.clone(),
        // explicit-agent-dispatch: forced dispatch of `researcher`.
        Some(crate::agent::subagent::ForcedDispatch {
            subagent: "researcher".into(),
            task: "Find all .rs files under src/.".into(),
            model_id: None,
        }),
        // 2026-06-30 (ask_user_question task): per-test
        // QuestionStore. The forced-dispatch test exercises the
        // worker path, where `ask_user_question` is stripped via
        // STRUCTURALLY_DISABLED — the store sits unused on this
        // thread but is signature-required.
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

    // ONLY the worker's single turn — the parent contributed ZERO
    // LLM calls (the forced short-circuit). This is the invariant
    // that makes the dispatch "explicit": the user, not the LLM,
    // decided which agent runs.
    assert_eq!(
        mock.call_count(),
        1,
        "forced dispatch must not call the parent LLM (only the worker's 1 turn)"
    );

    // One dispatch_subagent tool:result carrying the worker summary.
    let results = emitter.tool_results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "exactly one dispatch_subagent tool:result"
    );
    assert!(
        results[0].content.contains("[status: completed]"),
        "tool_result carries [status: completed] + worker summary, got: {}",
        results[0].content
    );
    assert!(
        results[0].content.contains("found 3 files"),
        "tool_result carries the worker's summary, got: {}",
        results[0].content
    );

    // The assistant turn is persisted carrying the worker's summary
    // (so a reload shows the result in the main conversation).
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    // load_session returns role as a String + content as a JSON
    // Value (the DB-stored serialization); match loosely so the
    // assertion survives role-casing / MessageContent shape changes.
    let assistant_blob = loaded
        .messages
        .iter()
        .filter(|m| m.role.to_lowercase().contains("assistant"))
        .map(|m| m.content.to_string())
        .collect::<String>();
    assert!(
        assistant_blob.contains("found 3 files"),
        "persisted assistant turn must carry the worker summary, got: {}",
        assistant_blob
    );

    // (h is partially moved into run_chat_loop above; its remaining
    // fields — incl. the tempdir backing db / app_data_dir — drop at
    // function end, after the load_session assertions.)
}
