//! GCE M4b (discussion-library search) — db-layer tests for the
//! field-level group-chat session browse/search
//! (`db/search_group_chat.rs`). Covers browse ordering + the
//! never-return-chat red line, project/stop_reason filters, keyword
//! matching across all four searchable fields (title / summary /
//! task_name / participants), wildcard literalism, malformed-metadata
//! resilience, and mid-flight-session browse-by-title (AC5).

#![cfg(test)]

use serde_json::json;

use super::test_support::test_pool;
use crate::db::search_group_chat::{
    list_group_chat_sessions, search_group_chat_discussions, GroupChatSessionFilters,
    GroupChatSessionHit,
};
use crate::db::sessions::create_session;

/// Metadata blob shape the scheduler / group-chat creator persists
/// (`participants[].name`, `scheduled_task_name` for 定时场).
fn gc_metadata() -> String {
    json!({
        "participants": [
            { "name": "Alice", "model": "model-a" },
            { "name": "Bob", "model": "model-b" },
        ],
        "created_via": "scheduled",
        "scheduled_task_name": "每周审议",
    })
    .to_string()
}

/// Seed one group-chat session row with full control over the
/// lifecycle columns + timestamps (raw UPDATE so ordering tests don't
/// race `Utc::now()`). Returns the session id.
#[allow(clippy::too_many_arguments)] // 8-arg test fixture helper (mirrors create_session)
async fn seed_gc(
    pool: &sqlx::SqlitePool,
    project_id: &str,
    id: &str,
    title: &str,
    metadata: Option<&str>,
    summary: Option<&str>,
    stop_reason: Option<&str>,
    updated_at: &str,
) {
    create_session(
        pool,
        id,
        project_id,
        "/tmp/gc_seed",
        "GLM-4.7",
        None,
        Some("group_chat"),
        metadata,
    )
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE sessions
        SET title = ?, discussion_summary = ?, stop_reason = ?, updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(title)
    .bind(summary)
    .bind(stop_reason)
    .bind(updated_at)
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_chat(pool: &sqlx::SqlitePool, project_id: &str, id: &str, title: &str) {
    create_session(
        pool,
        id,
        project_id,
        "/tmp/gc_seed",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE sessions SET title = ? WHERE id = ?")
        .bind(title)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

fn ids(hits: &[GroupChatSessionHit]) -> Vec<&str> {
    hits.iter().map(|h| h.session_id.as_str()).collect()
}

#[tokio::test]
async fn browse_returns_all_group_chats_desc_and_never_chat() {
    let pool = test_pool().await;
    let pa = crate::db::projects::create_project(&pool, "pa", "/tmp/gc_pa", false, None)
        .await
        .unwrap();
    let pb = crate::db::projects::create_project(&pool, "pb", "/tmp/gc_pb", false, None)
        .await
        .unwrap();

    // Two group chats in pa (different updated_at for ordering) + one
    // in pb + one classic chat in pa (must never surface).
    seed_gc(
        &pool,
        &pa.id,
        "gc-old",
        "权限系统重构讨论",
        Some(&gc_metadata()),
        Some("结论:采用 Rust 重写。"),
        Some("group_chat_end"),
        "2026-09-05T10:00:00+00:00",
    )
    .await;
    seed_gc(
        &pool,
        &pa.id,
        "gc-new",
        "定时架构复盘",
        Some(&gc_metadata()),
        Some("结论:保留事件溯源。"),
        Some("max_rounds"),
        "2026-09-06T10:00:00+00:00",
    )
    .await;
    seed_gc(
        &pool,
        &pb.id,
        "gc-other-project",
        "别的项目讨论",
        Some(&gc_metadata()),
        None,
        None,
        "2026-09-07T10:00:00+00:00",
    )
    .await;
    seed_chat(&pool, &pa.id, "chat-1", "经典对话:含 group_chat 字样").await;

    let all = list_group_chat_sessions(&pool, &GroupChatSessionFilters::default())
        .await
        .unwrap();
    assert_eq!(
        ids(&all),
        vec!["gc-other-project", "gc-new", "gc-old"],
        "updated_at DESC; classic chat row never returned"
    );

    let newest = &all[0];
    assert_eq!(newest.session_id, "gc-other-project");
    assert_eq!(newest.project_id, pb.id);
    // No finalize → summary / stop_reason absent; config still parsed.
    assert_eq!(newest.discussion_summary, None);
    assert_eq!(newest.stop_reason, None);
    assert_eq!(newest.task_name.as_deref(), Some("每周审议"));
    assert_eq!(newest.participants, vec!["Alice", "Bob"]);

    let finalized = &all[1];
    assert_eq!(
        finalized.discussion_summary.as_deref(),
        Some("结论:保留事件溯源。")
    );
    assert_eq!(finalized.stop_reason.as_deref(), Some("max_rounds"));
    assert_eq!(finalized.title, "定时架构复盘");
}

#[tokio::test]
async fn browse_filters_by_project_and_stop_reason() {
    let pool = test_pool().await;
    let pa = crate::db::projects::create_project(&pool, "pa", "/tmp/gc_pa2", false, None)
        .await
        .unwrap();
    let pb = crate::db::projects::create_project(&pool, "pb", "/tmp/gc_pb2", false, None)
        .await
        .unwrap();
    seed_gc(
        &pool,
        &pa.id,
        "gc-a1",
        "A 讨论",
        Some(&gc_metadata()),
        None,
        Some("group_chat_end"),
        "2026-09-05T10:00:00+00:00",
    )
    .await;
    seed_gc(
        &pool,
        &pa.id,
        "gc-a2",
        "A 讨论二",
        Some(&gc_metadata()),
        None,
        Some("cancelled"),
        "2026-09-06T10:00:00+00:00",
    )
    .await;
    seed_gc(
        &pool,
        &pb.id,
        "gc-b1",
        "B 讨论",
        Some(&gc_metadata()),
        None,
        Some("group_chat_end"),
        "2026-09-07T10:00:00+00:00",
    )
    .await;

    let pa_only = list_group_chat_sessions(
        &pool,
        &GroupChatSessionFilters {
            project_id: Some(pa.id.clone()),
            stop_reason: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(ids(&pa_only), vec!["gc-a2", "gc-a1"]);

    let ended_in_pa = list_group_chat_sessions(
        &pool,
        &GroupChatSessionFilters {
            project_id: Some(pa.id),
            stop_reason: Some("group_chat_end".to_string()),
        },
    )
    .await
    .unwrap();
    assert_eq!(ids(&ended_in_pa), vec!["gc-a1"]);
}

/// Metadata with a distinctive task name + roster, so each keyword
/// test can target exactly one field.
fn task_metadata(task_name: &str, names: &[&str]) -> String {
    json!({
        "participants": names.iter().map(|n| json!({ "name": n, "model": "model-a" })).collect::<Vec<_>>(),
        "scheduled_task_name": task_name,
    })
    .to_string()
}

#[tokio::test]
async fn keyword_matches_title_summary_task_name_and_participant() {
    let pool = test_pool().await;
    let pa = crate::db::projects::create_project(&pool, "pa", "/tmp/gc_pa3", false, None)
        .await
        .unwrap();
    // Title-only hit (every other field unique to its own seed).
    seed_gc(
        &pool,
        &pa.id,
        "gc-title",
        "权限系统重构讨论",
        Some(&task_metadata("任务甲", &["Alice", "Bob"])),
        None,
        None,
        "2026-09-05T10:00:00+00:00",
    )
    .await;
    // Summary-only hit (matched term lives only in the summary).
    seed_gc(
        &pool,
        &pa.id,
        "gc-summary",
        "架构方向",
        Some(&task_metadata("任务乙", &["Alice", "Bob"])),
        Some("结论:保留事件溯源方案,不引 K8s。"),
        Some("group_chat_end"),
        "2026-09-06T10:00:00+00:00",
    )
    .await;
    // Task-name-only hit.
    seed_gc(
        &pool,
        &pa.id,
        "gc-task",
        "普通标题",
        Some(&task_metadata("架构复盘专项", &["Alice", "Bob"])),
        None,
        Some("group_chat_end"),
        "2026-09-07T10:00:00+00:00",
    )
    .await;
    // Participant-only hit (Carol is not in any other roster).
    seed_gc(
        &pool,
        &pa.id,
        "gc-participant",
        "无关标题",
        Some(&task_metadata("任务丙", &["Carol"])),
        None,
        Some("group_chat_end"),
        "2026-09-08T10:00:00+00:00",
    )
    .await;

    let f = &GroupChatSessionFilters::default();
    assert_eq!(
        ids(&search_group_chat_discussions(&pool, "权限系统", f)
            .await
            .unwrap()),
        vec!["gc-title"],
        "title hit"
    );
    assert_eq!(
        ids(&search_group_chat_discussions(&pool, "K8s", f)
            .await
            .unwrap()),
        vec!["gc-summary"],
        "summary hit"
    );
    assert_eq!(
        ids(&search_group_chat_discussions(&pool, "架构复盘专项", f)
            .await
            .unwrap()),
        vec!["gc-task"],
        "task_name hit"
    );
    assert_eq!(
        ids(&search_group_chat_discussions(&pool, "Carol", f)
            .await
            .unwrap()),
        vec!["gc-participant"],
        "participant-name hit"
    );
    // A query that matches a shared roster member surfaces every
    // session that carries that participant (dedup across fields is
    // inherent — one row per session, no per-field duplicates).
    assert_eq!(
        ids(&search_group_chat_discussions(&pool, "Bob", f)
            .await
            .unwrap()),
        vec!["gc-task", "gc-summary", "gc-title"],
        "Bob rides on every seed carrying him, one hit per session"
    );
}

#[tokio::test]
async fn participant_match_is_precise_not_model_id_or_persona() {
    // The participant LIKE must match `participants[].name` only — a
    // search for a model id stored in `participants[].model` (or text
    // that only exists in the raw metadata JSON) must NOT hit.
    let pool = test_pool().await;
    let pa = crate::db::projects::create_project(&pool, "pa", "/tmp/gc_pa4", false, None)
        .await
        .unwrap();
    seed_gc(
        &pool,
        &pa.id,
        "gc-1",
        "模型选择讨论",
        Some(&gc_metadata()), // model ids: "model-a"/"model-b"
        None,
        None,
        "2026-09-05T10:00:00+00:00",
    )
    .await;
    let f = &GroupChatSessionFilters::default();
    assert!(
        search_group_chat_discussions(&pool, "model-a", f)
            .await
            .unwrap()
            .is_empty(),
        "participant model id is not a searchable field"
    );
}

#[tokio::test]
async fn empty_and_no_hit_queries_behave() {
    let pool = test_pool().await;
    let pa = crate::db::projects::create_project(&pool, "pa", "/tmp/gc_pa5", false, None)
        .await
        .unwrap();
    seed_gc(
        &pool,
        &pa.id,
        "gc-1",
        "唯一讨论",
        Some(&gc_metadata()),
        None,
        None,
        "2026-09-05T10:00:00+00:00",
    )
    .await;

    let f = &GroupChatSessionFilters::default();
    // Empty / whitespace keyword = full browse (R2 — panel opens with
    // the whole library visible).
    assert_eq!(
        ids(&search_group_chat_discussions(&pool, "", f).await.unwrap()),
        vec!["gc-1"]
    );
    assert_eq!(
        ids(&search_group_chat_discussions(&pool, "   ", f)
            .await
            .unwrap()),
        vec!["gc-1"]
    );
    // No hit → empty array, not an error.
    assert!(search_group_chat_discussions(&pool, "量子纠缠", f)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn like_wildcards_in_query_are_literal() {
    // `_` must match the literal underscore, not any-single-char (the
    // messages-search wildcard-literalism contract applies to the
    // field-level LIKE too): a query `a_b` must NOT surface a session
    // whose title has `aXb` (one-char gap) instead.
    let pool = test_pool().await;
    let pa = crate::db::projects::create_project(&pool, "pa", "/tmp/gc_pa6", false, None)
        .await
        .unwrap();
    seed_gc(
        &pool,
        &pa.id,
        "gc-literal",
        "标题含 a_b 下划线",
        Some(&gc_metadata()),
        None,
        None,
        "2026-09-05T10:00:00+00:00",
    )
    .await;
    seed_gc(
        &pool,
        &pa.id,
        "gc-gap",
        "标题含 aXb 单字符差",
        Some(&gc_metadata()),
        None,
        None,
        "2026-09-06T10:00:00+00:00",
    )
    .await;
    let f = &GroupChatSessionFilters::default();
    assert_eq!(
        ids(&search_group_chat_discussions(&pool, "a_b", f)
            .await
            .unwrap()),
        vec!["gc-literal"],
        "underscore stays literal — aXb must not be treated as a match"
    );
    assert!(
        search_group_chat_discussions(&pool, "%标题", f)
            .await
            .unwrap()
            .is_empty(),
        "% in the query is literal — must not act as a leading wildcard"
    );
}

#[tokio::test]
async fn malformed_metadata_does_not_raise_browse_or_search() {
    // A corrupt `metadata` JSON must degrade to no task/participants
    // AND must not raise the whole query (json_valid guards on both
    // metadata-backed LIKE branches).
    let pool = test_pool().await;
    let pa = crate::db::projects::create_project(&pool, "pa", "/tmp/gc_pa7", false, None)
        .await
        .unwrap();
    seed_gc(
        &pool,
        &pa.id,
        "gc-bad",
        "含损坏配置的讨论",
        Some("not-json{"),
        Some("结论正常落库。"),
        Some("group_chat_end"),
        "2026-09-05T10:00:00+00:00",
    )
    .await;

    let f = &GroupChatSessionFilters::default();
    let all = list_group_chat_sessions(&pool, f).await.unwrap();
    assert_eq!(ids(&all), vec!["gc-bad"]);
    assert_eq!(all[0].task_name, None);
    assert!(all[0].participants.is_empty());

    // Summary still searchable through the corruption.
    assert_eq!(
        ids(&search_group_chat_discussions(&pool, "结论正常", f)
            .await
            .unwrap()),
        vec!["gc-bad"]
    );
    // Non-matching search over the corrupt row must not error either.
    assert!(search_group_chat_discussions(&pool, "没有这个词", f)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn mid_flight_session_searchable_by_title_not_summary() {
    // AC5: a discussion that never finalized (stop_reason + summary
    // both NULL) stays searchable by title / task / participants.
    let pool = test_pool().await;
    let pa = crate::db::projects::create_project(&pool, "pa", "/tmp/gc_pa8", false, None)
        .await
        .unwrap();
    seed_gc(
        &pool,
        &pa.id,
        "gc-live",
        "进行中的议题",
        Some(&gc_metadata()),
        None,
        None,
        "2026-09-05T10:00:00+00:00",
    )
    .await;

    let f = &GroupChatSessionFilters::default();
    assert_eq!(
        ids(&search_group_chat_discussions(&pool, "进行中", f)
            .await
            .unwrap()),
        vec!["gc-live"],
        "title hit before finalize"
    );
    assert!(
        search_group_chat_discussions(&pool, "未落库的摘要词", f)
            .await
            .unwrap()
            .is_empty(),
        "summary is NULL pre-finalize — no summary hit required (AC5)"
    );
}

// ---------------------------------------------------------------------------
// total_tokens (09-08-gce-m4c, cost governance)
// ---------------------------------------------------------------------------

/// Seed one usage-bearing assistant turn (speaker row + trace row)
/// for a group-chat session — the same derived data the
/// `group_chat_token_usage` query aggregates.
async fn seed_usage_turn(
    pool: &sqlx::SqlitePool,
    sid: &str,
    seq: i64,
    speaker: &str,
    usage_json: &str,
) {
    use crate::db::sessions::persist_turn;
    use crate::llm::types::{MessageContent, Role};
    persist_turn(
        pool,
        sid,
        Role::Assistant,
        &MessageContent::Text(format!("turn {seq}")),
        seq,
        None,
        Some(speaker),
    )
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO turn_trace (session_id, run_id, seq, token_usage_json)
        VALUES (?, '', ?, ?)
        "#,
    )
    .bind(sid)
    .bind(seq)
    .bind(usage_json)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn browse_carries_total_tokens_and_none_for_usageless_sessions() {
    let pool = test_pool().await;
    let pa = crate::db::projects::create_project(&pool, "pa-cost", "/tmp/gc_pa_cost", false, None)
        .await
        .unwrap();

    // Session WITH usage: two speaker turns, four-field sums
    // 210 + 240 = 450.
    seed_gc(
        &pool,
        &pa.id,
        "gc-cost",
        "有消耗的场",
        Some(&gc_metadata()),
        Some("结论"),
        Some("group_chat_end"),
        "2026-09-05T10:00:00+00:00",
    )
    .await;
    seed_usage_turn(
        &pool,
        "gc-cost",
        1,
        "Alice",
        r#"{"input_tokens":100,"output_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":100,"context_input_tokens":1000}"#,
    )
    .await;
    seed_usage_turn(
        &pool,
        "gc-cost",
        2,
        "moderator",
        r#"{"input_tokens":200,"output_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":20,"context_input_tokens":200}"#,
    )
    .await;

    // Session WITHOUT any turn: total must be None (frontend "—"),
    // not 0 — "never ran" and "ran but zero-cost" stay distinct.
    seed_gc(
        &pool,
        &pa.id,
        "gc-fresh",
        "还没开场的群",
        Some(&gc_metadata()),
        None,
        None,
        "2026-09-05T09:00:00+00:00",
    )
    .await;

    let hits = list_group_chat_sessions(&pool, &GroupChatSessionFilters::default())
        .await
        .unwrap();
    let cost = hits.iter().find(|h| h.session_id == "gc-cost").unwrap();
    assert_eq!(cost.total_tokens, Some(450), "hit: {cost:?}");
    let fresh = hits.iter().find(|h| h.session_id == "gc-fresh").unwrap();
    assert_eq!(fresh.total_tokens, None, "hit: {fresh:?}");

    // The keyword path (search) carries the same column.
    let found = search_group_chat_discussions(&pool, "有消耗", &GroupChatSessionFilters::default())
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].total_tokens, Some(450));
}
