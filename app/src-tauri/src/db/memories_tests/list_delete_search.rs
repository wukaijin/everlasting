#![cfg(test)]

use super::make_pool;
use super::memories::{
    count_memories_for_session, delete_memory, get_memory_by_id, insert_memory, list_memories,
    search_memories_fts, test_helpers::insert_raw, MemoryInput, MemoryInsertError, MemoryKind,
    MemoryScope, MemoryStatus, RecallStatusFilter,
};

/// `list_memories` filters by scope correctly:
/// - User scope → only user rows (project rows excluded even if
///   they share the project_id arg).
/// - Project scope + id → only that project's rows.
/// - None + id → user rows + that project's rows (H2 branch (c),
///   the MemoryPreview panel view).
/// - None + None → all rows (admin "全部项目" view).
/// Ordered newest-first.
#[tokio::test]
async fn list_memories_filters_by_scope_correctly() {
    let pool = make_pool().await;
    // 3 rows: 1 user, 2 project (different projects).
    insert_raw(
        &pool,
        "u1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "user fact",
        "user content",
    )
    .await
    .unwrap();
    insert_raw(
        &pool,
        "p1",
        MemoryScope::Project,
        Some("proj-a"),
        MemoryKind::Fact,
        MemoryStatus::Active,
        "proj-a fact",
        "content a",
    )
    .await
    .unwrap();
    insert_raw(
        &pool,
        "p2",
        MemoryScope::Project,
        Some("proj-b"),
        MemoryKind::Fact,
        MemoryStatus::Active,
        "proj-b fact",
        "content b",
    )
    .await
    .unwrap();

    // User scope — project_id arg is ignored.
    let rows = list_memories(&pool, Some(MemoryScope::User), Some("proj-a"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "u1");

    // Project scope + proj-a — only proj-a's row.
    let rows = list_memories(&pool, Some(MemoryScope::Project), Some("proj-a"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "p1");

    // Project scope + None → Err.
    let err = list_memories(&pool, Some(MemoryScope::Project), None)
        .await
        .unwrap_err();
    assert!(matches!(err, MemoryInsertError::ProjectScopeMissingId));

    // None scope + proj-a → user row + proj-a's row; proj-b
    // excluded (H2 branch (c) — the MemoryPreview panel view).
    let rows = list_memories(&pool, None, Some("proj-a")).await.unwrap();
    assert_eq!(rows.len(), 2);
    let ids: Vec<&str> = rows.iter().map(|r| r.memory_id.as_str()).collect();
    assert!(ids.contains(&"u1"));
    assert!(ids.contains(&"p1"));
    assert!(
        !ids.contains(&"p2"),
        "other project excluded from panel view"
    );

    // None scope + None → all 3 rows (admin "全部项目" view; the
    // only deliberately unfiltered shape).
    let rows = list_memories(&pool, None, None).await.unwrap();
    assert_eq!(rows.len(), 3);
}

/// Project isolation for the LIST path: a `scope=project` memory in
/// proj-a is NOT returned when listing via (None, Some("proj-b")).
/// Regression for the 2026-09-02 leak — the old scope=None arm had
/// no WHERE clause at all, so the MemoryPreview panel surfaced every
/// project's memories regardless of the queried project (observed
/// live: jjh-mono's list returned everlasting's rows over the daemon
/// HTTP API). The admin (None, None) view is unaffected.
#[tokio::test]
async fn list_memories_project_isolation() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "leak-a",
        MemoryScope::Project,
        Some("proj-a"),
        MemoryKind::Fact,
        MemoryStatus::Active,
        "proj-a leak canary",
        "the proj-a leak canary content",
    )
    .await
    .unwrap();
    insert_raw(
        &pool,
        "global-u",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "global user note",
        "user-scope content",
    )
    .await
    .unwrap();

    // List proj-b → only the user row; proj-a's row must not leak.
    let rows = list_memories(&pool, None, Some("proj-b")).await.unwrap();
    assert_eq!(rows.len(), 1, "only the user-scope row for proj-b");
    assert_eq!(rows[0].memory_id, "global-u");

    // The admin view still sees everything (1 user + 1 project row).
    let rows = list_memories(&pool, None, None).await.unwrap();
    assert_eq!(rows.len(), 2);
}

/// `delete_memory` removes the row AND the FTS index entries (via
/// the `am_fts_delete` trigger). Returns 0 for unknown memory_id.
#[tokio::test]
async fn delete_memory_removes_row_and_fts_index() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "del-1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "title to delete",
        "content searchable text here",
    )
    .await
    .unwrap();
    // Sanity: FTS reachable before delete.
    let before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM autonomous_memories_fts \
         WHERE autonomous_memories_fts MATCH '\"searchable\"'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, 1, "FTS reachable before delete");
    // Delete.
    let n = delete_memory(&pool, "del-1").await.unwrap();
    assert_eq!(n, 1, "1 row deleted");
    // Row gone.
    assert!(get_memory_by_id(&pool, "del-1").await.unwrap().is_none());
    // FTS gone.
    let after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM autonomous_memories_fts \
         WHERE autonomous_memories_fts MATCH '\"searchable\"'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, 0, "FTS index cleared after delete");
    // Unknown id → 0 rows affected.
    let n = delete_memory(&pool, "does-not-exist").await.unwrap();
    assert_eq!(n, 0, "unknown id → 0");
}

// ---------------------------------------------------------------------------
// P1 PR3: search_memories_fts + find_pitfalls_by_trigger +
//         bump_hit_count + update_status
// ---------------------------------------------------------------------------

/// `search_memories_fts` returns matching rows ranked by bm25.
/// Covers: insert N rows → search keyword → assert the right rows
/// surface in bm25 order. Only active/verified rows are returned
/// (candidate / demoted are excluded).
#[tokio::test]
async fn search_memories_fts_bm25_ranking_and_status_filter() {
    let pool = make_pool().await;
    // 3 rows: 2 active (different relevance), 1 candidate (excluded).
    insert_raw(
        &pool,
        "s1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "cargo build notes",
        "the cargo build command compiles the workspace",
    )
    .await
    .unwrap();
    insert_raw(
        &pool,
        "s2",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "unrelated",
        "this row mentions cargo once in passing",
    )
    .await
    .unwrap();
    insert_raw(
        &pool,
        "s3",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Candidate,
        "candidate cargo",
        "this cargo row is candidate status so excluded",
    )
    .await
    .unwrap();

    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "cargo",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    // Only the 2 active rows; the candidate is filtered out.
    assert_eq!(rows.len(), 2, "candidate excluded");
    let ids: Vec<&str> = rows.iter().map(|r| r.memory_id.as_str()).collect();
    assert!(ids.contains(&"s1"));
    assert!(ids.contains(&"s2"));
    assert!(!ids.contains(&"s3"), "candidate not surfaced");
    // bm25: s1 (title + content both hit) ranks before s2 (content only).
    assert_eq!(rows[0].memory_id, "s1", "higher-relevance row ranks first");
}

/// `escape_fts5` neutralizes FTS5 operators. A query containing
/// `"`, `NEAR`, `AND`, `OR`, `NOT`, `*`, `^` is treated as a
/// literal phrase, not a boolean expression. Verifies the AC:
/// "含 `"WSL cargo" test`/`NEAR`/`*` 等特殊字符的 query 经
/// escape_fts5 不报错、不误解析".
#[tokio::test]
async fn search_memories_fts_escapes_special_characters() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "e1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "WSL cargo test",
        "running cargo test in WSL needs PKG_CONFIG_PATH",
    )
    .await
    .unwrap();

    // Query with `*` — without escaping, FTS5 treats `cargo*` as a
    // prefix search. With escaping it's a literal phrase "cargo*"
    // which won't match (the content has "cargo" not "cargo*").
    // We assert NO error + NO false positive.
    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "cargo*",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert!(
        rows.is_empty(),
        "escaped 'cargo*' should not prefix-match; got {} rows",
        rows.len()
    );

    // Query with `NEAR` — without escaping, FTS5 treats it as the
    // proximity operator. With escaping it's a literal phrase.
    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "NEAR",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert!(rows.is_empty(), "escaped 'NEAR' is a literal phrase");

    // `AND` — without escaping FTS5 treats `cargo AND test` as a
    // boolean (both terms present → would match the e1 row). With
    // escaping it's the literal phrase "cargo AND test" (contiguous,
    // in order) which the content does NOT contain → 0 rows. This
    // proves AND is neutralized, not parsed as a boolean operator.
    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "cargo AND test",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert!(
        rows.is_empty(),
        "escaped 'cargo AND test' is a literal phrase, not a boolean; got {} rows",
        rows.len()
    );

    // `OR` — same reasoning. Without escape, `cargo OR nonexistent`
    // would match (cargo present). With escape it's the literal
    // phrase "cargo OR nonexistent" which the content lacks.
    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "cargo OR nonexistent",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert!(
        rows.is_empty(),
        "escaped 'cargo OR nonexistent' is a literal phrase; got {} rows",
        rows.len()
    );

    // `NOT` — without escape, `cargo NOT nonexistent` is a boolean
    // (cargo present AND not-nonexistent → match). With escape it's
    // the literal phrase → no match.
    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "cargo NOT nonexistent",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert!(
        rows.is_empty(),
        "escaped 'cargo NOT nonexistent' is a literal phrase; got {} rows",
        rows.len()
    );

    // `^` (column-prefix anchor) and embedded `"` — must NOT crash
    // and must NOT be parsed as syntax. A query with an embedded
    // double quote exercises the `""` escape path inside escape_fts5.
    // `cargo"test` → escaped to `"cargo""test"` (a valid FTS5 phrase
    // containing a literal quote). Content lacks it → 0 rows, no error.
    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "cargo\"test",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .expect("embedded quote does not crash");
    assert!(
        rows.is_empty(),
        "escaped embedded-quote phrase is literal; got {} rows",
        rows.len()
    );

    // Plain query still works.
    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "cargo",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);

    // Empty / whitespace query → empty result (no syntax error).
    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "   ",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert!(rows.is_empty(), "empty query → empty result");
}

/// scope/project_id interaction (H2) — three semantics:
/// (a) User scope ignores project_id.
/// (b) Project scope + None → Err.
/// (c) None scope searches both layers; project_id required.
#[tokio::test]
async fn search_memories_fts_scope_project_id_interaction() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "u",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "user cargo note",
        "user-scope cargo fact",
    )
    .await
    .unwrap();
    insert_raw(
        &pool,
        "pa",
        MemoryScope::Project,
        Some("proj-a"),
        MemoryKind::Fact,
        MemoryStatus::Active,
        "proj-a cargo note",
        "proj-a cargo fact",
    )
    .await
    .unwrap();
    insert_raw(
        &pool,
        "pb",
        MemoryScope::Project,
        Some("proj-b"),
        MemoryKind::Fact,
        MemoryStatus::Active,
        "proj-b cargo note",
        "proj-b cargo fact",
    )
    .await
    .unwrap();

    // (a) User scope — project_id arg ignored; only the user row.
    let rows = search_memories_fts(
        &pool,
        Some("proj-a"),
        Some(MemoryScope::User),
        "cargo",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "u");

    // (b) Project scope + None → Err.
    let err = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::Project),
        "cargo",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, MemoryInsertError::ProjectScopeMissingId));

    // (b2) Project scope + proj-a → only proj-a's row (proj-b excluded).
    let rows = search_memories_fts(
        &pool,
        Some("proj-a"),
        Some(MemoryScope::Project),
        "cargo",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "pa");

    // (c) None scope + proj-a → user row + proj-a's row (proj-b excluded).
    let rows = search_memories_fts(
        &pool,
        Some("proj-a"),
        None,
        "cargo",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    let ids: Vec<&str> = rows.iter().map(|r| r.memory_id.as_str()).collect();
    assert!(ids.contains(&"u"));
    assert!(ids.contains(&"pa"));
    assert!(!ids.contains(&"pb"), "other project excluded");

    // (c2) None scope + None → Err (project branch of OR needs id).
    let err = search_memories_fts(
        &pool,
        None,
        None,
        "cargo",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, MemoryInsertError::ProjectScopeMissingId));
}

/// Project isolation: a `scope=project` memory in proj-a is NOT
/// surfaced when searching proj-b. (Cross-cutting with the H2 test
/// above, but this one focuses on the isolation invariant.)
#[tokio::test]
async fn search_memories_fts_project_isolation() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "secret-a",
        MemoryScope::Project,
        Some("proj-a"),
        MemoryKind::Fact,
        MemoryStatus::Active,
        "proj-a secret sauce",
        "the proj-a secret sauce is the cargo config",
    )
    .await
    .unwrap();
    // Search proj-b for "cargo" — must NOT see proj-a's row.
    let rows = search_memories_fts(
        &pool,
        Some("proj-b"),
        Some(MemoryScope::Project),
        "cargo",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    assert!(rows.is_empty(), "proj-a memory isolated from proj-b");
    // And the None-scope search from proj-b also excludes proj-a.
    let rows = search_memories_fts(
        &pool,
        Some("proj-b"),
        None,
        "cargo",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.memory_id.as_str()).collect();
    assert!(
        !ids.contains(&"secret-a"),
        "proj-a isolated in None-scope too"
    );
}

/// P2 (2026-06-29, ADR-lite decision): session-start recall passes
/// `IncludeCandidate` so candidate-status memories ARE surfaced.
/// Pre-promotion-mechanism (P5 not landed), P2's `remember` tool
/// writes fixed-candidate — excluding candidate would make every
/// hand-written memory never recallable, breaking the core AC.
/// `ActiveVerifiedOnly` (the original P1 semantics, used by P3
/// pitfall pre-tool recall + P5) excludes candidate.
#[tokio::test]
async fn search_memories_fts_status_filter_candidate_inclusion() {
    let pool = make_pool().await;
    // 3 rows with the same keyword, different statuses.
    insert_raw(
        &pool,
        "c1",
        MemoryScope::User,
        None,
        MemoryKind::Preference,
        MemoryStatus::Candidate,
        "candidate cargo",
        "candidate status cargo note",
    )
    .await
    .unwrap();
    insert_raw(
        &pool,
        "a1",
        MemoryScope::User,
        None,
        MemoryKind::Preference,
        MemoryStatus::Active,
        "active cargo",
        "active status cargo note",
    )
    .await
    .unwrap();
    insert_raw(
        &pool,
        "d1",
        MemoryScope::User,
        None,
        MemoryKind::Preference,
        MemoryStatus::Demoted,
        "demoted cargo",
        "demoted status cargo note",
    )
    .await
    .unwrap();

    // P2 recall path — IncludeCandidate → c1 + a1 surface (d1 demoted
    // always excluded).
    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "cargo",
        10,
        RecallStatusFilter::IncludeCandidate,
    )
    .await
    .unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.memory_id.as_str()).collect();
    assert!(ids.contains(&"c1"), "candidate included in P2 recall");
    assert!(ids.contains(&"a1"), "active included");
    assert!(!ids.contains(&"d1"), "demoted always excluded");
    assert_eq!(rows.len(), 2);

    // P1/P3/P5 path — ActiveVerifiedOnly → only a1 surfaces.
    let rows = search_memories_fts(
        &pool,
        None,
        Some(MemoryScope::User),
        "cargo",
        10,
        RecallStatusFilter::ActiveVerifiedOnly,
    )
    .await
    .unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.memory_id.as_str()).collect();
    assert!(!ids.contains(&"c1"), "candidate excluded in P1/P3/P5 path");
    assert!(ids.contains(&"a1"));
    assert_eq!(rows.len(), 1);
}

/// P2 frequency-control helper: `count_memories_for_session`
/// counts rows by `source_session_id` regardless of status (a
/// demoted row still occupies the per-session ≤50 slot).
#[tokio::test]
async fn count_memories_for_session_counts_across_statuses() {
    let pool = make_pool().await;
    assert_eq!(
        count_memories_for_session(&pool, "sess-empty").await,
        0,
        "unknown session → 0"
    );
    insert_raw(
        &pool,
        "m1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "title one",
        "content one for cargo",
    )
    .await
    .unwrap();
    // The raw insert helper doesn't set source_session_id; use
    // insert_memory so the column is populated.
    let inp = MemoryInput {
        scope: MemoryScope::User,
        project_id: None,
        kind: MemoryKind::Fact,
        status: MemoryStatus::Candidate,
        title: "title two".into(),
        content: "content two for cargo".into(),
        tags: "[]".into(),
        tool_name: None,
        command_pattern: None,
        path_globs: None,
        source_session_id: Some("sess-A".into()),
        source_ref: None,
    };
    insert_memory(&pool, &inp).await.unwrap();
    insert_memory(&pool, &inp).await.unwrap();
    assert_eq!(
        count_memories_for_session(&pool, "sess-A").await,
        2,
        "2 rows from sess-A"
    );
    assert_eq!(
        count_memories_for_session(&pool, "sess-B").await,
        0,
        "sess-B isolated"
    );
}
