#![cfg(test)]

use sqlx::{Row, SqlitePool};

use crate::db::migrations::*;

/// Helper: open a pool against a tempfile path, return (pool, path)
/// so the test can assert pragmas then let both drop.
async fn fresh_pool() -> (SqlitePool, tempfile::TempPath) {
    let file = tempfile::NamedTempFile::new().expect("create tempfile");
    let (file, path) = file.into_parts();
    let pool = init_pool(&path).await.expect("init_pool");
    // Keep the TempPath alive (drops the file on test end); the
    // NamedTempFile's file handle is discarded — sqlite opens its
    // own handle via the path.
    drop(file);
    (pool, path)
}

/// P2.4 D8: `init_pool` must set `journal_mode = WAL` so concurrent
/// readers don't block a writer (eliminates SQLITE_BUSY in the
/// dual-process daemon scenario). The pragma is set per-connection
/// via `SqliteConnectOptions`, so we verify it on an acquired
/// connection.
#[tokio::test]
async fn init_pool_sets_wal_journal_mode() {
    let (pool, _path) = fresh_pool().await;
    let row = sqlx::query("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .expect("query journal_mode");
    let mode: String = row.try_get::<String, _>(0).expect("get journal_mode");
    // SQLite returns "wal" (lowercase) for WAL mode.
    assert_eq!(
        mode.to_lowercase(),
        "wal",
        "init_pool must set journal_mode=WAL (got {mode})"
    );
}

/// P2.4 D8: `init_pool` must set `busy_timeout` so transient lock
/// contention waits instead of immediately returning SQLITE_BUSY.
/// 5000ms is the configured value.
#[tokio::test]
async fn init_pool_sets_busy_timeout() {
    let (pool, _path) = fresh_pool().await;
    let row = sqlx::query("PRAGMA busy_timeout")
        .fetch_one(&pool)
        .await
        .expect("query busy_timeout");
    let timeout: i64 = row.try_get::<i64, _>(0).expect("get busy_timeout");
    assert_eq!(
        timeout, 5000,
        "init_pool must set busy_timeout=5000ms (got {timeout})"
    );
}

/// P2.4 D8: foreign_keys ON is preserved (the pre-P2.4 behavior).
#[tokio::test]
async fn init_pool_sets_foreign_keys_on() {
    let (pool, _path) = fresh_pool().await;
    let row = sqlx::query("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .expect("query foreign_keys");
    let fk: i64 = row.try_get::<i64, _>(0).expect("get foreign_keys");
    assert_eq!(fk, 1, "init_pool must set foreign_keys=ON");
}

/// P2.4 D8: concurrent reads while a write is in-flight must NOT
/// return SQLITE_BUSY under WAL. Two pools on the same file
/// simulates the dual-process scenario (daemon + a second reader).
///
/// Uses `pool.begin()` (NOT raw `BEGIN`) so the transaction is bound
/// to a single pooled connection — raw `BEGIN`/`COMMIT` via
/// `.execute(&pool)` would run on different connections and the txn
/// state wouldn't carry. This is the core dual-process guarantee: a
/// browser reader hitting the daemon's DB while the agent loop is
/// mid-write must not see a busy error.
#[tokio::test]
async fn concurrent_read_during_write_under_wal() {
    let file = tempfile::NamedTempFile::new().expect("create tempfile");
    let (_file, path) = file.into_parts();
    // Two independent pools on the same file (daemon + reader).
    let writer = init_pool(&path).await.expect("writer pool");
    let reader = init_pool(&path).await.expect("reader pool");
    // Set up a table + one row so the reader has something to read.
    sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY)")
        .execute(&writer)
        .await
        .expect("create table");
    sqlx::query("INSERT INTO t (id) VALUES (1)")
        .execute(&writer)
        .await
        .expect("insert");

    // Begin a write transaction bound to ONE writer connection
    // (holds the reserved lock). Held open across the read below.
    let mut txn = writer.begin().await.expect("begin write txn");
    sqlx::query("INSERT INTO t (id) VALUES (2)")
        .execute(&mut *txn)
        .await
        .expect("insert in txn");

    // Concurrent read from the OTHER pool — must NOT error (no
    // SQLITE_BUSY). Under WAL the reader sees the last committed
    // snapshot regardless of the in-flight write.
    let read_result = sqlx::query("SELECT COUNT(*) FROM t")
        .fetch_one(&reader)
        .await;
    assert!(
        read_result.is_ok(),
        "concurrent read under WAL must not return SQLITE_BUSY; got: {:?}",
        read_result.err()
    );
    txn.commit().await.expect("commit");
}

/// 08-31-sched-per-run-session:旧形表(F2b 时代,target_session_id
/// NOT NULL)经 `rebuild_scheduled_tasks_for_target_mode` 重建 —— 数据
/// 保全 + 新列种子(target_mode='fixed';model_id / last_run_session_id
/// NULL)+ 幂等(重跑 no-op)。旧形表手工搭:drop 新形表后按旧 DDL 建,
/// 行内引用的 project/session 由种子保证 FK 成立。
#[tokio::test]
async fn rebuild_scheduled_tasks_preserves_rows_and_seeds_target_mode() {
    use sqlx::Row;
    let (pool, _path) = fresh_pool().await;
    // init_pool 只设 pragma,schema 由 run_migrations 建(测试要在新形
    // 表上搭旧形表,先跑完整迁移)。
    run_migrations(&pool).await.expect("run migrations");

    let name = format!("rebuild-{}", uuid::Uuid::new_v4().simple());
    let path = format!("/tmp/rebuild-{name}");
    crate::db::create_project(&pool, &name, &path, false, None)
        .await
        .expect("create project");
    let project = crate::db::list_projects(&pool, false)
        .await
        .expect("list projects")
        .into_iter()
        .find(|p| p.name == name)
        .expect("project row");
    let session_id = uuid::Uuid::new_v4().to_string();
    crate::db::create_session(
        &pool,
        &session_id,
        &project.id,
        &path,
        "mock-model",
        None,
        None,
        None,
    )
    .await
    .expect("create session");

    sqlx::query("DROP TABLE scheduled_tasks")
        .execute(&pool)
        .await
        .expect("drop new-shape table");
    sqlx::query(
        r#"
        CREATE TABLE scheduled_tasks (
          id TEXT PRIMARY KEY,
          project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
          target_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
          name TEXT NOT NULL,
          prompt TEXT NOT NULL,
          schedule TEXT NOT NULL,
          enabled INTEGER NOT NULL DEFAULT 1,
          created_by TEXT NOT NULL DEFAULT 'user',
          created_at INTEGER NOT NULL,
          last_fired_at INTEGER,
          next_fire_at INTEGER NOT NULL,
          run_count INTEGER NOT NULL DEFAULT 0,
          max_runs INTEGER,
          ends_at INTEGER
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("create old-shape table");
    sqlx::query(
        "INSERT INTO scheduled_tasks \
         (id, project_id, target_session_id, name, prompt, schedule, enabled, \
          created_by, created_at, last_fired_at, next_fire_at, run_count, max_runs, ends_at) \
         VALUES ('t1', ?, ?, '旧任务', 'p', '{\"kind\":\"daily\",\"at\":\"09:00\"}', \
                 1, 'user', 100, 200, 300, 2, 5, NULL)",
    )
    .bind(&project.id)
    .bind(&session_id)
    .execute(&pool)
    .await
    .expect("insert legacy row");

    rebuild_scheduled_tasks_for_target_mode(&pool)
        .await
        .expect("rebuild");
    let row = sqlx::query(
        "SELECT target_session_id, target_mode, model_id, last_run_session_id, \
                name, run_count, max_runs, enabled FROM scheduled_tasks WHERE id = 't1'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy row preserved");
    assert_eq!(
        row.try_get::<Option<String>, _>("target_session_id")
            .expect("target"),
        Some(session_id),
        "fixed target preserved through the rebuild"
    );
    assert_eq!(
        row.try_get::<String, _>("target_mode").expect("mode"),
        "fixed"
    );
    assert!(row
        .try_get::<Option<String>, _>("model_id")
        .expect("model")
        .is_none());
    assert!(row
        .try_get::<Option<String>, _>("last_run_session_id")
        .expect("run sid")
        .is_none());
    assert_eq!(row.try_get::<String, _>("name").expect("name"), "旧任务");
    assert_eq!(row.try_get::<i64, _>("run_count").expect("run_count"), 2);
    assert_eq!(row.try_get::<i64, _>("max_runs").expect("max_runs"), 5);

    // 幂等:重跑 no-op(行不重复、不丢)。
    rebuild_scheduled_tasks_for_target_mode(&pool)
        .await
        .expect("rebuild again");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_tasks")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);
}
