#![cfg(test)]

use std::sync::Arc;

use sqlx::SqlitePool;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::ChatEvent;

// ---------------------------------------------------------------------------
// 12) RULE-A-007 (2026-06-17): error arm persists partial turn
// ---------------------------------------------------------------------------

/// Helper: extract the persisted assistant message rows from a
/// session, in `seq` order. Used by the RULE-A-007 tests to
/// verify the error path landed the partial turn (text +
/// ERROR_MARKER + thinking + tool_use) in the DB.
async fn load_assistant_rows(db: &SqlitePool, session_id: &str) -> Vec<db::MessageRow> {
    let loaded = db::load_session(db, session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    loaded
        .messages
        .into_iter()
        .filter(|m| m.role == "assistant")
        .collect()
}

/// RULE-A-007 (2026-06-17): when the LLM stream emits `Delta`
/// and then `Error` mid-turn, the agent loop MUST persist the
/// partial text (+ ERROR_MARKER) so a reload shows it. Before
/// the fix the error arm did `return` immediately, dropping
/// already-rendered text — an asymmetry vs the cancel path.
///
/// Script: `Delta("partial")` → `Error(Server)`. After the
/// loop runs, the DB has one assistant row whose `text` contains
/// both "partial" AND `ERROR_MARKER`.
#[tokio::test]
async fn agent_loop_error_persists_partial_text() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: "partial".into(),
        }),
        Err(LlmError::Server {
            status: 503,
            message: "service unavailable".into(),
            retry_after: None,
        }),
    ])]));

    run_chat_loop(
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-err-partial".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    // Exactly one Error event (the pre-emit from the per-event
    // arm). RULE-A-007 decision B: no second terminal Error from
    // a persist failure path.
    assert_eq!(
        emitter.error_event_count(),
        1,
        "exactly one Error event (no double-terminal)"
    );

    let assistants = load_assistant_rows(&h.db, &h.session_id).await;
    assert_eq!(
        assistants.len(),
        1,
        "exactly one assistant row (the partial turn persisted)"
    );
    let text = &assistants[0].text;
    assert!(
        text.contains("partial"),
        "partial text must survive in DB, got: {}",
        text
    );
    assert!(
        text.contains(crate::agent::helpers::ERROR_MARKER),
        "ERROR_MARKER must be appended, got: {}",
        text
    );
    // RULE-PERSIST-001 (08-24): the error path's final persist goes
    // through finalize_turn_persist — the in_progress placeholder row
    // (written at stream ready) must be overwritten AND status cleared
    // back to NULL (terminal).
    assert!(
        assistants[0].status.is_none(),
        "error-path final persist must clear status (got {:?})",
        assistants[0].status
    );
}

/// RULE-A-007 edge case: an error event with NO preceding delta
/// must persist a row whose text is exactly `ERROR_MARKER`
/// (symmetric to cancel's empty-text → CANCELLED_MARKER branch).
#[tokio::test]
async fn agent_loop_error_empty_text_uses_error_marker() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::ErrThenEnd(
        LlmError::Server {
            status: 503,
            message: "service unavailable".into(),
            retry_after: None,
        },
    )]));

    run_chat_loop(
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-err-empty".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    assert_eq!(emitter.error_event_count(), 1);
    let assistants = load_assistant_rows(&h.db, &h.session_id).await;
    assert_eq!(assistants.len(), 1);
    assert_eq!(
        assistants[0].text,
        crate::agent::helpers::ERROR_MARKER,
        "empty-text error → text is exactly ERROR_MARKER"
    );
}

/// RULE-A-007: thinking + tool_use blocks accumulated before
/// the error event MUST also survive in the persisted turn's
/// `content` JSON (not just the `text` column). Verifies the
/// `finalized_thinking` / `tool_calls` paths are persisted,
/// not just `text_parts`.
#[tokio::test]
async fn agent_loop_error_persists_thinking_and_tool_calls() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ThinkingDelta { text: "hmm".into() }),
        Ok(ChatEvent::SignatureDelta {
            signature: "sig".into(),
        }),
        Ok(ChatEvent::ToolCall {
            id: "toolu_err".into(),
            name: "list_dir".into(),
            input: serde_json::json!({"path": "."}),
        }),
        Err(LlmError::Server {
            status: 500,
            message: "boom".into(),
            retry_after: None,
        }),
    ])]));

    run_chat_loop(
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-err-blocks".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    assert_eq!(emitter.error_event_count(), 1);
    let assistants = load_assistant_rows(&h.db, &h.session_id).await;
    assert_eq!(assistants.len(), 1);
    let row = &assistants[0];
    // has_tool_calls flag set by persist_turn.
    assert!(row.has_tool_calls, "tool_use block must be flagged");
    // Content JSON carries thinking + tool_use blocks.
    let content_str = row.content.to_string();
    assert!(
        content_str.contains("hmm"),
        "thinking text must survive in content JSON: {}",
        content_str
    );
    assert!(
        content_str.contains("toolu_err"),
        "tool_use id must survive in content JSON: {}",
        content_str
    );
    assert!(
        content_str.contains("\"thinking\""),
        "thinking block variant must be present: {}",
        content_str
    );
}

/// RULE-A-007 decision B: on the error path, a `persist_turn`
/// failure must NOT emit a second terminal Error event. The
/// per-event arm already emitted one; emitting again would be a
/// conflicting double-terminal. Symmetric to the cancel path's
/// synthetic tool_result persist (log-only).
///
/// Test: install a trigger that blocks assistant-turn INSERTs,
/// script a `Delta` + `Error` turn, then assert exactly one
/// Error event survives (the pre-emit one — no second from
/// the persist failure path).
#[tokio::test]
async fn agent_loop_error_persist_failure_is_log_only() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    // Block INSERTs into `messages` AFTER the initial user
    // message is already persisted (so pre-flight succeeds).
    // We use a BEFORE INSERT trigger; the user-message persist
    // happens first, so we install the trigger AFTER
    // run_chat_loop starts... but that's not possible without
    // a thread. Instead, scope the trigger to assistant-role
    // rows only: the user message has role='user', the partial
    // assistant turn has role='assistant'. The trigger raises
    // only for assistant inserts.
    sqlx::query(
        r#"CREATE TRIGGER messages_no_assistant_insert BEFORE INSERT ON messages
           WHEN NEW.role = 'assistant'
           BEGIN
               SELECT RAISE(ABORT, 'simulated assistant persist failure');
           END"#,
    )
    .execute(&h.db)
    .await
    .expect("install assistant-only fail-insert trigger");

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: "partial".into(),
        }),
        Err(LlmError::Server {
            status: 503,
            message: "service unavailable".into(),
            retry_after: None,
        }),
    ])]));

    run_chat_loop(
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-err-persist-fail".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    // The single Error event is the pre-emit from the per-event
    // arm. The persist failure on the error path MUST NOT add a
    // second one (RULE-A-007 decision B).
    assert_eq!(
        emitter.error_event_count(),
        1,
        "persist failure on error path must be log-only (no double-terminal Error)"
    );
    // And no Done event either (the loop returns without
    // emitting Done — Error is the terminal).
    let done_count = emitter
        .chat_events()
        .into_iter()
        .filter(|p| matches!(p.event, ChatEvent::Done { .. }))
        .count();
    assert_eq!(done_count, 0, "no Done event on error path");
}

/// RULE-A-007 decision C: after the error path persists the
/// partial turn, a `ChatEvent::TurnComplete` MUST be emitted
/// (same as cancel / normal paths) so the frontend has the seq
/// + latency breakdown for the partial row. The TurnComplete
/// coexists with the pre-emit Error event.
#[tokio::test]
async fn agent_loop_error_emits_turn_complete() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: "partial".into(),
        }),
        Err(LlmError::Server {
            status: 503,
            message: "service unavailable".into(),
            retry_after: None,
        }),
    ])]));

    run_chat_loop(
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-err-tc".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    // Exactly one TurnComplete, pointing at the persisted
    // partial assistant row's seq. The user message has seq=0
    // (initial persist), so the assistant turn is seq=1.
    let turn_completes: Vec<i64> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::TurnComplete { seq, .. } => Some(seq),
            _ => None,
        })
        .collect();
    assert_eq!(
        turn_completes.len(),
        1,
        "exactly one TurnComplete expected on error path, got {}",
        turn_completes.len()
    );
    assert_eq!(
        turn_completes[0], 1,
        "TurnComplete seq points at partial turn"
    );

    // And the row actually exists at that seq.
    let assistants = load_assistant_rows(&h.db, &h.session_id).await;
    assert_eq!(assistants.len(), 1);
    assert_eq!(assistants[0].seq, 1);
}

// ---------------------------------------------------------------------------
// 09-01-subagent-network-resume: worker error exit captures messages
// ---------------------------------------------------------------------------

/// A worker run (skip_persist=true) whose stream dies mid-turn MUST
/// still snapshot its `Vec<ChatMessage>` via the sink, so
/// `subagent_runs.messages_json` carries the conversation and a later
/// `dispatch_subagent { resume_from: <run_id> }` can CONTINUE instead
/// of restarting from scratch. Before the fix the error exit dropped
/// the history — session 2e438939 lost a 3.8-minute checker run to a
/// mid-stream network drop and the parent re-dispatched a duplicate.
///
/// Pair-safety precondition verified too: the partial assistant turn
/// carries ERROR_MARKER, and (unlike cancel) the history is exactly
/// what the main session would persist + reload after an error.
#[tokio::test]
async fn worker_error_exit_snapshots_messages_for_resume() {
    use crate::agent::subagent::SubagentBufferSink;
    use crate::llm::error::LlmError;

    let h = make_harness().await;
    let worker_sink = Arc::new(SubagentBufferSink::new_without_app_handle(
        "run-err-resume".into(),
        h.session_id.clone(),
    ));
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ThinkingDelta {
            text: "verifying item 6/7".into(),
        }),
        Ok(ChatEvent::Delta {
            text: "assertion 1-5 passed".into(),
        }),
        Err(LlmError::Network(
            "stream read: error decoding response body".into(),
        )),
    ])]));

    let mut role = parent_role(&h);
    role.is_worker = Some(true);
    role.skip_persist = true;
    role.skip_session_active = true;

    let _ = run_chat_loop(
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-worker-err".into(),
            h.session_id.clone(),
            test_messages(),
            worker_sink.clone(),
        ),
        chat_loop_deps(&h),
        role,
    )
    .await;

    assert!(worker_sink.had_error(), "sink must flag the error exit");
    let messages = worker_sink.worker_messages();
    assert!(
        !messages.is_empty(),
        "worker error exit must capture messages for resume"
    );
    let last = messages.last().expect("captured");
    assert_eq!(last.role, crate::llm::types::Role::Assistant);
    let text = last.content.to_text();
    assert!(
        text.contains("assertion 1-5 passed"),
        "partial text survives in the snapshot, got: {}",
        text
    );
    assert!(
        text.contains(crate::agent::helpers::ERROR_MARKER),
        "ERROR_MARKER marks the break point, got: {}",
        text
    );

    // The main-session DB must stay untouched (worker mode): the
    // snapshot is in-memory only, consumed by run_subagent's persist.
    let assistants = load_assistant_rows(&h.db, &h.session_id).await;
    assert!(
        assistants.is_empty(),
        "worker must not persist rows to the parent session table"
    );
}
