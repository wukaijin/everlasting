#![cfg(test)]

//! Messages-domain integration tests (split from `db/tests.rs` on 2026-06-23).
//!
//! Coverage:
//! - D3 PR1: `edit_user_message` (cascade delete + metadata + audit
//!   + no-op fast path + atomic rollback)
//! - D3 PR3: `record_message_resend_audit` round-trip + FK safety

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::llm::types::{ContentBlock, MessageContent, Role};
use crate::projects::DEFAULT_PROJECT_ID;

use super::{
    migrations::run_migrations,
    permissions::list_audit_events,
    sessions::{create_session, delete_session, edit_user_message, load_session, persist_turn},
};

async fn test_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    // Mirror what `init_pool` does.
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    pool
}

async fn make_pool() -> SqlitePool {
    test_pool().await // alias for readability inside this section
}
// =====================================================================
// D3 PR1 (2026-06-17): edit_user_message tests
//
// Coverage:
// - cascade delete of subsequent messages (assistant turn +
// tool_result rows)
// - metadata edits: `edited_at` stamp + `original_content` backup
// - audit row insert with kind='edit_message' + JSON payload
// - no-op fast path when content is unchanged (no audit, no
// metadata bump)
// - idempotent on subsequent edits (original_content preserved,
// edited_at bumped)
// - foreign-key safe: edit + cascade delete run in one tx
// - atomic rollback: a synthetic INSERT failure inside the tx
// reverts the edit (no partial commit)
// - unknown (session_id, seq) pair is a silent no-op (defensive)
// =====================================================================

/// Helper: build a fresh session + persist user + assistant +
/// tool_result rows at seq 0 / 1 / 2. The assistant turn carries
/// a `tool_use` block; the user turn at seq 2 carries a
/// `tool_result` block that references it. Used as the canonical
/// setup for cascade-delete assertions.
async fn setup_session_with_3_turns(pool: &SqlitePool) -> String {
    let s = create_session(
        pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // seq 0: user prompt
    persist_turn(
        pool,
        &s.id,
        Role::User,
        &MessageContent::Text("old prompt text".to_string()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    // seq 1: assistant turn with tool_use
    let assistant_content = MessageContent::Blocks(vec![ContentBlock::ToolUse {
        id: "toolu_abc".to_string(),
        name: "read_file".to_string(),
        input: serde_json::json!({"path": "/etc/hostname"}),
    }]);
    persist_turn(
        pool,
        &s.id,
        Role::Assistant,
        &assistant_content,
        1,
        None,
        None,
    )
    .await
    .unwrap();
    // seq 2: user(tool_result) — the assistant's tool result.
    let tool_result_content = MessageContent::Blocks(vec![ContentBlock::ToolResult {
        tool_use_id: "toolu_abc".to_string(),
        content: "host1".to_string(),
        is_error: false,
    }]);
    persist_turn(pool, &s.id, Role::User, &tool_result_content, 2, None, None)
        .await
        .unwrap();
    s.id
}

#[tokio::test]
async fn edit_user_message_cascade_deletes_subsequent_messages() {
    // The cascade DELETE removes every strictly-later message in
    // the session — the assistant turn (seq 1) + the user(tool_result)
    // turn (seq 2). Only the edited user message (seq 0) survives.
    let pool = make_pool().await;
    let sid = setup_session_with_3_turns(&pool).await;

    let new_content = MessageContent::Text("new prompt text".to_string());
    edit_user_message(&pool, &sid, 0, &new_content)
        .await
        .unwrap();

    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 1, "only the edited row survives");
    assert_eq!(loaded.messages[0].seq, 0);
    assert_eq!(loaded.messages[0].role, "user");
    // The new text landed (denormalized `text` column).
    assert_eq!(loaded.messages[0].text, "new prompt text");
}

#[tokio::test]
async fn edit_user_message_writes_edited_at_metadata() {
    // First edit: stamps `edited_at` + `original_content` into the
    // metadata JSON. `edited_at` parses back as a non-null string
    // (RFC3339).
    let pool = make_pool().await;
    let sid = setup_session_with_3_turns(&pool).await;

    let new_content = MessageContent::Text("v2 text".to_string());
    edit_user_message(&pool, &sid, 0, &new_content)
        .await
        .unwrap();

    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    let md = loaded.messages[0]
        .metadata
        .as_ref()
        .expect("metadata should be set after first edit");
    let edited_at = md
        .get("edited_at")
        .and_then(|v| v.as_str())
        .expect("edited_at should be set");
    assert!(!edited_at.is_empty(), "edited_at is RFC3339 timestamp");
    // Spot-check format (contains "T" + timezone offset or "Z").
    assert!(edited_at.contains('T'), "edited_at should be RFC3339");
}

#[tokio::test]
async fn edit_user_message_preserves_original_content_on_first_edit() {
    // `original_content` is the JSON-serialized pre-edit value of
    // the row's `content` column. For a plain text message that
    // serializes as a JSON string, `original_content` should be
    // that string verbatim.
    let pool = make_pool().await;
    let sid = setup_session_with_3_turns(&pool).await;

    let new_content = MessageContent::Text("v2".to_string());
    edit_user_message(&pool, &sid, 0, &new_content)
        .await
        .unwrap();

    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    let md = loaded.messages[0].metadata.as_ref().unwrap();
    let original = md
        .get("original_content")
        .expect("original_content should be set on first edit");
    // Pre-edit content was "old prompt text" → JSON string form.
    assert_eq!(
        original.as_str(),
        Some("old prompt text"),
        "original_content should be the pre-edit value"
    );
}

#[tokio::test]
async fn edit_user_message_preserves_original_across_subsequent_edits() {
    // Re-editing an already-edited row must NOT overwrite
    // `original_content`. It always points at the pre-ANY-edit
    // value, so a future "undo edit" affordance has a stable
    // restore target.
    let pool = make_pool().await;
    let sid = setup_session_with_3_turns(&pool).await;

    // First edit: original prompt → "v2"
    edit_user_message(&pool, &sid, 0, &MessageContent::Text("v2".to_string()))
        .await
        .unwrap();
    // Re-insert a new assistant row so the second edit has
    // something to cascade-delete (the first edit wiped
    // seqs 1 + 2).
    persist_turn(
        &pool,
        &sid,
        Role::Assistant,
        &MessageContent::Text("assistant reply 2".to_string()),
        1,
        None,
        None,
    )
    .await
    .unwrap();
    // Second edit: "v2" → "v3"
    edit_user_message(&pool, &sid, 0, &MessageContent::Text("v3".to_string()))
        .await
        .unwrap();

    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    let md = loaded.messages[0].metadata.as_ref().unwrap();
    let original = md.get("original_content").unwrap();
    // Still points at "old prompt text" — NOT at "v2".
    assert_eq!(original.as_str(), Some("old prompt text"));
    // And the latest content reflects the second edit.
    assert_eq!(loaded.messages[0].text, "v3");
}

#[tokio::test]
async fn edit_user_message_records_audit_event() {
    // The single-transaction flow appends one row to
    // `session_audit_events` with kind='edit_message' and a JSON
    // payload carrying message_seq + new_text_preview + edited_at.
    let pool = make_pool().await;
    let sid = setup_session_with_3_turns(&pool).await;

    edit_user_message(
        &pool,
        &sid,
        0,
        &MessageContent::Text("edited prompt".to_string()),
    )
    .await
    .unwrap();

    // list_audit_events is session-scoped + sorted ts DESC.
    let events = list_audit_events(&pool, &sid).await.unwrap();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.kind, "edit_message");
    let payload = event
        .payload_json
        .as_ref()
        .expect("payload_json should be set");
    let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
    assert_eq!(parsed["message_seq"], serde_json::json!(0));
    assert_eq!(
        parsed["new_text_preview"],
        serde_json::json!("edited prompt")
    );
    assert!(parsed["edited_at"].is_string());
}

#[tokio::test]
async fn edit_user_message_is_noop_when_content_unchanged() {
    // Save-without-change: the JSON of `new_content` matches the
    // current row's `content` column verbatim. The function must
    // return Ok(()) without writing any state — no audit row, no
    // `edited_at` bump on the metadata.
    let pool = make_pool().await;
    let sid = setup_session_with_3_turns(&pool).await;

    let same_content = MessageContent::Text("old prompt text".to_string());
    edit_user_message(&pool, &sid, 0, &same_content)
        .await
        .unwrap();

    // Tail rows still present (no cascade delete).
    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 3, "no cascade on no-op");
    // Metadata is null (no edit happened).
    assert!(loaded.messages[0].metadata.is_none());
    // Audit log empty (no event on no-op).
    let events = list_audit_events(&pool, &sid).await.unwrap();
    assert_eq!(events.len(), 0, "no audit row on no-op");
}

#[tokio::test]
async fn edit_user_message_on_unknown_seq_is_silent_noop() {
    // Defensive: an edit request whose (session_id, seq) pair
    // resolves to nothing (e.g. cancel race wiping the row) is a
    // no-op, not an error. Mirrors the F5 latency IPC contract.
    let pool = make_pool().await;
    let sid = setup_session_with_3_turns(&pool).await;

    edit_user_message(
        &pool,
        &sid,
        999, // unknown seq
        &MessageContent::Text("garbage".to_string()),
    )
    .await
    .unwrap();

    // Original 3-turn history is intact.
    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 3);
}

#[tokio::test]
async fn edit_user_message_atomic_rollback_on_db_error() {
    // Synthetic failure path: we register a SQLite trigger that
    // throws on the second INSERT inside the transaction (the
    // audit row insert). The transaction must roll back: the
    // UPDATE on the user row + the cascade DELETE on the tail
    // must NOT commit. End state: the 3-turn history is intact
    // (no edit, no cascade), no audit row.
    //
    // SQLite raises "Abort due to constraint violation" from
    // RAISE(FAIL, ...) inside a trigger — this is the cleanest
    // way to force a deterministic mid-transaction failure
    // without filesystem-level fault injection.
    let pool = make_pool().await;
    let sid = setup_session_with_3_turns(&pool).await;

    // Trigger: when an INSERT into session_audit_events happens,
    // abort the transaction with a clear message. The first INSERT
    // (from the edit's audit row) will hit this trigger and fail.
    sqlx::query(
        r#"
 CREATE TRIGGER fail_edit_audit_insert
 BEFORE INSERT ON session_audit_events
 WHEN NEW.kind = 'edit_message'
 BEGIN
 SELECT RAISE(FAIL, 'synthetic: forced failure for rollback test');
 END
 "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let result = edit_user_message(
        &pool,
        &sid,
        0,
        &MessageContent::Text("this edit will fail".to_string()),
    )
    .await;
    assert!(
        result.is_err(),
        "edit_user_message must error when audit insert fails"
    );

    // State after the failed transaction:
    // - The user row at seq 0 still has the ORIGINAL content
    // - The assistant turn at seq 1 + the tool_result at seq 2
    // are still present (no cascade)
    // - No audit row was appended
    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 3, "all 3 turns intact on rollback");
    assert_eq!(loaded.messages[0].text, "old prompt text");
    let events = list_audit_events(&pool, &sid).await.unwrap();
    assert_eq!(
        events.len(),
        0,
        "no audit row should have committed before the trigger fired"
    );
}

/// D3 PR3 (2026-06-17): the resend audit helper writes a
/// `resend_message` row through `record_audit_event` with the
/// expected payload shape (`message_seq` + `content_text_preview`)
/// and round-trips through `list_audit_events`. Mirrors the
/// `tool_executed_audit_round_trips_via_list_audit_events` test
/// (C4 PR1, 2026-06-14) so the new variant is locked the same way:
/// the audit-log UI can dispatch on the kind string and parse the
/// payload without further coordination.
#[tokio::test]
async fn resend_message_audit_round_trips_via_list_audit_events() {
    use crate::agent::permissions::record_message_resend_audit;

    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Case 1: typical resend of a short user prompt.
    record_message_resend_audit(
        &pool,
        &s.id,
        3,
        "re-run: explain the cancellation token pattern",
        None,
    )
    .await
    .unwrap();

    let events = list_audit_events(&pool, &s.id).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "resend_message");
    let payload: serde_json::Value =
        serde_json::from_str(events[0].payload_json.as_deref().unwrap()).unwrap();
    assert_eq!(payload["message_seq"], 3);
    assert_eq!(
        payload["content_text_preview"],
        "re-run: explain the cancellation token pattern"
    );

    // Case 2: long content is truncated to 80 chars (matches the
    // edit audit preview budget).
    let long = "a".repeat(200);
    record_message_resend_audit(&pool, &s.id, 5, &long, None)
        .await
        .unwrap();
    let events = list_audit_events(&pool, &s.id).await.unwrap();
    assert_eq!(events.len(), 2);
    let target = "5";
    let second = events
        .iter()
        .find(|e| e.kind == "resend_message" && e.payload_json.as_deref().unwrap().contains(target))
        .unwrap();
    let payload: serde_json::Value =
        serde_json::from_str(second.payload_json.as_deref().unwrap()).unwrap();
    let preview = payload["content_text_preview"].as_str().unwrap();
    assert_eq!(preview.chars().count(), 80, "preview truncated to 80 chars");
    assert!(
        preview.chars().all(|c| c == 'a'),
        "preview is the leading 80 a chars"
    );
}

/// D3 PR3 (2026-06-17): the resend audit helper returns
/// `Result<(), sqlx::Error>` like the other audit helpers; a DB
/// write failure surfaces to the caller (the agent loop
/// log-and-swallow path at the user-message persist site).
/// Asserted by inserting into a deleted session (FK constraint
/// fires; helper returns `Err`).
#[tokio::test]
async fn resend_message_audit_on_deleted_session_returns_error() {
    use crate::agent::permissions::record_message_resend_audit;

    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    delete_session(&pool, &s.id).await.unwrap();
    let result = record_message_resend_audit(&pool, &s.id, 0, "after-delete", None).await;
    assert!(result.is_err(), "audit insert must fail on missing session");
}

// =====================================================================
// C2+ (2026-07-05): record_loop_intervention_audit round-trip
//
// Mirrors `resend_message_audit_round_trips_via_list_audit_events`
// shape — the audit row written by `record_loop_intervention_audit`
// must round-trip through `list_audit_events` with the documented
// payload (`hit_count` / `verdict_kind` / `action` / `run_id`) so
// the C4 audit-log UI can dispatch on the kind string without
// further coordination.
// =====================================================================

/// C2+ (2026-07-05): the `loop_intervention` row written by the
/// `record_loop_intervention_audit` helper round-trips through
/// `list_audit_events`. Covers the three `action` variants
/// (`asked` / `terminated` / `continued`) + both `verdict_kind`
/// variants (`hard` / `soft`) + the `run_id: Option` shape
/// (main-loop path passes `None`; worker path passes `Some(id)`).
#[tokio::test]
async fn loop_intervention_audit_round_trips_via_list_audit_events() {
    use crate::agent::permissions::record_loop_intervention_audit;

    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Case 1: main-loop path, action="asked", verdict_kind="hard".
    record_loop_intervention_audit(&pool, &s.id, None, 3, "hard", "asked", None)
        .await
        .unwrap();

    // Case 2: main-loop path, action="terminated", verdict_kind="soft".
    record_loop_intervention_audit(&pool, &s.id, None, 5, "soft", "terminated", None)
        .await
        .unwrap();

    // Case 3: worker path, action="continued", run_id=Some. PR1
    // ships before the worker path actually fires this helper
    // (worker C2+ goes through transcript, not the audit log —
    // PRD R5 / design §3.2), but the helper signature must accept
    // `Some(run_id)` so the future worker surface can opt in
    // without another signature change.
    record_loop_intervention_audit(
        &pool,
        &s.id,
        Some("worker-run-uuid-1234"),
        3,
        "hard",
        "continued",
        None,
    )
    .await
    .unwrap();

    let events = list_audit_events(&pool, &s.id).await.unwrap();
    assert_eq!(events.len(), 3);
    for ev in &events {
        assert_eq!(ev.kind, "loop_intervention");
    }

    // Case 1 payload.
    let p1: serde_json::Value =
        serde_json::from_str(events[0].payload_json.as_deref().unwrap()).unwrap();
    assert_eq!(p1["hit_count"], 3);
    assert_eq!(p1["verdict_kind"], "hard");
    assert_eq!(p1["action"], "asked");
    assert!(
        p1["run_id"].is_null(),
        "main-loop run_id is None → JSON null"
    );

    // Case 2 payload.
    let p2: serde_json::Value =
        serde_json::from_str(events[1].payload_json.as_deref().unwrap()).unwrap();
    assert_eq!(p2["hit_count"], 5);
    assert_eq!(p2["verdict_kind"], "soft");
    assert_eq!(p2["action"], "terminated");
    assert!(p2["run_id"].is_null());

    // Case 3 payload (worker run_id surfaces verbatim).
    let p3: serde_json::Value =
        serde_json::from_str(events[2].payload_json.as_deref().unwrap()).unwrap();
    assert_eq!(p3["hit_count"], 3);
    assert_eq!(p3["verdict_kind"], "hard");
    assert_eq!(p3["action"], "continued");
    assert_eq!(p3["run_id"], "worker-run-uuid-1234");
}

/// C2+ (2026-07-05): the loop-intervention audit helper returns
/// `Result<(), sqlx::Error>` like the other audit helpers; a DB
/// write failure surfaces to the caller (the agent loop
/// log-and-swallow path). Asserted by inserting into a deleted
/// session (FK constraint fires; helper returns `Err`). Mirrors
/// `resend_message_audit_on_deleted_session_returns_error`.
#[tokio::test]
async fn loop_intervention_audit_on_deleted_session_returns_error() {
    use crate::agent::permissions::record_loop_intervention_audit;

    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    delete_session(&pool, &s.id).await.unwrap();
    let result = record_loop_intervention_audit(&pool, &s.id, None, 3, "hard", "asked", None).await;
    assert!(result.is_err(), "audit insert must fail on missing session");
}

// ---------------------------------------------------------------------
// Group chat (07-29-group-chat, Phase 4 Step 1 TODO-A/B): speaker
// round-trip — `persist_turn` writes the optional `speaker`
// column, `load_session` reads it back as `MessageRow.speaker`.
// Phase 1 added the column + write path; Phase 4 closed the
// read loop + extended `MessageRow`. Two tests:
//   1. With a speaker — round-trip preserves the speaker string.
//   2. Without a speaker (`None`) — round-trip yields `None`,
//      i.e. classic chat sessions stay unaffected (zero
//      regression on the pre-Phase 4 default).
// ---------------------------------------------------------------------

#[tokio::test]
async fn persist_turn_writes_speaker_and_load_session_round_trips() {
    let pool = make_pool().await;
    let sid = Uuid::new_v4().to_string();
    let s = create_session(
        &pool,
        &sid,
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // seq 0: user prompt (no speaker — user's turns are human by
    // definition in any session type, including group chat).
    persist_turn(
        &pool,
        &s.id,
        Role::User,
        &MessageContent::Text("prompt".to_string()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    // seq 1: moderator turn — speaker = "moderator".
    persist_turn(
        &pool,
        &s.id,
        Role::Assistant,
        &MessageContent::Text("Welcome — opening the discussion.".to_string()),
        1,
        None,
        Some("moderator"),
    )
    .await
    .unwrap();
    // seq 2: participant turn — speaker = the participant's name.
    persist_turn(
        &pool,
        &s.id,
        Role::Assistant,
        &MessageContent::Text("I disagree.".to_string()),
        2,
        None,
        Some("Alex"),
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 3);
    assert_eq!(loaded.messages[0].speaker, None, "user row has no speaker");
    assert_eq!(
        loaded.messages[1].speaker.as_deref(),
        Some("moderator"),
        "moderator turn speaker round-tripped"
    );
    assert_eq!(
        loaded.messages[2].speaker.as_deref(),
        Some("Alex"),
        "participant turn speaker round-tripped"
    );
}

#[tokio::test]
async fn persist_turn_without_speaker_round_trips_as_none() {
    // Regression test for the pre-Phase 4 default: classic chat
    // sessions persist turns with `speaker = None`, and the read
    // path returns `None` (NOT a default string like "user" or
    // "assistant"). The frontend's `v-if="message.speaker"` chip
    // condition relies on this exact null semantics.
    let pool = make_pool().await;
    let sid = Uuid::new_v4().to_string();
    let s = create_session(
        &pool,
        &sid,
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    persist_turn(
        &pool,
        &s.id,
        Role::Assistant,
        &MessageContent::Text("classic chat reply".to_string()),
        0,
        None,
        None,
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(
        loaded.messages[0].speaker, None,
        "classic chat speaker must be None (no behavior change vs pre-Phase 4)"
    );
}
