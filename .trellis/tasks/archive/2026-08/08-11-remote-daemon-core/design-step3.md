# S3 Design — DB 层(nodes / devices / pairing_codes + RemoteState)

> **What/Why**:[prd.md](./prd.md) · **How(总)**:[design.md](./design.md) §3.3/§3.4/§7 · **执行**:implement.md Step 3。
> 本文件是 Step 3 的细化设计(函数签名 / 语义 / 错误 / 测试),供实施前评审。

---

## 1. 范围

对应 implement.md Step 3:

1. `db/pool.rs` — `init_pool(db_path)`:sqlx SQLite pool,WAL + busy_timeout(仿 daemon `db/migrations/pool.rs`,remote 独立实现,不依赖 daemon crate)
2. `db/schema.rs` — `run_migrations(pool)`:三表 + 索引,幂等
3. `db/crud.rs` — 节点/设备/配对码 CRUD + 撞码 retry + redeem 事务
4. `config.rs` — `RemoteState`(axum 共享 state,对应 daemon `AppState` 角色)

**明确不做**(后续 step):WSS(5)/auth(4)/proxy(6)/pairing HTTP(7)/nodes HTTP(8)。

## 2. 模块布局(新增,design.md §1.2 对应)

```
crates/everlasting-remote/src/
├── db/
│   ├── mod.rs      (pub mod pool/schema/crud;Node/Device 模型;now_ms();状态常量)
│   ├── pool.rs     (init_pool)
│   ├── schema.rs   (run_migrations)
│   └── crud.rs     (upsert_node / update_node_status / get_node / insert_device /
│                    get_device_by_token / insert_pairing_code /
│                    generate_and_store_pairing_code / redeem_pairing_code + 错误类型)
```

## 3. 数据模型(design.md §3.3 原样,remote.db)

```sql
CREATE TABLE IF NOT EXISTS nodes (
    id TEXT PRIMARY KEY,              -- PC daemon 自报的稳定 node_id
    display_name TEXT NOT NULL,       -- "公司 PC" / "家里 PC"(PC 自报或默认)
    status TEXT NOT NULL,             -- "online" / "offline"
    last_seen_at INTEGER NOT NULL,    -- unix epoch ms
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS devices (
    token TEXT PRIMARY KEY,           -- 32 字节 hex(配对时签发,64 hex chars)
    node_id TEXT NOT NULL REFERENCES nodes(id),
    display_name TEXT,                -- "Carlos 的 iPhone"(redeem 时传)
    last_seen_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS pairing_codes (
    code TEXT PRIMARY KEY,            -- 6 位数字字符串
    node_id TEXT NOT NULL REFERENCES nodes(id),
    expires_at INTEGER NOT NULL,      -- unix epoch ms
    used INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_devices_node ON devices(node_id);
```

**Rust 模型**(`db/mod.rs`,`#[derive(sqlx::FromRow)]`):

```rust
pub struct Node { pub id: String, pub display_name: String, pub status: String,
                  pub last_seen_at: i64, pub created_at: i64 }
pub struct Device { pub token: String, pub node_id: String, pub display_name: Option<String>,
                    pub last_seen_at: i64, pub created_at: i64, pub revoked: i64 }

pub const NODE_STATUS_ONLINE: &str = "online";
pub const NODE_STATUS_OFFLINE: &str = "offline";
pub fn now_ms() -> i64  // SystemTime::now() 距 UNIX_EPOCH 的毫秒
```

- `status` 用 `String` + 常量,不做枚举(轻量;SQLite 侧无约束,枚举需自定义 sqlx 映射,不值)。
- 时间戳统一 `i64` epoch ms(design schema 定义),**不引入 chrono** 类型(sqlx 的 `chrono` feature 已声明但不需要直接用;`now_ms` 用 `std::time`)。

## 4. pool 配置(`db/pool.rs`)

照抄 daemon `db/migrations/pool.rs` 的 pragma 组合(design §7 对齐):

- `journal_mode = WAL`(多连接并发读 + 单写,防 SQLITE_BUSY)
- `busy_timeout = 5000`(写锁竞争等 5s 才报 BUSY)
- `foreign_keys = ON`(devices→nodes 外键生效)
- `mode=rwc`(不存在即创建)+ 自动建父目录
- `test_before_acquire(false)`(pragma 初始化兼作连接验证)

签名:`pub async fn init_pool(db_path: &Path) -> Result<SqlitePool, sqlx::Error>`

## 5. CRUD API(`db/crud.rs`)

全部 `&SqlitePool` 参数(sqlx 0.8,`query_as` + `FromRow`;不引入 `query!` 宏依赖——虽然 feature 已开,保持与 daemon 运行时 SQL 风格一致)。

| 函数 | 签名 | 语义 | 调用方 |
|---|---|---|---|
| `upsert_node` | `(pool, node_id, display_name) -> Result<(), sqlx::Error>` | **upsert 而非纯 insert**(implement.md 的 "insert_node" 是简称):`INSERT ... ON CONFLICT(id) DO UPDATE SET display_name/status='online'/last_seen_at`;created_at 首插保留。WSS 连接时注册节点(design §2.1 "upsert nodes 表(online)") | Step 5 ws.rs |
| `update_node_status` | `(pool, node_id, status, at_ms) -> Result<(), sqlx::Error>` | 心跳/超时改状态(心跳续 last_seen_at;90s 无 pong → offline) | Step 5 心跳 task |
| `get_node` | `(pool, node_id) -> Result<Option<Node>, sqlx::Error>` | 按 id 查 | Step 7 redeem 取 display_name 之外,暂由内部 SQL 处理 |
| `insert_device` | `(pool, token, node_id, display_name: Option<&str>) -> Result<(), sqlx::Error>` | 配对成功落 devices | Step 7 |
| `get_device_by_token` | `(pool, token) -> Result<Option<Device>, sqlx::Error>` | **不过滤 revoked**,完整行返回,吊销语义留给 auth 中间件(Step 4) | Step 4/6 |
| `insert_pairing_code` | `(pool, code, node_id, expires_at_ms) -> Result<(), PairingCodeError>` | 纯插入;撞 PRIMARY KEY → `PairingCodeError::Conflict` | 被下者调用 |
| `generate_and_store_pairing_code` | `(pool, node_id, expires_at_ms) -> Result<String, PairingCodeError>` | `random_six_digit()` 生成 → `insert_pairing_code`;**Conflict 则重生成,最多 3 次**(P2-3),仍冲突 → `PairingCodeError::RetryExhausted` | Step 7 internal RPC |
| `redeem_pairing_code` | `(pool, code, device_name) -> Result<Redeemed, RedeemError>` | 事务:校验 → 签发 token → 写 devices → 标 used → 返回 | Step 7 HTTP |

```rust
pub struct Redeemed { pub device_token: String, pub node_id: String, pub node_display_name: String }
```

### 5.1 `redeem_pairing_code` 事务细节(并发安全)

```text
BEGIN
  1. SELECT code, node_id, expires_at, used FROM pairing_codes WHERE code = ?
     - 不存在 → InvalidOrExpiredCode
  2. expires_at <= now 或 used = 1 → InvalidOrExpiredCode
  3. token = 64 hex(Uuid::new_v4().simple() × 2 拼接,见 §7)
  4. INSERT INTO devices(token, node_id, display_name, last_seen_at, created_at)
  5. UPDATE pairing_codes SET used = 1 WHERE code = ? AND used = 0
     - rows_affected == 0 → 并发 redeem 已抢先 → InvalidOrExpiredCode(回滚)
  6. SELECT display_name FROM nodes WHERE id = node_id → node_display_name
COMMIT
```

并发安全点:两步校验(2/5)都按 `used = 0` 兜底 —— SQLite 写锁串行化下,第二个并发 redeem 在 UPDATE 处拿锁后 `rows_affected == 0`,不会双发 token。**外键**:devices 先 INSERT 后 nodes SELECT —— nodes 行必须在 redeem 前存在(配对码生成时 node 已注册,Step 5/7 保证)。

### 5.2 错误类型(`db/crud.rs` 或 `db/mod.rs`)

```rust
pub enum PairingCodeError { Conflict, RetryExhausted, Db(sqlx::Error) }   // + From<sqlx::Error>
pub enum RedeemError { InvalidOrExpiredCode, Db(sqlx::Error) }            // + From<sqlx::Error>
```

- CRUD 层返回**领域错误**,HTTP 层(Step 7)映射到 `AppError`(Step 2 已建:`InvalidOrExpiredCode → 400`;DB → Server)。跟随 daemon 模式(daemon CRUD 返回 MemoryInsertError 等,command 层映射)。
- `Display`/`Error` 手写(不引 thiserror,daemon 风格)。

## 6. `RemoteState`(config.rs,Step 3 形态 + 演进)

```rust
pub struct RemoteState {
    pub db: SqlitePool,
    pub shared_secret: String,
    // Step 5 加:pub node_connections: Arc<DashMap<String, TunnelConn>>
    // Step 6 加:pub pending: DashMap<u64, PendingReply>
}

impl RemoteState {
    /// init_pool + run_migrations + Arc::new(main 单点调用)
    pub async fn load(config: &RemoteConfig) -> Result<Arc<Self>, sqlx::Error>;
}
```

**与 implement.md 字面 `{ db, shared_secret, node_connections, pending }` 的偏差(说明)**:`TunnelConn`(Step 5)/`PendingReply`(Step 6)类型在 Step 3 尚不存在,先定义占位类型再替换是破坏性浪费。改为**按 step 演进加字段**,每个字段在定义它的 step 落地 —— 更符合"每 step 独立 commit"原则,最终形态一致。

**接线改动**(Step 3 一并完成,让 DB 真正跑起来):
- `main.rs`:`let state = RemoteState::load(&config).await.expect("db init")` → `serve_remote(state, config.port)`
- `server.rs`:`serve_remote(state: Arc<RemoteState>, port)` → `build_router(state)`
- `routes/mod.rs`:`router(state: Arc<RemoteState>)`(health 保持无状态 handler)
- `server.rs` 现有 `serve_remote_serves_health_without_signal` 测试:改用 tempdir 构造 `RemoteState::load`

## 7. 随机源(uuid,零新依赖)

- **device_token(32 字节 = 64 hex)**:`Uuid::new_v4().simple()` × 2 拼接。uuid v4 = 122 位 CSPRNG 随机,两次 = 256 位,足够;`uuid` 已是 Step 2 依赖(health 用),**不新增 rand**。
- **6 位配对码**:`random_six_digit()` = `(u64::from_be_bytes(uuid.as_bytes()) % 1_000_000)` 补零到 6 位,`format!("{:06}", n)`。

## 8. 测试设计(`cargo test -p everlasting-remote --lib db::`)

基础设施:

```rust
// db/tests 用(dev-dep tempfile;TempDir 必须活得比 pool 久 → holder 结构)
struct TestDb { _dir: tempfile::TempDir, pool: SqlitePool }
async fn test_db() -> TestDb   // tempdir → init_pool → run_migrations
```

| 测试 | 断言 |
|---|---|
| `run_migrations` 幂等(连跑两次不报错)+ 三表存在(sqlite_master) | ✓ |
| `upsert_node` 首次插入 + 二次 upsert 更新 display_name/status、行数仍 1 | ✓ |
| `update_node_status` 改状态 + last_seen_at | ✓ |
| `get_node` 命中 / 未命中(None) | ✓ |
| `insert_device` + `get_device_by_token` 命中(含 revoked 原样返回)/ 未命中 | ✓ |
| `random_six_digit` 格式(6 位数字、前导零保留)—— 跑 100 次 | ✓ |
| `insert_pairing_code` 成功 + 同码二次 → `Conflict` | ✓ |
| `generate_and_store_pairing_code` 成功:返回 6 位码、库中可查到、expires_at 正确 | ✓ |
| `redeem_pairing_code` 成功:返回 token(64 hex)/node_id/node_display_name,code used=1,devices 有行 | ✓ |
| redeem 重复(二次)→ `InvalidOrExpiredCode` | ✓ |
| redeem 过期(expires_at 过去)→ `InvalidOrExpiredCode` | ✓ |
| redeem 不存在码 → `InvalidOrExpiredCode` | ✓ |

**撞码 retry 的确定性测试说明**:随机生成撞已有码的概率 1/1M,不可靠构造。覆盖策略:① `insert_pairing_code` 的 `Conflict` 语义单测(确定性);② `generate_and_store_pairing_code` 的正常路径 + `RetryExhausted` 分支靠代码审查(循环 ≤3 次)。诚实记录:retry 循环不做概率性测试。

## 9. 实施顺序(Step 3 内部,一个 commit)

1. `db/mod.rs`(模型 + now_ms + 状态常量)+ `db/pool.rs` + `db/schema.rs` + 迁移测试
2. `db/crud.rs` + 全部 CRUD 测试
3. `config.rs` `RemoteState` + main/server/routes 接线 + 修 server 既有测试
4. 验证:`cargo test -p everlasting-remote`(全绿,含 db::)+ release 冒烟(启动日志确认 remote.db 创建 + 三表就绪)+ daemon 回归不受影响
5. commit:`feat(remote): DB 层(nodes/devices/pairing_codes + CRUD + conflict retry)`

## 10. 与后续 step 的契约(防返工)

| 后续 step | 本 step 提供 | 不变式 |
|---|---|---|
| Step 4 auth | `get_device_by_token`(完整行,含 revoked) | 吊销判断在中间件 |
| Step 5 ws | `upsert_node`(连接注册)+ `update_node_status`(心跳/超时) | 状态常量 `NODE_STATUS_ONLINE/OFFLINE` |
| Step 6 proxy | (auth 经 Step 4) | — |
| Step 7 pairing | `generate_and_store_pairing_code`(internal RPC)+ `redeem_pairing_code`(HTTP) | `Redeemed` wire 直接对应 §3.4 响应字段 |
| Step 8 nodes | `get_node` 之外需按 token 查 node → 复用 `get_device_by_token` + `get_node` | 不新增 SQL |

**验证命令**(implement.md Step 3):
```bash
cargo test -p everlasting-remote --lib db::
```
