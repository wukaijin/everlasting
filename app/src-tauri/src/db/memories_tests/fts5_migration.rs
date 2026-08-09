#![cfg(test)]

use super::make_pool;

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
    assert_eq!(
        rows.len(),
        1,
        "ASCII term 'cargo' in CJK must MATCH via trigram"
    );
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
