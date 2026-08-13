//! SQLite pool 初始化(WAL / busy_timeout / foreign_keys pragma)。
//!
//! 配置与 daemon `db/migrations/pool.rs` 同款(design §7 对齐),remote
//! 独立实现、不依赖 daemon crate:
//! - `journal_mode = WAL` —— 多连接并发读 + 单写,防 `SQLITE_BUSY`
//! - `busy_timeout = 5000` —— 写锁竞争等 5s 才报 BUSY,掩盖瞬时竞争
//! - `foreign_keys = ON` —— `devices.node_id → nodes(id)` 外键生效
//!
//! pragma 经 `SqliteConnectOptions`(per-connection,每次 acquire 应用),
//! 而非一次性 `PRAGMA` 执行 —— pool 懒增长时新连接也带同样设置。

use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

/// 打开(或创建)`db_path` 的 SQLite 文件并返回连接池。
/// `db_path` 父目录不存在时自动创建。
pub async fn init_pool(db_path: &Path) -> Result<SqlitePool, sqlx::Error> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            sqlx::Error::Configuration(
                format!("failed to create db parent dir {}: {}", parent.display(), e).into(),
            )
        })?;
    }

    tracing::info!(
        db_path = %db_path.display(),
        "opening remote sqlite pool (WAL + busy_timeout=5s)"
    );

    // `mode=rwc`(read/write/create)打开;pragma 经 builder 逐连接设置。
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let options = SqliteConnectOptions::from_str(&url)?
        .pragma("journal_mode", "WAL")
        .pragma("busy_timeout", "5000")
        .pragma("foreign_keys", "ON");

    let pool = SqlitePoolOptions::new()
        // pragma 初始化兼作连接验证;test_before_acquire 关掉避免双倍往返。
        .test_before_acquire(false)
        .connect_with(options)
        .await?;

    Ok(pool)
}
