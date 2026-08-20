#![cfg(test)]

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::messages_to_text;
use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};
use crate::llm::{ContentBlock, MessageContent};

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
        None,
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
        None,
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
// block appended to turn 3's tool_result message, which turn 4's
// `send` therefore sees. Action is SOFT per §2.5.4: the tool still
// executes (one `tool:result` per turn) and the loop is NOT terminated
// by the hit (turn 4 runs normally and ends via end_turn).
//
// HINT POSITION (chat_loop.rs ~2328): the hint is APPENDED to
// `result_blocks` (not inserted at index 0). Putting it at the head
// would make the wire fan-out produce `user(text) → tool×N`, which
// OpenAI rejects with 400 "tool_calls must be followed by tool
// messages". Appended yields `tool×N → user(text)` — both providers
// accept it. See `wire::orphan_tool_call_order` for the wire-layer
// diagnostic that catches a regression here.
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
        None,
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
        None,        // worker_catalog
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        None,        // system_prompt_override
        None,        // worker_run_id
        h.subagent_cache.clone(), // L3d subagent cache
        None,
        // L3b (2026-06-27): production-style caller → worktree_override = None.
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
        None,
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
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        None,
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

    assert_eq!(mock.call_count(), 3);
    // No hint anywhere across all sends.
    let any_hint = mock.sent_messages().iter().flatten().any(|m| {
        matches!(&m.content, MessageContent::Blocks(blocks)
                if blocks.iter().any(|b| matches!(b,
                    ContentBlock::Text { text, .. } if text.contains("loop detected"))))
    });
    assert!(
        !any_hint,
        "distinct tool calls must not trigger a loop hint"
    );
}
