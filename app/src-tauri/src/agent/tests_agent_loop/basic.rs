#![cfg(test)]

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

// ---------------------------------------------------------------------------
// 1) Basic text-only response
// ---------------------------------------------------------------------------

/// The simplest turn-orchestration invariant: a single-turn
/// text-only response results in exactly 1 `send` call and
/// one terminal `Done { stop_reason: Some("end_turn") }`
/// event. Covers the regression where pre-fix the agent loop
/// called `send` twice for a single-turn response (the
/// "thinking_ms only on last turn" bug class — same family
/// of off-by-one).
#[tokio::test]
async fn agent_loop_basic_text_only_completes() {
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
        "rid-basic".into(),
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
        // B6 Subagent (2026-06-22): max_turns = None keeps the
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
    )
    .await;

    assert_eq!(mock.call_count(), 1, "expected exactly 1 send call");
    let done = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect::<Vec<_>>();
    // `filter_map` flattens one layer of `Option`, so `done` is
    // `Vec<String>` — the extracted `stop_reason` values that were
    // `Some(...)`. The `Some("end_turn")` case means we see
    // exactly one entry here.
    assert_eq!(done, vec!["end_turn".to_string()]);
}

// ---------------------------------------------------------------------------
// 2) Tool use → tool result loop
// ---------------------------------------------------------------------------

/// Turn 1: model emits `tool_use` (the agent loop's `stop_reason`
/// becomes "tool_use"). The agent loop MUST execute the tool
/// (default-allow for read tools) and call `send` a SECOND time.
/// Turn 2: model emits a final text response. The loop MUST
/// terminate with `Done { stop_reason: Some("end_turn") }`.
///
/// This is the "tool_use triggers another turn" invariant — if
/// the agent loop's tool execution path is broken (e.g. the
/// `should_continue` branch fails to re-enter the outer loop),
/// this test fails with `mock.call_count() == 1`.
#[tokio::test]
async fn agent_loop_tool_use_triggers_tool_result_turn() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: tool_use. The MockProvider script
        // auto-exhausts on this call index.
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
        // Turn 2: text response (after the agent loop
        // built the tool_result message).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
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
        "rid-tool".into(),
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
        // B6 Subagent (2026-06-22): max_turns = None keeps the
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
    )
    .await;

    assert_eq!(
        mock.call_count(),
        2,
        "tool_use must trigger exactly one more turn (2 sends total)"
    );
    assert_eq!(emitter.tool_call_count(), 1);
    // list_dir is a read-only tool that goes through Tier 5
    // default-allow; the agent loop emits one `tool:result`
    // (success path, is_error=false) before re-entering the
    // outer loop.
    assert_eq!(emitter.tool_result_count(), 1);
}

/// 08-07-group-chat-role-history-isolation follow-up fix: some
/// OpenAI-compatible providers (Console Go) end a tool_use stream with
/// a NON-"tool_use" finish_reason (e.g. "stop" → normalized
/// "end_turn"). The pre-fix `should_continue` predicate required
/// `stop_reason == Some("tool_use")`, so the tools were NEVER executed
/// and no tool_result was persisted → the next reload built a context
/// with an orphan assistant(tool_calls) → every later turn 400'd with
/// "An assistant message with 'tool_calls' must be followed by tool
/// messages responding to each 'tool_call_id'" and the group chat
/// burned MAX_ORCHESTRATION_ROUNDS on [生成出错中断] retries (DB
/// session d7fe451c: seq 5 emitted 3 read_file tool_uses, zero
/// tool_results, 26 consecutive error turns).
///
/// The fix keys tool execution on `tool_calls` alone. This test scripts
/// turn 1 as tool_use + `stop_reason: Some("end_turn")` (the broken
/// input) and asserts the tools STILL execute (2 sends + 1
/// tool:result), then turn 2 terminates normally.
#[tokio::test]
async fn agent_loop_tool_use_with_non_tool_use_stop_reason_still_executes() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: tool_use, but the provider reports a NON-"tool_use"
        // stop_reason ("end_turn" — what Console Go emits for "stop"
        // after a tool_calls-only stream).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_1".into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: text response (after the agent loop executed the
        // tool and fed the result back).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
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
        "rid-tool-endturn".into(),
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
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        None,
        None,
        h.subagent_cache.clone(),
        None,
        None,
        None,
        h.app_data_dir.clone(),
        None,
        h.question_store.clone(),
        None,
        None,
        None,
    )
    .await;

    assert_eq!(
        mock.call_count(),
        2,
        "tool_use with a non-'tool_use' stop_reason must still execute the tools"
    );
    assert_eq!(emitter.tool_call_count(), 1);
    assert_eq!(
        emitter.tool_result_count(),
        1,
        "tool_result must be emitted/persisted (pair atomicity, llm-contract §Pair Atomicity)"
    );
}

// ---------------------------------------------------------------------------
// 2b) B4 use_skill loads the skill body into the tool_result
// ---------------------------------------------------------------------------

/// B4: turn 1 model emits `use_skill("review-pr")`. The agent loop
/// resolves the skill body from the SkillCache (a real skill file
/// seeded under the project's `.everlasting/skills/`) and feeds it
/// back as the tool_result — L1 activation via the tool_result path
/// (PR2 brainstorm Q2). Turn 2: final text. Asserts the body lands
/// in the tool_result with is_error=false.
#[tokio::test]
async fn agent_loop_use_skill_loads_body_into_tool_result() {
    let h = make_harness().await;
    // Seed a real skill the loader will scan.
    let skill_dir = h
        .project_path
        .join(".everlasting")
        .join("skills")
        .join("review-pr");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review-pr\ndescription: review a PR\n---\nREVIEW-SKILL-BODY",
    )
    .unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_skill".into(),
                name: "use_skill".into(),
                input: serde_json::json!({"skill_name": "review-pr"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "applied".into(),
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
        "rid-skill".into(),
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
    )
    .await;

    assert_eq!(
        mock.call_count(),
        2,
        "use_skill must trigger a second turn (body fed back as tool_result)"
    );
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1, "exactly one tool_result for use_skill");
    assert!(
        results[0].content.contains("REVIEW-SKILL-BODY"),
        "tool_result must carry the skill body, got: {}",
        results[0].content
    );
    assert!(
        !results[0].is_error,
        "resolved skill must be a success tool_result"
    );
}

/// B4: `use_skill("nope")` with no matching skill returns
/// is_error=true — the standard ⑫ error-feedback path so the LLM
/// can self-correct.
#[tokio::test]
async fn agent_loop_use_skill_unknown_returns_error() {
    let h = make_harness().await;
    // No skill files seeded → "nope" won't resolve.

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_miss".into(),
                name: "use_skill".into(),
                input: serde_json::json!({"skill_name": "nope"}),
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
    ]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-skill-miss".into(),
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
    )
    .await;

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1);
    assert!(
        results[0].is_error,
        "unknown skill must be is_error so the LLM can self-correct"
    );
    assert!(
        results[0].content.contains("not found"),
        "error content should name the missing skill, got: {}",
        results[0].content
    );
}

// ---------------------------------------------------------------------------
// 3) Cancel in turn 2 kills the loop
// ---------------------------------------------------------------------------

/// Spawn `run_chat_loop` and cancel its token after turn 1 has
/// cleanly completed and turn 2's `send` has been observed. The
/// agent loop MUST:
/// - emit `Done { stop_reason: Some("cancelled") }` (exactly
///   one, not zero, not two)
/// - call `send` exactly twice (turn 1 tool_use + the cancelled
///   turn 2)
/// - NOT emit `tool:result` for a turn 2 tool (turn 2 is a
///   HangingThenCancel so no tool_use arrives)
///
/// The semantics here match PRD R3 "2 turn cancel":
/// - Turn 1 emits `tool_use` (`list_dir` with `path: "."`). The
///   agent loop runs the read tool through Tier 5 default-allow,
///   persists the tool_result, and re-enters the outer loop for
///   turn 2. Critically, `run_chat_loop` does NOT exit on
///   `tool_use` (only `end_turn` / non-`tool_use` exits) — see
///   `chat_loop.rs`'s `should_continue` branch.
/// - Turn 2 is `HangingThenCancel`: the stream is forever
///   pending. The cancel side-channel polls `call_count` and
///   fires the cancel token once `call_count >= 2` (turn 2's
///   `send` has been called). The agent loop's `select!` cancel
///   arm (`biased;` first) wins over the pending stream and
///   emits exactly one `Done("cancelled")`.
///
/// We gate the cancel on `call_count >= 2` (not 1) so that turn
/// 1 completes normally — earlier versions gated on 1, which
/// races with the tool-execution path and can flip the
/// `cancelled` flag mid-tool.
#[tokio::test]
async fn agent_loop_cancel_in_turn_2_kills_loop() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: tool_use. `list_dir` is a read tool → Tier 5
        // default-allow (no permission ask), the agent loop
        // executes it, persists the tool_result, and re-enters
        // the outer loop for turn 2.
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
        // Turn 2: this script entry is consumed (call_count → 2)
        // but the agent loop is cancelled mid-stream (the
        // `HangingThenCancel` arm keeps the stream pending
        // until the cancel arm wins the `select!`).
        MockResponse::HangingThenCancel,
    ]));
    let call_handle = mock.call_count_handle();
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let cancel_handle = tokio::spawn(async move {
        // Poll until call_count >= 2 (turn 2's send has been
        // observed by the agent loop), then cancel. Gating on 2
        // lets turn 1's tool_use + tool execution + tool_result
        // persist complete cleanly before the cancel fires —
        // the cancel races only against turn 2's pending stream.
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
    )
    .await;
    cancel_handle.await.unwrap();

    assert_eq!(
        mock.call_count(),
        2,
        "agent loop should call send twice (turn 1 tool_use + the cancelled turn 2)"
    );
    assert_eq!(
        emitter.cancel_done_count(),
        1,
        "exactly one Done(cancelled) event expected"
    );
    assert_eq!(emitter.max_turns_done_count(), 0);
}

// ---------------------------------------------------------------------------
// 4) MAX_TURNS fallback
// ---------------------------------------------------------------------------

/// Script the mock to always emit `tool_use` (no end_turn),
/// forcing the agent loop to hit MAX_TURNS. The agent loop
/// MUST emit `Done { stop_reason: Some("max_turns") }` and
/// must call `send` exactly MAX_TURNS times.
///
/// This covers the "infinite tool loop" pathological case
/// (C3 + MAX_TURNS safety net, see context.rs for the C3
/// half; this test is the MAX_TURNS half).
#[tokio::test]
async fn agent_loop_max_turns_emits_done_marker() {
    use crate::agent::MAX_TURNS;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    // Build a script with MAX_TURNS tool_use responses.
    // The agent loop will keep emitting tool_use, executing
    // the tool (Tier 5 default-allow for list_dir), and
    // calling send again. After MAX_TURNS iterations, the
    // outer loop bails.
    let mut script = Vec::with_capacity(MAX_TURNS);
    for i in 0..MAX_TURNS {
        script.push(MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: format!("toolu_max_{}", i),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]));
    }
    let mock = Arc::new(MockProvider::new(script));

    // C2+ (2026-07-05): 200 identical `list_dir({path: "."})`
    // calls also feed the C2 hard-loop detector (HARD_WINDOW =
    // 5 trailing identical signatures). Without intervention,
    // the C2+ state machine would trigger a QuestionStore ask
    // at turn 3 and block forever (no resolver attached).
    // We spawn a "continue-all" resolver that loops: each time
    // a pending entry appears, it resolves with「继续」so the
    // test exercises the MAX_TURNS backstop (not C2+ early
    // termination). The C2+ path is covered by the dedicated
    // tests in `tests_c2plus.rs`.
    let store_for_resolver = h.question_store.clone();
    let session_for_resolver = h.session_id.clone();
    tokio::spawn(async move {
        loop {
            if store_for_resolver
                .get_payload(&session_for_resolver)
                .await
                .is_some()
            {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                let _ = store_for_resolver
                    .resolve(
                        &session_for_resolver,
                        crate::agent::question_store::InteractionResponse::Answered(
                            serde_json::to_value(vec![
                                crate::agent::question_store::QuestionAnswer {
                                    question: String::new(),
                                    header: None,
                                    options: vec!["继续".into()],
                                    multi_select: false,
                                    custom: None,
                                },
                            ])
                            .unwrap(),
                        ),
                    )
                    .await;
                // Brief yield so the agent loop's select! arm
                // observes the resolve before we re-poll.
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                continue;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-maxturns".into(),
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
    )
    .await;

    assert_eq!(
        mock.call_count(),
        MAX_TURNS,
        "agent loop should call send MAX_TURNS times"
    );
    assert_eq!(
        emitter.max_turns_done_count(),
        1,
        "exactly one Done(max_turns) event expected"
    );
}

// ---------------------------------------------------------------------------
// 5) MockProvider script exhaustion
// ---------------------------------------------------------------------------

/// When the agent loop asks for more turns than the test
/// scripted, MockProvider surfaces a typed
/// `LlmError::InvalidRequest { "exhausted" }` and the agent
/// loop bails with `had_error = true`. The test asserts:
/// - the typed error message made it to the agent loop
///   (proves the exhaustion contract is observable)
/// - exactly one `send` was attempted (the second was never
///   reached because the first hit the error path)
///
/// This guards against silent script-overflow regressions.
#[tokio::test]
async fn agent_loop_mock_provider_exhaustion_surfaces_error() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // Script has 0 entries — the very first send hits
    // exhaustion. The agent loop's `if had_error` branch
    // returns before persisting any assistant turn.
    let mock = Arc::new(MockProvider::new(vec![]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-exhaust".into(),
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
    )
    .await;

    // The agent loop's error path emits one `ChatEvent::Error`
    // and returns; we expect at least one error event in the
    // recorded events.
    assert_eq!(emitter.error_event_count(), 1, "one error event");
    assert!(emitter.chat_events().iter().any(|p| matches!(&p.event,
            ChatEvent::Error { message, .. } if message.contains("exhausted"))));
    assert_eq!(mock.call_count(), 1);
}
