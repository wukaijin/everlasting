//! Phase E2 (07-07-request-mode-change-tool) — `get_pending_interaction`
//! IPC handler behavior tests.
//!
//! These tests verify the `get_pending_interaction` Tauri command
//! (the unified `Option<PendingInteractionEntry>` IPC that replaces
//! legacy shims, since removed). The IPC handler is a
//! thin wrapper around `QuestionStore::get_payload`, so we exercise
//! that method directly — `tauri::test::mock_app` is not used in
//! this codebase (per the established `permission_response`
//! precedent; see `commands/permissions.rs::permission_response`'s
//! docstring for the rationale).
//!
//! ## Coverage
//!
//! 1. Register question → get → `Some({kind: "question",
//!    payload: ToolQuestionPayload})` — original `ask_user_question`
//!    round-trip.
//! 2. Register mode_change → get → `Some({kind: "mode_change",
//!    payload: ModeChangePayload})` — `request_mode_change`
//!    round-trip.
//! 3. Resolve → get → `None` — IPC returns "no pending" after the
//!    interaction has been resolved (session switch recovery).
//! 4. No session → get → `None` — IPC returns "no pending" for a
//!    session that was never registered (idempotent read path).

#![cfg(test)]

use crate::agent::question_store::{
    InteractionKind, InteractionResponse, ModeChangePayload, PendingInteraction, Question,
    QuestionOption, QuestionStore, ToolQuestionPayload,
};

/// Build a canonical `ToolQuestionPayload` for tests. Mirrors the
/// helper in `agent/question_store.rs::tests::make_payload`.
fn make_question_payload(session_id: &str, tool_use_id: &str) -> ToolQuestionPayload {
    ToolQuestionPayload {
        session_id: session_id.to_string(),
        tool_use_id: tool_use_id.to_string(),
        questions: vec![Question {
            question: "Pick one".into(),
            header: None,
            options: vec![
                QuestionOption {
                    label: "A".into(),
                    description: None,
                    preview: None,
                },
                QuestionOption {
                    label: "B".into(),
                    description: None,
                    preview: None,
                },
            ],
            multi_select: false,
            allow_custom: false,
        }],
        ts: 1_700_000_000_000,
    }
}

/// Build a canonical `ModeChangePayload` for tests. Mirrors the
/// helper in `agent/question_store.rs::tests::make_mode_change_payload`.
fn make_mode_change_payload(
    session_id: &str,
    tool_use_id: &str,
    target_mode: &str,
) -> ModeChangePayload {
    ModeChangePayload {
        session_id: session_id.to_string(),
        tool_use_id: tool_use_id.to_string(),
        target_mode: target_mode.to_string(),
        current_mode: Some("plan".to_string()),
        reason: Some("need to write code".to_string()),
        ts: 1_700_000_000_001,
    }
}

// ---------------------------------------------------------------------------
// Test 1 — Register question → get → Some({kind: "question", payload})
//
// Mirrors the original `ask_user_question` round-trip: the
// `QuestionStore::get_payload` returns a typed entry with
// `kind = Question` and the `payload` carries the original
// `ToolQuestionPayload` (session_id / tool_use_id / questions / ts).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn get_pending_interaction_register_question_returns_question_entry() {
    let store = QuestionStore::new();
    let payload = make_question_payload("s1", "tu_1");
    store
        .register("s1", "tu_1", PendingInteraction::Question(payload.clone()))
        .await
        .expect("register ok");

    // The IPC body: `state.question_store.get_payload(&session_id).await`.
    let entry = store
        .get_payload("s1")
        .await
        .expect("get_pending_interaction returns Some");

    assert_eq!(entry.kind, InteractionKind::Question);
    match entry.payload {
        PendingInteraction::Question(p) => {
            assert_eq!(p.session_id, "s1");
            assert_eq!(p.tool_use_id, "tu_1");
            assert_eq!(p.questions.len(), 1);
            assert_eq!(p.questions[0].question, "Pick one");
            assert_eq!(p.ts, 1_700_000_000_000);
        }
        _ => panic!("expected Question payload"),
    }
}

// ---------------------------------------------------------------------------
// Test 2 — Register mode_change → get → Some({kind: "mode_change",
// payload})
//
// The `request_mode_change` round-trip: `kind = ModeChange` and
// the payload carries target_mode / current_mode / reason / ts.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn get_pending_interaction_register_mode_change_returns_mode_change_entry() {
    let store = QuestionStore::new();
    let payload = make_mode_change_payload("s1", "tu_mc", "edit");
    store
        .register(
            "s1",
            "tu_mc",
            PendingInteraction::ModeChange(payload.clone()),
        )
        .await
        .expect("register ok");

    let entry = store
        .get_payload("s1")
        .await
        .expect("get_pending_interaction returns Some");

    assert_eq!(entry.kind, InteractionKind::ModeChange);
    match entry.payload {
        PendingInteraction::ModeChange(p) => {
            assert_eq!(p.session_id, "s1");
            assert_eq!(p.tool_use_id, "tu_mc");
            assert_eq!(p.target_mode, "edit");
            assert_eq!(p.current_mode.as_deref(), Some("plan"));
            assert_eq!(p.reason.as_deref(), Some("need to write code"));
            assert_eq!(p.ts, 1_700_000_000_001);
        }
        _ => panic!("expected ModeChange payload"),
    }
}

// ---------------------------------------------------------------------------
// Test 3 — Resolve → get → None
//
// Once the interaction has been resolved (the user allowed /
// denied / the agent loop timed out), `get_payload` returns
// `None`. The IPC handler maps this to `Ok(None)` so the
// frontend's session-switch recovery drops the stale card.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn get_pending_interaction_resolve_then_get_returns_none() {
    let store = QuestionStore::new();
    let payload = make_mode_change_payload("s1", "tu_mc", "edit");
    store
        .register("s1", "tu_mc", PendingInteraction::ModeChange(payload))
        .await
        .expect("register ok");

    // User clicked "允许" → resolve as Answered(true). This is
    // exactly what `resolve_mode_change_internal` does on the
    // success path (the question-store resolve unblocks the
    // agent loop's `tokio::select!` arm).
    let _ = store
        .resolve("s1", InteractionResponse::Answered(serde_json::json!(true)))
        .await
        .expect("resolve ok");

    // Post-resolve, the IPC returns `None` — the frontend's
    // `getPendingInteraction` rehydrate path renders nothing.
    assert!(store.get_payload("s1").await.is_none());
}

// ---------------------------------------------------------------------------
// Test 4 — No session → get → None
//
// A session_id that was never registered returns `None`. The IPC
// handler does NOT error on this — it's the "no pending
// interaction" happy path (the frontend treats `null` as
// "nothing to render").
// ---------------------------------------------------------------------------
#[tokio::test]
async fn get_pending_interaction_unknown_session_returns_none() {
    let store = QuestionStore::new();
    // No register — store is empty.
    assert!(
        store.get_payload("nonexistent-session").await.is_none(),
        "get_payload on unknown session returns None"
    );
}
