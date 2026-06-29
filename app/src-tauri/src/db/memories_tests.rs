#![cfg(test)]

//! autonomous_memories-domain integration tests.
//!
//! Split into a dedicated `*_tests.rs` file (mirrors the 6 existing
//! `db/*_tests.rs` domain files). The `test_pool` helper is copied
//! verbatim — no `common` module (project convention; each domain
//! owns its own pool setup so the test files stay independent).
//!
//! Coverage:
//! - P1 PR1a/PR1b: migration idempotency, FTS5 availability + CJK
//!   trigram tokenizer verification (Open Q#1), FTS trigger sync.
//! - P1 PR2: insert_memory (write safety net) + list + delete +
//!   boundary (empty / over-length / unique / enum check reject).
//! - P1 PR3: search_memories_fts (bm25 + escape + scope semantics),
//!   find_pitfalls_by_trigger (tool_name exact match),
//!   bump_hit_count + update_status (transactional state machine),
//!   project isolation, EXPLAIN QUERY PLAN index coverage.

use sqlx::SqlitePool;

use super::memories::{
    bump_hit_count, count_memories_for_session, delete_memory, find_pitfalls_by_trigger,
    get_memory_by_id, insert_memory, list_memories, promote_if_eligible, search_memories_fts,
    update_status,
    MemoryInput, MemoryInsertError, MemoryKind, MemoryScope, MemoryStatus, RecallStatusFilter,
    StatusTransitionError,
    test_helpers::insert_raw,
    ACTIVE_TO_VERIFIED_AGE_DAYS, ACTIVE_TO_VERIFIED_AT, CANDIDATE_TO_ACTIVE_AT,
};

/// In-memory pool with migrations + FK pragma. Mirrors the
/// `test_pool` in every other `db/*_tests.rs` file (project
/// convention: no shared `common` module; each domain copies the
/// helper so test files stay independent).
async fn test_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    crate::db::migrations::run_migrations(&pool)
        .await
        .unwrap();
    pool
}

async fn make_pool() -> SqlitePool {
    test_pool().await // alias for readability inside this section
}

// ---------------------------------------------------------------------------
// P1 PR1a/PR1b: migration + FTS5 verification (Open Q#1)
// ---------------------------------------------------------------------------

/// Open Q#1 (blocking first step): FTS5 must be compiled into the
/// linked SQLite, and the `trigram` tokenizer must be available.
/// This test is the empirical verification that the system SQLite
/// (sqlx is non-bundled, links system libsqlite3) supports FTS5 +
/// trigram. If this test fails on a new machine, FTS5 is not
/// compiled in and the project must either enable sqlx's
/// `bundled-sqlite` feature (compile-time sqlite with FTS5) or
/// fall back to `content LIKE '%kw%'` (the documented escape
/// hatch in prd §Open Q#1).
#[tokio::test]
async fn fts5_trigram_tokenizer_is_available_for_cjk() {
    let pool = make_pool().await;
    // The migration already created `autonomous_memories_fts` with
    // tokenize='trigram'. Insert a row mixing CJK + ASCII and verify
    // both kinds of terms MATCH.
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO autonomous_memories
        (memory_id, scope, project_id, kind, status, title, content, tags,
         tool_name, command_pattern, path_globs, source_session_id, source_ref,
         confidence, hit_count, last_used_at, created_at, updated_at, demoted_reason)
        VALUES
        ('m1','user',NULL,'pitfall','active',
         'WSL下跑cargo会权限不足',
         '在WSL中跑cargo test会因为系统库路径找不到而失败',
         '["wsl","cargo"]',
         'shell','cargo test',NULL,NULL,NULL,
         0.5,0,NULL,?,?,NULL)
        "#,
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .unwrap();
    // ASCII term embedded in CJK run — trigram must MATCH.
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT m.id, m.title FROM autonomous_memories_fts f \
         JOIN autonomous_memories m ON m.id = f.rowid \
         WHERE autonomous_memories_fts MATCH ? ORDER BY bm25(autonomous_memories_fts)",
    )
    .bind("\"cargo\"")
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "ASCII term 'cargo' in CJK must MATCH via trigram");
    // 3+ char CJK term — trigram must MATCH.
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT m.id, m.title FROM autonomous_memories_fts f \
         JOIN autonomous_memories m ON m.id = f.rowid \
         WHERE autonomous_memories_fts MATCH ? ORDER BY bm25(autonomous_memories_fts)",
    )
    .bind("\"权限不足\"")
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "3+ char CJK term '权限不足' must MATCH via trigram"
    );
}

/// Migration idempotency: re-running `run_migrations` on an
/// already-migrated DB is a no-op (CREATE TABLE/INDEX/VIRTUAL
/// TABLE/TRIGGER all use IF NOT EXISTS). Re-running must NOT error
/// and must NOT drop existing rows.
#[tokio::test]
async fn am_migration_is_idempotent() {
    let pool = make_pool().await;
    // Insert one row so we can verify re-migration doesn't wipe it.
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        r#"INSERT INTO autonomous_memories
           (memory_id, scope, project_id, kind, status, title, content, tags,
            created_at, updated_at)
           VALUES ('m-idem','user',NULL,'fact','active','t','c','[]',?,?)"#,
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .unwrap();
    crate::db::migrations::run_migrations(&pool)
        .await
        .expect("re-run is idempotent");
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM autonomous_memories WHERE memory_id = 'm-idem'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "row survives the idempotent re-run");
    // The FTS trigger must still be wired after re-migration.
    // Use a trigram-friendly query (≥3 chars). The insert above set
    // content='c' (single char) — that's NOT trigram-searchable; we
    // re-insert a second row with a multi-char content and verify
    // it's FTS-reachable.
    let now2 = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        r#"INSERT INTO autonomous_memories
           (memory_id, scope, project_id, kind, status, title, content, tags,
            created_at, updated_at)
           VALUES ('m-idem-fts','user',NULL,'fact','active','title here',
                   ' searchable content ', '[]', ?, ?)"#,
    )
    .bind(&now2)
    .bind(&now2)
    .execute(&pool)
    .await
    .unwrap();
    let fts_hit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM autonomous_memories_fts \
         WHERE autonomous_memories_fts MATCH '\"searchable\"'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(fts_hit, 1, "FTS index intact after re-run");
}

/// DB-side CHECK constraints reject out-of-range enum values and
/// over-length title/content. This is the DB-level guard; the
/// Rust enum + write safety net are the application-level guard.
/// Both layers must agree (PRD AC §boundary).
#[tokio::test]
async fn am_db_check_rejects_invalid_enum_and_oversize() {
    let pool = make_pool().await;
    let now = chrono::Utc::now().to_rfc3339();
    // Invalid scope.
    let r = sqlx::query(
        r#"INSERT INTO autonomous_memories
           (memory_id, scope, kind, status, title, content, created_at, updated_at)
           VALUES ('x1','galactic','fact','active','t','c',?,?)"#,
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(r.is_err(), "invalid scope rejected by CHECK");
    // Invalid kind.
    let r = sqlx::query(
        r#"INSERT INTO autonomous_memories
           (memory_id, scope, kind, status, title, content, created_at, updated_at)
           VALUES ('x2','user','magic','active','t','c',?,?)"#,
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(r.is_err(), "invalid kind rejected by CHECK");
    // Invalid status.
    let r = sqlx::query(
        r#"INSERT INTO autonomous_memories
           (memory_id, scope, kind, status, title, content, created_at, updated_at)
           VALUES ('x3','user','fact','frozen','t','c',?,?)"#,
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(r.is_err(), "invalid status rejected by CHECK");
    // Over-length title (201 chars).
    let long_title = "t".repeat(201);
    let r = sqlx::query(
        r#"INSERT INTO autonomous_memories
           (memory_id, scope, kind, status, title, content, created_at, updated_at)
           VALUES ('x4','user','fact','active',?,'c',?,?)"#,
    )
    .bind(&long_title)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(r.is_err(), "title > 200 rejected by CHECK");
    // Over-length content (501 chars).
    let long_content = "c".repeat(501);
    let r = sqlx::query(
        r#"INSERT INTO autonomous_memories
           (memory_id, scope, kind, status, title, content, created_at, updated_at)
           VALUES ('x5','user','fact','active','t',?,?,?)"#,
    )
    .bind(&long_content)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(r.is_err(), "content > 500 rejected by CHECK");
    // memory_id UNIQUE conflict.
    sqlx::query(
        r#"INSERT INTO autonomous_memories
           (memory_id, scope, kind, status, title, content, created_at, updated_at)
           VALUES ('dup','user','fact','active','t','c',?,?)"#,
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .unwrap();
    let r = sqlx::query(
        r#"INSERT INTO autonomous_memories
           (memory_id, scope, kind, status, title, content, created_at, updated_at)
           VALUES ('dup','user','fact','active','t2','c2',?,?)"#,
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(r.is_err(), "duplicate memory_id rejected by UNIQUE");
}

/// SQLite PRAGMA status check (Open Q#4). Project does NOT set
/// `journal_mode` or `busy_timeout` anywhere in `init_pool` —
/// defaults apply. PRD decision: WAL is out of scope for P1 (it
/// affects the entire DB layer); record the current state here so
/// a future task can pick it up. `update_status` uses a transaction
/// (P1 PR3) which is sufficient for the current single-writer
/// access pattern.
#[tokio::test]
async fn am_pragma_status_recorded_for_open_q4() {
    let pool = make_pool().await;
    let jm: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .unwrap();
    // In-memory DB reports 'memory' journal mode; file-backed
    // reports 'delete' (rollback journal) by default since the
    // project doesn't enable WAL. Either is acceptable for P1.
    assert!(
        matches!(jm.as_str(), "memory" | "delete" | "wal"),
        "journal_mode is {} (no WAL toggle in init_pool; expected)",
        jm
    );
    let bt: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    // Default is 5000ms in modern SQLite; just record, don't assert.
    let _ = bt;
}

// ---------------------------------------------------------------------------
// P1 PR2: insert_memory + write safety net + list + delete
// ---------------------------------------------------------------------------

/// Helper: build a minimal valid `MemoryInput` with the caller's
/// overrides. Keeps the per-test boilerplate down.
fn input<'a>(
    scope: MemoryScope,
    kind: MemoryKind,
    title: &'a str,
    content: &'a str,
) -> MemoryInput {
    MemoryInput {
        scope,
        project_id: None,
        kind,
        status: MemoryStatus::Candidate,
        title: title.to_string(),
        content: content.to_string(),
        tags: "[]".to_string(),
        tool_name: None,
        command_pattern: None,
        path_globs: None,
        source_session_id: None,
        source_ref: None,
    }
}

/// Happy-path roundtrip: insert → read back → assert every column
/// landed correctly. The auto-generated `memory_id` is a UUID v7
/// (time-ordered, RFC 9562) — we assert it's a valid UUID shape.
#[tokio::test]
async fn insert_memory_happy_path_roundtrip() {
    let pool = make_pool().await;
    let mut inp = input(
        MemoryScope::Project,
        MemoryKind::Pitfall,
        "WSL cargo test fails",
        "Run with PKG_CONFIG_PATH set, or it can't find gdk-pixbuf",
    );
    inp.project_id = Some("proj-1".to_string());
    inp.status = MemoryStatus::Active;
    inp.tool_name = Some("shell".to_string());
    inp.command_pattern = Some("cargo test".to_string());
    inp.path_globs = Some(r#"["app/src-tauri/*"]"#.to_string());
    inp.tags = r#"["wsl","cargo"]"#.to_string();
    inp.source_session_id = Some("sess-1".to_string());
    inp.source_ref = Some("turn-3".to_string());

    let row = insert_memory(&pool, &inp).await.expect("insert ok");
    assert!(row.id > 0, "auto-id assigned");
    assert!(uuid::Uuid::parse_str(&row.memory_id).is_ok(), "memory_id is a UUID");
    assert_eq!(row.scope, "project");
    assert_eq!(row.project_id.as_deref(), Some("proj-1"));
    assert_eq!(row.kind, "pitfall");
    assert_eq!(row.status, "active");
    assert_eq!(row.title, "WSL cargo test fails");
    assert!(row.content.starts_with("Run with PKG_CONFIG_PATH"));
    assert_eq!(row.tags, r#"["wsl","cargo"]"#);
    assert_eq!(row.tool_name.as_deref(), Some("shell"));
    assert_eq!(row.command_pattern.as_deref(), Some("cargo test"));
    assert_eq!(row.path_globs.as_deref(), Some(r#"["app/src-tauri/*"]"#));
    assert_eq!(row.source_session_id.as_deref(), Some("sess-1"));
    assert_eq!(row.source_ref.as_deref(), Some("turn-3"));
    // forward-compat defaults.
    assert_eq!(row.confidence, 0.5);
    assert_eq!(row.hit_count, 0);
    assert!(row.last_used_at.is_none());
    assert!(row.demoted_reason.is_none());
    assert!(!row.created_at.is_empty());
    assert_eq!(row.created_at, row.updated_at, "fresh row: equal ts");
}

/// scope=User with a project_id is rejected (H2: User scope is
/// global to the user, not project-bound).
#[tokio::test]
async fn insert_memory_user_scope_with_project_id_is_rejected() {
    let pool = make_pool().await;
    let mut inp = input(MemoryScope::User, MemoryKind::Fact, "t", "c");
    inp.project_id = Some("proj-x".to_string());
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::UserScopeHasProjectId(_)));
}

/// scope=Project without a project_id is rejected (H2).
#[tokio::test]
async fn insert_memory_project_scope_without_id_is_rejected() {
    let pool = make_pool().await;
    let inp = input(MemoryScope::Project, MemoryKind::Fact, "t", "c");
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::ProjectScopeMissingId));
}

/// Empty title / empty content are rejected (2.2 / B1).
#[tokio::test]
async fn insert_memory_rejects_empty_title_and_content() {
    let pool = make_pool().await;
    // Empty title.
    let inp = input(MemoryScope::User, MemoryKind::Fact, "   ", "content");
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::EmptyTitle), "empty title");
    // Empty content.
    let inp = input(MemoryScope::User, MemoryKind::Fact, "title", "");
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::EmptyContent), "empty content");
}

/// Over-length title / content are rejected by the safety net
/// BEFORE hitting the DB (the error message is actionable; the
/// DB CHECK is the backstop).
#[tokio::test]
async fn insert_memory_rejects_oversize_title_and_content() {
    let pool = make_pool().await;
    let long_title: String = "t".repeat(201);
    let inp = input(MemoryScope::User, MemoryKind::Fact, &long_title, "c");
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::TitleTooLong(201)));
    let long_content: String = "c".repeat(501);
    let inp = input(MemoryScope::User, MemoryKind::Fact, "t", &long_content);
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::ContentTooLong(501)));
}

/// Sensitive-content regex (api_key/secret/password/token/bearer)
/// is rejected in BOTH title and content.
#[tokio::test]
async fn insert_memory_rejects_sensitive_content() {
    let pool = make_pool().await;
    // api_key in title.
    let inp = input(
        MemoryScope::User,
        MemoryKind::Fact,
        "the ANTHROPIC_API_KEY is sk-...",
        "regular content here",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::SensitiveContent), "api_key in title");
    // password in content.
    let inp = input(
        MemoryScope::User,
        MemoryKind::Fact,
        "regular title",
        "the database password is hunter2",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::SensitiveContent), "password in content");
    // bearer token in content.
    let inp = input(
        MemoryScope::User,
        MemoryKind::Fact,
        "regular title",
        "Authorization: bearer xyz",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::SensitiveContent), "bearer in content");
    // token= (query-param form) in content.
    let inp = input(
        MemoryScope::User,
        MemoryKind::Fact,
        "regular title",
        "url with token=abc123",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::SensitiveContent), "token= in content");
}

/// Sensitive-path components (.ssh / .aws / .gnupg / credentials /
/// id_rsa) are rejected in any path-like field.
#[tokio::test]
async fn insert_memory_rejects_sensitive_path_components() {
    let pool = make_pool().await;
    // .ssh in content.
    let inp = input(
        MemoryScope::User,
        MemoryKind::Pitfall,
        "key location",
        "the key is in /home/user/.ssh/id_ed25519",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::SensitivePath(_)), ".ssh denied");
    // .aws in path_globs (the JSON string carries the component).
    let mut inp = input(
        MemoryScope::User,
        MemoryKind::Pitfall,
        "aws creds",
        "be careful with the credentials file",
    );
    inp.path_globs = Some(r#"["/home/user/.aws/*"]"#.to_string());
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(
        matches!(err, MemoryInsertError::SensitivePath(_)),
        ".aws in path_globs denied"
    );
}

/// Temporary-path prefixes (/tmp/ / /var/log/) are rejected —
/// they're ephemeral and a memory referencing them is useless.
#[tokio::test]
async fn insert_memory_rejects_temporary_paths() {
    let pool = make_pool().await;
    let inp = input(
        MemoryScope::User,
        MemoryKind::Pitfall,
        "temp file",
        "the build output is in /tmp/build.log",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::TemporaryPath(_)));
}

/// `/home/<user>/` paths in content are generalized to `~/` so the
/// stored memory is username-agnostic (the spike-005 §4 leak rule).
#[tokio::test]
async fn insert_memory_generalizes_home_path() {
    let pool = make_pool().await;
    let inp = input(
        MemoryScope::User,
        MemoryKind::Fact,
        "the project lives at /home/alice/code/everlasting",
        "the config is at /home/alice/.config/everlasting/",
    );
    let row = insert_memory(&pool, &inp).await.expect("insert ok");
    assert!(
        !row.title.contains("/home/alice"),
        "title generalized: {}",
        row.title
    );
    assert!(row.title.contains("~/code/everlasting"), "title has ~/ prefix");
    assert!(
        !row.content.contains("/home/alice"),
        "content generalized: {}",
        row.content
    );
    assert!(
        row.content.contains("~/.config/everlasting/"),
        "content has ~/.config prefix"
    );
}

/// `list_memories` filters by scope correctly:
/// - User scope → only user rows (project rows excluded even if
///   they share the project_id arg).
/// - Project scope + id → only that project's rows.
/// - None → all rows.
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

    // None scope — all 3 rows.
    let rows = list_memories(&pool, None, None).await.unwrap();
    assert_eq!(rows.len(), 3);
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

    let rows = search_memories_fts(&pool, None, Some(MemoryScope::User), "cargo", 10, RecallStatusFilter::ActiveVerifiedOnly)
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
    let rows = search_memories_fts(&pool, None, Some(MemoryScope::User), "cargo*", 10, RecallStatusFilter::ActiveVerifiedOnly)
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "escaped 'cargo*' should not prefix-match; got {} rows",
        rows.len()
    );

    // Query with `NEAR` — without escaping, FTS5 treats it as the
    // proximity operator. With escaping it's a literal phrase.
    let rows = search_memories_fts(&pool, None, Some(MemoryScope::User), "NEAR", 10, RecallStatusFilter::ActiveVerifiedOnly)
        .await
        .unwrap();
    assert!(rows.is_empty(), "escaped 'NEAR' is a literal phrase");

    // `AND` — without escaping FTS5 treats `cargo AND test` as a
    // boolean (both terms present → would match the e1 row). With
    // escaping it's the literal phrase "cargo AND test" (contiguous,
    // in order) which the content does NOT contain → 0 rows. This
    // proves AND is neutralized, not parsed as a boolean operator.
    let rows = search_memories_fts(&pool, None, Some(MemoryScope::User), "cargo AND test", 10, RecallStatusFilter::ActiveVerifiedOnly)
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
    let rows = search_memories_fts(&pool, None, Some(MemoryScope::User), "cargo\"test", 10, RecallStatusFilter::ActiveVerifiedOnly)
        .await
        .expect("embedded quote does not crash");
    assert!(
        rows.is_empty(),
        "escaped embedded-quote phrase is literal; got {} rows",
        rows.len()
    );

    // Plain query still works.
    let rows = search_memories_fts(&pool, None, Some(MemoryScope::User), "cargo", 10, RecallStatusFilter::ActiveVerifiedOnly)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    // Empty / whitespace query → empty result (no syntax error).
    let rows = search_memories_fts(&pool, None, Some(MemoryScope::User), "   ", 10, RecallStatusFilter::ActiveVerifiedOnly)
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
    let rows = search_memories_fts(&pool, Some("proj-a"), Some(MemoryScope::User), "cargo", 10, RecallStatusFilter::ActiveVerifiedOnly)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "u");

    // (b) Project scope + None → Err.
    let err = search_memories_fts(&pool, None, Some(MemoryScope::Project), "cargo", 10, RecallStatusFilter::ActiveVerifiedOnly)
        .await
        .unwrap_err();
    assert!(matches!(err, MemoryInsertError::ProjectScopeMissingId));

    // (b2) Project scope + proj-a → only proj-a's row (proj-b excluded).
    let rows = search_memories_fts(&pool, Some("proj-a"), Some(MemoryScope::Project), "cargo", 10, RecallStatusFilter::ActiveVerifiedOnly)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "pa");

    // (c) None scope + proj-a → user row + proj-a's row (proj-b excluded).
    let rows = search_memories_fts(&pool, Some("proj-a"), None, "cargo", 10, RecallStatusFilter::ActiveVerifiedOnly)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let ids: Vec<&str> = rows.iter().map(|r| r.memory_id.as_str()).collect();
    assert!(ids.contains(&"u"));
    assert!(ids.contains(&"pa"));
    assert!(!ids.contains(&"pb"), "other project excluded");

    // (c2) None scope + None → Err (project branch of OR needs id).
    let err = search_memories_fts(&pool, None, None, "cargo", 10, RecallStatusFilter::ActiveVerifiedOnly)
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
    let rows = search_memories_fts(&pool, Some("proj-b"), Some(MemoryScope::Project), "cargo", 10, RecallStatusFilter::ActiveVerifiedOnly)
        .await
        .unwrap();
    assert!(rows.is_empty(), "proj-a memory isolated from proj-b");
    // And the None-scope search from proj-b also excludes proj-a.
    let rows = search_memories_fts(&pool, Some("proj-b"), None, "cargo", 10, RecallStatusFilter::ActiveVerifiedOnly)
        .await
        .unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.memory_id.as_str()).collect();
    assert!(!ids.contains(&"secret-a"), "proj-a isolated in None-scope too");
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

/// `find_pitfalls_by_trigger` matches pitfall memories by `tool_name`
/// exact equality (indexed by `idx_am_pitfall`). Other tool_names
/// and non-pitfall kinds are NOT matched.
#[tokio::test]
async fn find_pitfalls_by_trigger_tool_name_exact_match() {
    let pool = make_pool().await;
    // Pitfall for `shell` tool, path-agnostic (path_globs=NULL).
    insert_raw(
        &pool,
        "pit-shell",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "cargo test needs PKG_CONFIG_PATH",
        "run with PKG_CONFIG_PATH set",
    )
    .await
    .unwrap();
    // Set the trigger key columns via a direct UPDATE (insert_raw
    // doesn't set them; production uses insert_memory).
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pit-shell'")
        .execute(&pool)
        .await
        .unwrap();
    // A non-pitfall (preference) with the same tool_name — excluded.
    insert_raw(
        &pool,
        "pref-shell",
        MemoryScope::User,
        None,
        MemoryKind::Preference,
        MemoryStatus::Active,
        "user prefers shell",
        "always use shell tool",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pref-shell'")
        .execute(&pool)
        .await
        .unwrap();
    // A pitfall for a DIFFERENT tool — excluded.
    insert_raw(
        &pool,
        "pit-edit",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "edit_file pitfall",
        "be careful with edit_file",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='edit_file' WHERE memory_id='pit-edit'")
        .execute(&pool)
        .await
        .unwrap();

    // Probe for `shell` → only pit-shell (preference excluded despite same tool_name).
    let rows = find_pitfalls_by_trigger(&pool, "shell", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "pit-shell");
    // Probe for `edit_file` → only pit-edit.
    let rows = find_pitfalls_by_trigger(&pool, "edit_file", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "pit-edit");
    // Probe for an unknown tool → empty.
    let rows = find_pitfalls_by_trigger(&pool, "grep", None, None)
        .await
        .unwrap();
    assert!(rows.is_empty());
}

/// `path_globs=NULL` means the pitfall is path-agnostic (fires for
/// ANY path). `path_globs=Some(globs)` requires the caller-supplied
/// path to match at least one glob (M2).
#[tokio::test]
async fn find_pitfalls_path_globs_semantics() {
    let pool = make_pool().await;
    // Path-agnostic pitfall (path_globs=NULL) — fires for any path.
    insert_raw(
        &pool,
        "pit-any",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "any path",
        "fires for any path",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pit-any'")
        .execute(&pool)
        .await
        .unwrap();
    // Path-bound pitfall (path_globs=["app/src-tauri/*"]).
    // `session_tool_permissions`-style glob: `*` does NOT cross `/`
    // (NOT native SQLite GLOB — native SQLite `'a/b' GLOB 'a*'` would
    // match; see the `glob_matches_path` doc comment for the
    // empirical verification). So `app/src-tauri/*` matches
    // `app/src-tauri/<single-segment>` but NOT
    // `app/src-tauri/src/lib.rs` (the `*` would have to cross the
    // `/` between `src` and `lib.rs`). spike-007 re-grill explicitly
    // standardized on no `**` recursion.
    insert_raw(
        &pool,
        "pit-bound",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "bound path",
        "only fires for app/src-tauri",
    )
    .await
    .unwrap();
    sqlx::query(
        r#"UPDATE autonomous_memories
           SET tool_name='shell', path_globs='["app/src-tauri/*"]'
           WHERE memory_id='pit-bound'"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // Probe with NO path: path-agnostic fires; path-bound does NOT
    // (can't confirm the glob match; precision-first).
    let rows = find_pitfalls_by_trigger(&pool, "shell", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "only path-agnostic fires without a path");
    assert_eq!(rows[0].memory_id, "pit-any");

    // Probe with a MATCHING path (single-segment after the prefix):
    // both fire.
    let rows = find_pitfalls_by_trigger(
        &pool,
        "shell",
        None,
        Some("app/src-tauri/Cargo.toml"),
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "both fire when path matches the glob");

    // Probe with a NON-MATCHING path: only path-agnostic fires.
    // `app/src-tauri/src/lib.rs` does NOT match `app/src-tauri/*`
    // (`session_tool_permissions`-style glob: `*` doesn't cross `/`).
    let rows = find_pitfalls_by_trigger(
        &pool,
        "shell",
        None,
        Some("app/src-tauri/src/lib.rs"),
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "deep path doesn't match single-segment glob");
    assert_eq!(rows[0].memory_id, "pit-any");

    // Probe with a totally unrelated path: only path-agnostic fires.
    let rows = find_pitfalls_by_trigger(
        &pool,
        "shell",
        None,
        Some("/home/user/some-other-dir/foo"),
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "unrelated path → only path-agnostic");
    assert_eq!(rows[0].memory_id, "pit-any");
}

/// `command_pattern` is a secondary substring filter — a pitfall
/// fires only if the caller-supplied command contains the stored
/// pattern.
#[tokio::test]
async fn find_pitfalls_command_pattern_substring_filter() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "pit-cp",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "cargo test pattern",
        "the command pattern is cargo test",
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE autonomous_memories SET tool_name='shell', command_pattern='cargo test' \
         WHERE memory_id='pit-cp'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Matching command — fires.
    let rows = find_pitfalls_by_trigger(
        &pool,
        "shell",
        Some("cargo test --lib"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);

    // Non-matching command — does NOT fire.
    let rows = find_pitfalls_by_trigger(&pool, "shell", Some("cargo build"), None)
        .await
        .unwrap();
    assert!(rows.is_empty(), "non-matching command_pattern filtered");
}

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
    bump_hit_count(&pool, "unknown").await.expect("no error on unknown id");
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
    sqlx::query("UPDATE autonomous_memories SET content='now about rustc instead' WHERE memory_id='tr-1'")
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

// ---------------------------------------------------------------------------
// P5 (2026-06-29, 06-29-am-p5-quality): promote_if_eligible
// state-machine auto-promotion (design D2 + §5)
// ---------------------------------------------------------------------------

/// Insert a row with an explicit `created_at` (so the age gate for
/// active→verified can be tested without waiting 3 real days).
/// `insert_raw` always stamps `now`; this helper re-stamps afterwards.
async fn reseat_created_at(pool: &SqlitePool, memory_id: &str, days_ago: i64) {
    let ts = (chrono::Utc::now() - chrono::Duration::days(days_ago)).to_rfc3339();
    sqlx::query("UPDATE autonomous_memories SET created_at = ? WHERE memory_id = ?")
        .bind(&ts)
        .bind(memory_id)
        .execute(pool)
        .await
        .unwrap();
}

/// `promote_if_eligible` promotes a candidate to active when
/// `hit_count` crosses `CANDIDATE_TO_ACTIVE_AT` (D2: 2).
#[tokio::test]
async fn p5_promote_candidate_to_active_at_threshold() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "p5-1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Candidate,
        "title",
        "content",
    )
    .await
    .unwrap();
    // hit_count=1 → below threshold, stays candidate.
    bump_hit_count(&pool, "p5-1").await.unwrap();
    assert_eq!(
        get_memory_by_id(&pool, "p5-1").await.unwrap().unwrap().status,
        "candidate"
    );
    // hit_count=2 → crosses CANDIDATE_TO_ACTIVE_AT → promoted.
    bump_hit_count(&pool, "p5-1").await.unwrap();
    let row = get_memory_by_id(&pool, "p5-1").await.unwrap().unwrap();
    assert_eq!(row.status, "active");
    assert_eq!(row.hit_count, CANDIDATE_TO_ACTIVE_AT);
}

/// Active → verified requires BOTH the hit threshold AND the age
/// gate. Below either, stays active.
#[tokio::test]
async fn p5_promote_active_to_verified_needs_hits_and_age() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "p5-2",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "title",
        "content",
    )
    .await
    .unwrap();

    // Freshly created (age=0). Bump to ACTIVE_TO_VERIFIED_AT — but
    // age gate not met → stays active.
    for _ in 0..ACTIVE_TO_VERIFIED_AT {
        bump_hit_count(&pool, "p5-2").await.unwrap();
    }
    let row = get_memory_by_id(&pool, "p5-2").await.unwrap().unwrap();
    assert_eq!(row.status, "active", "age gate not met → stays active");
    assert_eq!(row.hit_count, ACTIVE_TO_VERIFIED_AT);

    // Now pretend it was created 3+ days ago. Next bump (hit=6)
    // re-checks + promotes.
    reseat_created_at(&pool, "p5-2", ACTIVE_TO_VERIFIED_AGE_DAYS).await;
    bump_hit_count(&pool, "p5-2").await.unwrap();
    let row = get_memory_by_id(&pool, "p5-2").await.unwrap().unwrap();
    assert_eq!(row.status, "verified");
    assert!(row.hit_count > ACTIVE_TO_VERIFIED_AT);

    // Symmetric control: an active memory at the hit threshold but
    // 1 day short of the age gate stays active.
    insert_raw(
        &pool,
        "p5-2b",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "title",
        "content",
    )
    .await
    .unwrap();
    for _ in 0..ACTIVE_TO_VERIFIED_AT {
        bump_hit_count(&pool, "p5-2b").await.unwrap();
    }
    reseat_created_at(&pool, "p5-2b", ACTIVE_TO_VERIFIED_AGE_DAYS - 1).await;
    bump_hit_count(&pool, "p5-2b").await.unwrap();
    assert_eq!(
        get_memory_by_id(&pool, "p5-2b").await.unwrap().unwrap().status,
        "active",
        "1 day short of age gate → stays active"
    );
}

/// A `demoted` row is never re-promoted by `promote_if_eligible` —
/// re-promotion is the hygiene job's job (design §5).
#[tokio::test]
async fn p5_promote_skips_demoted_rows() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "p5-3",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Demoted,
        "title",
        "content",
    )
    .await
    .unwrap();
    // Bump many times — should NOT promote (matrix + the D2 match
    // arm only fires on Candidate/Active).
    for _ in 0..(ACTIVE_TO_VERIFIED_AT + 2) {
        bump_hit_count(&pool, "p5-3").await.unwrap();
    }
    assert_eq!(
        get_memory_by_id(&pool, "p5-3").await.unwrap().unwrap().status,
        "demoted",
        "demoted rows stay demoted through bump"
    );
}

/// `promote_if_eligible` is a no-op on an unknown memory_id
/// (NotFound is benign — row deleted between bump + promote).
#[tokio::test]
async fn p5_promote_unknown_id_is_noop() {
    let pool = make_pool().await;
    promote_if_eligible(&pool, "does-not-exist")
        .await
        .expect("unknown id → Ok(()) benign no-op");
}

/// A verified row stays verified through further bumps (no
/// demotion via bump — demotion is the hygiene job's job).
#[tokio::test]
async fn p5_promote_verified_stays_verified() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "p5-5",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Verified,
        "title",
        "content",
    )
    .await
    .unwrap();
    bump_hit_count(&pool, "p5-5").await.unwrap();
    bump_hit_count(&pool, "p5-5").await.unwrap();
    assert_eq!(
        get_memory_by_id(&pool, "p5-5").await.unwrap().unwrap().status,
        "verified"
    );
}

/// A candidate below the active threshold stays candidate through
/// bumps (hit=1 < CANDIDATE_TO_ACTIVE_AT).
#[tokio::test]
async fn p5_promote_candidate_below_threshold_stays_candidate() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "p5-6",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Candidate,
        "title",
        "content",
    )
    .await
    .unwrap();
    // Single bump → hit=1 < CANDIDATE_TO_ACTIVE_AT(=2) → no promote.
    bump_hit_count(&pool, "p5-6").await.unwrap();
    assert_eq!(
        get_memory_by_id(&pool, "p5-6").await.unwrap().unwrap().status,
        "candidate"
    );
}
