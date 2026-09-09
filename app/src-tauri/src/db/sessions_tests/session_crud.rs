#![cfg(test)]

use uuid::Uuid;

use crate::llm::types::{ContentBlock, MessageContent, Role};
use crate::projects::DEFAULT_PROJECT_ID;

use super::projects::create_project;
use super::sessions::{
    clear_group_chat_lifecycle, create_session, delete_messages_by_session, delete_session,
    finalize_group_chat_lifecycle, get_group_chat_checkpoint, list_sessions, load_session,
    persist_turn, recover_group_chat_checkpoints, upsert_group_chat_checkpoint,
};
use super::test_pool;

#[tokio::test]
async fn create_session_scopes_to_project() {
    let pool = test_pool().await;
    let p = create_project(
        &pool,
        "p",
        "/tmp/everlasting_test_session_proj",
        false,
        None,
    )
    .await
    .unwrap();

    let s1 = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &p.id,
        "/tmp/foo",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let s2 = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &p.id,
        "/tmp/bar",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(s1.project_id, p.id);
    assert_eq!(s1.current_cwd, "/tmp/foo");
    assert_eq!(s2.current_cwd, "/tmp/bar");

    let list = list_sessions(&pool, &p.id).await.unwrap();
    assert_eq!(list.len(), 2);
    // Cross-project isolation: legacy project's sessions are not
    // in this list.
    let legacy = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
    assert_eq!(legacy.len(), 0);
}

#[tokio::test]
async fn load_session_returns_none_for_missing() {
    let pool = test_pool().await;
    let result = load_session(&pool, "nonexistent").await.unwrap();
    assert!(result.is_none());
}

/// 2026-08-18 (5df29977 问题4) regression: the INSERT must persist
/// `mode='edit'`, agreeing with the returned struct's `Mode::Edit`.
/// b999803 accidentally wrote the legacy `'chat'` into the mode slot
/// (confused with session_type's DEFAULT 'chat'); the every-init
/// `chat→edit` scrub migration masked the disagreement, so only
/// sessions created after the last process start kept the bad value.
#[tokio::test]
async fn create_session_persists_edit_mode() {
    let pool = test_pool().await;
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
    assert_eq!(s.mode, crate::db::Mode::Edit, "struct field is Edit");
    // The persisted row must agree — pre-fix the struct said Edit
    // while the row held the legacy 'chat'.
    let row: (String,) = sqlx::query_as("SELECT mode FROM sessions WHERE id = ?")
        .bind(&s.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "edit", "INSERT must persist edit, not legacy chat");
}

#[tokio::test]
async fn persist_and_load_messages() {
    let pool = test_pool().await;
    let session = create_session(
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

    let user_msg = MessageContent::Text("read the file".to_string());
    persist_turn(&pool, &session.id, Role::User, &user_msg, 0, None, None)
        .await
        .unwrap();

    let assistant_blocks = vec![
        ContentBlock::Text {
            text: "OK reading".to_string(),
            cache_control: None,
        },
        ContentBlock::ToolUse {
            id: "toolu_abc".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "/etc/hostname"}),
        },
    ];
    let assistant_msg = MessageContent::Blocks(assistant_blocks);
    persist_turn(
        &pool,
        &session.id,
        Role::Assistant,
        &assistant_msg,
        1,
        None,
        None,
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.messages[0].seq, 0);
    assert_eq!(loaded.messages[0].text, "read the file");
    assert_eq!(loaded.messages[1].seq, 1);
    assert!(loaded.messages[1].has_tool_calls);
    assert!(!loaded.messages[1].has_tool_results);

    let blocks: Vec<ContentBlock> =
        serde_json::from_value(loaded.messages[1].content.clone()).unwrap();
    assert_eq!(blocks.len(), 2);
    assert!(matches!(&blocks[1], ContentBlock::ToolUse { name, .. } if name == "read_file"));
}

#[tokio::test]
async fn first_user_message_auto_titles_session() {
    let pool = test_pool().await;
    let session = create_session(
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

    let msg = MessageContent::Text("帮我读一下 /etc/hostname".to_string());
    persist_turn(&pool, &session.id, Role::User, &msg, 0, None, None)
        .await
        .unwrap();

    let updated = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(updated.session.title, "帮我读一下 /etc/hostname");
}

#[tokio::test]
async fn second_user_message_does_not_overwrite_title() {
    let pool = test_pool().await;
    let session = create_session(
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

    persist_turn(
        &pool,
        &session.id,
        Role::User,
        &MessageContent::Text("first".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    persist_turn(
        &pool,
        &session.id,
        Role::User,
        &MessageContent::Text("second".into()),
        1,
        None,
        None,
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(loaded.session.title, "first");
}

#[tokio::test]
async fn delete_session_cascades_messages() {
    let pool = test_pool().await;
    let session = create_session(
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
    persist_turn(
        &pool,
        &session.id,
        Role::User,
        &MessageContent::Text("hi".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();

    delete_session(&pool, &session.id).await.unwrap();
    assert!(load_session(&pool, &session.id).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_messages_by_session_keeps_session_drops_messages() {
    let pool = test_pool().await;
    let session = create_session(
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
    persist_turn(
        &pool,
        &session.id,
        Role::User,
        &MessageContent::Text("hi".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();

    // Sanity: the message was persisted.
    let before = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(before.messages.len(), 1);

    // B3 /clear: messages gone, session row + metadata survive.
    delete_messages_by_session(&pool, &session.id)
        .await
        .unwrap();
    let after = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert!(after.messages.is_empty(), "messages should be cleared");
    assert_eq!(
        after.session.id, session.id,
        "session row must survive /clear"
    );
    assert_eq!(
        after.session.title, before.session.title,
        "metadata preserved"
    );
}

#[tokio::test]
async fn list_sessions_preview_truncates_at_80_chars() {
    let pool = test_pool().await;
    let session = create_session(
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
    let long = "a".repeat(120);
    persist_turn(
        &pool,
        &session.id,
        Role::User,
        &MessageContent::Text(long),
        0,
        None,
        None,
    )
    .await
    .unwrap();

    let list = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
    assert!(list[0].preview.starts_with("a".repeat(80).as_str()));
    assert!(list[0].preview.ends_with('…'));
}

// ---------------------------------------------------------------------------
// Group-chat discussion lifecycle (2026-09-05, BUGLIST-group-chat GC2/GC7)
// ---------------------------------------------------------------------------

/// `finalize_group_chat_lifecycle` + `clear_group_chat_lifecycle`
/// round-trip: stop_reason + discussion_summary persist, load_session
/// exposes both as first-class fields, list_sessions carries the stop
/// reason, and clear resets a reused session for its next run.
#[tokio::test]
async fn group_chat_lifecycle_columns_round_trip() {
    let pool = test_pool().await;
    let sid = Uuid::new_v4().to_string();
    create_session(
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

    // Fresh session: both lifecycle fields NULL.
    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(loaded.session.stop_reason, None);
    assert_eq!(loaded.session.discussion_summary, None);

    // Normal end: stop_reason + summary + structured detail persisted,
    // readable without parsing any tool_result content blocks (GC7's
    // core ask; C2 adds discussion_detail).
    let detail_json =
        r#"{"conclusions":[{"claim":"A","anchors":[],"stance":"verified"}],"open_questions":[]}"#;
    finalize_group_chat_lifecycle(
        &pool,
        &sid,
        "group_chat_end",
        Some("## 共识清单\n- A"),
        Some(detail_json),
    )
    .await
    .unwrap();
    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("group_chat_end")
    );
    assert_eq!(
        loaded.session.discussion_summary.as_deref(),
        Some("## 共识清单\n- A")
    );
    assert_eq!(
        loaded.session.discussion_detail.as_deref(),
        Some(detail_json),
        "C2: structured detail persists alongside the summary"
    );

    // Non-end exit (max_rounds / cancelled / error): stop_reason
    // updates, an absent summary leaves the column untouched.
    finalize_group_chat_lifecycle(&pool, &sid, "max_rounds", None, None)
        .await
        .unwrap();
    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(loaded.session.stop_reason.as_deref(), Some("max_rounds"));
    assert_eq!(
        loaded.session.discussion_summary.as_deref(),
        Some("## 共识清单\n- A"),
        "None must NOT clear the summary column (start-of-run clear owns that)"
    );
    assert_eq!(
        loaded.session.discussion_detail.as_deref(),
        Some(detail_json),
        "None must NOT clear the detail column either (same COALESCE contract)"
    );

    // list_sessions summary carries the stop reason (the poller's
    // !busy + stop_reason derivation).
    let summaries = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
    let me = summaries.iter().find(|s| s.id == sid).unwrap();
    assert_eq!(me.stop_reason.as_deref(), Some("max_rounds"));

    // Reuse: the next orchestration's start clears all three columns
    // (a stale detail would feed consumers the previous run's
    // anchor-checked conclusions).
    clear_group_chat_lifecycle(&pool, &sid).await.unwrap();
    let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(loaded.session.stop_reason, None);
    assert_eq!(loaded.session.discussion_summary, None);
    assert_eq!(loaded.session.discussion_detail, None);

    // The three terminal reasons are distinguishable post-hoc (GC2's
    // core ask) — smoke each value through the column.
    for reason in ["group_chat_end", "max_rounds", "cancelled", "error"] {
        finalize_group_chat_lifecycle(&pool, &sid, reason, None, None)
            .await
            .unwrap();
        let loaded = load_session(&pool, &sid).await.unwrap().unwrap();
        assert_eq!(loaded.session.stop_reason.as_deref(), Some(reason));
    }
}

// ---------------------------------------------------------------------------
// Group-chat checkpoint (2026-09-06, GCE P1a — task
// 09-06-gc-p1a-checkpoint-resume)
// ---------------------------------------------------------------------------

/// P1a AC1 (DB 面):upsert round-trip、`started_at` 行生命周期内
/// 不可变(ON CONFLICT 不触)、get/delete 语义。
#[tokio::test]
async fn group_chat_checkpoint_upsert_round_trip_and_started_at_immutable() {
    let pool = test_pool().await;
    let sid = Uuid::new_v4().to_string();
    create_session(
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

    // Round head progression: round 0 (streak 0) → round 2 (streak 1).
    upsert_group_chat_checkpoint(&pool, &sid, 0, 0)
        .await
        .unwrap();
    let first = get_group_chat_checkpoint(&pool, &sid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.round, 0);
    assert_eq!(first.error_streak, 0);
    let started_at = first.started_at.clone();

    upsert_group_chat_checkpoint(&pool, &sid, 2, 1)
        .await
        .unwrap();
    let second = get_group_chat_checkpoint(&pool, &sid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.round, 2);
    assert_eq!(second.error_streak, 1);
    assert_eq!(
        second.started_at, started_at,
        "ON CONFLICT must never touch started_at (row lifetime = one discussion)"
    );

    // delete → None (terminal-exit / fresh-start path).
    super::sessions::delete_group_chat_checkpoint(&pool, &sid)
        .await
        .unwrap();
    assert!(get_group_chat_checkpoint(&pool, &sid)
        .await
        .unwrap()
        .is_none());

    // Fresh row after delete gets a NEW started_at (new discussion).
    upsert_group_chat_checkpoint(&pool, &sid, 0, 0)
        .await
        .unwrap();
    let third = get_group_chat_checkpoint(&pool, &sid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(third.round, 0);
    assert_eq!(third.error_streak, 0);
}

/// P1a AC2 (DB 面):boot sweep 三断言——① crash 残留(行在 +
/// stop_reason=NULL)标 `interrupted` 且**不动 updated_at**;② 已有
/// 终态值不被覆盖;③ 孤儿行(行在 + 终局 stop_reason)被清。
#[tokio::test]
async fn group_chat_checkpoint_boot_sweep_marks_interrupted_and_heals_orphans() {
    let pool = test_pool().await;
    async fn mk_group_session(pool: &sqlx::SqlitePool, tag: &str) -> String {
        let sid = format!("p1a-{}", tag);
        create_session(
            pool,
            &sid,
            DEFAULT_PROJECT_ID,
            "/tmp",
            "GLM-4.7",
            None,
            Some("group_chat"),
            None,
        )
        .await
        .unwrap();
        sid
    }

    // ① crash residue: row present, stop_reason NULL.
    let crashed = mk_group_session(&pool, "crash").await;
    upsert_group_chat_checkpoint(&pool, &crashed, 3, 0)
        .await
        .unwrap();
    let before = load_session(&pool, &crashed).await.unwrap().unwrap();
    let updated_before = before.session.updated_at.clone();
    // Resume-affecting nuance: the mark must NOT bump updated_at —
    // sleep a hair so a buggy write would produce a different value.
    tokio::time::sleep(std::time::Duration::from_millis(15)).await;

    // ② terminal residue: row stranded under a terminal stop_reason
    // (simulates finalize-ok-but-delete-failed).
    let orphan = mk_group_session(&pool, "orphan").await;
    upsert_group_chat_checkpoint(&pool, &orphan, 7, 0)
        .await
        .unwrap();
    finalize_group_chat_lifecycle(&pool, &orphan, "group_chat_end", Some("done"), None)
        .await
        .unwrap();

    // ③ already-terminal WITHOUT row → untouched; and a cancelled
    // (resumable) session keeps both row and stop_reason as-is.
    let cancelled = mk_group_session(&pool, "cancelled").await;
    upsert_group_chat_checkpoint(&pool, &cancelled, 5, 0)
        .await
        .unwrap();
    finalize_group_chat_lifecycle(&pool, &cancelled, "cancelled", None, None)
        .await
        .unwrap();

    let report = recover_group_chat_checkpoints(&pool).await.unwrap();
    assert_eq!(report.marked_interrupted, 1);
    assert_eq!(report.orphan_rows_deleted, 1);

    let marked = load_session(&pool, &crashed).await.unwrap().unwrap();
    assert_eq!(marked.session.stop_reason.as_deref(), Some("interrupted"));
    assert_eq!(
        marked.session.updated_at, updated_before,
        "sweep must not touch sessions.updated_at (sidebar sorts by it)"
    );

    assert!(
        get_group_chat_checkpoint(&pool, &orphan)
            .await
            .unwrap()
            .is_none(),
        "terminal-residue row must be healed away"
    );
    let orphan_session = load_session(&pool, &orphan).await.unwrap().unwrap();
    assert_eq!(
        orphan_session.session.stop_reason.as_deref(),
        Some("group_chat_end"),
        "heal deletes the ROW, never rewrites a terminal stop_reason"
    );

    let (kept_row, kept_session) = (
        get_group_chat_checkpoint(&pool, &cancelled).await.unwrap(),
        load_session(&pool, &cancelled).await.unwrap().unwrap(),
    );
    assert!(
        kept_row.is_some(),
        "resumable (cancelled) row survives the sweep"
    );
    assert_eq!(
        kept_session.session.stop_reason.as_deref(),
        Some("cancelled")
    );

    // Idempotent: a second sweep is a no-op.
    let again = recover_group_chat_checkpoints(&pool).await.unwrap();
    assert_eq!(again.marked_interrupted, 0);
    assert_eq!(again.orphan_rows_deleted, 0);
}

/// Session delete cascades the checkpoint row (FK ON DELETE CASCADE).
#[tokio::test]
async fn group_chat_checkpoint_cascades_on_session_delete() {
    let pool = test_pool().await;
    let sid = Uuid::new_v4().to_string();
    create_session(
        &pool,
        &sid,
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        Some("group_chat"),
        None,
    )
    .await
    .unwrap();
    upsert_group_chat_checkpoint(&pool, &sid, 1, 0)
        .await
        .unwrap();
    assert!(get_group_chat_checkpoint(&pool, &sid)
        .await
        .unwrap()
        .is_some());

    delete_session(&pool, &sid).await.unwrap();
    assert!(get_group_chat_checkpoint(&pool, &sid)
        .await
        .unwrap()
        .is_none());
}
