#![cfg(test)]

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, TokenUsage};
use crate::llm::{MessageContent, Role};

/// Error-path orphan fix (llm-contract.md §Pair Atomicity tool_use↔tool_result
/// Pair Atomicity). When the LLM stream emits a `tool_use` and THEN
/// errors mid-turn, the agent loop still pushes the `assistant(tool_use)`
/// turn (RULE-A-007). Pre-fix the error path returned without appending
/// a matching `tool_result`, orphaning the tool_use — the next request
/// then failed upstream (OpenAI 400 "insufficient tool messages
/// following tool_calls" / Anthropic 2013). The fix, symmetric with the
/// cancel path, appends one synthetic `is_error` tool_result per emitted
/// tool_use. Asserted by reloading the persisted messages and checking
/// no orphan tool_use remains.
#[tokio::test]
async fn agent_loop_error_after_tool_use_appends_synthetic_result() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // Turn 1: Start → ToolCall → Error. The stream dies right after
    // the model emitted a tool_use, so `had_error` fires with a
    // non-empty `tool_calls`.
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ToolCall {
            id: "toolu_err1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/x"}),
        }),
        Ok(ChatEvent::Error {
            message: "simulated mid-stream error after tool_use".into(),
            category: crate::llm::LlmErrorCategory::Server,
        }),
    ])]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-err-tool".into(),
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
        // skip_persist = false: the error turn (assistant + synthetic
        // tool_result) lands in the `messages` table so the test can
        // reload and assert pair atomicity.
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

    // Error path taken: one error event, exactly one send.
    assert_eq!(emitter.error_event_count(), 1, "error path fired");
    assert_eq!(mock.call_count(), 1, "error aborts before turn 2");

    // Reload persisted messages and assert pair atomicity: every
    // assistant tool_use_id has a matching tool_result. Pre-fix this
    // would report `["toolu_err1"]`.
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT role, content FROM messages WHERE session_id = ? ORDER BY seq")
            .bind(&h.session_id)
            .fetch_all(&h.db)
            .await
            .expect("fetch messages");

    let msgs: Vec<ChatMessage> = rows
        .into_iter()
        .filter_map(|(role, content)| {
            let mc: MessageContent = serde_json::from_str(&content).ok()?;
            let role = match role.as_str() {
                "assistant" => Role::Assistant,
                "user" => Role::User,
                _ => return None,
            };
            Some(ChatMessage {
                role,
                content: mc,
                speaker: None,
            })
        })
        .collect();

    let orphans = crate::llm::provider::wire::orphan_tool_use_ids(&msgs);
    assert!(
        orphans.is_empty(),
        "error path must append synthetic tool_result to keep the pair atomic; orphans={:?}",
        orphans
    );
}

// ---------------------------------------------------------------------------
// 6) C3 compaction preserves the agent loop (no panic / no error)
// ---------------------------------------------------------------------------

/// Force C3 compaction by setting a tiny context_window (10
/// tokens). The agent loop MUST:
/// - NOT panic (C3 returns whatever it can trim; with an
///   empty messages vec after compaction, the turn body
///   short-circuits and the model just sees the system
///   prompt + nothing)
/// - emit `Done` (some stop_reason) — the loop must
///   terminate, not hang
///
/// This is the safety-net test for C3 (the bigger
/// pair-atomicity invariant — RULE-A-001 — is covered by
/// the upstream `agent::context::tests`; this integration
/// test just asserts "the agent loop survives a C3 run").
#[tokio::test]
async fn agent_loop_c3_compaction_does_not_panic() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: "after-c3".into(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));

    // Construct messages that force a CLEAN compaction
    // (`DegradationKind::None`): head[2 tiny protected] +
    // middle[1 large droppable] + tail[1 tiny protected].
    // context_window = 1000 → trigger = 800, target = 500.
    // `big_middle` (~4.8KB ≈ 1200 tokens) pushes tokens_before
    // past the 800 trigger; dropping it leaves head(2 tiny) +
    // tail(1 tiny) ≈ 10 tokens, well under target 500 → `None`
    // (safe-to-proceed). The provider IS called and emits Done —
    // the loop completes normally.
    //
    // This is the clean-compaction counterpart to
    // `agent_loop_c3_still_over_emits_error_and_skips_provider`
    // (which forces `StillOver` via a huge tail so the provider
    // is NEVER called). Together they cover both C3 exits:
    // clean drop → continue; exhausted → Error + abort.
    //
    // RULE-A-017 (2026-06-28): the original setup
    // (`test_messages()` = `["hello"]` + context_window = 10)
    // was dwarfed by run_chat_loop's B5/skill head injection,
    // which pushed the post-drop estimate past target 5 and
    // tripped `StillOver` (emit Error, no Done) — the opposite
    // of this test's "loop survives" intent. The 4-message /
    // window-1000 shape mirrors the still_over test so it stays
    // stable under the same injection.
    let big_middle = {
        // Same filler helper as the still_over test — repeated
        // ASCII that cl100k_base encodes at ~4 chars/token.
        "the quick brown fox jumps over the lazy dog. "
            .repeat(4_800 / 45 + 1)
            .chars()
            .take(4_800)
            .collect::<String>()
    };
    let messages = vec![
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text("tiny head 1".into()),
            speaker: None,
        },
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text("tiny head 2".into()),
            speaker: None,
        },
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(big_middle),
            speaker: None,
        },
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text("tiny tail".into()),
            speaker: None,
        },
    ];
    run_chat_loop(
        vec![],
        mock.clone(),
        // Force compaction to trigger (tokens_before > trigger 800)
        // but resolve cleanly (post-drop < target 500 → None).
        1000,
        "rid-c3".into(),
        h.session_id.clone(),
        messages,
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

    // (1) Clean compaction (None) lets the turn proceed — the
    //     provider is called exactly once. (StillOver, by
    //     contrast, skips the provider — see the still_over test.)
    assert_eq!(
        mock.call_count(),
        1,
        "provider.send MUST be called once after a clean (None) C3 compaction"
    );

    // (2) The loop terminates with a Done event.
    let events = emitter.chat_events();
    assert!(
        events
            .iter()
            .any(|p| matches!(&p.event, ChatEvent::Done { .. })),
        "agent loop must terminate with a Done event after a clean C3 compaction"
    );

    // (3) No error events — clean compaction is non-fatal.
    assert_eq!(
        emitter.error_event_count(),
        0,
        "no error events expected after a clean (None) C3 compaction"
    );
}

// ---------------------------------------------------------------------------
// 9) Error path emits ChatEvent::Error
// ---------------------------------------------------------------------------

/// The `ErrThenEnd` script entry must surface to the
/// frontend as a `ChatEvent::Error` event, NOT a silent
/// loop. This is the canonical error-path contract.
#[tokio::test]
async fn agent_loop_error_path_emits_chat_event_error() {
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
        "rid-err".into(),
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

    let error_events: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Error { message, category } => Some((message, category)),
            _ => None,
        })
        .collect();
    assert_eq!(error_events.len(), 1, "one error event expected");
    let (msg, _cat) = &error_events[0];
    assert!(
        msg.contains("请求无效"),
        "expected InvalidRequest message, got: {}",
        msg
    );
    // InvalidRequest category (non-retryable 4xx-class).
    assert_eq!(
        error_events[0].1,
        crate::llm::LlmErrorCategory::InvalidRequest
    );
}

// ---------------------------------------------------------------------------
// 10) C3 degradation — `StillOver` aborts the turn with an Error event
//     (RULE-A-002, 2026-06-14)
// ---------------------------------------------------------------------------

/// When `compact_messages` runs out of safe droppable candidates but
/// the budget is still over the target, the agent loop MUST:
///
/// 1. Emit exactly one `ChatEvent::Error` with
///    `LlmErrorCategory::InvalidRequest`.
/// 2. NOT call `provider.send` (the over-budget request would 400
///    on `prompt is too long`).
/// 3. NOT emit a terminal `Done` event (the chat is aborted, not
///    completed). The frontend treats `Error` as terminal.
///
/// This is the integration-test guard for RULE-A-002. The unit
/// level is covered by `agent::context::tests::compact_emits_still_over_degradation`;
/// this test verifies the agent loop body (in `chat_loop.rs`)
/// translates the signal into the Error event correctly. It MUST
/// mirror the production `chat.rs` C3 block (see module docstring
/// "Drift hazard" — the two implementations share the same wire
/// contract here).
#[tokio::test]
async fn agent_loop_c3_still_over_emits_error_and_skips_provider() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // The provider script has ONE turn's worth of events. If the
    // agent loop's C3 guard works, `send` is never called and the
    // script is left unconsumed. If the guard is broken, the
    // provider WILL be called and we'll see call_count == 1.
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: "should never reach".into(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));

    // Construct messages that force `DegradationKind::StillOver`:
    // head[2 small] + middle[1 small droppable] + tail[1 HUGE].
    // context_window = 1000 → trigger = 800, target = 500.
    // After dropping the middle, head(2 tiny) + tail(1 huge > 500)
    // is still over target → StillOver.
    //
    // big_pad(8_000) ≈ 8KB ≈ ~2000 tokens (well over target 500).
    let huge = {
        // Mirror the helper used by context.rs tests — repeated
        // ASCII filler that cl100k_base encodes at ~4 chars/token.
        "the quick brown fox jumps over the lazy dog. "
            .repeat(8_000 / 45 + 1)
            .chars()
            .take(8_000)
            .collect::<String>()
    };
    let messages = vec![
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text("tiny head 1".into()),
            speaker: None,
        },
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text("tiny head 2".into()),
            speaker: None,
        },
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text("droppable middle".into()),
            speaker: None,
        },
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(huge),
            speaker: None,
        },
    ];

    run_chat_loop(
        vec![],
        mock.clone(),
        // Force tiny context_window so compaction triggers and
        // StillOver fires.
        1000,
        "rid-c3-still-over".into(),
        h.session_id.clone(),
        messages,
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

    // (1) `provider.send` was NEVER called — the C3 guard
    //     short-circuited before dispatch.
    assert_eq!(
        mock.call_count(),
        0,
        "provider.send MUST NOT be called when C3 degradation is StillOver"
    );

    // (2) Exactly one Error event with the InvalidRequest category.
    let error_events: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Error { message, category } => Some((message, category)),
            _ => None,
        })
        .collect();
    assert_eq!(
        error_events.len(),
        1,
        "exactly one Error event expected on StillOver (got {})",
        error_events.len()
    );
    let (err_msg, err_cat) = &error_events[0];
    assert!(
        err_msg.contains("Context window exceeded after compaction"),
        "Error message should describe the over-budget state, got: {}",
        err_msg
    );
    assert_eq!(
        *err_cat,
        crate::llm::LlmErrorCategory::InvalidRequest,
        "category should be InvalidRequest (mirrors prompt-too-long 400)"
    );

    // (3) No terminal Done event — the chat is aborted via Error,
    //     not completed via Done.
    let done_count = emitter
        .chat_events()
        .iter()
        .filter(|p| matches!(&p.event, ChatEvent::Done { .. }))
        .count();
    assert_eq!(
        done_count, 0,
        "no Done event expected — the turn was aborted, not completed"
    );
}

// ---------------------------------------------------------------------------
// 10) RULE-A-003: persist_turn failure surfaces a typed Error
// ---------------------------------------------------------------------------

/// RULE-A-003 (2026-06-15): when `persist_turn` fails (disk full /
/// DB-lock contention) on a NORMAL persist site (initial user
/// message / assistant turn / tool_result turn), the agent loop
/// must NOT stay silent — it emits a `ChatEvent::Error { Server }`
/// and aborts. Previously the failure was `tracing::error!`-only,
/// so the message was rendered to the user but never reached the
/// DB; the next session reload was blank, and the in-memory seq
/// drifted out of sync with the DB. The cancel-path persist sites
/// intentionally stay log-only (no Error) to avoid emitting two
/// terminal events — that's a code-review invariant, not a
/// runtime path this test exercises.
///
/// We force the failure with a `BEFORE INSERT ON messages` trigger
/// that always ABORTs. This blocks only INSERT (what `persist_turn`
/// does); SELECT (what `load_session` / `get_project` do) is
/// unaffected, so the loop reaches the persist site cleanly. The
/// initial user-message persist runs before the `for turn` loop, so
/// `provider.send` is never called (`call_count == 0`).
#[tokio::test]
async fn agent_loop_persist_failure_emits_error() {
    let h = make_harness().await;
    // Poison INSERTs into `messages`: persist_turn's INSERT will
    // RAISE, but load_session's SELECT on `messages` still works.
    sqlx::query(
        r#"CREATE TRIGGER messages_no_insert BEFORE INSERT ON messages
           BEGIN
               SELECT RAISE(ABORT, 'simulated persist failure');
           END"#,
    )
    .execute(&h.db)
    .await
    .expect("install fail-insert trigger");

    let emitter = Arc::new(MockEmitter::new());
    // The provider script is never consumed (call_count stays 0).
    // Provided as a sentinel so a broken fix that skipped the
    // abort would surface as call_count == 1.
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: "should never reach".into(),
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
        "rid-persist-fail".into(),
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

    // (1) provider.send was never called — the initial user-message
    //     persist (before the `for turn` loop) failed and aborted.
    assert_eq!(
        mock.call_count(),
        0,
        "persist failure must abort before provider.send is called"
    );

    // (2) Exactly one Error event, category Server, persist-failure
    //     copy. Mirrors the StillOver test's assertion shape.
    let error_events: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Error { message, category } => Some((message, category)),
            _ => None,
        })
        .collect();
    assert_eq!(
        error_events.len(),
        1,
        "exactly one Error event expected on persist failure (got {})",
        error_events.len()
    );
    let (err_msg, err_cat) = &error_events[0];
    assert!(
        err_msg.contains("保存对话记录失败"),
        "Error message should be the persist-failure copy, got: {}",
        err_msg
    );
    assert_eq!(
        *err_cat,
        crate::llm::LlmErrorCategory::Server,
        "category should be Server (system-side, not a bad request)"
    );
}

// ---------------------------------------------------------------------------
// 11) RULE-A-004: a cancelled tool is NOT recorded as tool_executed
// ---------------------------------------------------------------------------

/// RULE-A-004 (2026-06-15): `record_tool_executed_audit` must run
/// AFTER the `token.is_cancelled()` check. A tool whose execution
/// was interrupted by a cancel must NOT get a `tool_executed` audit
/// row — recording it would lie to the audit log (the user hit
/// Stop; the tool did not complete from their intent).
///
/// Turn 1 emits `tool_use` (`list_dir` — a read tool that does NOT
/// consult the cancel token, so execute_tool runs to completion
/// regardless). A side task cancels the token once `call_count >= 1`
/// (turn 1's `send` has been called). The cancel task `yield_now`s
/// (no sleep) so it re-checks at every agent-loop await point and
/// cancels as early as possible. Two landing spots, both correct:
/// - mid-stream → the `select!`'s biased cancel arm wins, the tool
///   never executes → no audit row (trivially correct).
/// - at/after execute_tool returns → `token.is_cancelled()` is true
///   at the audit check → audit skipped (the RULE-A-004 fix).
/// Either way `session_audit_events` has zero `tool_executed` rows.
///
/// Contrast `agent_loop_tool_use_triggers_tool_result_turn` (no
/// cancel): the same `list_dir` DOES write an audit row there — so
/// this is a real regression guard, not a tautology.
#[tokio::test]
async fn agent_loop_cancel_skips_audit_for_cancelled_tool() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: tool_use. `list_dir` is a read tool → Tier 5
        // default-allow, and it does NOT consult the cancel token,
        // so execute_tool runs to completion even after cancel.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_1".into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2 sentinel — only consumed if the loop re-enters
        // (it shouldn't; cancel aborts before turn 2).
        MockResponse::HangingThenCancel,
    ]));
    let call_handle = mock.call_count_handle();
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let cancel_handle = tokio::spawn(async move {
        // yield_now (not sleep) so the cancel task re-runs at every
        // agent-loop await point and cancels as soon as turn 1's
        // send has been observed.
        loop {
            if call_handle.load(Ordering::SeqCst) >= 1 {
                cancel_for_task.cancel();
                break;
            }
            tokio::task::yield_now().await;
        }
    });

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-audit-cancel".into(),
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
    cancel_handle.await.unwrap();

    // No tool_executed audit row for this session — the cancelled
    // tool must not leave a "this tool ran" record behind it.
    let audit_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM session_audit_events
           WHERE session_id = ? AND kind = 'tool_executed'"#,
    )
    .bind(&h.session_id)
    .fetch_one(&h.db)
    .await
    .expect("count tool_executed audit rows");
    assert_eq!(
        audit_count, 0,
        "a cancelled tool must NOT be recorded as tool_executed (RULE-A-004)"
    );
}
