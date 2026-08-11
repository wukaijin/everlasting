//! 幂等 schema bootstrap(design.md §3.3 原样)。
//!
//! 与 daemon 的 migration 风格一致:一函数全 `CREATE TABLE IF NOT EXISTS`
//! + `CREATE INDEX IF NOT EXISTS`,启动时无条件跑(`RemoteState::load`
//! 里 `init_pool` 后调用),不引入 timestamped SQL 文件。

use sqlx::SqlitePool;

/// 建三表 + 索引。幂等 —— 每次启动安全执行。
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
 CREATE TABLE IF NOT EXISTS nodes (
     id TEXT PRIMARY KEY,
     display_name TEXT NOT NULL,
     status TEXT NOT NULL,
     last_seen_at INTEGER NOT NULL,
     created_at INTEGER NOT NULL
 )
 "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
 CREATE TABLE IF NOT EXISTS devices (
     token TEXT PRIMARY KEY,
     node_id TEXT NOT NULL REFERENCES nodes(id),
     display_name TEXT,
     last_seen_at INTEGER NOT NULL,
     created_at INTEGER NOT NULL,
     revoked INTEGER NOT NULL DEFAULT 0
 )
 "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
 CREATE TABLE IF NOT EXISTS pairing_codes (
     code TEXT PRIMARY KEY,
     node_id TEXT NOT NULL REFERENCES nodes(id),
     expires_at INTEGER NOT NULL,
     used INTEGER NOT NULL DEFAULT 0,
     created_at INTEGER NOT NULL
 )
 "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
 CREATE INDEX IF NOT EXISTS idx_devices_node ON devices(node_id)
 "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{test_db, TestDb};

    /// 迁移幂等:连跑两次不报错(启动每次都会跑,必须可重入)。
    #[tokio::test]
    async fn run_migrations_is_idempotent() {
        let db: TestDb = test_db().await;
        run_migrations(&db.pool)
            .await
            .expect("second run must succeed");
    }

    /// 三表 + 索引都建出来了。
    #[tokio::test]
    async fn schema_creates_all_tables_and_index() {
        let db = test_db().await;
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='table' \
             AND name IN ('nodes','devices','pairing_codes') ORDER BY name",
        )
        .fetch_all(&db.pool)
        .await
        .expect("query sqlite_master");
        assert_eq!(tables, vec!["devices", "nodes", "pairing_codes"]);

        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_devices_node'",
        )
        .fetch_all(&db.pool)
        .await
        .expect("query sqlite_master");
        assert_eq!(indexes, vec!["idx_devices_node"]);
    }
}
