#![cfg(test)]

use super::make_pool;
use super::memories::{
    bump_hit_count, get_memory_by_id, test_helpers::insert_raw, update_status, MemoryKind,
    MemoryScope, MemoryStatus, StatusTransitionError,
};

/// `bump_hit_count` increments hit_count and stamps last_used_at.
#[tokio::test]
async fn bump_hit_count_increments_and_stamps_last_used() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "bump-1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "title",
        "content",
    )
    .await
    .unwrap();
    // Initial state.
    let row = get_memory_by_id(&pool, "bump-1").await.unwrap().unwrap();
    assert_eq!(row.hit_count, 0);
    assert!(row.last_used_at.is_none());
    // Bump twice.
    bump_hit_count(&pool, "bump-1").await.unwrap();
    bump_hit_count(&pool, "bump-1").await.unwrap();
    let row = get_memory_by_id(&pool, "bump-1").await.unwrap().unwrap();
    assert_eq!(row.hit_count, 2);
    assert!(row.last_used_at.is_some(), "last_used_at stamped");
    // Unknown memory_id → no error (UPDATE matches 0 rows).
    bump_hit_count(&pool, "unknown")
        .await
        .expect("no error on unknown id");
}

/// `update_status` accepts legal transitions and rejects illegal
/// ones. Wraps the read + write in a transaction (concurrent
/// bump_hit_count can't race the status read).
#[tokio::test]
async fn update_status_legal_and_illegal_transitions() {
    let pool = make_pool().await;
    // Start at candidate.
    insert_raw(
        &pool,
        "st-1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Candidate,
        "title",
        "content",
    )
    .await
    .unwrap();

    // Legal: candidate → active.
    update_status(&pool, "st-1", MemoryStatus::Active, None)
        .await
        .unwrap();
    let row = get_memory_by_id(&pool, "st-1").await.unwrap().unwrap();
    assert_eq!(row.status, "active");
    assert!(row.demoted_reason.is_none(), "no reason on non-demote");

    // Legal: active → verified.
    update_status(&pool, "st-1", MemoryStatus::Verified, None)
        .await
        .unwrap();
    let row = get_memory_by_id(&pool, "st-1").await.unwrap().unwrap();
    assert_eq!(row.status, "verified");

    // Legal: verified → demoted (with reason).
    update_status(
        &pool,
        "st-1",
        MemoryStatus::Demoted,
        Some("superseded by newer memory"),
    )
    .await
    .unwrap();
    let row = get_memory_by_id(&pool, "st-1").await.unwrap().unwrap();
    assert_eq!(row.status, "demoted");
    assert_eq!(
        row.demoted_reason.as_deref(),
        Some("superseded by newer memory")
    );

    // Legal: demoted → active (re-promotion clears demoted_reason).
    update_status(&pool, "st-1", MemoryStatus::Active, None)
        .await
        .unwrap();
    let row = get_memory_by_id(&pool, "st-1").await.unwrap().unwrap();
    assert_eq!(row.status, "active");
    assert!(
        row.demoted_reason.is_none(),
        "demoted_reason cleared on re-promotion"
    );

    // Illegal: active → candidate (demotion is one-way; can't un-verify).
    let err = update_status(&pool, "st-1", MemoryStatus::Candidate, None)
        .await
        .unwrap_err();
    assert!(matches!(err, StatusTransitionError::Illegal { .. }));

    // Unknown memory_id → NotFound.
    let err = update_status(&pool, "unknown", MemoryStatus::Active, None)
        .await
        .unwrap_err();
    assert!(matches!(err, StatusTransitionError::NotFound(_)));
}

// ---------------------------------------------------------------------------
// P1 PR3: FTS trigger sync + EXPLAIN QUERY PLAN
// ---------------------------------------------------------------------------

/// FTS sync triggers: INSERT/UPDATE/DELETE on the base table keep
/// the FTS index in sync. AC: "INSERT/UPDATE/DELETE 后 FTS 同步".
#[tokio::test]
async fn fts_triggers_sync_on_insert_update_delete() {
    let pool = make_pool().await;
    // INSERT → FTS reachable.
    insert_raw(
        &pool,
        "tr-1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "initial title",
        "initial content about cargo",
    )
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM autonomous_memories_fts \
         WHERE autonomous_memories_fts MATCH '\"cargo\"'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "INSERT → FTS reachable");

    // UPDATE content (change the keyword) → old FTS entry replaced.
    sqlx::query(
        "UPDATE autonomous_memories SET content='now about rustc instead' WHERE memory_id='tr-1'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM autonomous_memories_fts \
         WHERE autonomous_memories_fts MATCH '\"cargo\"'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "UPDATE → old keyword gone from FTS");
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM autonomous_memories_fts \
         WHERE autonomous_memories_fts MATCH '\"rustc\"'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "UPDATE → new keyword in FTS");

    // DELETE base row → FTS entry removed.
    sqlx::query("DELETE FROM autonomous_memories WHERE memory_id='tr-1'")
        .execute(&pool)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM autonomous_memories_fts \
         WHERE autonomous_memories_fts MATCH '\"rustc\"'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "DELETE → FTS entry removed");
}

/// EXPLAIN QUERY PLAN: the session-start recall query (scope +
/// project_id filter) hits `idx_am_recall`. The pitfall probe
/// hits `idx_am_pitfall`. AC: "EXPLAIN QUERY PLAN 确认走索引".
///
/// SQLite EXPLAIN QUERY PLAN returns 4 columns:
/// `id|parent|notused|detail`. Only `detail` (col index 3) is the
/// human-readable plan text. We fetch it as `String` via
/// `query_scalar` with an explicit column offset.
#[tokio::test]
async fn explain_query_plan_uses_index() {
    let pool = make_pool().await;
    // Seed one row so the query has something to scan.
    insert_raw(
        &pool,
        "q-1",
        MemoryScope::Project,
        Some("proj-x"),
        MemoryKind::Fact,
        MemoryStatus::Active,
        "title",
        "content",
    )
    .await
    .unwrap();

    // The recall query shape: scope + project_id + status + kind.
    // The 4-column EXPLAIN output is projected to the `detail`
    // column (index 3) via `query_scalar`'s default first-column
    // fetch — but that returns the INTEGER `id` column. Instead,
    // we SELECT the detail column explicitly.
    let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        r#"EXPLAIN QUERY PLAN
           SELECT id FROM autonomous_memories
           WHERE scope='project' AND project_id='proj-x'
             AND status='active' AND kind='fact'"#,
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let joined = rows
        .iter()
        .map(|(_, _, _, d)| d.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("idx_am_recall"),
        "recall query must use idx_am_recall (not a full-table scan); got: {}",
        joined
    );
    // Belt-and-suspenders: also assert it's a SEARCH/USING INDEX (not
    // `SCAN autonomous_memories` which would mean a full table scan).
    assert!(
        !joined.contains("SCAN autonomous_memories"),
        "recall query must NOT be a full-table scan; got: {}",
        joined
    );

    // AC #8 explicitly calls out `scope='user' AND project_id IS NULL`
    // as a query shape that must hit idx_am_recall (the worry: SQLite
    // sometimes can't use a multi-column index when a non-leading
    // column is probed with `IS NULL`). Verified empirically: SQLite
    // 3.53 DOES use the covering index for this shape (NULL is
    // indexable). Lock it so a future index redesign doesn't regress.
    let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        r#"EXPLAIN QUERY PLAN
           SELECT id FROM autonomous_memories
           WHERE scope='user' AND project_id IS NULL
             AND status='active' AND kind='fact'"#,
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let joined = rows
        .iter()
        .map(|(_, _, _, d)| d.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("idx_am_recall"),
        "user+NULL project_id query must use idx_am_recall; got: {}",
        joined
    );

    // The pitfall probe shape: tool_name equality.
    let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        r#"EXPLAIN QUERY PLAN
           SELECT id FROM autonomous_memories
           WHERE tool_name='shell'
             AND kind='pitfall'
             AND status IN ('active','verified')"#,
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let joined = rows
        .iter()
        .map(|(_, _, _, d)| d.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("idx_am_pitfall"),
        "pitfall probe must use idx_am_pitfall (not a full-table scan); got: {}",
        joined
    );
    assert!(
        !joined.contains("SCAN autonomous_memories"),
        "pitfall probe must NOT be a full-table scan; got: {}",
        joined
    );
}
