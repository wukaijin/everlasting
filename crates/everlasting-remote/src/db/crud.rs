//! nodes / devices / pairing_codes 的 CRUD(design-step3.md §5)。
//!
//! 错误契约:CRUD 层返回**领域错误**(`PairingCodeError` / `RedeemError`),
//! HTTP 层(Step 7)映射到 `AppError` —— 跟随 daemon 模式(daemon CRUD
//! 返回领域错误,command 层映射)。

use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::db::{now_ms, Device, Node, NODE_STATUS_OFFLINE, NODE_STATUS_ONLINE};

// =========================================================================
// nodes
// =========================================================================

/// 注册 / 刷新节点(design §2.1 "upsert nodes 表(online)")。
///
/// implement.md 的 "insert_node" 实为 upsert:`ON CONFLICT(id) DO UPDATE`
/// 刷新 display_name / status='online' / last_seen_at,`created_at` 首插保留。
/// Step 5 WSS 连接时调用。
pub async fn upsert_node(
    pool: &SqlitePool,
    node_id: &str,
    display_name: &str,
) -> Result<(), sqlx::Error> {
    let now = now_ms();
    sqlx::query(
        r#"
 INSERT INTO nodes (id, display_name, status, last_seen_at, created_at)
 VALUES (?, ?, ?, ?, ?)
 ON CONFLICT(id) DO UPDATE SET
   display_name = excluded.display_name,
   status = excluded.status,
   last_seen_at = excluded.last_seen_at
 "#,
    )
    .bind(node_id)
    .bind(display_name)
    .bind(NODE_STATUS_ONLINE)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// 更新节点状态(Step 5 心跳 / 超时)。
///
/// 语义:`status == online` 时同时续 `last_seen_at`(心跳续期);
/// `offline` 时**只改 status**(保留最后在线时刻,不把离线判定时刻
/// 记成"最后在线")。
pub async fn update_node_status(
    pool: &SqlitePool,
    node_id: &str,
    status: &str,
    at_ms: i64,
) -> Result<(), sqlx::Error> {
    if status == NODE_STATUS_ONLINE {
        sqlx::query("UPDATE nodes SET status = ?, last_seen_at = ? WHERE id = ?")
            .bind(status)
            .bind(at_ms)
            .bind(node_id)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("UPDATE nodes SET status = ? WHERE id = ?")
            .bind(status)
            .bind(node_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// 按 id 查节点;不存在 → `None`。
pub async fn get_node(pool: &SqlitePool, node_id: &str) -> Result<Option<Node>, sqlx::Error> {
    sqlx::query_as::<_, Node>(
        "SELECT id, display_name, status, last_seen_at, created_at FROM nodes WHERE id = ?",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await
}

/// 全量置 offline(RemoteState::load 启动时调)。
///
/// boot 不变量:remote 重启后**没有任何**隧道连接,所有节点的状态
/// 都是陈旧值 —— 在下次 WSS 连接(upsert 置 online)之前必须全部视为
/// 离线,否则 Step 8 的节点 API 会对已断开的 PC 误报 online。
pub async fn mark_all_offline(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE nodes SET status = ? WHERE status = ?")
        .bind(NODE_STATUS_OFFLINE)
        .bind(NODE_STATUS_ONLINE)
        .execute(pool)
        .await?;
    Ok(())
}

// =========================================================================
// devices
// =========================================================================

/// 配对成功落 devices(Step 7 签发 token 后调用)。
/// `display_name` 可空(schema 列可空;redeem 时传设备名)。
pub async fn insert_device(
    pool: &SqlitePool,
    token: &str,
    node_id: &str,
    display_name: Option<&str>,
) -> Result<(), sqlx::Error> {
    let now = now_ms();
    sqlx::query(
        r#"
 INSERT INTO devices (token, node_id, display_name, last_seen_at, created_at)
 VALUES (?, ?, ?, ?, ?)
 "#,
    )
    .bind(token)
    .bind(node_id)
    .bind(display_name)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// 按 token 查设备;不存在 → `None`。
///
/// **不过滤 `revoked`**(完整行返回)——吊销语义留给 Step 4 auth
/// 中间件,CRUD 层保持数据完整。
pub async fn get_device_by_token(
    pool: &SqlitePool,
    token: &str,
) -> Result<Option<Device>, sqlx::Error> {
    sqlx::query_as::<_, Device>(
        "SELECT token, node_id, display_name, last_seen_at, created_at, revoked \
         FROM devices WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(pool)
    .await
}

// =========================================================================
// pairing_codes
// =========================================================================

/// 纯插入配对码;撞 PRIMARY KEY(6 位码已被别的 node 占用)→ `Conflict`。
/// 由 [`generate_and_store_pairing_code`] 消费(retry 循环)。
pub async fn insert_pairing_code(
    pool: &SqlitePool,
    code: &str,
    node_id: &str,
    expires_at_ms: i64,
) -> Result<(), PairingCodeError> {
    let now = now_ms();
    // 撞码(unique violation)必须识别为 `Conflict` 而非笼统 `Db`,
    // retry 循环靠它决定重生成。
    match sqlx::query(
        r#"
 INSERT INTO pairing_codes (code, node_id, expires_at, created_at)
 VALUES (?, ?, ?, ?)
 "#,
    )
    .bind(code)
    .bind(node_id)
    .bind(expires_at_ms)
    .bind(now)
    .execute(pool)
    .await
    {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(PairingCodeError::Conflict),
        Err(e) => Err(PairingCodeError::Db(e)),
    }
}

/// 生成 6 位码并落库,撞码自动重试(P2-3:两个 node 同时生成可能撞
/// 6 位码)。**最多 3 次尝试**(初始 + 2 次重试),仍冲突 →
/// [`PairingCodeError::RetryExhausted`](1/1M 量级,正常不可达)。
///
/// 返回成功落库的码。Step 7 internal RPC 调用。
pub async fn generate_and_store_pairing_code(
    pool: &SqlitePool,
    node_id: &str,
    expires_at_ms: i64,
) -> Result<String, PairingCodeError> {
    for _ in 0..3 {
        let code = random_six_digit();
        match insert_pairing_code(pool, &code, node_id, expires_at_ms).await {
            Ok(()) => return Ok(code),
            Err(PairingCodeError::Conflict) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(PairingCodeError::RetryExhausted)
}

/// 6 位数字码(前导零保留)。随机源 = uuid v4(122 位 CSPRNG),
/// 取前 8 字节转 u128 模 1_000_000 —— 零新依赖(design-step3.md §7)。
fn random_six_digit() -> String {
    let n = u128::from_be_bytes(*Uuid::new_v4().as_bytes()) % 1_000_000;
    format!("{n:06}")
}

/// 配对成功签发 device_token:两次 uuid v4 simple 拼接 = 64 hex chars
/// (32 字节,design §3.4 / design-step3.md §7)。零新依赖。
fn random_token_hex() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// redeem 成功结果,wire 直接对应 design §3.4 响应字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redeemed {
    pub device_token: String,
    pub node_id: String,
    pub node_display_name: String,
}

/// 手机 redeem 配对码(design §2.3 / §3.4):事务内校验 → 签发 token →
/// 落 devices → 标 used。Step 7 HTTP 调用。
///
/// 并发安全:校验(未过期 + 未用)+ `UPDATE ... WHERE used = 0` 检查
/// `rows_affected == 0` 兜底 —— 两个 redeem 同时抢同一码时,SQLite
/// 写锁串行化,后到者在 UPDATE 处影响 0 行 → `InvalidOrExpiredCode`,
/// 不会双发 token。
pub async fn redeem_pairing_code(
    pool: &SqlitePool,
    code: &str,
    device_name: &str,
) -> Result<Redeemed, RedeemError> {
    let now = now_ms();
    let mut tx = pool.begin().await.map_err(RedeemError::Db)?;

    // 1. 取码行(不存在 → InvalidOrExpired)
    let row =
        sqlx::query("SELECT code, node_id, expires_at, used FROM pairing_codes WHERE code = ?")
            .bind(code)
            .fetch_optional(&mut *tx)
            .await
            .map_err(RedeemError::Db)?;
    let Some(row) = row else {
        return Err(RedeemError::InvalidOrExpiredCode);
    };
    let node_id: String = row.try_get("node_id").map_err(RedeemError::Db)?;
    let expires_at: i64 = row.try_get("expires_at").map_err(RedeemError::Db)?;
    let used: i64 = row.try_get("used").map_err(RedeemError::Db)?;

    // 2. 校验:未过期 + 未用(design §2.3:过期/已用统一 400)
    if expires_at <= now || used != 0 {
        return Err(RedeemError::InvalidOrExpiredCode);
    }

    // 3. 签发 token + 落 devices
    let token = random_token_hex();
    sqlx::query(
        r#"
 INSERT INTO devices (token, node_id, display_name, last_seen_at, created_at)
 VALUES (?, ?, ?, ?, ?)
 "#,
    )
    .bind(&token)
    .bind(&node_id)
    .bind(device_name)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(RedeemError::Db)?;

    // 4. 标 used(带 used = 0 条件 —— 并发兜底,见函数文档)
    let res = sqlx::query("UPDATE pairing_codes SET used = 1 WHERE code = ? AND used = 0")
        .bind(code)
        .execute(&mut *tx)
        .await
        .map_err(RedeemError::Db)?;
    if res.rows_affected() == 0 {
        return Err(RedeemError::InvalidOrExpiredCode);
    }

    // 5. 取 node display_name(配对码生成时 node 已注册,Step 5/7 保证;
    //    防御性兜底空串)
    let node_display_name: Option<String> =
        sqlx::query_scalar("SELECT display_name FROM nodes WHERE id = ?")
            .bind(&node_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(RedeemError::Db)?;

    tx.commit().await.map_err(RedeemError::Db)?;

    Ok(Redeemed {
        device_token: token,
        node_id,
        node_display_name: node_display_name.unwrap_or_default(),
    })
}

// =========================================================================
// 领域错误(design-step3.md §5.2)
// =========================================================================

/// 配对码插入错误。
#[derive(Debug)]
pub enum PairingCodeError {
    /// 6 位码撞 PRIMARY KEY(已被占用)→ 调用方重生成。
    Conflict,
    /// 撞码重试 3 次仍冲突(1/1M 量级,正常不可达)。
    RetryExhausted,
    Db(sqlx::Error),
}

impl std::fmt::Display for PairingCodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairingCodeError::Conflict => write!(f, "pairing code collision"),
            PairingCodeError::RetryExhausted => {
                write!(f, "pairing code generation retries exhausted")
            }
            PairingCodeError::Db(e) => write!(f, "db error: {e}"),
        }
    }
}

impl std::error::Error for PairingCodeError {}

impl From<sqlx::Error> for PairingCodeError {
    fn from(e: sqlx::Error) -> Self {
        PairingCodeError::Db(e)
    }
}

/// redeem 错误。
#[derive(Debug)]
pub enum RedeemError {
    /// 码不存在 / 过期 / 已用(design §3.4 统一 400 `invalid_or_expired_code`)。
    InvalidOrExpiredCode,
    Db(sqlx::Error),
}

impl std::fmt::Display for RedeemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RedeemError::InvalidOrExpiredCode => write!(f, "invalid or expired code"),
            RedeemError::Db(e) => write!(f, "db error: {e}"),
        }
    }
}

impl std::error::Error for RedeemError {}

impl From<sqlx::Error> for RedeemError {
    fn from(e: sqlx::Error) -> Self {
        RedeemError::Db(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{test_db, TestDb, NODE_STATUS_OFFLINE};

    // ---- nodes ----

    #[tokio::test]
    async fn upsert_node_inserts_then_updates_without_duplicate() {
        let db: TestDb = test_db().await;

        upsert_node(&db.pool, "pc-1", "公司 PC")
            .await
            .expect("insert");
        upsert_node(&db.pool, "pc-1", "公司 PC 改名")
            .await
            .expect("upsert");

        let node = get_node(&db.pool, "pc-1")
            .await
            .expect("query")
            .expect("exists");
        assert_eq!(node.display_name, "公司 PC 改名");
        assert_eq!(node.status, NODE_STATUS_ONLINE);
        assert!(node.last_seen_at > 0);
        assert!(node.created_at > 0);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "upsert 不得产生重复行");
    }

    #[tokio::test]
    async fn update_node_status_offline_keeps_last_seen_at() {
        let db = test_db().await;
        upsert_node(&db.pool, "pc-1", "PC").await.expect("insert");
        let last_seen = get_node(&db.pool, "pc-1")
            .await
            .unwrap()
            .unwrap()
            .last_seen_at;

        // offline:只改 status,last_seen_at 保留最后在线时刻。
        update_node_status(&db.pool, "pc-1", NODE_STATUS_OFFLINE, last_seen + 1000)
            .await
            .expect("offline");
        let node = get_node(&db.pool, "pc-1").await.unwrap().unwrap();
        assert_eq!(node.status, NODE_STATUS_OFFLINE);
        assert_eq!(node.last_seen_at, last_seen);

        // 再 online:续 last_seen_at。
        update_node_status(&db.pool, "pc-1", NODE_STATUS_ONLINE, last_seen + 2000)
            .await
            .expect("online");
        let node = get_node(&db.pool, "pc-1").await.unwrap().unwrap();
        assert_eq!(node.status, NODE_STATUS_ONLINE);
        assert_eq!(node.last_seen_at, last_seen + 2000);
    }

    #[tokio::test]
    async fn get_node_missing_returns_none() {
        let db = test_db().await;
        assert!(get_node(&db.pool, "nope").await.unwrap().is_none());
    }

    /// boot 不变量:mark_all_offline 只把 online 置 offline,offline
    /// 保持原状。
    #[tokio::test]
    async fn mark_all_offline_flips_online_only() {
        let db = test_db().await;
        upsert_node(&db.pool, "pc-1", "PC").await.unwrap(); // online
        upsert_node(&db.pool, "pc-3", "PC").await.unwrap();
        update_node_status(&db.pool, "pc-3", NODE_STATUS_OFFLINE, now_ms())
            .await
            .expect("offline");

        mark_all_offline(&db.pool).await.expect("mark_all_offline");

        // pc-1:online → offline
        let n1 = get_node(&db.pool, "pc-1").await.unwrap().unwrap();
        assert_eq!(n1.status, NODE_STATUS_OFFLINE);
        // pc-3:本就 offline,WHERE status='online' 过滤,不受影响
        let n3 = get_node(&db.pool, "pc-3").await.unwrap().unwrap();
        assert_eq!(n3.status, NODE_STATUS_OFFLINE);
    }

    // ---- devices ----

    #[tokio::test]
    async fn insert_and_get_device_by_token() {
        let db = test_db().await;
        upsert_node(&db.pool, "pc-1", "PC").await.unwrap();

        insert_device(&db.pool, "tok-1", "pc-1", Some("Carlos 的 iPhone"))
            .await
            .expect("insert");

        let device = get_device_by_token(&db.pool, "tok-1")
            .await
            .expect("query")
            .expect("exists");
        assert_eq!(device.node_id, "pc-1");
        assert_eq!(device.display_name.as_deref(), Some("Carlos 的 iPhone"));
        assert_eq!(device.revoked, 0);

        // 未知 token → None
        assert!(get_device_by_token(&db.pool, "nope")
            .await
            .unwrap()
            .is_none());
    }

    // ---- pairing_codes ----

    #[test]
    fn random_six_digit_is_6_digit_with_leading_zeros() {
        for _ in 0..100 {
            let code = random_six_digit();
            assert_eq!(code.len(), 6);
            assert!(code.chars().all(|c| c.is_ascii_digit()), "got {code}");
        }
    }

    #[tokio::test]
    async fn insert_pairing_code_ok_then_conflict() {
        let db = test_db().await;
        upsert_node(&db.pool, "pc-1", "PC").await.unwrap();

        insert_pairing_code(&db.pool, "123456", "pc-1", now_ms() + 60_000)
            .await
            .expect("first insert");
        let err = insert_pairing_code(&db.pool, "123456", "pc-2", now_ms() + 60_000)
            .await
            .expect_err("duplicate code must conflict");
        assert!(matches!(err, PairingCodeError::Conflict));
    }

    #[tokio::test]
    async fn generate_and_store_pairing_code_succeeds_and_persists() {
        let db = test_db().await;
        upsert_node(&db.pool, "pc-1", "PC").await.unwrap();

        let expires_at = now_ms() + 60_000;
        let code = generate_and_store_pairing_code(&db.pool, "pc-1", expires_at)
            .await
            .expect("generate");
        assert_eq!(code.len(), 6);

        let row = sqlx::query("SELECT node_id, expires_at, used FROM pairing_codes WHERE code = ?")
            .bind(&code)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(row.try_get::<String, _>("node_id").unwrap(), "pc-1");
        assert_eq!(row.try_get::<i64, _>("expires_at").unwrap(), expires_at);
        assert_eq!(row.try_get::<i64, _>("used").unwrap(), 0);
    }

    // ---- redeem ----

    /// 完整成功路径:预置 node + code → redeem → token 64 hex + 码标 used
    /// + devices 有行 + 返回 node_display_name。
    #[tokio::test]
    async fn redeem_pairing_code_success() {
        let db = test_db().await;
        upsert_node(&db.pool, "pc-1", "公司 PC").await.unwrap();
        let code = generate_and_store_pairing_code(&db.pool, "pc-1", now_ms() + 60_000)
            .await
            .unwrap();

        let redeemed = redeem_pairing_code(&db.pool, &code, "Carlos 的 iPhone")
            .await
            .expect("redeem");
        assert_eq!(redeemed.node_id, "pc-1");
        assert_eq!(redeemed.node_display_name, "公司 PC");
        // 32 字节 hex = 64 hex chars
        assert_eq!(redeemed.device_token.len(), 64);
        assert!(redeemed.device_token.chars().all(|c| c.is_ascii_hexdigit()));

        // 码已标 used
        let used: i64 = sqlx::query_scalar("SELECT used FROM pairing_codes WHERE code = ?")
            .bind(&code)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(used, 1);

        // devices 有行,token 可查
        let device = get_device_by_token(&db.pool, &redeemed.device_token)
            .await
            .unwrap()
            .expect("device row");
        assert_eq!(device.node_id, "pc-1");
        assert_eq!(device.display_name.as_deref(), Some("Carlos 的 iPhone"));
    }

    #[tokio::test]
    async fn redeem_twice_second_is_invalid_or_expired() {
        let db = test_db().await;
        upsert_node(&db.pool, "pc-1", "PC").await.unwrap();
        let code = generate_and_store_pairing_code(&db.pool, "pc-1", now_ms() + 60_000)
            .await
            .unwrap();

        redeem_pairing_code(&db.pool, &code, "dev-1")
            .await
            .expect("first");
        let err = redeem_pairing_code(&db.pool, &code, "dev-2")
            .await
            .expect_err("second redeem must fail");
        assert!(matches!(err, RedeemError::InvalidOrExpiredCode));
    }

    #[tokio::test]
    async fn redeem_expired_code_is_invalid() {
        let db = test_db().await;
        upsert_node(&db.pool, "pc-1", "PC").await.unwrap();
        let code = generate_and_store_pairing_code(&db.pool, "pc-1", now_ms() - 1)
            .await
            .unwrap();

        let err = redeem_pairing_code(&db.pool, &code, "dev-1")
            .await
            .expect_err("expired code must fail");
        assert!(matches!(err, RedeemError::InvalidOrExpiredCode));
    }

    #[tokio::test]
    async fn redeem_unknown_code_is_invalid() {
        let db = test_db().await;
        let err = redeem_pairing_code(&db.pool, "999999", "dev-1")
            .await
            .expect_err("unknown code must fail");
        assert!(matches!(err, RedeemError::InvalidOrExpiredCode));
    }

    /// 撞码 retry 的确定性测试说明(design-step3.md §8):随机生成撞已有
    /// 码概率 1/1M,不可靠构造。确定性覆盖 = `insert_pairing_code` 的
    /// `Conflict` 语义(见 `insert_pairing_code_ok_then_conflict`);
    /// retry 循环 ≤3 次靠代码审查。这里补一个语义断言:生成成功返回的
    /// 码与库中行一一对应(非概率性)。
    #[tokio::test]
    async fn generate_codes_are_distinct_rows() {
        let db = test_db().await;
        upsert_node(&db.pool, "pc-1", "PC").await.unwrap();
        let a = generate_and_store_pairing_code(&db.pool, "pc-1", now_ms() + 60_000)
            .await
            .unwrap();
        let b = generate_and_store_pairing_code(&db.pool, "pc-1", now_ms() + 60_000)
            .await
            .unwrap();
        // 两次生成应落在不同行(若撞码则第二次 retry 后也会不同)。
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pairing_codes")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        assert!(a != b);
    }
}
