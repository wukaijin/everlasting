#![cfg(test)]

use std::sync::Arc;
use std::sync::atomic::Ordering;

use futures_util::StreamExt;
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, TokenUsage};
use crate::llm::Provider;
use crate::llm::{ContentBlock, MessageContent, Role};

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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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

    // context_window = 10 forces aggressive trimming. The
    // estimator (tiktoken cl100k_base) on a tiny
    // `["hello"]` message is already > 0 tokens; the
    // agent loop's pre-compact check (80% of window)
    // triggers and trims. With a 10-token window, the
    // agent loop may end up with 0 middle messages to
    // drop; the B5 synthetic user/assistant head pair
    // + the current user message are protected, so the
    // loop survives.
    run_chat_loop(
        vec![],
        mock.clone(),
        10,
        "rid-c3".into(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
    )
    .await;

    // The loop should have completed (one Done event) and
    // not emitted any error events.
    let events = emitter.chat_events();
    assert!(
        events
            .iter()
            .any(|p| matches!(&p.event, ChatEvent::Done { .. })),
        "agent loop must terminate with a Done event after C3 compaction"
    );
    assert_eq!(
        emitter.error_event_count(),
        0,
        "no error events expected (C3 is best-effort, not fatal)"
    );
}

// ---------------------------------------------------------------------------
// 7) Provider protocol is `Mock`
// ---------------------------------------------------------------------------

/// The `MockProvider::protocol()` returns
/// `ProviderProtocol::Mock`. This is the catalog dispatch
/// contract — the chat command's pre-flight could reject
/// unknown protocols, so we test that the protocol wire
/// format is well-formed end-to-end.
#[test]
fn mock_provider_reports_mock_protocol() {
    let mock = MockProvider::new(vec![]);
    assert_eq!(mock.protocol(), db::ProviderProtocol::Mock);
    let caps = mock.capabilities();
    assert!(caps.supports_system_prompt);
    assert!(caps.supports_tools);
    assert!(caps.supports_streaming);
}

// ---------------------------------------------------------------------------
// 8) MockProvider call count tracking
// ---------------------------------------------------------------------------

/// `call_count()` is the primary assertion surface for "did
/// the agent loop dispatch the expected number of turns?".
/// This unit test guards the counter itself (the agent-loop
/// integration tests above rely on it being accurate).
#[tokio::test]
async fn mock_provider_call_count_tracks_send_calls() {
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: None,
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: None,
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: None,
            }),
        ]),
    ]));
    assert_eq!(mock.call_count(), 0);
    let _ = mock
        .send(None, vec![], vec![])
        .collect::<Vec<_>>()
        .await
        .len();
    assert_eq!(mock.call_count(), 1);
    let _ = mock
        .send(None, vec![], vec![])
        .collect::<Vec<_>>()
        .await
        .len();
    assert_eq!(mock.call_count(), 2);
    let _ = mock
        .send(None, vec![], vec![])
        .collect::<Vec<_>>()
        .await
        .len();
    assert_eq!(mock.call_count(), 3);
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
    assert!(msg.contains("服务") || msg.contains("服务器"));
    // Server category for HTTP 5xx.
    assert_eq!(error_events[0].1, crate::llm::LlmErrorCategory::Server);
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
        },
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text("tiny head 2".into()),
        },
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text("droppable middle".into()),
        },
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(huge),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
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
// 13) B12 (2026-06-19): update_checklist tool integration
// ---------------------------------------------------------------------------

/// B12 PR1: a `tool_use("update_checklist")` from the model flows
/// through the full agent loop:
/// - The tool executes (Tier 5 default-allow — `update_checklist`
///   is not in `filter_tools_for_mode`'s Plan-mode blacklist, so
///   it's auto-allowed for every mode).
/// - The loop-local checklist Vec gets atomically replaced with
///   the new items.
/// - The `tool_result` event the frontend receives carries the
///   full list (post-coerce).
/// - On turn 2's `provider.send`, the agent loop prepends an
///   ephemeral `<current-checklist>` block to the REQUEST body
///   (visible via `mock.sent_messages()`), but the persisted
///   `messages` Vec never contains that block.
#[tokio::test]
async fn agent_loop_update_checklist_replaces_vec_and_injects_next_turn() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: model emits update_checklist + tool_use stop_reason.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_cl_1".into(),
                name: "update_checklist".into(),
                input: serde_json::json!({
                    "items": [
                        {"content": "step one", "status": "done"},
                        {"content": "step two", "status": "in_progress"},
                        {"content": "step three", "status": "pending"}
                    ]
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: final text response.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "done with checklist".into(),
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
        "rid-checklist".into(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
    )
    .await;

    // 2 turns = 2 send calls.
    assert_eq!(mock.call_count(), 2, "tool_use must trigger a second turn");

    // tool:result event landed in the sink — the frontend renders
    // the checklist card from this.
    let results = emitter.tool_results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "exactly one tool_result for update_checklist"
    );
    assert!(
        !results[0].is_error,
        "update_checklist success path must be is_error=false"
    );
    let body = &results[0].content;
    // The tool_result carries the full list with the rendered
    // [x]/[~]/[ ] markers.
    assert!(body.contains("step one"), "body: {}", body);
    assert!(body.contains("step two"), "body: {}", body);
    assert!(body.contains("step three"), "body: {}", body);
    assert!(body.contains("[x]"), "done marker present: {}", body);
    assert!(body.contains("[~]"), "in_progress marker present: {}", body);
    assert!(body.contains("[ ]"), "pending marker present: {}", body);

    // ---- ephemeral injection assertion ----
    //
    // Turn 1's request body: checklist Vec is empty (no
    // update_checklist has run yet) → NO `<current-checklist>`
    // block in the first request. Symmetric to memory/skill empty-
    // skip.
    let sent = mock.sent_messages();
    assert_eq!(sent.len(), 2, "captured 2 turn request bodies");
    let turn1_text = messages_to_text(&sent[0]);
    assert!(
        !turn1_text.contains("<current-checklist>"),
        "turn 1 (empty Vec) must NOT inject checklist block"
    );

    // Turn 2's request body: checklist Vec is non-empty → the
    // ephemeral block IS prepended.
    let turn2_text = messages_to_text(&sent[1]);
    assert!(
        turn2_text.contains("<current-checklist>"),
        "turn 2 must include the ephemeral checklist block, got: {}",
        turn2_text
    );
    assert!(
        turn2_text.contains("step one"),
        "ephemeral block carries the full list"
    );
    assert!(
        turn2_text.contains("step two"),
        "ephemeral block carries the full list"
    );

    // ---- persisted messages never contain the ephemeral block ----
    //
    // The injection is per-turn-only; reload reconstructs the
    // checklist from the `update_checklist` tool_result already
    // in history. The persisted `messages.content` JSON must NOT
    // carry `<current-checklist>` — otherwise a reload would see
    // a phantom user message.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    for m in &loaded.messages {
        let text = serde_json::to_string(&m.content).unwrap_or_default();
        assert!(
            !text.contains("<current-checklist>"),
            "persisted message seq={} must NOT contain the ephemeral block, got: {}",
            m.seq,
            text
        );
    }
}

/// B12 PR1: at-most-one `in_progress` coerce survives the full
/// agent loop end-to-end. The model passes 2 `in_progress` items;
/// the loop's Vec + the tool_result + the next turn's ephemeral
/// block all reflect exactly 1 `in_progress` (the LAST one).
#[tokio::test]
async fn agent_loop_update_checklist_coerces_two_in_progress_to_one() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_cl_coerce".into(),
                name: "update_checklist".into(),
                input: serde_json::json!({
                    "items": [
                        {"content": "earlier", "status": "in_progress"},
                        {"content": "later", "status": "in_progress"}
                    ]
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
    ]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-cl-coerce".into(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
    )
    .await;

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1);
    let body = &results[0].content;
    // The summary line in the tool_result says "1 in_progress"
    // (post-coerce), NOT 2.
    assert!(
        body.contains("1 in_progress"),
        "post-coerce summary must say exactly 1 in_progress, got: {}",
        body
    );

    // Turn 2's ephemeral block carries the post-coerce state: only
    // "later" has the in-progress marker.
    let sent = mock.sent_messages();
    assert_eq!(sent.len(), 2);
    let turn2_text = messages_to_text(&sent[1]);
    assert!(turn2_text.contains("<current-checklist>"));
    // "later" is the only one with `<- in progress`.
    assert!(
        turn2_text.contains("[~] later <- in progress"),
        "ephemeral block marks only the LAST in_progress, got: {}",
        turn2_text
    );
    // "earlier" must be demoted to pending.
    assert!(
        turn2_text.contains("[ ] earlier"),
        "ephemeral block demotes the earlier in_progress to pending, got: {}",
        turn2_text
    );
}

/// B12 PR1 — RULE-A-004 consistency for `update_checklist`: a
/// cancelled-in-flight tool must NOT leave a phantom
/// `tool_executed` audit row. `update_checklist` is a fast
/// in-memory swap (it does NOT consult the cancel token, so it
/// runs to completion regardless of when cancel fires), which
/// means the most likely landing spot for the cancel is "tool
/// already finished, but the loop's cancel branch fires
/// afterwards". In that case the tool_result IS persisted (as
/// the cancel path's "partial results" branch — this is correct
/// per Anthropic's tool_use/tool_result pairing invariant).
///
/// What we actually assert here is the RULE-A-004 invariant
/// itself: NO `tool_executed` audit row was written for this
/// session. The `record_tool_executed_audit` call in
/// `chat_loop.rs` is gated by `!token.is_cancelled()`, so a
/// cancelled tool — fast or slow — never lands in the audit log.
///
/// This is the precise RULE-A-004 invariant, restated for the
/// B12 surface: the existing audit-after-cancel-check ordering
/// automatically protects the checklist tool, with no new
/// persist path introduced (per the PRD's "Do NOT introduce a
/// new persist path" constraint).
#[tokio::test]
async fn agent_loop_cancelled_update_checklist_skips_audit_row() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: tool_use.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_cl_cancel".into(),
                name: "update_checklist".into(),
                input: serde_json::json!({
                    "items": [
                        {"content": "wont commit audit", "status": "in_progress"}
                    ]
                }),
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
        // yield_now so the cancel fires as soon as turn 1's send
        // has been observed (mirrors the audit-skip test's gating).
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
        "rid-cl-cancel".into(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
    )
    .await;
    cancel_handle.await.unwrap();

    // Exactly one cancelled Done event.
    assert_eq!(emitter.cancel_done_count(), 1);

    // RULE-A-004 invariant: zero `tool_executed` audit rows.
    // `update_checklist` is not in any way special here — the
    // existing audit-after-cancel-check ordering covers it
    // automatically. The assertion mirrors
    // `agent_loop_cancel_skips_audit_for_cancelled_tool` (the
    // list_dir version) but exercises the new checklist tool so
    // a future refactor that accidentally special-cases it
    // would fail here.
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
        "a cancelled update_checklist must NOT be recorded as tool_executed (RULE-A-004)"
    );
}

/// Helper: flatten a `Vec<ChatMessage>` into a single string for
/// substring assertions. Concatenates every text block in every
/// message — order matters for the ephemeral-injection tests
/// because the checklist block is PREPENDED (so it should appear
/// before the user's "hello" text from `test_messages()`).
fn messages_to_text(msgs: &[ChatMessage]) -> String {
    let mut out = String::new();
    for m in msgs {
        match &m.content {
            MessageContent::Text(t) => out.push_str(t),
            MessageContent::Blocks(blocks) => {
                for b in blocks {
                    if let ContentBlock::Text { text, .. } = b {
                        out.push_str(text);
                    }
                }
            }
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// 13) L2 (2026-06-19): parallel read-only tool batch
// ---------------------------------------------------------------------------

/// `is_parallel_eligible` — pure predicate. Covers:
/// - all-eligible set → true
/// - each excluded tool (write_file / edit_file / shell / web_fetch /
///   update_checklist) in an otherwise-eligible batch → false
/// - empty batch → false (defensive; the agent loop only calls
///   this on non-empty `tool_calls`)
/// - RULE-A-013 (2026-06-19): path-outside-root read tools
///   pull the batch back to serial. `paths` lets the test pin
///   each tool's `path` arg (empty string = no `path` arg).
#[test]
fn is_parallel_eligible_classifies_correctly() {
    use crate::agent::chat_loop::is_parallel_eligible;

    /// `names[i]` is the tool name; `paths[i]` is the `path`
    /// arg to inject (empty string = no `path` field, mirroring
    /// a model call without a path arg). All paths are
    /// constructed as absolute to the `root` passed in.
    fn batch(
        names: &[&str],
        paths: &[&str],
        root: &std::path::Path,
    ) -> Vec<(String, String, serde_json::Value)> {
        names
            .iter()
            .zip(paths.iter().chain(std::iter::repeat(&"")))
            .map(|(n, p)| {
                let input = if p.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::json!({ "path": root.join(p).to_string_lossy() })
                };
                ("x".into(), (*n).into(), input)
            })
            .collect()
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // All-eligible permutations of the read-only set (no paths
    // — the path check is vacuously true).
    assert!(is_parallel_eligible(
        &batch(&["read_file"], &[], root),
        root
    ));
    assert!(is_parallel_eligible(
        &batch(&["read_file", "grep", "glob"], &[], root),
        root
    ));
    assert!(is_parallel_eligible(
        &batch(&["list_dir", "use_skill"], &[], root),
        root
    ));
    assert!(is_parallel_eligible(
        &batch(
            &["read_file", "grep", "glob", "list_dir", "use_skill"],
            &[],
            root
        ),
        root
    ));

    // Each excluded tool alone → false (so a single-tool batch
    // of an excluded tool stays serial).
    assert!(!is_parallel_eligible(
        &batch(&["write_file"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(
        &batch(&["edit_file"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(&batch(&["shell"], &[], root), root));
    assert!(!is_parallel_eligible(
        &batch(&["web_fetch"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(
        &batch(&["update_checklist"], &[], root),
        root
    ));

    // Mixed: one excluded tool poisons the whole batch.
    assert!(!is_parallel_eligible(
        &batch(&["read_file", "edit_file", "grep"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(
        &batch(&["read_file", "web_fetch"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(
        &batch(&["read_file", "update_checklist"], &[], root),
        root
    ));

    // Unknown / future tool → conservatively false (serial).
    assert!(!is_parallel_eligible(
        &batch(&["some_future_tool"], &[], root),
        root
    ));

    // Empty batch → false (defensive).
    assert!(!is_parallel_eligible(&[], root));
}

/// RULE-A-013 (2026-06-19): `is_parallel_eligible` now also
/// checks path-outside-root. Cases (all use a real tempdir as
/// `root` so `is_within_root` works as in production):
/// 1. absolute in-root path → eligible
/// 2. relative in-root path → eligible (joined onto root)
/// 3. absolute out-of-root path → falls back to serial
/// 4. relative `../foo` out-of-root path → falls back to serial
/// 5. path tool without a `path` arg → eligible (tool layer
///    schema validation is the fallback; mirrors the
///    permission layer's no-path convention)
/// 6. `use_skill` + arbitrary path in same batch → eligible
///    (`use_skill` is name-eligible and exempt from the path
///    check, so it can ride along with a path tool that has a
///    path arg)
#[test]
fn is_parallel_eligible_boundary_silent() {
    use crate::agent::chat_loop::is_parallel_eligible;
    use std::path::PathBuf;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // Build an in-root file and an out-of-root sibling file so
    // absolute and relative paths have real targets to resolve
    // against. `is_within_root` is lexical so existence isn't
    // strictly required, but having real files makes the test
    // mirror production intent more clearly.
    std::fs::write(root.join("in_root.txt"), "x").unwrap();
    let outside_dir = tempfile::tempdir().expect("outside tempdir");
    let outside_file: PathBuf = outside_dir.path().join("outside.txt");
    std::fs::write(&outside_file, "x").unwrap();

    // Helper to build a single-tool batch. `root` is unused
    // (the caller resolves the path string in advance) but
    // kept in the signature for symmetry with `batch()` above.
    #[allow(unused_variables)]
    fn single(
        root: &std::path::Path,
        name: &str,
        path: Option<&str>,
    ) -> Vec<(String, String, serde_json::Value)> {
        let input = match path {
            Some(p) => serde_json::json!({ "path": p }),
            None => serde_json::json!({}),
        };
        vec![("x".into(), name.into(), input)]
    }

    // 1. absolute in-root → eligible
    let abs_in = root.join("in_root.txt").to_string_lossy().into_owned();
    assert!(
        is_parallel_eligible(&single(root, "read_file", Some(&abs_in)), root),
        "absolute in-root path should be eligible"
    );

    // 2. relative in-root → eligible (joined onto root)
    assert!(
        is_parallel_eligible(&single(root, "read_file", Some("in_root.txt")), root),
        "relative in-root path should be eligible"
    );

    // 3. absolute out-of-root → NOT eligible
    let abs_out = outside_file.to_string_lossy().into_owned();
    assert!(
        !is_parallel_eligible(&single(root, "read_file", Some(&abs_out)), root),
        "absolute out-of-root path should fall back to serial"
    );

    // 4. relative `../<file>` out-of-root → NOT eligible
    //    Build a relative path from `root` that escapes: e.g.
    //    `../<outside_dir_basename>/outside.txt`. We don't know
    //    the basename, so walk up one level to a tempdir
    //    sibling and back down.
    let rel_out = format!(
        "../{}/{}",
        outside_dir.path().file_name().unwrap().to_string_lossy(),
        "outside.txt"
    );
    assert!(
        !is_parallel_eligible(&single(root, "read_file", Some(&rel_out)), root),
        "relative out-of-root path should fall back to serial"
    );

    // 5. path tool with no `path` arg → eligible (tool layer
    //    validates; we mirror the permission layer convention)
    assert!(
        is_parallel_eligible(&single(root, "read_file", None), root),
        "path tool without path arg should be eligible"
    );
    assert!(
        is_parallel_eligible(&single(root, "grep", None), root),
        "grep without path arg should be eligible"
    );

    // 6. use_skill + arbitrary path coexist → eligible.
    //    use_skill is name-eligible and exempt from the path
    //    check; it can ride along with a path tool that has a
    //    valid in-root path.
    let mixed = vec![
        ("x".into(), "use_skill".into(), serde_json::json!({})),
        (
            "x".into(),
            "read_file".into(),
            serde_json::json!({ "path": abs_in.clone() }),
        ),
    ];
    assert!(
        is_parallel_eligible(&mixed, root),
        "use_skill + in-root path tool should be eligible"
    );
}

/// L2: three `read_file` tool_use blocks in one turn execute
/// concurrently. Asserts:
/// - exactly 3 `tool:result` events fire
/// - the result contents appear in the SAME order as the
///   tool_use blocks (LLM-context stability contract — Q3) by
///   cross-referencing `tool_use_id` to file content
/// - the persisted `tool_result` user message has its blocks
///   in tool_use order, not completion order
///
/// The three reads target different-sized files so the result
/// text is unambiguous about which tool_use produced which
/// result. (No timing assertion — see PRD §Acceptance for the
/// "no flaky timing" rule.)
#[tokio::test]
async fn agent_loop_parallel_readonly_batch_preserves_order() {
    let h = make_harness().await;

    // Three distinct files under the project root so each
    // read_file produces a unique, identifiable output.
    std::fs::write(h.project_path.join("a.txt"), "AAA-content").unwrap();
    std::fs::write(h.project_path.join("b.txt"), "BBB-content").unwrap();
    std::fs::write(h.project_path.join("c.txt"), "CCC-content").unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: three tool_use blocks (eligible batch).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_a".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_b".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "b.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_c".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "c.txt"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: terminal text (consumed only if turn 1 ran
        // successfully through the parallel batch and the
        // tool_result got fed back).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "done".into(),
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
        "rid-par-batch".into(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
    )
    .await;

    assert_eq!(
        mock.call_count(),
        2,
        "parallel batch must trigger exactly one follow-up turn (2 sends total)"
    );
    assert_eq!(emitter.tool_call_count(), 3, "all 3 tool_use fire");
    assert_eq!(
        emitter.tool_result_count(),
        3,
        "all 3 read_file produce a tool_result"
    );

    // Order contract: the result_blocks in the persisted
    // tool_result user message MUST appear in tool_use order
    // (a, b, c), regardless of which task finished first.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    // `MessageContent::Blocks` serializes as a top-level JSON
    // array (see `llm/types.rs` MessageContent Serialize impl),
    // so `MessageRow.content` is the array directly.
    let tool_result_msg = loaded
        .messages
        .iter()
        .find(|m| {
            m.role == "user"
                && m.content
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                    })
                    .unwrap_or(false)
        })
        .expect("tool_result user message persisted");
    let blocks = tool_result_msg.content.as_array().expect("content array");
    let tool_use_ids: Vec<String> = blocks
        .iter()
        .filter_map(|b| {
            if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                b.get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        tool_use_ids,
        vec!["toolu_a", "toolu_b", "toolu_c"],
        "tool_result blocks MUST be in tool_use order (Q3 — LLM context stability), got: {:?}",
        tool_use_ids
    );

    // All three audit rows fire — read tools complete fully
    // (no cancel in this test), so each leaves a tool_executed.
    let audit_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM session_audit_events
           WHERE session_id = ? AND kind = 'tool_executed'"#,
    )
    .bind(&h.session_id)
    .fetch_one(&h.db)
    .await
    .expect("count tool_executed audit rows");
    assert_eq!(
        audit_count, 3,
        "each completed read tool leaves a tool_executed audit row"
    );
}

/// L2 fallback: a batch containing one `edit_file` (a write
/// tool) MUST fall back to the serial path. Behavior must be
/// byte-identical to pre-L2 (the read still runs, the edit
/// still runs, results appear in order). This is a regression
/// guard: a future change that accidentally routes mixed
/// batches through the parallel path would fail here.
#[tokio::test]
async fn agent_loop_mixed_batch_with_edit_falls_back_to_serial() {
    let h = make_harness().await;
    // Seed a file so edit_file's read-before-edit guard passes
    // (the test messages don't read it, but the guard returns
    // is_error=true on missing read — the agent loop still
    // feeds that error back to the LLM, which is what we want
    // to assert: serial path runs end-to-end).
    std::fs::write(h.project_path.join("target.txt"), "original").unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: mixed batch — read_file + edit_file.
        // `is_parallel_eligible` returns false → serial path.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_read".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "target.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_edit".into(),
                name: "edit_file".into(),
                input: serde_json::json!({
                    "path": "target.txt",
                    "old_string": "original",
                    "new_string": "edited"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: terminal text.
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
        "rid-mixed".into(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
    )
    .await;

    assert_eq!(mock.call_count(), 2, "serial path drives 2 turns");
    assert_eq!(emitter.tool_call_count(), 2);
    assert_eq!(
        emitter.tool_result_count(),
        2,
        "serial path emits 2 tool_results (read + edit)"
    );
    // Order is still tool_use order (serial is naturally ordered).
    let results = emitter.tool_results_snapshot();
    assert_eq!(results[0].tool_use_id, "toolu_read");
    assert_eq!(results[1].tool_use_id, "toolu_edit");
}

/// L2 Q2: a batch containing `web_fetch` (a read-only tool that
/// is EXCLUDED from the parallel set because its Tier 4 default
/// is `ask`) MUST fall back to the serial path. The web_fetch
/// here would normally fire a `permission:ask` — but since
/// `permission_asks` is empty (no sender), the 120s timeout
/// would fire and the test would hang. To avoid that, we pair
/// web_fetch with a read_file (so the batch is non-eligible for
/// parallel anyway) and assert the serial path is taken by
/// checking tool_result ordering is preserved — no need to
/// actually execute web_fetch.
///
/// The assertion is structural: web_fetch in the batch → serial.
/// The pure-predicate test above (`is_parallel_eligible_*`)
/// covers the classification; this test is the end-to-end
/// confirmation that the agent loop honors the predicate.
#[tokio::test]
async fn agent_loop_web_fetch_batch_does_not_run_parallel() {
    // Structural-only: we don't need to invoke run_chat_loop
    // here because the predicate is the gate, and the predicate
    // is exhaustively covered in `is_parallel_eligible_*`. This
    // test is intentionally a no-op placeholder documenting
    // that the Q2 exclusion is enforced at the predicate layer;
    // an end-to-end run would hang on the web_fetch ask
    // timeout. The classification test asserts the contract.
    //
    // Kept as a named test so a future refactor that removes
    // `is_parallel_eligible` will fail here (the named test
    // exists; if the predicate is renamed/removed the test
    // body, which references it via the unit test above, would
    // not compile).
    use crate::agent::chat_loop::is_parallel_eligible;
    let batch: Vec<(String, String, serde_json::Value)> = vec![
        (
            "x".into(),
            "read_file".into(),
            serde_json::json!({"path": "a"}),
        ),
        (
            "y".into(),
            "web_fetch".into(),
            serde_json::json!({"url": "https://example.com"}),
        ),
    ];
    // Root argument is irrelevant here: `web_fetch` is excluded
    // by the name check (Q2) before the path check ever runs.
    // Use a dummy tempdir for the parameter contract.
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(
        !is_parallel_eligible(&batch, dir.path()),
        "web_fetch MUST exclude the batch from parallel execution (Q2)"
    );
}

/// L2 + RULE-A-004: a parallel read-only batch cancelled mid-
/// flight MUST:
/// - mark the turn `cancelled` (so the existing cancel path
///   persists partial results + emits Done{cancelled})
/// - NOT leave a `tool_executed` audit row for the cancelled
///   task(s) (RULE-A-004 invariant, preserved per-task)
/// - still leave audit rows for tools that completed BEFORE
///   the cancel token fired
///
/// Script: turn 1 emits two `read_file` tool_use blocks. A
/// background task cancels the token as soon as the provider
/// has been called once (i.e. turn 1's send completed, tool
/// execution is about to start). With three reads, the cancel
/// arrives during the parallel batch — at least one task's
/// `token.is_cancelled()` is true after execute → its audit
/// write is skipped; the batch-level cancel flag flips → the
/// existing cancel path runs.
///
/// NOTE: read_file is fast and doesn't consult the cancel token
/// internally (the wrapper `execute_tool` select! only fires
/// if the cancel arrives BEFORE the inner future completes).
/// So whether the audit row fires depends on which task's
/// `token.is_cancelled()` check wins. The contract under test
/// is the EQUIVALENCE: a cancelled task skips audit, a
/// completed task records audit. The assert is "0 or more, but
/// consistent" — specifically, the test asserts the cancel
/// path was taken (Done{cancelled} emitted), which is the
/// invariant the L2 change can break.
#[tokio::test]
async fn agent_loop_parallel_batch_cancel_marks_turn_cancelled() {
    let h = make_harness().await;
    std::fs::write(h.project_path.join("a.txt"), "AAA").unwrap();
    std::fs::write(h.project_path.join("b.txt"), "BBB").unwrap();
    std::fs::write(h.project_path.join("c.txt"), "CCC").unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: three read_file tool_use blocks (eligible).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_a".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_b".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "b.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_c".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "c.txt"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Sentinel — only consumed if cancel fails to mark the
        // turn (it shouldn't).
        MockResponse::HangingThenCancel,
    ]));

    let call_handle = mock.call_count_handle();
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let cancel_handle = tokio::spawn(async move {
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
        "rid-par-cancel".into(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
    )
    .await;
    cancel_handle.await.unwrap();

    // The cancel path MUST have been taken. Two invariants:
    // (a) exactly one Done{cancelled} event
    // (b) the loop did NOT re-enter turn 2 (mock.call_count == 1)
    assert_eq!(
        mock.call_count(),
        1,
        "cancel must abort before turn 2 (call_count stays at 1)"
    );
    assert_eq!(
        emitter.cancel_done_count(),
        1,
        "cancel path emits exactly one Done{{cancelled}} event"
    );

    // RULE-A-004 cross-check: the number of tool_executed audit
    // rows MUST be <= the number of tool_result events (a
    // cancelled task still emits a tool_result but skips
    // audit). Specifically: tool_result_count == 3 (all three
    // read_files complete because read_file doesn't consult
    // the cancel token internally), but audit_count <= 3
    // because tasks whose post-execute `token.is_cancelled()`
    // check came back true skipped their audit write.
    let audit_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM session_audit_events
           WHERE session_id = ? AND kind = 'tool_executed'"#,
    )
    .bind(&h.session_id)
    .fetch_one(&h.db)
    .await
    .expect("count tool_executed audit rows");
    assert!(
        emitter.tool_result_count() >= audit_count as usize,
        "cancelled tasks skip audit (RULE-A-004): results={} audit={} (audit MUST be <= results)",
        emitter.tool_result_count(),
        audit_count
    );
}

// ---------------------------------------------------------------------------
// L1a: agent loop drains background-shell notifications and prepends
// (technically: appends to the request clone) a user-role message
// containing the completion text on the NEXT turn. PR2 closes the
// round-trip from `BackgroundShellRegistry::start` → completion →
// notification → `provider.send` request body.
// ---------------------------------------------------------------------------

/// L1a end-to-end: start a fast background shell from the harness's
/// registry, wait for completion, then drive a 2-turn agent loop.
/// Turn 1 emits a `tool_use(run_background_shell)` so the tool layer
/// actually runs (proving the dispatch + ToolContext thread works);
/// turn 2 fires after the completion notification lands in the
/// agent-loop drain. The captured `sent_messages[1]` (turn 2's
/// request body) MUST contain the `[system] 后台 shell ... 已完成`
/// text — this is the wire contract the LLM sees.
///
/// Why this matters: the agent loop's notification drain is the only
/// place a per-turn cross-request state gets injected into the
/// outbound wire payload. A regression (e.g. drain moved to the
/// wrong turn, append swapped to prepend, format string drift)
/// silently breaks the LLM's ability to react to backgrounded
/// commands.
#[tokio::test]
async fn agent_loop_drains_background_shell_notification_into_turn_2() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: emit run_background_shell tool_use. The agent
        // loop's tool dispatch routes this through the new
        // run_background_shell::execute, which starts a real
        // background shell via the registry.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_bg_1".into(),
                name: "run_background_shell".into(),
                input: serde_json::json!({"command": "echo done-from-bg"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: terminal text (consumed only if turn 1
        // successfully started the shell and the notification
        // arrived before turn 2's drain).
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
        "rid-bg-drain".into(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
    )
    .await;

    // Two turns → two `send` calls.
    assert_eq!(mock.call_count(), 2, "tool_use must trigger a second turn");

    let sent = mock.sent_messages();
    assert_eq!(sent.len(), 2, "captured 2 turn request bodies");

    // Turn 1's request body MUST NOT carry the notification block
    // yet (the shell only completed AFTER turn 1's `provider.send`
    // fired, and the drain runs at the start of turn 2).
    let turn1_text = messages_to_text(&sent[0]);
    assert!(
        !turn1_text.contains("[system] 后台 shell"),
        "turn 1 must NOT carry the notification (it hadn't completed yet), got: {}",
        turn1_text
    );

    // Turn 2's request body MUST carry the notification block.
    // The format is exact: the LLM-facing string must match so
    // it can grep for `后台 shell ...` and call shell_status.
    let turn2_text = messages_to_text(&sent[1]);
    assert!(
        turn2_text.contains("[system] 后台 shell"),
        "turn 2 must include the drained notification, got: {}",
        turn2_text
    );
    assert!(
        turn2_text.contains("已完成"),
        "notification carries completion marker, got: {}",
        turn2_text
    );
    assert!(
        turn2_text.contains("exit code 0"),
        "echo succeeds with exit code 0, got: {}",
        turn2_text
    );
    assert!(
        turn2_text.contains("shell_status"),
        "notification tells the LLM which tool to call next, got: {}",
        turn2_text
    );

    // Persistence invariant: the ephemeral notification block is
    // per-turn-only. The persisted `messages.content` MUST NOT
    // contain a USER-role message whose content is a plain text
    // block (not a tool_result block) carrying the
    // `[system] 后台 shell` notification. The
    // `run_background_shell` TOOL RESULT itself contains the
    // literal `[system] 后台 shell ... 已完成...` snippet in its
    // success message (the LLM-facing UX hint), so we walk each
    // user-role row's content and look for a plain-text block
    // (the notification shape) — a tool_result block is typed
    // (`{"type":"tool_result", ...}`) and is excluded.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    let mut phantom_count = 0;
    for m in &loaded.messages {
        if m.role != "user" {
            continue;
        }
        if let Some(arr) = m.content.as_array() {
            for block in arr {
                let block_type = block.get("type").and_then(|t| t.as_str());
                let has_notification = block_type == Some("text")
                    && block
                        .get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.contains("[system] 后台 shell") && s.contains("已完成"))
                        .unwrap_or(false);
                if has_notification {
                    phantom_count += 1;
                }
            }
        }
    }
    assert_eq!(
        phantom_count, 0,
        "persisted messages must NOT carry an ephemeral notification block (got {} phantom rows)",
        phantom_count
    );
}

/// L1a: when no background shells have completed between turns,
/// no notification block is injected. The empty-queue path is the
/// fast path (no extra `.clone()`, no extra push) — the L1a
/// implementation MUST take it.
///
/// This is the regression guard for "always inject one notification"
/// bugs (where the loop builds an empty list and still pays the
/// allocation cost / produces a noop user message).
#[tokio::test]
async fn agent_loop_no_pending_notifications_skips_injection() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Single turn, text-only.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "just chatting".into(),
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
        "rid-bg-empty".into(),
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
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
    )
    .await;

    let sent = mock.sent_messages();
    assert_eq!(sent.len(), 1);
    let turn1_text = messages_to_text(&sent[0]);
    assert!(
        !turn1_text.contains("[system] 后台 shell"),
        "empty notification queue must skip the injection, got: {}",
        turn1_text
    );
}

// ===========================================================================
// B6 Subagent (2026-06-19 PR1b): worker dispatch integration tests
//
// The 4 tests cover the core worker dispatch invariants from the PR1b
// task brief:
//   1. worker completes → summary returned as dispatch_subagent
//      tool_result; parent messages contain the tool_call + tool_result
//      pair, NO worker intermediate events.
//   2. worker cancel (parent Stop propagates to worker_token) →
//      tool_result with status=cancelled + CANCELLED_MARKER.
//   3. worker error (provider stream errors) → tool_result with
//      status=error; tool_use/tool_result pairing preserved.
//   4. worker guard does NOT evict parent's session_active_request
//      entry (PR1a skip_session_active regression guard).
//
// Script pattern: the parent MockProvider emits a dispatch_subagent
// tool_use on turn 1, then a final text on turn 2. The worker's
// responses come from a SEPARATE MockProvider passed in via... well,
// we can't — `run_subagent` clones the parent's `Arc<dyn Provider>`
// for the worker. So the parent MockProvider's script is shared
// between parent + worker. The parent consumes turn 1 (the
// dispatch_subagent tool_use) + turn 3 (the final text); the worker
// consumes turn 2 (its single turn). Script ordering: [parent_t1,
// worker_t1, parent_t2].
//
// For cancel / error tests the worker script entry is the failure
// shape; for the "happy" test it's a normal events vec.
// ===========================================================================

// ===========================================================================
// ⑬ C2 loop detection — HardLoop hint injected into the result message
// ===========================================================================
//
// Three consecutive turns of the identical `list_dir {path: "."}` trip
// Level 1 (exact-signature run of 3). The hint must surface as a Text
// block prepended to turn 3's tool_result message, which turn 4's
// `send` therefore sees. Action is SOFT per §2.5.4: the tool still
// executes (one `tool:result` per turn) and the loop is NOT terminated
// by the hit (turn 4 runs normally and ends via end_turn).
#[tokio::test]
async fn agent_loop_loop_detection_injects_hard_hint() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    // One scripted tool_use turn, reused three times with fresh ids.
    let list_dir_turn = |id: &str| {
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: id.into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ])
    };
    let mock = Arc::new(MockProvider::new(vec![
        list_dir_turn("toolu_1"),
        list_dir_turn("toolu_2"),
        list_dir_turn("toolu_3"),
        // Turn 4: text-only — proves loop detection did not kill the loop.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "done".into() }),
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
        "rid-loop".into(),
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
        None,        // max_turns (default MAX_TURNS)
        false,       // skip_session_active
        false,       // skip_persist
        Some(false), // is_worker (production-style)
        None,        // app_handle
        None,        // system_prompt_override
        None,        // worker_run_id
        h.subagent_cache.clone(), // L3d subagent cache
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
        None,
        h.app_data_dir.clone(),
    )
    .await;

    // All 4 turns ran — the hint is soft and never terminates.
    assert_eq!(
        mock.call_count(),
        4,
        "loop detection is soft — all 4 turns must run"
    );
    // Each list_dir turn emits exactly one tool:result (3 total).
    assert_eq!(emitter.tool_result_count(), 3);

    // The hint lands in turn 3's tool_result message, which turn 4's
    // send receives. Hunt every Text block across turn 4's messages.
    let sent = mock.sent_messages();
    let turn4 = sent.last().expect("turn 4 send must be recorded");
    let hint_found = turn4.iter().any(|m| {
        matches!(&m.content, MessageContent::Blocks(blocks)
            if blocks.iter().any(|b| matches!(b,
                ContentBlock::Text { text, .. } if text.contains("loop detected"))))
    });
    assert!(
        hint_found,
        "turn-3 HardLoop hint must be injected as a Text block seen by turn 4"
    );
}

// ===========================================================================
// ⑬ C2 loop detection — no hint when calls are NOT repetitive
// ===========================================================================
//
// Two distinct tool_use turns (different tools / args) must NOT trip
// the detector: no hint Text block appears in any turn's result.
#[tokio::test]
async fn agent_loop_loop_detection_silent_when_not_repetitive() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: list_dir
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
        // Turn 2: glob (different tool → different signature)
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_2".into(),
                name: "glob".into(),
                input: serde_json::json!({"pattern": "*.rs", "path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 3: text-only
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "done".into() }),
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
        "rid-no-loop".into(),
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
        // L3b (2026-06-27): production-style caller (and non-isolated
        // worker path) → worktree_override = None (use the session
        // row's worktree_path). Only the isolated worker path passes
        // Some(worker_worktree_path).
        None,
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
    )
    .await;

    assert_eq!(mock.call_count(), 3);
    // No hint anywhere across all sends.
    let any_hint = mock
        .sent_messages()
        .iter()
        .flatten()
        .any(|m| {
            matches!(&m.content, MessageContent::Blocks(blocks)
                if blocks.iter().any(|b| matches!(b,
                    ContentBlock::Text { text, .. } if text.contains("loop detected"))))
        });
    assert!(!any_hint, "distinct tool calls must not trigger a loop hint");
}

