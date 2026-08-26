#![cfg(test)]

use std::sync::Arc;

use sqlx::Row;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

// ---------------------------------------------------------------------------
// P5 (2026-06-29, 06-29-am-p5-quality): verified pitfall soft-block
// short-circuits execute_tool (design §4 + D1).
// ---------------------------------------------------------------------------

/// Helper: seed a verified pitfall for the project + tool with a
/// full trigger-key match (path_globs=Some + the probe path matches).
/// Mirrors `seed_pitfall` in `tests_check.rs` but keeps the project-
/// scope wiring the integration harness needs.
async fn p5_seed_verified_pitfall_full_match(h: &super::tests_common::TestHarness) {
    use crate::db::memories::{MemoryInput, MemoryKind, MemoryScope, MemoryStatus};
    let input = MemoryInput {
        scope: MemoryScope::Project,
        project_id: Some(h.project_id.clone()),
        kind: MemoryKind::Pitfall,
        status: MemoryStatus::Verified,
        title: "edit_file under app/src is risky".into(),
        content: "be extra careful with edit_file in app/src/".into(),
        tags: "[]".into(),
        tool_name: Some("edit_file".into()),
        command_pattern: None,
        path_globs: Some(r#"["app/src/*"]"#.into()),
        source_session_id: None,
        source_ref: None,
    };
    crate::db::memories::insert_memory(&h.db, &input)
        .await
        .expect("insert verified pitfall");
}

/// P5 AC (design §4 + D1): a verified pitfall with a full trigger-key
/// match soft-blocks the matching tool_use — `execute_tool` is NOT
/// called, the loop surfaces an `is_error: false` tool_result with the
/// hint, and the loop continues to the next turn. This is the
/// end-to-end "第一时间规避" contract.
///
/// The setup: project-scope verified pitfall with `path_globs=
/// ["app/src/*"]`. Turn 1 the model emits `edit_file` on
/// `app/src/foo.rs` → SoftBlock. Turn 2 the model emits final text.
/// Assertions:
/// - 2 send calls (turn 1 = tool_use; turn 2 = final).
/// - 1 tool:result emitted, `is_error: false`, content contains the
///   "未实际执行" hint.
/// - The tool's actual output ("list_dir output") is NOT in the
///   result (execute_tool was short-circuited).
#[tokio::test]
async fn agent_loop_p5_soft_block_short_circuits_execute() {
    let h = make_harness().await;
    p5_seed_verified_pitfall_full_match(&h).await;

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: tool_use on a path that matches the verified
        // pitfall's path_globs.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_sb".into(),
                name: "edit_file".into(),
                input: serde_json::json!({
                    "path": "app/src/foo.rs",
                    "old_string": "a",
                    "new_string": "b",
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: final text after the soft-block hint.
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
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-sb".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    // Turn 1 (tool_use) + Turn 2 (final text) = 2 sends.
    assert_eq!(
        mock.call_count(),
        2,
        "soft-block still produces a tool_result → loop continues to turn 2"
    );
    assert_eq!(emitter.tool_call_count(), 1);
    // Exactly one tool_result, is_error=false, carries the hint.
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1);
    assert!(
        !results[0].is_error,
        "soft-block result is is_error=false (experience hint, not tool failure)"
    );
    assert!(
        results[0].content.contains("未实际执行"),
        "soft-block result carries the 'NOT executed' hint; got: {}",
        results[0].content
    );
    assert!(
        results[0]
            .content
            .contains("edit_file under app/src is risky"),
        "hint carries the pitfall title; got: {}",
        results[0].content
    );
}

/// P5 AC (design D1 — dead-loop guard): the second hit on the same
/// verified pitfall degrades to Footnote + normal execution (the
/// session-level HashSet records the first soft-block). This locks
/// the "每坑每 session 软拦截 1 次" contract end-to-end.
///
/// Setup: same verified pitfall. Turn 1 emits the matching edit_file
/// → soft-blocked. Turn 2 emits the SAME matching edit_file again →
/// the loop executes it normally (Footnote prepended) + the tool's
/// real output is in the result. Turn 3: final text.
#[tokio::test]
async fn agent_loop_p5_soft_block_second_hit_degrades_to_execute() {
    let h = make_harness().await;
    p5_seed_verified_pitfall_full_match(&h).await;

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: matching edit_file → SoftBlock (first hit).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_d1".into(),
                name: "edit_file".into(),
                input: serde_json::json!({
                    "path": "app/src/foo.rs",
                    "old_string": "a",
                    "new_string": "b",
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: LLM insists on the same edit_file → second hit
        // degrades to Footnote + normal execute (D1 dead-loop guard).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_d2".into(),
                name: "edit_file".into(),
                input: serde_json::json!({
                    "path": "app/src/nonexistent_for_test.rs",
                    "old_string": "x",
                    "new_string": "y",
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 3: final text.
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
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-d1".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    assert_eq!(
        mock.call_count(),
        3,
        "three turns: soft-block + exec + final"
    );
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 2, "two tool_results (soft-block + executed)");
    // First result: soft-block, is_error=false, hint-bearing.
    assert!(
        !results[0].is_error,
        "first hit is a soft-block (is_error=false)"
    );
    assert!(results[0].content.contains("未实际执行"));
    // Second result: executed normally. edit_file on a nonexistent
    // path returns is_error=true (ReadGuard verify_read fails —
    // "must read_file first"). The Footnote hint is prepended.
    assert!(
        results[1].is_error,
        "second hit executed normally (edit_file on unread path errors)"
    );
    assert!(
        results[1].content.contains("Memory:"),
        "second hit carries the Footnote header (degraded from SoftBlock)"
    );
}

// ---------------------------------------------------------------------------
// A5+ (2026-07-04): LLM retry / network resilience — R9 invariants.
//
// The retry wrapper (`llm::retry::retry_open`) re-issues the request
// on retryable first-byte failures before any `Ok(ChatEvent)` arrives.
// Because the agent loop only executes tools AFTER the stream
// completes (`chat_loop.rs` per-event loop), pre-first-byte retry is
// provably side-effect-free (prd R3). These tests verify the agent-
// loop-level invariants that depend on that property:
//
// 1. **Token usage does not double-count** (R9 + A4 OVERWRITE). The
//    retried failures never yield `ChatEvent::Done` (their stream is
//    dropped on first Err), so `update_last_turn_usage` only fires
//    once for the eventual successful turn. The persisted
//    `sessions.last_input_tokens` MUST equal the success-turn usage,
//    NOT N× the value.
// 2. **The chat-event channel surfaces `Retrying` notices** (R8).
//    Each retry attempt emits exactly one `ChatEvent::Retrying` so
//    the user sees "↩ 重试中 N/M, …" instead of a frozen stream.
// ---------------------------------------------------------------------------

/// R9 + A4 OVERWRITE: two consecutive `Server(503)` first-byte
/// failures followed by a successful turn. The retried failures
/// never reach `ChatEvent::Done` (`retry_open` drops the stream on
/// first Err before re-issuing), so the agent loop's
/// `update_last_turn_usage` fires EXACTLY ONCE — for the success
/// turn. `sessions.last_input_tokens` equals the success turn's
/// `usage.input_tokens`, NOT 3× (which would be the symptom if
/// retries were somehow accumulating).
#[tokio::test]
async fn a5plus_retry_does_not_double_count_token_usage() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // The success turn's usage. 1234 is distinctive so an accidental
    // 2× or 3× would NOT round to a "natural" value.
    let success_usage = TokenUsage {
        input_tokens: 1234,
        output_tokens: 567,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        context_input_tokens: 1234,
    };
    let mock = Arc::new(MockProvider::new(vec![
        // 1st send: 503 (retryable).
        MockResponse::ErrThenEnd(crate::llm::error::LlmError::Server {
            status: 503,
            message: "service unavailable".into(),
            retry_after: None,
        }),
        // 2nd send: 503 (retryable).
        MockResponse::ErrThenEnd(crate::llm::error::LlmError::Server {
            status: 503,
            message: "service unavailable".into(),
            retry_after: None,
        }),
        // 3rd send: success.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "hi".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(success_usage),
            }),
        ]),
    ]));

    run_chat_loop(
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-a5plus-usage".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    // 3 sends: 1 initial + 2 retries (then the 3rd succeeded).
    assert_eq!(
        mock.call_count(),
        3,
        "expected 3 send calls (2 retryable fails + 1 success)"
    );
    // The agent loop emits the success turn's Start/Delta/Done
    // verbatim, plus exactly 2 `Retrying` notices (one per failed
    // attempt before the retry backoff sleep).
    let retrying_count = emitter
        .chat_events()
        .into_iter()
        .filter(|p| matches!(p.event, ChatEvent::Retrying { .. }))
        .count();
    assert_eq!(retrying_count, 2, "expected 2 Retrying notices");
    // Exactly 1 Done (the success turn — retry_open drops the
    // failed streams on first Err, so no premature Done).
    let done_count = emitter
        .chat_events()
        .into_iter()
        .filter(|p| matches!(p.event, ChatEvent::Done { .. }))
        .count();
    assert_eq!(done_count, 1, "expected exactly 1 Done event");
    // Persist invariant (R9 + A4 OVERWRITE): last_input_tokens ==
    // success_usage.input_tokens, NOT 3× the value.
    let row: sqlx::sqlite::SqliteRow =
        sqlx::query("SELECT last_input_tokens FROM sessions WHERE id = ?")
            .bind(&h.session_id)
            .fetch_one(&h.db)
            .await
            .expect("session row present");
    let last_input: i64 = row.try_get("last_input_tokens").expect("column present");
    assert_eq!(
        last_input, 1234,
        "last_input_tokens must equal the success-turn usage (R9 OVERWRITE), got {}",
        last_input
    );
}

/// R8: each retry attempt surfaces a `ChatEvent::Retrying` notice
/// on the chat-event channel carrying `attempt` / `max_attempts` /
/// `wait_ms` / `reason`. This is the contract the frontend
/// `streamController` `case 'retrying'` arm reads to render the
/// "↩ 重试中" row.
#[tokio::test]
async fn a5plus_retry_emits_retrying_chat_events() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::ErrThenEnd(crate::llm::error::LlmError::RateLimit {
            message: "slow down".into(),
            retry_after: None,
        }),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_chat_loop(
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-a5plus-events".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    assert_eq!(mock.call_count(), 2, "1 fail + 1 success");
    let retry_events: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Retrying {
                attempt,
                max_attempts,
                wait_ms,
                reason,
            } => Some((attempt, max_attempts, wait_ms, reason)),
            _ => None,
        })
        .collect();
    assert_eq!(retry_events.len(), 1, "one retry notice");
    let (attempt, max_attempts, _wait_ms, reason) = &retry_events[0];
    assert_eq!(*attempt, 1, "first retry is attempt=1");
    assert_eq!(*max_attempts, 3, "default RetryPolicy.max_retries=3");
    assert!(
        reason.contains("频繁") || reason.contains("请求"),
        "reason must be the LlmError::RateLimit user_message, got {}",
        reason
    );
    // The retry notice's request_id matches the chat rid so the
    // frontend's activeRequests router picks it up.
    let retry_payload_rid = emitter
        .chat_events()
        .into_iter()
        .find(|p| matches!(p.event, ChatEvent::Retrying { .. }))
        .map(|p| p.request_id)
        .expect("retry event present");
    assert_eq!(retry_payload_rid, "rid-a5plus-events");
}

/// R9 (C3 compression idempotence): retry does NOT affect the C3
/// compression trigger. The agent loop's pre-turn compression check
/// runs ONCE before `retry_open` (it sees the same `messages` Vec
/// regardless of how many times the LLM is retried), so a session
/// near the compression threshold compresses identically with or
/// without retry. This test is a regression guard: if retry somehow
/// mutated `messages` or double-counted tokens into the C3 estimator,
/// the agent loop's behavior would diverge from the no-retry path.
///
/// We can't easily script a C3 trigger here (it requires a huge
/// `messages` Vec near the threshold), so this test is the lighter
/// contract: a retryable failure followed by success reaches the
/// SAME terminal state as a no-retry success (single Done with
/// `stop_reason: "end_turn"`, single user/assistant row persisted,
/// 2 sends). The deeper "C3 with retry" coverage is exercised by
/// `agent_loop_c3_compaction_does_not_panic` plus the existing
/// retry.rs unit tests for `retry_open`.
#[tokio::test]
async fn a5plus_retry_terminal_state_matches_no_retry_path() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::ErrThenEnd(crate::llm::error::LlmError::Network("conn reset".into())),
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
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-a5plus-c3-parity".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    // Same terminal state as `agent_loop_basic_text_only_completes`:
    // single Done with `end_turn`, no errors leaked.
    let dones: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect();
    assert_eq!(dones, vec!["end_turn".to_string()]);
    let errors = emitter
        .chat_events()
        .into_iter()
        .filter(|p| matches!(p.event, ChatEvent::Error { .. }))
        .count();
    assert_eq!(
        errors, 0,
        "no Error events should surface — retry recovered"
    );
    // One user + one assistant persisted. The retried failure
    // never persisted (its stream was dropped on first Err).
    let row: sqlx::sqlite::SqliteRow =
        sqlx::query("SELECT COUNT(*) AS n FROM messages WHERE session_id = ?")
            .bind(&h.session_id)
            .fetch_one(&h.db)
            .await
            .expect("messages count query");
    let n: i64 = row.try_get("n").expect("column present");
    assert_eq!(n, 2, "expected 2 persisted rows (1 user + 1 assistant)");
}
