#![cfg(test)]

use std::sync::Arc;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
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
        vec![],
        mock.clone(),
        200_000,
        "rid-err-partial".into(),
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
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
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
        // L3b (2026-06-27): production-style caller (and non-isolated
        // worker path) → worktree_override = None (use the session
        // row's worktree_path). Only the isolated worker path passes
        // Some(worker_worktree_path).
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
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
        vec![],
        mock.clone(),
        200_000,
        "rid-err-empty".into(),
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
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
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
        // L3b (2026-06-27): production-style caller (and non-isolated
        // worker path) → worktree_override = None (use the session
        // row's worktree_path). Only the isolated worker path passes
        // Some(worker_worktree_path).
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
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
        vec![],
        mock.clone(),
        200_000,
        "rid-err-blocks".into(),
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
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
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
        // L3b (2026-06-27): production-style caller (and non-isolated
        // worker path) → worktree_override = None (use the session
        // row's worktree_path). Only the isolated worker path passes
        // Some(worker_worktree_path).
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
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
        vec![],
        mock.clone(),
        200_000,
        "rid-err-persist-fail".into(),
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
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
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
        // L3b (2026-06-27): production-style caller (and non-isolated
        // worker path) → worktree_override = None (use the session
        // row's worktree_path). Only the isolated worker path passes
        // Some(worker_worktree_path).
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
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
        vec![],
        mock.clone(),
        200_000,
        "rid-err-tc".into(),
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
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
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
        // L3b (2026-06-27): production-style caller (and non-isolated
        // worker path) → worktree_override = None (use the session
        // row's worktree_path). Only the isolated worker path passes
        // Some(worker_worktree_path).
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
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
