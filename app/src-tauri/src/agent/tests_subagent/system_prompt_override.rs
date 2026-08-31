#![cfg(test)]

use std::sync::Arc;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
};
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
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-worker-override".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        {
            let mut role = parent_role(&h);
            // B6 PR2b: production-style caller is NOT a worker
            // (this is the worker-path test, so the
            // `is_worker` flag itself is `Some(false)` — the
            // "worker-ness" is conveyed by the
            // `system_prompt_override` param, not by `is_worker`).
            // The `is_worker` flag governs the ⑨ 关 Tier 4
            // collapse; the override is a separate concern.
            //
            // 2026-06-22 (RULE-FrontSubagent-003 fix): this test
            // exercises the worker prompt override (B6 review defect
            // A); it's NOT a worker ask test. The
            // `is_worker=Some(false)` already routes ask_path to the
            // parent branch. worker_run_id stays None.
            // The actual fix being tested.
            role.system_prompt_override = Some(worker_prompt.clone());
            role
        },
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
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-parent-override-none".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
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
    let base_prompt = build_system_prompt(&loaded.session, &project, &worktree_path);
    // D3: the SHA no longer rides the system prompt; it rides the
    // per-turn tail repo-state block. Keep the lookup above only to
    // assert the prompt is SHA-free (byte-stable within a session).
    assert!(
        !base_prompt.contains(&head_sha),
        "D3 invariant: system prompt must not embed head_sha ({head_sha})"
    );
    let expected = assemble_system_prompt(mode_system_prefix(loaded.session.mode), &base_prompt);
    assert_eq!(
        received, &expected,
        "parent path (override=None) must send the parent's \
         `assemble_system_prompt(mode_prefix, base_prompt)` output; \
         the worker-path fix must NOT regress the parent path"
    );
}
