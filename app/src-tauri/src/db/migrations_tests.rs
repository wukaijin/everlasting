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

/// 09-07-gce-m4a-scheduled-deliberation:per_run 时代旧形表(有
/// target_mode 三列、无 group_chat 两列)经
/// `rebuild_scheduled_tasks_for_group_chat` 重建 —— 行保全 + 两新列种子
/// NULL + 新 CHECK 五臂生效 + 幂等。旧形表手工搭:drop 新形表后按
/// per_run 时代 DDL 建。
#[tokio::test]
async fn rebuild_scheduled_tasks_group_chat_preserves_rows_and_updates_checks() {
    use sqlx::Row;
    let (pool, _path) = fresh_pool().await;
    run_migrations(&pool).await.expect("run migrations");

    let name = format!("rebuild-gc-{}", uuid::Uuid::new_v4().simple());
    let path = format!("/tmp/rebuild-gc-{name}");
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

    // 搭 per_run 时代旧形表(无 group_chat_config / last_fire_outcome,
    // CHECK 还是两臂旧白名单)。
    sqlx::query("DROP TABLE scheduled_tasks")
        .execute(&pool)
        .await
        .expect("drop new-shape table");
    sqlx::query(
        r#"
        CREATE TABLE scheduled_tasks (
          id TEXT PRIMARY KEY,
          project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
          target_session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
          target_mode TEXT NOT NULL DEFAULT 'fixed',
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
          ends_at INTEGER,
          model_id TEXT,
          last_run_session_id TEXT,
          CHECK (target_mode = 'fixed' OR target_mode = 'per_run'),
          CHECK (target_mode = 'per_run' OR target_session_id IS NOT NULL)
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("create per_run-era table");
    sqlx::query(
        "INSERT INTO scheduled_tasks \
         (id, project_id, target_session_id, target_mode, name, prompt, schedule, enabled, \
          created_by, created_at, last_fired_at, next_fire_at, run_count, max_runs, ends_at, \
          model_id, last_run_session_id) \
         VALUES ('t1', ?, ?, 'fixed', '固定档', 'p', '{}', 1, 'user', 100, 200, 300, 2, 5, NULL, NULL, NULL)",
    )
    .bind(&project.id)
    .bind(&session_id)
    .execute(&pool)
    .await
    .expect("insert fixed row");
    sqlx::query(
        "INSERT INTO scheduled_tasks \
         (id, project_id, target_session_id, target_mode, name, prompt, schedule, enabled, \
          created_by, created_at, last_fired_at, next_fire_at, run_count, max_runs, ends_at, \
          model_id, last_run_session_id) \
         VALUES ('t2', ?, NULL, 'per_run', '每次新建', 'p', '{}', 1, 'user', 100, 200, 300, 0, NULL, NULL, NULL, 'run-1')",
    )
    .bind(&project.id)
    .execute(&pool)
    .await
    .expect("insert per_run row");

    rebuild_scheduled_tasks_for_group_chat(&pool)
        .await
        .expect("rebuild");
    // 行保全 + 种子:两新列 NULL,既有列原值。
    for (id, expect_target, expect_run_sid) in
        [("t1", Some(&session_id), None), ("t2", None, Some("run-1"))]
    {
        let row = sqlx::query(
            "SELECT target_session_id, target_mode, last_run_session_id, run_count, \
                    group_chat_config, last_fire_outcome FROM scheduled_tasks WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("row preserved");
        assert_eq!(
            row.try_get::<Option<String>, _>("target_session_id")
                .expect("target"),
            expect_target.map(|s| s.to_string()),
            "{id}: target preserved"
        );
        assert_eq!(
            row.try_get::<Option<String>, _>("last_run_session_id")
                .expect("run sid"),
            expect_run_sid.map(|s| s.to_string()),
            "{id}: run sid preserved"
        );
        assert!(row
            .try_get::<Option<String>, _>("group_chat_config")
            .expect("config")
            .is_none());
        assert!(row
            .try_get::<Option<String>, _>("last_fire_outcome")
            .expect("outcome")
            .is_none());
    }

    // 新 CHECK 五臂:mode 白名单 / group_chat ⇒ target NULL + config
    // 非空 / fixed ⇔ target / outcome 白名单。
    let cfg = r#"{"moderator_model_id":"m","participants":[{"name":"甲","model_id":"m"}]}"#;
    let ok_gc = sqlx::query(
        "INSERT INTO scheduled_tasks (id, project_id, target_session_id, target_mode, name, \
         prompt, schedule, enabled, created_by, created_at, next_fire_at, group_chat_config) \
         VALUES ('g1', ?, NULL, 'group_chat', '审议', 'topic', '{}', 1, 'user', 1, 2, ?)",
    )
    .bind(&project.id)
    .bind(cfg)
    .execute(&pool)
    .await;
    assert!(
        ok_gc.is_ok(),
        "group_chat row without target + with config is legal"
    );

    let bad = sqlx::query(
        "INSERT INTO scheduled_tasks (id, project_id, target_session_id, target_mode, name, \
         prompt, schedule, enabled, created_by, created_at, next_fire_at, group_chat_config) \
         VALUES ('g2', ?, NULL, 'group_chat', 'x', 'p', '{}', 1, 'user', 1, 2, NULL)",
    )
    .bind(&project.id)
    .execute(&pool)
    .await;
    assert!(bad.is_err(), "group_chat without config must violate CHECK");

    let bad = sqlx::query(
        "INSERT INTO scheduled_tasks (id, project_id, target_session_id, target_mode, name, \
         prompt, schedule, enabled, created_by, created_at, next_fire_at, group_chat_config) \
         VALUES ('g3', ?, ?, 'group_chat', 'x', 'p', '{}', 1, 'user', 1, 2, ?)",
    )
    .bind(&project.id)
    .bind(&session_id)
    .bind(cfg)
    .execute(&pool)
    .await;
    assert!(
        bad.is_err(),
        "group_chat with a fixed target must violate CHECK"
    );

    let bad = sqlx::query(
        "INSERT INTO scheduled_tasks (id, project_id, target_session_id, target_mode, name, \
         prompt, schedule, enabled, created_by, created_at, next_fire_at) \
         VALUES ('g4', ?, NULL, 'fixed', 'x', 'p', '{}', 1, 'user', 1, 2)",
    )
    .bind(&project.id)
    .execute(&pool)
    .await;
    assert!(bad.is_err(), "fixed without target must violate CHECK");

    let bad = sqlx::query(
        "INSERT INTO scheduled_tasks (id, project_id, target_session_id, target_mode, name, \
         prompt, schedule, enabled, created_by, created_at, next_fire_at) \
         VALUES ('g5', ?, NULL, 'yolo', 'x', 'p', '{}', 1, 'user', 1, 2)",
    )
    .bind(&project.id)
    .execute(&pool)
    .await;
    assert!(bad.is_err(), "unknown mode must violate CHECK");

    let bad = sqlx::query("UPDATE scheduled_tasks SET last_fire_outcome = 'nope' WHERE id = 'g1'")
        .execute(&pool)
        .await;
    assert!(
        bad.is_err(),
        "outcome outside the five-value whitelist must violate CHECK"
    );
    let ok = sqlx::query(
        "UPDATE scheduled_tasks SET last_fire_outcome = 'skipped_busy' WHERE id = 'g1'",
    )
    .execute(&pool)
    .await;
    assert!(ok.is_ok(), "whitelisted outcome is writable");

    // 幂等:重跑 no-op(行不重复、不丢)。
    rebuild_scheduled_tasks_for_group_chat(&pool)
        .await
        .expect("rebuild again");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_tasks")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 3, "t1 + t2 + g1 survive the idempotent re-run");
}
