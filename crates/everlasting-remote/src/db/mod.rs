//! remote 自己的 SQLite 层(design.md §3.3,独立于 daemon 的 `everlasting.db`)。
//!
//! **不变量(design §1.3)**:remote.db 只存 token / devices / pairing_codes,
//! 绝不碰 agent 数据;`remote` 零依赖 daemon 的 db 模块,此处全部自持。
//!
//! 布局:
//! - [`pool`] — `init_pool`(WAL + busy_timeout,仿 daemon `migrations/pool.rs`)
//! - [`schema`] — `run_migrations`(三表 + 索引,幂等)
//! - [`crud`] — 节点 / 设备 / 配对码 CRUD + 撞码 retry + redeem 事务

pub mod crud;
pub mod pool;
pub mod schema;

use sqlx::FromRow;

/// 节点在线状态常量(`nodes.status` 列值)。String + 常量而非枚举:
/// SQLite 侧无 CHECK 约束,枚举需自定义 sqlx 映射,轻量场景不值。
pub const NODE_STATUS_ONLINE: &str = "online";
pub const NODE_STATUS_OFFLINE: &str = "offline";

/// `nodes` 表行。
#[derive(Debug, Clone, FromRow)]
pub struct Node {
    /// PC daemon 自报的稳定 node_id。
    pub id: String,
    /// "公司 PC" / "家里 PC"(PC 自报或默认)。
    pub display_name: String,
    /// `NODE_STATUS_ONLINE` / `NODE_STATUS_OFFLINE`。
    pub status: String,
    /// 最后在线时刻(unix epoch ms)。
    pub last_seen_at: i64,
    pub created_at: i64,
}

/// `devices` 表行。`revoked` 原样返回 —— 吊销判断在 auth 中间件
/// (Step 4),不在 CRUD 层过滤。
#[derive(Debug, Clone, FromRow)]
pub struct Device {
    /// 配对时签发的 32 字节 hex(64 chars)。
    pub token: String,
    pub node_id: String,
    /// 设备名(redeem 时传,可空)。
    pub display_name: Option<String>,
    pub last_seen_at: i64,
    pub created_at: i64,
    /// 0 = 正常,1 = 吊销。
    pub revoked: i64,
}

/// 当前 unix epoch 毫秒。schema 时间戳统一 i64 epoch ms(design §3.3),
/// 不引入 chrono 类型。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// db 层测试基础设施:TempDir 必须活得比 pool 久(连接还开着时删目录
/// 会让后续查询失效),故用 holder 结构。`pub(crate)` 供 server 等
/// 其他模块的测试复用。
#[cfg(test)]
pub(crate) struct TestDb {
    /// 保活 tempdir(drop 时删目录;holder 字段顺序保证 pool 先 drop)。
    _dir: tempfile::TempDir,
    pub pool: sqlx::SqlitePool,
}

#[cfg(test)]
pub(crate) async fn test_db() -> TestDb {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir.path().join("test.db");
    let pool = pool::init_pool(&db_path).await.expect("init pool");
    schema::run_migrations(&pool).await.expect("run migrations");
    TestDb { _dir: dir, pool }
}
