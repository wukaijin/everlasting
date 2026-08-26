#![cfg(test)]

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::messages_to_text;
use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

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
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-checklist".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
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
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-cl-coerce".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
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
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-cl-cancel".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        {
            let mut deps = chat_loop_deps(&h);
            deps.token = cancel_token;
            deps
        },
        parent_role(&h),
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
