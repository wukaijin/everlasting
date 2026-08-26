//! D2 (cross-session search) — db-layer tests. Covers the FTS
//! migration guard, trigger sync invariants (`UPDATE OF text`
//! red line, AC7 delete propagation), the ≥3-chars FTS / <3-chars
//! LIKE dispatch, project filtering, and the title-hit rider.

#![cfg(test)]

use super::test_support::test_pool;
use uuid::Uuid;

use crate::db::memories::escape_fts5;
use crate::db::projects::create_project;
use crate::db::search::{search_messages, SearchHitKind};
use crate::db::sessions::{create_session, delete_session, persist_turn, rename_session};
use crate::llm::types::{ContentBlock, MessageContent, Role};

/// Persist one text message into `session` at `seq`. Exercises the
/// production insert path so the FTS insert trigger is what indexes
/// the row (not a hand-written INSERT).
async fn say(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    role: Role,
    text: &str,
    seq: i64,
    speaker: Option<&str>,
) {
    let content = MessageContent::Blocks(vec![ContentBlock::Text {
        text: text.to_string(),
        cache_control: None,
    }]);
    persist_turn(pool, session_id, role, &content, seq, None, speaker)
        .await
        .unwrap();
}

async fn fts_doc_count(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts_docsize")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn search_hits_cjk_english_and_two_char_fallback() {
    let pool = test_pool().await;
    let p = create_project(&pool, "p1", "/tmp/d2_p1", false, None)
        .await
        .unwrap();
    let s1 = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &p.id,
        "/tmp/d2_p1",
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
        "/tmp/d2_p1",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    say(
        &pool,
        &s1.id,
        Role::User,
        "在WSL中跑cargo会因为权限不足而失败",
        0,
        None,
    )
    .await;
    say(
        &pool,
        &s2.id,
        Role::Assistant,
        "totally different topic about gardening",
        0,
        None,
    )
    .await;
    say(
        &pool,
        &s2.id,
        Role::User,
        "权限系统的设计原则是什么",
        1,
        Some("moderator"),
    )
    .await;

    // ≥3 chars → FTS path: CJK phrase across sessions.
    let hits = search_messages(&pool, "权限不足", None, None)
        .await
        .unwrap();
    let content_hits: Vec<_> = hits
        .iter()
        .filter(|h| h.kind == SearchHitKind::Content)
        .collect();
    assert_eq!(content_hits.len(), 1, "3-char CJK query hits via FTS");
    assert_eq!(content_hits[0].session_id, s1.id);
    assert_eq!(content_hits[0].seq, Some(0));
    assert_eq!(content_hits[0].role.as_deref(), Some("user"));
    assert_eq!(content_hits[0].speaker, None);
    assert!(
        content_hits[0]
            .snippet
            .as_deref()
            .unwrap()
            .contains("权限不足"),
        "snippet contains the matched text"
    );

    // ASCII term embedded in CJK — trigram substring property.
    let hits = search_messages(&pool, "cargo", None, None).await.unwrap();
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Content)
            .count(),
        1,
        "ASCII inside CJK matches via trigram"
    );

    // <3 chars → LIKE fallback: 2-char Chinese must still match.
    let hits = search_messages(&pool, "权限", None, None).await.unwrap();
    let content_hits: Vec<_> = hits
        .iter()
        .filter(|h| h.kind == SearchHitKind::Content)
        .collect();
    assert_eq!(
        content_hits.len(),
        2,
        "2-char query falls back to LIKE and matches both sessions"
    );
    assert!(content_hits
        .iter()
        .any(|h| h.session_id == s2.id && h.speaker.as_deref() == Some("moderator")));

    // English full words via FTS.
    let hits = search_messages(&pool, "gardening", None, None)
        .await
        .unwrap();
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Content)
            .count(),
        1
    );

    // No match → empty (and no title hits either).
    let hits = search_messages(&pool, "量子纠缠", None, None)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn search_backfills_pre_existing_rows_after_rebuild() {
    // Simulate the upgrade path: a DB that had messages BEFORE the
    // FTS migration exists. We create the pool, insert rows, then
    // drop the FTS artifacts (vtable + triggers) and re-run
    // run_migrations — the docsize guard must detect the stale
    // (empty) index and rebuild so old rows become searchable.
    let pool = test_pool().await;
    let p = create_project(&pool, "p1", "/tmp/d2_backfill", false, None)
        .await
        .unwrap();
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &p.id,
        "/tmp/d2_backfill",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    say(
        &pool,
        &s.id,
        Role::User,
        "historical message about trigram indexing",
        0,
        None,
    )
    .await;
    // Simulate the pre-migration state: kill the index + triggers.
    sqlx::query("DROP TRIGGER messages_fts_insert")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP TRIGGER messages_fts_delete")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP TRIGGER messages_fts_update")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP TABLE messages_fts")
        .execute(&pool)
        .await
        .unwrap();
    // (vtable drop takes the %_docsize shadow with it — the
    // migration guard treats the missing table as 0 docs.)

    // Re-run migrations: recreates vtable + triggers, docsize guard
    // (0 ≠ 1) fires rebuild, historical row becomes searchable.
    crate::db::migrations::run_migrations(&pool).await.unwrap();
    let hits = search_messages(&pool, "trigram", None, None).await.unwrap();
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Content)
            .count(),
        1,
        "pre-existing row searchable after guarded rebuild"
    );
    // Second migration run must NOT rebuild again (docsize now in
    // sync) and must stay idempotent/searchable.
    crate::db::migrations::run_migrations(&pool).await.unwrap();
    let hits = search_messages(&pool, "trigram", None, None).await.unwrap();
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Content)
            .count(),
        1
    );
}

#[tokio::test]
async fn deleted_session_leaves_no_search_traces() {
    // AC7: delete_session physically removes messages; the FTS
    // delete trigger must keep the index in lockstep.
    let pool = test_pool().await;
    let p = create_project(&pool, "p1", "/tmp/d2_del", false, None)
        .await
        .unwrap();
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &p.id,
        "/tmp/d2_del",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    say(
        &pool,
        &s.id,
        Role::User,
        "ephemeral content about sqlite",
        0,
        None,
    )
    .await;
    // Auto-title: the first user message also becomes the session
    // title, so one matching message legitimately yields BOTH a
    // Title hit and a Content hit.
    let hits = search_messages(&pool, "sqlite", None, None).await.unwrap();
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Content)
            .count(),
        1
    );
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Title)
            .count(),
        1
    );

    delete_session(&pool, &s.id).await.unwrap();
    let hits = search_messages(&pool, "sqlite", None, None).await.unwrap();
    assert!(
        hits.is_empty(),
        "deleted session leaves no hits of either kind (AC7)"
    );
    let msg_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        msg_count,
        fts_doc_count(&pool).await,
        "docsize probe stays in sync after cascade"
    );
}

#[tokio::test]
async fn non_text_updates_do_not_touch_fts_index() {
    // The `AFTER UPDATE OF text` red line: latency / metadata
    // patches are frequent per-message writes — they must not
    // trigger FTS delete+insert churn.
    let pool = test_pool().await;
    let p = create_project(&pool, "p1", "/tmp/d2_upd", false, None)
        .await
        .unwrap();
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &p.id,
        "/tmp/d2_upd",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    say(
        &pool,
        &s.id,
        Role::Assistant,
        "stable content for update test",
        0,
        None,
    )
    .await;
    let before = fts_doc_count(&pool).await;

    // Metadata + latency style updates (column set only).
    sqlx::query("UPDATE messages SET metadata = '{}' WHERE session_id = ?")
        .bind(&s.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE messages SET ttfb_ms = 12, gen_ms = 34, total_ms = 46 WHERE session_id = ?",
    )
    .bind(&s.id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        before,
        fts_doc_count(&pool).await,
        "non-text updates cause no FTS doc churn"
    );
    let hits = search_messages(&pool, "stable content", None, None)
        .await
        .unwrap();
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Content)
            .count(),
        1,
        "still searchable after non-text updates"
    );

    // A real text rewrite (D3 edit path shape) must swap the index.
    sqlx::query(
        "UPDATE messages SET text = 'rewritten content about postgres' WHERE session_id = ?",
    )
    .bind(&s.id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        before,
        fts_doc_count(&pool).await,
        "text rewrite keeps doc count stable (delete+insert pair)"
    );
    // (The session title still echoes the old text — auto-title —
    // so scope this assertion to Content hits.)
    let old = search_messages(&pool, "stable content", None, None)
        .await
        .unwrap();
    assert!(
        old.iter().all(|h| h.kind == SearchHitKind::Title),
        "old text no longer indexed (title echo allowed)"
    );
    let new = search_messages(&pool, "postgres", None, None)
        .await
        .unwrap();
    assert_eq!(new.len(), 1, "new text indexed");
}

#[tokio::test]
async fn project_filter_and_limit_apply() {
    let pool = test_pool().await;
    let pa = create_project(&pool, "pa", "/tmp/d2_pa", false, None)
        .await
        .unwrap();
    let pb = create_project(&pool, "pb", "/tmp/d2_pb", false, None)
        .await
        .unwrap();
    let sa = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &pa.id,
        "/tmp/d2_pa",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let sb = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &pb.id,
        "/tmp/d2_pb",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    say(
        &pool,
        &sa.id,
        Role::User,
        "shared keyword pineapple pizza one",
        0,
        None,
    )
    .await;
    say(
        &pool,
        &sb.id,
        Role::User,
        "shared keyword pineapple pizza two",
        0,
        None,
    )
    .await;

    // Unfiltered: both projects (2 content hits; auto-titles add
    // 2 title hits for the same sessions).
    let hits = search_messages(&pool, "pineapple", None, None)
        .await
        .unwrap();
    let content: Vec<_> = hits
        .iter()
        .filter(|h| h.kind == SearchHitKind::Content)
        .collect();
    assert_eq!(content.len(), 2);
    let names: Vec<Option<&str>> = content.iter().map(|h| h.project_name.as_deref()).collect();
    assert!(names.contains(&Some("pa")) && names.contains(&Some("pb")));

    // Project-scoped: only pb (content kind asserted; pb's title hit
    // also rides along under the same filter).
    let hits = search_messages(&pool, "pineapple", Some(&pb.id), None)
        .await
        .unwrap();
    let content: Vec<_> = hits
        .iter()
        .filter(|h| h.kind == SearchHitKind::Content)
        .collect();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0].session_id, sb.id);
    assert_eq!(content[0].project_id, pb.id);
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Title)
            .count(),
        1
    );

    // Limit applies per kind (title rider and content pool each cap
    // independently — one kind's flood must not drown the other).
    let hits = search_messages(&pool, "pineapple", None, Some(1))
        .await
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Content)
            .count(),
        1
    );
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Title)
            .count(),
        1
    );
}

#[tokio::test]
async fn empty_and_trivial_queries_return_no_rows() {
    let pool = test_pool().await;
    assert!(search_messages(&pool, "", None, None)
        .await
        .unwrap()
        .is_empty());
    assert!(search_messages(&pool, "   ", None, None)
        .await
        .unwrap()
        .is_empty());
    // 1-char LIKE is allowed by contract (returns whatever matches).
    let hits = search_messages(&pool, "x", None, None).await.unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn title_hits_ride_along_with_kind_discriminant() {
    let pool = test_pool().await;
    let p = create_project(&pool, "p1", "/tmp/d2_title", false, None)
        .await
        .unwrap();
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &p.id,
        "/tmp/d2_title",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // Rename the session (title) without any matching message body.
    rename_session(&pool, &s.id, "权限系统重构讨论")
        .await
        .unwrap();

    // 2-char query → LIKE path, title hit still surfaces.
    let hits = search_messages(&pool, "权限", None, None).await.unwrap();
    let title: Vec<_> = hits
        .iter()
        .filter(|h| h.kind == SearchHitKind::Title)
        .collect();
    assert_eq!(title.len(), 1);
    assert_eq!(title[0].session_id, s.id);
    assert_eq!(title[0].session_title, "权限系统重构讨论");
    assert!(title[0].seq.is_none() && title[0].snippet.is_none() && title[0].role.is_none());

    // ≥3-char query → FTS path for content, title rider unchanged.
    let hits = search_messages(&pool, "权限系统", None, None)
        .await
        .unwrap();
    assert_eq!(
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Title)
            .count(),
        1
    );
}

#[tokio::test]
async fn like_wildcards_in_query_are_literal() {
    // A query containing LIKE metacharacters must not widen the
    // pattern (`%` / `_` escaped by escape_like).
    let pool = test_pool().await;
    let p = create_project(&pool, "p1", "/tmp/d2_wild", false, None)
        .await
        .unwrap();
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &p.id,
        "/tmp/d2_wild",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    say(&pool, &s.id, Role::User, "aXb a_b axb", 0, None).await;
    // `_` must match literally, not as any-single-char (a_b only,
    // not aXb/axb).
    let hits = search_messages(&pool, "a_b", None, None).await.unwrap();
    let content: Vec<_> = hits
        .iter()
        .filter(|h| h.kind == SearchHitKind::Content)
        .collect();
    assert_eq!(
        content.len(),
        1,
        "underscore stays literal in LIKE fallback"
    );
}

#[test]
fn fts_phrase_escape_helper_neutralizes_operators() {
    // Reused from memories — one behavioral lock here so D2's
    // dispatch path can't silently regress the escaping contract.
    assert_eq!(escape_fts5("cargo AND test"), "\"cargo AND test\"");
    assert_eq!(escape_fts5("he said \"hi\""), "\"he said \"\"hi\"\"\"");
}
