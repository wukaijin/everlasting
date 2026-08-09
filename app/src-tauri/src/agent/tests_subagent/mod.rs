#![cfg(test)]

// Re-export so cluster files can keep their `use super::tests_common::...`
// imports unchanged from the pre-split single-file layout (pure relocation).
#[allow(unused_imports)]
use super::tests_common;

mod dispatch_main;
mod forced_dispatch;
mod l3a_concurrent;
mod l3a_unit;
mod l3b_concurrent;
mod l3b_discard;
mod l3b_merge;
mod persist_audit_token;
mod plan_mode;
mod system_prompt_override;

use std::sync::Arc;

use super::tests_common::MockEmitter;
use crate::agent::chat_loop::run_chat_loop;
use crate::llm::provider::mock::MockProvider;

/// Helper that runs `run_chat_loop` with the standard test arguments
/// (mirrors the call sites in the B6 tests above but lets the L3a
/// tests specify only the script + rid + token). Reduces the 23+
/// parameter boilerplate per test.
pub(super) async fn run_loop(
    h: &super::tests_common::TestHarness,
    mock: Arc<MockProvider>,
    emitter: Arc<MockEmitter>,
    rid: &str,
    messages: Vec<crate::llm::types::ChatMessage>,
    token: tokio_util::sync::CancellationToken,
) {
    run_chat_loop(
        vec![],
        mock,
        200_000,
        rid.into(),
        h.session_id.clone(),
        messages,
        emitter,
        h.db.clone(),
        h.cancellations.clone(),
        h.session_active_request.clone(),
        h.read_guard.clone(),
        h.memory_cache.clone(),
        h.skill_cache.clone(),
        h.permission_asks.clone(),
        token,
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
    )
    .await;
}
