#![cfg(test)]

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::agent::system_prompt::build_system_prompt;
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

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
        None,
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
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
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
        // F1 queue driver (2026-08-25): single-shot call site —
        // guard-owned cleanup (not a continuation round).
        false,
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
        None,
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
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        // Production path: `None` override.
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
        // F1 queue driver (2026-08-25): single-shot call site —
        // guard-owned cleanup (not a continuation round).
        false,
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
