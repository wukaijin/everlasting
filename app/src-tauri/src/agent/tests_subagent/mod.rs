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
use super::tests_common::{chat_loop_deps, chat_loop_request, parent_role};
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
        chat_loop_request(
            vec![],
            mock,
            200_000,
            rid.into(),
            h.session_id.clone(),
            messages,
            emitter,
        ),
        {
            let mut deps = chat_loop_deps(&h);
            deps.token = token;
            deps
        },
        parent_role(&h),
    )
    .await;
}
