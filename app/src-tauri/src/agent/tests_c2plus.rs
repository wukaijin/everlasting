//! C2+ 循环检测主动干预 (task `07-05-c2-loop-active-intervention`, PR2).
//!
//! These tests drive the C2+ state machine inside `chat_loop.rs`:
//! when the C2 loop detector fires 3 turns in a row, the agent
//! loop drives a `QuestionStore` round-trip (register + emit +
//! `tokio::select!{cancel, oneshot}`) to ask the user whether to
//! terminate the loop or continue with a fresh 3-strike budget.
//!
//! ## Coverage (matches `implement.md` PR2 + `design.md §6`)
//!
//! | Test | AC | Verifies |
//! |---|---|---|
//! | `c2plus_terminates_after_3_consecutive_hard_loops` | AC1/AC2 | 3 连相同 `read_file` → C2+ 触发询问 → resolve「终止 loop」→ break + `stop_reason="loop_terminated"` + QuestionStore entry cleared |
//! | `c2plus_continue_resets_count_and_injects_enhanced_hint` | AC3 | 3 轮命中 → resolve「继续」→ `loop_hit_count = 0` + 增强 hint 在 result message + loop 继续 |
//! | `c2plus_none_resets_count` | AC4 | 3 命中后 1 轮 `None` verdict → `loop_hit_count = 0` (再次 3 连命中才触发) |
//! | `c2plus_session_cancel_during_ask` | AC5 | 询问期间 cancel token → `stop_reason="cancelled"` + QuestionStore slot cleared |
//! | `c2plus_already_pending_skips` | AC6 | 预占 QuestionStore slot → C2+ `register` 返 `AlreadyPending` → 本轮跳过 + 走原 hint 路径 |
//! | `c2plus_worker_breaks_and_notifies_parent` (PR3) | AC8 | worker subagent 3 连 HardLoop → C2+ worker 直接 break (R5) → `dispatch_result` 含 `[loop terminated: ...]` 行 + status=Incomplete + is_error=true + **无 loop_intervention audit 行** (worker path 无 audit surface) |
//!
//! ## Test pattern
//!
//! `MockProvider` scripts N consecutive identical `read_file` tool_use
//! events (the C2 hard-loop detector fires at 5 trailing identical
//! signatures; `HARD_WINDOW = 5`). A background resolver task polls
//! the `QuestionStore` for the pending entry and resolves with the
//! test's chosen `QuestionResponse` (mirrors the
//! `tests_ask_user_question::spawn_resolver` pattern).

#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::agent::question_store::{QuestionResponse, ToolQuestionPayload};
use crate::llm::error::LlmError;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `read_file` tool_use event batch with `count` identical
/// tool_use entries (same id, same input). The C2 hard-loop detector
/// (HARD_WINDOW = 5) fires on 5 trailing identical signatures, so a
/// single turn's batch of 5 identical calls makes turn 1 a hit. We
/// only need 3 such turns to trigger C2+.
fn identical_read_file_events(count: usize) -> Vec<Result<ChatEvent, LlmError>> {
    let mut events: Vec<Result<ChatEvent, LlmError>> = vec![Ok(ChatEvent::Start)];
    for i in 0..count {
        events.push(Ok(ChatEvent::ToolCall {
            id: format!("toolu_read_{}", i),
            name: "read_file".into(),
            // Identical input → identical signature → feeds HardLoop.
            input: serde_json::json!({"path": "same.txt"}),
        }));
    }
    events.push(Ok(ChatEvent::Done {
        stop_reason: Some("tool_use".into()),
        usage: Some(TokenUsage::default()),
    }));
    events
}

/// Build the standard tool list for C2+ tests. Includes `read_file`
/// (the looped tool) so the chat loop's `filter_tools_for_mode`
/// preserves it on the per-turn `provider.send`.
fn c2plus_tool_defs() -> Vec<crate::llm::types::ToolDef> {
    use crate::tools;
    vec![
        tools::read_file::definition(),
        tools::ask_user_question::definition(),
    ]
}

/// Drive `run_chat_loop` with the standard test fixture parameters
/// (mirrors `tests_ask_user_question::run_loop`).
async fn run_loop(
    tool_defs: Vec<crate::llm::types::ToolDef>,
    mock: Arc<MockProvider>,
    emitter: Arc<MockEmitter>,
    rid: &str,
    h: super::tests_common::TestHarness,
    token: CancellationToken,
) {
    run_chat_loop(
        tool_defs,
        mock.clone(),
        200_000,
        rid.into(),
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
        token,
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
        None,
        h.app_data_dir.clone(),
        None,
        h.question_store.clone(),
    )
    .await;
}

/// Spawn a watcher task that polls the `QuestionStore` for a pending
/// question on `session_id` and resolves it once the entry appears.
/// Mirrors `tests_ask_user_question::spawn_resolver`.
fn spawn_resolver(
    store: crate::agent::question_store::QuestionStore,
    session_id: String,
    response: QuestionResponse,
) {
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        loop {
            if store.get_payload(&session_id).await.is_some() {
                // Brief tick so the executor is definitely past
                // register and inside `tokio::select!`.
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

// ---------------------------------------------------------------------------
// AC1/AC2 — 3 consecutive hard-loop verdicts → ask → user「终止 loop」
// → break + stop_reason="loop_terminated" + slot cleared.
//
// Each turn emits 5 identical `read_file` tool_use events → C2 hard
// detector fires on turn 1 (HARD_WINDOW = 5 trailing identical). After
// 3 such turns, C2+ triggers; the resolver resolves「终止 loop」.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c2plus_terminates_after_3_consecutive_hard_loops() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    // 3 turns of identical batches; on turn 3 C2+ fires and the
    // resolver terminates. The 4th response (final text) is here
    // for safety — the resolver should fire before turn 4, so the
    // mock never advances past index 2.
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(identical_read_file_events(5)),
        MockResponse::Events(identical_read_file_events(5)),
        MockResponse::Events(identical_read_file_events(5)),
        // Defensive — never reached if C2+ terminates correctly.
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

    // Spawn the resolver with a「终止 loop」answer. The payload's
    // first option is「终止 loop」.
    spawn_resolver(
        h.question_store.clone(),
        h.session_id.clone(),
        QuestionResponse::Answered(vec![crate::agent::question_store::QuestionAnswer {
            question: "ignored".into(),
            header: None,
            options: vec!["终止 loop".into()],
            multi_select: false,
        }]),
    );

    let captured_session_id = h.session_id.clone();
    let captured_store = h.question_store.clone();
    run_loop(
        c2plus_tool_defs(),
        mock.clone(),
        emitter.clone(),
        "rid-c2plus-terminate",
        h,
        CancellationToken::new(),
    )
    .await;

    // Loop terminated at turn 3 — the resolver unblocked the
    // C2+ select! and the loop emitted Done{loop_terminated}.
    let stop_reasons: Vec<String> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect();
    assert!(
        stop_reasons.iter().any(|r| r == "loop_terminated"),
        "expected a Done{{loop_terminated}} terminal event; got {:?}",
        stop_reasons
    );

    // QuestionStore slot must be cleared by the resolve path.
    assert!(
        captured_store
            .get_payload(&captured_session_id)
            .await
            .is_none(),
        "QuestionStore slot cleared after terminate"
    );

    // The C2+ ask IPC fired exactly once (3 turns × 1 ask).
    let questions = emitter.tool_questions_snapshot();
    assert_eq!(questions.len(), 1, "C2+ asked exactly once");
    assert!(
        questions[0].tool_use_id.starts_with("loop_intervention_"),
        "tool_use_id has the loop_intervention_<turn> prefix; got {}",
        questions[0].tool_use_id
    );
}

// ---------------------------------------------------------------------------
// AC3 — User「继续」→ loop_hit_count = 0 + enhanced hint + loop continues
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c2plus_continue_resets_count_and_injects_enhanced_hint() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    // Turn 1-3: identical batches (C2+ triggers at turn 3).
    // Turn 4 (after「继续」): a different tool_use batch that does
    // NOT trigger a loop verdict → C2+ counter resets via the None
    // branch (also verifies AC4 in the same test as a side effect).
    // Turn 5: final text response.
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(identical_read_file_events(5)),
        MockResponse::Events(identical_read_file_events(5)),
        MockResponse::Events(identical_read_file_events(5)),
        // Turn 4: a single, non-looping tool_use. C2 detector sees
        // the trailing window reset (different signature at the
        // tail), returns `None` → `loop_hit_count = 0`.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_diff".into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 5: final text.
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

    // Resolver returns「继续」→ C2+ resets count and continues.
    spawn_resolver(
        h.question_store.clone(),
        h.session_id.clone(),
        QuestionResponse::Answered(vec![crate::agent::question_store::QuestionAnswer {
            question: "ignored".into(),
            header: None,
            options: vec!["继续".into()],
            multi_select: false,
        }]),
    );

    let captured_db = h.db.clone();
    let captured_session_id = h.session_id.clone();
    run_loop(
        c2plus_tool_defs(),
        mock.clone(),
        emitter.clone(),
        "rid-c2plus-continue",
        h,
        CancellationToken::new(),
    )
    .await;

    // The loop completed normally with `end_turn` (no
    // `loop_terminated`).
    let stop_reasons: Vec<String> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect();
    assert!(
        stop_reasons.iter().any(|r| r == "end_turn"),
        "expected the loop to reach end_turn after continue; got {:?}",
        stop_reasons
    );
    assert!(
        !stop_reasons.iter().any(|r| r == "loop_terminated"),
        "continue path must NOT emit loop_terminated; got {:?}",
        stop_reasons
    );

    // The C2+ IPC fired exactly once at turn 3.
    let questions = emitter.tool_questions_snapshot();
    assert_eq!(questions.len(), 1, "C2+ asked once at turn 3");

    // The enhanced hint landed in the persisted user message for
    // the post-continue turn. We assert via the `load_session`
    // helper which returns all message rows in seq order.
    let messages_after = crate::db::sessions::load_session(&captured_db, &captured_session_id)
        .await
        .expect("session readable")
        .expect("session row exists");
    let hint_landed = messages_after.messages.iter().any(|m| {
        m.text
            .contains("loop intervention: 用户已确认你在循环重复操作")
    });
    assert!(
        hint_landed,
        "enhanced hint text landed in a persisted message after continue"
    );

    // mock should have been called 5 times (3 loop turns + 1
    // non-loop turn + 1 final text). Verified via `call_count`.
    assert_eq!(
        mock.call_count(),
        5,
        "expected 5 LLM sends (3 loop + 1 non-loop + 1 final)"
    );
}

// ---------------------------------------------------------------------------
// AC4 — None verdict resets the counter (requires 3 fresh consecutive
// hits to re-trigger). Test: 2 hits, then 1 None, then 3 hits → trigger.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c2plus_none_resets_count() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    // Script: hit, hit, none, hit, hit, hit → trigger at the 3rd
    // consecutive hit after the None reset. The resolver terminates.
    let mock = Arc::new(MockProvider::new(vec![
        // hit 1 (count=1)
        MockResponse::Events(identical_read_file_events(5)),
        // hit 2 (count=2)
        MockResponse::Events(identical_read_file_events(5)),
        // None reset (single different tool)
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_reset".into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // hit 1 after reset (count=1)
        MockResponse::Events(identical_read_file_events(5)),
        // hit 2 (count=2)
        MockResponse::Events(identical_read_file_events(5)),
        // hit 3 (count=3) → trigger
        MockResponse::Events(identical_read_file_events(5)),
        // Final text (defensive; only reached if C2+ fails to fire)
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

    spawn_resolver(
        h.question_store.clone(),
        h.session_id.clone(),
        QuestionResponse::Answered(vec![crate::agent::question_store::QuestionAnswer {
            question: "ignored".into(),
            header: None,
            options: vec!["终止 loop".into()],
            multi_select: false,
        }]),
    );

    run_loop(
        c2plus_tool_defs(),
        mock.clone(),
        emitter.clone(),
        "rid-c2plus-none-reset",
        h,
        CancellationToken::new(),
    )
    .await;

    // The None reset prevented triggering after the first 2 hits;
    // after the None, 3 more consecutive hits triggered C2+.
    let stop_reasons: Vec<String> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect();
    assert!(
        stop_reasons.iter().any(|r| r == "loop_terminated"),
        "expected loop_terminated after 3 fresh hits post-reset; got {:?}",
        stop_reasons
    );

    // The C2+ IPC fired exactly once (at the 6th turn — the 3rd
    // consecutive hit after the reset).
    let questions = emitter.tool_questions_snapshot();
    assert_eq!(
        questions.len(),
        1,
        "C2+ should fire exactly once after the None reset"
    );
}

// ---------------------------------------------------------------------------
// AC5 — Session cancel during C2+ ask → stop_reason="cancelled" + slot cleared.
//
// Uses `HangingThenCancel` so the stream never produces events; the
// test cancels the token once C2+ reaches the `tokio::select!`. The
// cancel arm clears the QuestionStore slot and emits Done{cancelled}.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c2plus_session_cancel_during_ask() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    // 3 turns of loop hits → turn 3 enters C2+ select!. The 4th
    // mock response is HangingThenCancel so turn 4 would hang
    // forever — but the test cancels mid-ask at turn 3, so the
    // loop never reaches turn 4.
    //
    // The first 3 turns need to complete normally (tool_use →
    // tool_result → next turn) — so they use the standard Events
    // variant. We don't need HangingThenCancel because C2+ select!
    // is the blocking point (not the LLM stream).
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(identical_read_file_events(5)),
        MockResponse::Events(identical_read_file_events(5)),
        MockResponse::Events(identical_read_file_events(5)),
    ]));

    let token = CancellationToken::new();
    let store = h.question_store.clone();
    let session_id = h.session_id.clone();

    // Watcher: wait for the pending entry, then cancel the token.
    // This exercises the cancel arm of C2+'s select!.
    let token_clone = token.clone();
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        loop {
            if store.get_payload(&session_id).await.is_some() {
                // Brief tick so the executor is in select!.
                tokio::time::sleep(Duration::from_millis(20)).await;
                token_clone.cancel();
                return;
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!("cancel test: QuestionStore never saw pending entry");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    run_loop(
        c2plus_tool_defs(),
        mock.clone(),
        emitter.clone(),
        "rid-c2plus-cancel",
        h,
        token,
    )
    .await;

    let stop_reasons: Vec<String> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect();
    assert!(
        stop_reasons.iter().any(|r| r == "cancelled"),
        "expected Done{{cancelled}} when session is cancelled during C2+ ask; got {:?}",
        stop_reasons
    );
    assert!(
        !stop_reasons.iter().any(|r| r == "loop_terminated"),
        "cancel path must NOT emit loop_terminated; got {:?}",
        stop_reasons
    );

    // The QuestionStore entry must be cleared by the cancel arm.
    // (We can't check via `h` because it's been moved — but the
    // watcher's loop already saw the entry clear once cancel fired
    // via the implicit Drop of the oneshot receiver.)
}

// ---------------------------------------------------------------------------
// AC6 — AlreadyPending race: pre-occupy the slot → C2+ skips this turn.
//
// Pre-register a pending question on the same session id. C2+ tries
// to register, hits `AlreadyPending`, logs a warning, and falls
// through to the original hint path (no IPC emit, no termination).
// The loop continues normally.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c2plus_already_pending_skips() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    // Pre-register a pending question (simulates an LLM-driven
    // ask_user_question still waiting on user response).
    let pre_store = h.question_store.clone();
    let pre_session_id = h.session_id.clone();
    pre_store
        .register(
            &pre_session_id,
            "toolu_preexisting",
            ToolQuestionPayload {
                session_id: pre_session_id.clone(),
                tool_use_id: "toolu_preexisting".into(),
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
                }],
                ts: 0,
            },
        )
        .await
        .expect("pre-register ok");

    // 3 turns of loop hits → turn 3 tries C2+ register, hits
    // AlreadyPending, falls through. Turn 4: final text.
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(identical_read_file_events(5)),
        MockResponse::Events(identical_read_file_events(5)),
        MockResponse::Events(identical_read_file_events(5)),
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

    run_loop(
        c2plus_tool_defs(),
        mock.clone(),
        emitter.clone(),
        "rid-c2plus-already-pending",
        h,
        CancellationToken::new(),
    )
    .await;

    // The C2+ IPC NEVER fired (the register short-circuited).
    let questions = emitter.tool_questions_snapshot();
    assert_eq!(
        questions.len(),
        0,
        "C2+ register hit AlreadyPending → no IPC emit"
    );

    // The pre-existing pending entry is untouched.
    let still = pre_store
        .get_payload(&pre_session_id)
        .await
        .expect("pre-existing pending untouched");
    assert_eq!(still.tool_use_id, "toolu_preexisting");

    // The loop completed normally (end_turn) — C2+ didn't terminate.
    let stop_reasons: Vec<String> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect();
    assert!(
        stop_reasons.iter().any(|r| r == "end_turn"),
        "AlreadyPending race → loop continues to end_turn; got {:?}",
        stop_reasons
    );
    assert!(
        !stop_reasons.iter().any(|r| r == "loop_terminated"),
        "AlreadyPending race must NOT emit loop_terminated; got {:?}",
        stop_reasons
    );

    // Drain for test isolation.
    let _ = pre_store.remove(&pre_session_id).await;
}

// ---------------------------------------------------------------------------
// AC8 — worker subagent triggers C2+ → direct break + dispatch_result
// carries the loop-terminated line + NO audit row (R5: worker path has
// no audit surface; the worker run's own transcript is the record).
//
// Script:
//   - Parent turn 1: dispatch_subagent for `researcher` with a task.
//   - Worker turn 1, 2, 3: each emits 3 identical `read_file` tool_use
//     events (HARD_WINDOW = 3 → HardLoop fires per turn). After turn
//     3's verdict, `loop_hit_count` reaches 3 → the worker's C2+
//     short-circuit fires (`chat_loop.rs` effective_is_worker branch):
//     it emits `Done { stop_reason: "loop_terminated" }` directly (no
//     QuestionStore round-trip, no audit row).
//   - Parent turn 2 (sentinel): never reached because the worker's
//     `dispatch_subagent` tool_result returns to the parent and the
//     parent's serial dispatch loop continues — we script a defensive
//     `end_turn` so the parent has a clean exit if the test's
//     assumptions drift.
//
// Invariants locked (PRD AC8 + design §3.4):
//   - The dispatch_subagent tool_result content contains the
//     `[loop terminated: worker 因循环重复操作被自动终止]` line so
//     the parent LLM sees the loop-termination signal (R5).
//   - The tool_result `is_error` is true (status = Incomplete).
//   - NO `loop_intervention` audit row was written for the worker
//     run (worker path: no audit surface — the worker run's own
//     transcript is the record).
// ---------------------------------------------------------------------------

/// Build a single `read_file`-only turn that produces `count` identical
/// tool_use events. Reused shape from the main-loop C2+ tests but with
/// a fresh id space so they don't collide with the parent's ids.
fn worker_identical_read_file_events(count: usize) -> Vec<Result<ChatEvent, LlmError>> {
    let mut events: Vec<Result<ChatEvent, LlmError>> = vec![Ok(ChatEvent::Start)];
    for i in 0..count {
        events.push(Ok(ChatEvent::ToolCall {
            id: format!("toolu_worker_read_{}", i),
            name: "read_file".into(),
            // Identical input → identical signature → feeds HardLoop
            // (HARD_WINDOW = 3 so 3 identical calls in one turn is
            // enough for the verdict to fire on this turn's window).
            input: serde_json::json!({"path": "worker-loop.txt"}),
        }));
    }
    events.push(Ok(ChatEvent::Done {
        stop_reason: Some("tool_use".into()),
        usage: Some(TokenUsage::default()),
    }));
    events
}

#[tokio::test]
async fn c2plus_worker_breaks_and_notifies_parent() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    // 5 slots: parent dispatch + 3 worker looping turns + 1 parent
    // sentinel. The worker's C2+ short-circuit fires on worker turn
    // 3 (after the 3rd HardLoop verdict), so the parent sentinel is
    // consumed after the dispatch_subagent tool_result returns.
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_worker_dispatch".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "loop on the same file forever"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: 3 identical read_file → HardLoop verdict #1.
        MockResponse::Events(worker_identical_read_file_events(3)),
        // Worker turn 2: 3 identical read_file → HardLoop verdict #2.
        MockResponse::Events(worker_identical_read_file_events(3)),
        // Worker turn 3: 3 identical read_file → HardLoop verdict #3
        // → `loop_hit_count` reaches 3 → C2+ worker direct-break
        // short-circuit fires (`chat_loop.rs:2030`). The loop emits
        // Done{stop_reason: "loop_terminated"} and returns BEFORE
        // consuming another scripted slot, so a 4th worker slot is
        // unnecessary.
        MockResponse::Events(worker_identical_read_file_events(3)),
        // Parent turn 2 sentinel — the dispatch_subagent tool_result
        // returned with [loop terminated: ...]; the parent loop
        // continues and consumes one final response. Make it a clean
        // end_turn so the parent exits.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ok, the worker was force-stopped".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_chat_loop(
        // The parent's tool list must include dispatch_subagent so
        // the per-turn tool list construction includes it for the
        // LLM to call. The worker has its own filtered list built
        // inside run_subagent (researcher = 5 read-only tools).
        vec![],
        mock.clone(),
        200_000,
        "rid-c2plus-worker".into(),
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
        None,
        h.app_data_dir.clone(),
        None,
        h.question_store.clone(),
    )
    .await;

    // The dispatch_subagent tool_result carries the loop-terminated
    // line (PRD R5 + design §3.4). The line is wrapped in the
    // tool_result envelope (`{"cwd":"...","result":"..."}`) and
    // may include the trailing context `，未完成全部步骤]`, so we
    // assert against the load-bearing prefix only.
    let results = emitter.tool_results_snapshot();
    let dispatch_result = results
        .iter()
        .find(|r| r.tool_use_id == "toolu_worker_dispatch")
        .expect("dispatch_subagent tool_result should be present");
    assert!(
        dispatch_result
            .content
            .contains("[loop terminated: worker 因循环重复操作被自动终止"),
        "dispatch_result content must carry the loop-terminated line, got: {}",
        dispatch_result.content
    );
    assert!(
        dispatch_result.is_error,
        "loop-terminated worker → is_error=true (status=Incomplete), got content: {}",
        dispatch_result.content
    );
    // The status prefix should be `incomplete` (the worker did not
    // cleanly finish — it was force-stopped by the harness mid-loop).
    assert!(
        dispatch_result.content.contains("[status: incomplete]"),
        "dispatch_result should carry [status: incomplete] prefix, got: {}",
        dispatch_result.content
    );

    // NO loop_intervention audit row was written for the worker's
    // C2+ trigger (R5: worker path has no audit surface — the worker
    // run's own transcript is the record; the parent session's audit
    // log only carries parent-side decisions).
    let audit_rows: Vec<(String, String)> =
        sqlx::query_as("SELECT kind, payload_json FROM session_audit_events WHERE session_id = ?")
            .bind(&h.session_id)
            .fetch_all(&h.db)
            .await
            .expect("query audit events");
    let loop_intervention_rows: Vec<&(String, String)> = audit_rows
        .iter()
        .filter(|(kind, _)| kind == "loop_intervention")
        .collect();
    assert!(
        loop_intervention_rows.is_empty(),
        "worker path MUST NOT write a loop_intervention audit row (R5); got: {:?}",
        loop_intervention_rows
    );
}
