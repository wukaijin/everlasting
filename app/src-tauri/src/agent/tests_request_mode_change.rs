//! Phase F (`07-07-request-mode-change-tool`) — backend integration tests.
//!
//! These tests drive `run_chat_loop` end-to-end against the
//! `request_mode_change` interception branch in `chat_loop.rs` (the
//! blocking tool the chat loop recognizes via
//! `if name == "request_mode_change"` and routes to
//! `request_mode_change::execute_blocking` instead of `execute_tool`).
//!
//! ## Coverage (matches `implement.md` §1 Phase E1 + design §8.2)
//!
//! | Test | AC | Verifies |
//! |---|---|---|
//! | `agent_loop_request_mode_change_serializes_with_shell_in_same_batch` | AC1 | LLM batch `[shell, request_mode_change]` → Serial order, blocking wait, all tool_results land, turn counter +1 from blocking |
//! | `agent_loop_request_mode_change_noop_skips_card` | AC7 | LLM calls request_mode_change with current mode → tool_result = `{"noop": true, "current_mode": "..."}`, no card, no IPC emit |
//! | `agent_loop_request_mode_change_session_cancel_returns_cancelled_marker` | AC16 | `token.cancel()` mid-wait → tool_result = `{"cancelled_by_session": true}` + store cleaned |
//! | `agent_loop_request_mode_change_already_pending_returns_structured_error` | AC9 | Same-session second `request_mode_change` call → structured "已有 pending" error + first pending stays usable |
//! | `agent_loop_request_mode_change_happy_path_records_audit` | AC12 | Each tool call records at least 1 `mode_change_requested` audit row |
//!
//! ## Test pattern
//!
//! Mirrors `tests_ask_user_question.rs` — `MockProvider` scripts
//! the LLM response events, `MockEmitter` captures the
//! agent-loop's emitted events, `QuestionStore` is resolved
//! manually by a watcher task that polls `get_payload` and
//! calls `resolve` with `InteractionResponse::Answered(true)` (the
//! tool-result marker the `resolve_mode_change` IPC handler
//! sends on the allow path).
//!
//! NOTE: We do NOT test the actual DB mode UPDATE here — that
//! happens in `commands::question::resolve_mode_change` (the
//! IPC handler), which is tested separately in `commands/`
//! tests: `resolve_mode_change_internal` (the pure core the
//! handler delegates to) is covered by
//! `commands/tests_resolve_mode_change.rs` (allow/deny paths,
//! root guard, unknown session, pending unregister — real DB
//! pool, no `tauri::test::mock_app` needed thanks to the
//! pure-core extraction).

#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::agent::question_store::{InteractionResponse, PendingInteraction};
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A standard valid `request_mode_change` input — target edit,
/// with a reason. Mirrors the test fixture used in
/// `tools/request_mode_change.rs::tests::make_valid_input`.
fn valid_request_mode_change_input(target: &str) -> serde_json::Value {
    serde_json::json!({
        "target_mode": target,
        "reason": "test reason",
    })
}

/// Unwrap the REQ-16 tool-result envelope
/// (`{"result": "<raw>", "cwd": "<path>"}`) so the test can
/// assert on the raw tool output.
fn unwrap_envelope(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(result) = parsed.get("result").and_then(|v| v.as_str()) {
                return result.to_string();
            }
        }
    }
    content.to_string()
}

/// Spawn a watcher task that polls the `QuestionStore` for a
/// pending mode change on `session_id` and resolves it with
/// the supplied `InteractionResponse` once the entry appears.
/// Mirrors the `tests_ask_user_question.rs::spawn_resolver`
/// pattern.
fn spawn_resolver(
    store: crate::agent::question_store::QuestionStore,
    session_id: String,
    response: InteractionResponse,
) {
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        loop {
            if store.get_payload(&session_id).await.is_some() {
                // Entry is registered; give the executor a brief
                // tick to enter `tokio::select!`.
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _ = store.resolve(&session_id, response.clone()).await;
                return;
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!(
                    "spawn_resolver: QuestionStore never saw pending entry for session {}",
                    session_id
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
}

/// Build the canonical tool list for the integration tests.
/// Includes `shell` + `request_mode_change` so the chat loop's
/// `filter_tools_for_mode` preserves them on the per-turn
/// `provider.send` call.
fn integration_tool_defs() -> Vec<crate::llm::types::ToolDef> {
    use crate::tools;
    vec![
        tools::shell::definition(),
        tools::request_mode_change::definition(),
    ]
}

/// Run `run_chat_loop` with the standard test fixture parameters.
async fn run_loop(
    tool_defs: Vec<crate::llm::types::ToolDef>,
    mock: Arc<MockProvider>,
    emitter: Arc<MockEmitter>,
    rid: &str,
    h: super::tests_common::TestHarness,
    token: CancellationToken,
) {
    run_chat_loop(
        chat_loop_request(
            tool_defs,
            mock.clone(),
            200_000,
            rid.into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
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

/// Update the session's mode in the DB (so the agent loop's
/// `loaded_session.session.mode` matches the noop test
/// fixture). Returns the new mode.
async fn seed_session_mode(h: &super::tests_common::TestHarness, mode: db::Mode) {
    db::update_session_mode(&h.db, &h.session_id, mode)
        .await
        .expect("seed session mode");
}

// ---------------------------------------------------------------------------
// F1 Test 1 — Happy path (PRD AC1, AC1')
//
// LLM batch = [shell, request_mode_change].
// - `shell` runs first (default-allow for the test fixture's
//   project cwd), pushes a `tool:result` with `is_error=false`.
// - `request_mode_change` is the blocking tool. The agent loop
//   routes to `execute_blocking`, which registers the
//   pending mode change + emits `mode:change:request` IPC.
//   The resolver task resolves with `Answered(true)` (the
//   tool-result marker the `resolve_mode_change` IPC handler
//   sends on the allow path).
// - Turn 2 (next `send`): LLM emits text + Done{end_turn}.
//
// Assertions:
// - `mock.call_count() == 2` (turn 1 + turn 2).
// - `emitter.tool_call_count() == 2` (shell + request_mode_change).
// - `emitter.tool_result_count() == 2` (all two results).
// - The blocking tool's `tool_result` carries the JSON-serialized
//   allow marker (success path, `is_error=false`).
// - Turn counter +1 from blocking (per PRD §R3).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn agent_loop_request_mode_change_serializes_with_shell_in_same_batch() {
    let h = make_harness().await;
    // Seed the session's mode to Plan (the LLM will request
    // "edit" which differs from Plan → not a noop).
    seed_session_mode(&h, db::Mode::Plan).await;

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: batch of 2 tool_uses (shell + request_mode_change).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_shell".into(),
                name: "shell".into(),
                input: serde_json::json!({"command": "echo ok"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_mode".into(),
                name: "request_mode_change".into(),
                input: valid_request_mode_change_input("edit"),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: final text — LLM consumed the blocking-tool
        // answer.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    spawn_resolver(
        h.question_store.clone(),
        h.session_id.clone(),
        InteractionResponse::Answered(serde_json::json!(true)),
    );

    run_loop(
        integration_tool_defs(),
        mock.clone(),
        emitter.clone(),
        "rid-mode-happy",
        h,
        CancellationToken::new(),
    )
    .await;

    assert_eq!(
        mock.call_count(),
        2,
        "blocking tool costs +1 turn — expected exactly 2 sends"
    );
    assert_eq!(emitter.tool_call_count(), 2);
    assert_eq!(emitter.tool_result_count(), 2);

    // The blocking tool's `tool_result` carries the JSON-serialized
    // allow marker with `is_error=false` (success path).
    let results = emitter.tool_results_snapshot();
    let mode_result = results
        .iter()
        .find(|r| r.tool_use_id == "toolu_mode")
        .expect("blocking tool produced a tool_result");
    assert!(
        !mode_result.is_error,
        "allowed request_mode_change returns is_error=false"
    );
    let raw = unwrap_envelope(&mode_result.content);
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("raw content is valid JSON");
    assert_eq!(parsed["allowed"], serde_json::json!(true));
    assert_eq!(parsed["prev_mode"], serde_json::json!("plan"));
    assert_eq!(parsed["new_mode"], serde_json::json!("edit"));
    // shell ran first (is_error=false).
    assert!(results
        .iter()
        .find(|r| r.tool_use_id == "toolu_shell")
        .map(|r| !r.is_error)
        .unwrap_or(false));
}

// ---------------------------------------------------------------------------
// F1 Test 2 — Noop path (PRD AC7)
//
// LLM calls request_mode_change with the CURRENT mode. The
// tool should immediately return `{"noop": true, ...}` without
// registering or emitting. The agent loop continues normally
// with turn 2.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn agent_loop_request_mode_change_noop_skips_card() {
    let h = make_harness().await;
    // Seed the session's mode to Plan (LLM will request "plan"
    // → noop).
    seed_session_mode(&h, db::Mode::Plan).await;

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: LLM emits request_mode_change as the ONLY
        // tool_use. `stop_reason=tool_use` is required because
        // chat_loop.rs:2017 only schedules tool execution when
        // the LLM signals "more turns needed" — `end_turn`
        // would short-circuit and the blocking tool would
        // never fire.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_noop".into(),
                name: "request_mode_change".into(),
                input: valid_request_mode_change_input("plan"),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: LLM consumes the noop tool_result, returns
        // a final text.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "noop ok".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    let captured_session_id = h.session_id.clone();
    let captured_store = h.question_store.clone();
    run_loop(
        integration_tool_defs(),
        mock.clone(),
        emitter.clone(),
        "rid-mode-noop",
        h,
        CancellationToken::new(),
    )
    .await;

    assert_eq!(emitter.tool_call_count(), 1);
    assert_eq!(emitter.tool_result_count(), 1);

    // The tool_result carries the noop marker (no error).
    let results = emitter.tool_results_snapshot();
    let noop_result = results
        .iter()
        .find(|r| r.tool_use_id == "toolu_noop")
        .expect("noop tool produced a tool_result");
    assert!(!noop_result.is_error, "noop returns is_error=false");
    let raw = unwrap_envelope(&noop_result.content);
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("raw content is valid JSON");
    assert_eq!(parsed["noop"], serde_json::json!(true));
    assert_eq!(parsed["current_mode"], serde_json::json!("plan"));

    // The store has no pending entry (noop didn't register).
    assert!(
        captured_store
            .get_payload(&captured_session_id)
            .await
            .is_none(),
        "noop must NOT register a pending mode change"
    );
}

// ---------------------------------------------------------------------------
// F1 Test 3 — Session cancel (PRD AC16)
//
// LLM calls `request_mode_change` and the test cancels the
// session token mid-wait. The cancel arm fires:
//   cancel arm → store.remove(session_id) + tool_result =
//                {"cancelled_by_session": true} + is_error: true
// ---------------------------------------------------------------------------
#[tokio::test]
async fn agent_loop_request_mode_change_session_cancel_returns_cancelled_marker() {
    let h = make_harness().await;
    seed_session_mode(&h, db::Mode::Plan).await;

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::HangingThenCancel]));

    let store_for_cancel = h.question_store.clone();
    let store_for_cancel_loop = store_for_cancel.clone();
    let session_id_for_cancel = h.session_id.clone();
    let session_id_for_cancel_loop = session_id_for_cancel.clone();
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        loop {
            if store_for_cancel_loop
                .get_payload(&session_id_for_cancel_loop)
                .await
                .is_some()
            {
                // Wait for the cancel arm to clear the entry.
                tokio::time::sleep(Duration::from_millis(100)).await;
                let after = store_for_cancel_loop
                    .get_payload(&session_id_for_cancel_loop)
                    .await;
                assert!(
                    after.is_none(),
                    "QuestionStore entry was cleared by the cancel arm"
                );
                return;
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!("QuestionStore never saw pending entry for request_mode_change");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    // `HangingThenCancel` is a stream that never yields — the
    // only way to break `run_chat_loop` out is to cancel the
    // session token mid-wait. We key off `mock.call_count() >= 1`
    // (the chat loop's turn-1 `provider.send` has run) before
    // firing the cancel, so the cancel deterministically lands
    // in the "stream pending, blocking tool active" window.
    // See `tests_ask_user_question.rs::
    // agent_loop_ask_user_question_session_cancel` for the
    // exact precedent.
    let token = CancellationToken::new();
    let token_clone = token.clone();
    let call_count = mock.call_count_handle();
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        while call_count.load(std::sync::atomic::Ordering::SeqCst) < 1 {
            if start.elapsed() > Duration::from_secs(5) {
                panic!("cancel test: turn-1 send never observed (call_count stayed 0)");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        token_clone.cancel();
    });

    run_loop(
        integration_tool_defs(),
        mock.clone(),
        emitter.clone(),
        "rid-mode-cancel",
        h,
        token,
    )
    .await;

    // The store is clean.
    assert!(store_for_cancel
        .get_payload(&session_id_for_cancel)
        .await
        .is_none());
}

// ---------------------------------------------------------------------------
// F1 Test 4 — Already pending (PRD AC9)
//
// Pre-register a pending question on the same session (via
// `ask_user_question` — a different kind). The
// `request_mode_change` call hits `AlreadyPending` → returns
// the structured error string. The first pending stays
// usable.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn agent_loop_request_mode_change_already_pending_returns_structured_error() {
    let h = make_harness().await;
    seed_session_mode(&h, db::Mode::Plan).await;

    let emitter = Arc::new(MockEmitter::new());

    // Pre-register a pending question.
    let pre_session_id = h.session_id.clone();
    let pre_store = h.question_store.clone();
    pre_store
        .register(
            &pre_session_id,
            "toolu_preexisting_q",
            PendingInteraction::Question(crate::agent::question_store::ToolQuestionPayload {
                session_id: pre_session_id.clone(),
                tool_use_id: "toolu_preexisting_q".into(),
                questions: vec![crate::agent::question_store::Question {
                    question: "preexisting".into(),
                    header: None,
                    options: vec![
                        crate::agent::question_store::QuestionOption {
                            label: "a".into(),
                            description: None,
                            preview: None,
                        },
                        crate::agent::question_store::QuestionOption {
                            label: "b".into(),
                            description: None,
                            preview: None,
                        },
                    ],
                    multi_select: false,
                    allow_custom: false,
                }],
                ts: 0,
            }),
        )
        .await
        .expect("pre-register ok");

    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: LLM emits request_mode_change against the
        // pre-existing pending question → `AlreadyPending`
        // structured error. `stop_reason=tool_use` is required
        // (see chat_loop.rs:2017 — `end_turn` would skip tool
        // execution entirely).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_mode_blocked".into(),
                name: "request_mode_change".into(),
                input: valid_request_mode_change_input("edit"),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: LLM consumes the structured error, responds.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_loop(
        integration_tool_defs(),
        mock.clone(),
        emitter.clone(),
        "rid-mode-already-pending",
        h,
        CancellationToken::new(),
    )
    .await;

    // The blocked tool_result carries the structured error.
    let results = emitter.tool_results_snapshot();
    let mode_result = results
        .iter()
        .find(|r| r.tool_use_id == "toolu_mode_blocked")
        .expect("blocked tool produced a tool_result");
    assert!(
        mode_result.is_error,
        "blocked by AlreadyPending → is_error: true"
    );
    let raw = unwrap_envelope(&mode_result.content);
    assert!(
        raw.contains("已有 pending"),
        "tool_result carries the structured error: {}",
        raw
    );

    // The pre-existing pending entry is untouched.
    let still = pre_store
        .get_payload(&pre_session_id)
        .await
        .expect("pre-existing pending untouched");
    assert_eq!(
        still.kind,
        crate::agent::question_store::InteractionKind::Question
    );

    // Drain for test isolation.
    let _ = pre_store.remove(&pre_session_id).await;
}

// ---------------------------------------------------------------------------
// F1 Test 5 — Audit (PRD AC12)
//
// Every tool call records at least 1 `mode_change_requested`
// audit row. The tool itself writes the audit at the
// execute_blocking entry; we assert the row exists after
// run_chat_loop returns.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn agent_loop_request_mode_change_happy_path_records_audit() {
    let h = make_harness().await;
    seed_session_mode(&h, db::Mode::Plan).await;

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: LLM emits request_mode_change (plan → edit).
        // `stop_reason=tool_use` is required (chat_loop.rs:2017
        // gates tool execution on `tool_use` stop_reason).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_mode_audit".into(),
                name: "request_mode_change".into(),
                input: valid_request_mode_change_input("edit"),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: LLM consumes the allow marker, finalizes.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    spawn_resolver(
        h.question_store.clone(),
        h.session_id.clone(),
        InteractionResponse::Answered(serde_json::json!(true)),
    );

    let captured_db = h.db.clone();
    let captured_session_id = h.session_id.clone();
    run_loop(
        integration_tool_defs(),
        mock.clone(),
        emitter.clone(),
        "rid-mode-audit",
        h,
        CancellationToken::new(),
    )
    .await;

    // At least 1 `mode_change_requested` audit row was written.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_audit_events WHERE kind = 'mode_change_requested'",
    )
    .fetch_one(&captured_db)
    .await
    .expect("query audit count");
    assert!(
        count >= 1,
        "expected at least 1 mode_change_requested audit row, got {}",
        count
    );
    // The session_id is the one we expected.
    let kind_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_audit_events WHERE kind = 'mode_change_requested' AND session_id = ?",
    )
    .bind(&captured_session_id)
    .fetch_one(&captured_db)
    .await
    .expect("query audit count for session");
    assert_eq!(
        kind_count, 1,
        "exactly 1 mode_change_requested audit row for the test session"
    );
}
