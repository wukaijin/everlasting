# S1 Design — everlasting-remote crate(WSS 服务端 + devices 表 + 反向代理骨架)

> **What/Why**:见 [prd.md](./prd.md)。本文是 **How**。
> **决策汇总**:[parent PRD](../08-11-remote-control-epic/prd.md)。
> **代码库事实**基于 2026-08-11 探查(单 crate + 两 bin,非 workspace;SSE 单全局 fan-out;`app_config` KV 表;axum 0.7 无 `ws` feature)。

---

## 1. 架构总览

### 1.1 进程拓扑(S1 完成态)

```
┌─────────────────────────────┐         ┌──────────────────────────────┐
│ 云服务器(国内 2C2G)         │         │ PC(NAT 后)                   │
│                             │         │                              │
│  everlasting-remote (bin)   │  ◄WSS── │  everlasting-daemon (bin)    │
│   ├─ axum HTTP server :7457  │ outbound│   ├─ agent core(不动)        │
│   ├─ /ws  (PC daemon 入口)   │ ────────│   ├─ tunnel client 模块(S2) │
│   ├─ /api/v1/* (手机入口)    │         │   └─ axum :7456(本地 API)   │
│   ├─ /health                 │         └──────────────────────────────┘
│   ├─ remote.db (SQLite)      │
│   │   ├─ nodes               │         ┌──────────────────────────────┐
│   │   ├─ devices             │         │ 手机 PWA / 家里电脑浏览器     │
│   │   └─ pairing_codes       │ ─HTTPS─►│                              │
│   └─ node_connections 表     │         │ (nginx 反代 + 证书,用户自理) │
│     (DashMap<node_id, conn>) │         └──────────────────────────────┘
└─────────────────────────────┘
```

S1 只建**云服务器那一侧**。PC 侧 tunnel client 是 S2,S3 联调。

### 1.2 模块边界(新增)

**repo 结构变更 —— 翻 workspace**(grill 决策):

```
Cargo.toml                              [新增] workspace 根
├── app/src-tauri/                      [改] 现有 crate,改为 workspace member
└── crates/                             [新增]
    ├── everlasting-remote/             [新] 本任务主体
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  (library: server logic, 可测试)
    │       ├── main.rs                 (bin: CLI 解析 + 调 lib)
    │       ├── frame.rs                (Frame enum,S1/S2/S3 共用)
    │       ├── config.rs               (CLI + env 解析)
    │       ├── db/
    │       │   ├── mod.rs
    │       │   ├── pool.rs             (sqlx SQLite pool,独立于 daemon 的 pool)
    │       │   ├── schema.rs           (nodes / devices / pairing_codes CREATE TABLE IF NOT EXISTS)
    │       │   └── crud.rs             (insert_node / get_device / redeem_pairing / ...)
    │       ├── server.rs               (axum router + bind + shutdown)
    │       ├── routes/
    │       │   ├── mod.rs              (router 装配)
    │       │   ├── health.rs           (GET /health,GET /api/v1/health)
    │       │   ├── ws.rs               (GET /ws,WebSocketUpgrade,PC daemon 入口)
    │       │   ├── pairing.rs          (POST /api/v1/pairing/redeem,手机 redeem)
    │       │   ├── nodes.rs            (GET /api/v1/nodes,带 token 中间件)
    │       │   └── proxy.rs            (手机 API 反向代理骨架)
    │       ├── tunnel_registry.rs      (DashMap<node_id, TunnelConn> + 在线/离线判)
    │       ├── auth.rs                 (shared_secret 校验 + device_token 中间件)
    │       └── error.rs                (AppError + IntoResponse)
    └── everlasting-remote-protocol/    [新,S1 首个 commit 创建,P1-1 修订]
        └── src/lib.rs                  (纯 Frame enum + StreamEvent,零依赖,S1/S2 共用)
```

> **P1-1 修订(协议 crate 时序)**:原 design 把协议 crate 排到 S3 才抽,S2 并行时无帧类型可用 → 矛盾。**S1 的第一个 commit 直接创建 `crates/everlasting-remote-protocol`**(纯类型 enum + serde,几十行零依赖),`everlasting-remote` 的 `frame.rs` 删掉、改 depend protocol crate。S3 的"抽出"改为"双端确认改用 crate、删除任何残留本地定义"。

**workspace 根 `Cargo.toml`**(P3-3 修订:加 `default-members` 避免根裸 build 编 Tauri 重依赖):
```toml
[workspace]
members  = ["app/src-tauri", "crates/everlasting-remote", "crates/everlasting-remote-protocol"]
resolver = "2"
default-members = ["crates/everlasting-remote", "crates/everlasting-remote-protocol"]
```

**`Cargo.lock` 迁移**:`app/src-tauri/Cargo.lock` → `/Cargo.lock`(根)。一次性 `cargo build` 重新生成。

### 1.3 关键不变量

1. **`everlasting-remote` 零依赖 daemon 的 `everlasting_lib`** —— 不拉 libgit2 / agent core / Tauri。依赖只:axum(+ws) / tokio / tokio-tungstenite / sqlx / serde / serde_json / dashmap / anyhow / tracing。确保 ubuntu 二进制最小。
2. **remote.db 独立于 daemon 的 everlasting.db** —— 各自 SQLite 文件,remote 只存 token/devices/pairing,**绝不碰 agent 数据**(Q10)。
3. **PC daemon 不连也能编译/运行** —— remote 是独立二进制,S2 才让 daemon 主动连它。
4. **`everlasting-remote-protocol` 零依赖** —— 纯类型 crate,S1 和 S2(`everlasting-remote-protocol` 加进 `everlasting` 的 deps)都 depend,帧定义单源。**S1 首个 commit 即创建**(P1-1 修订),非 S3 抽出。
5. **remote 伺服 PWA 静态文件**(P1-3 修订)—— remote daemon 加 `tower-http::services::ServeDir` + SPA fallback(与 PC daemon 同款,纯 Rust 不破坏零系统库依赖)。PC daemon 的 ServeDir 在 NAT 后手机够不到,必须由 remote 伺服前端,否则 S4 "手机打开 remote 域名加载 PWA" 验收无法成立。部署 = 服务器 scp 一份 `app/dist/`。

---

## 2. 数据流

### 2.1 PC daemon 连接流(WSS 握手)

```
PC daemon                           everlasting-remote
   │                                     │
   │  GET /ws?secret=<shared_secret>      │
   │  Upgrade: websocket                  │
   │ ───────────────────────────────────► │
   │                                     │  auth::verify_shared_secret(query.secret)
   │                                     │   ├─ ok → 继续(**常时比较** `subtle::ConstantTimeEq`,P3-4)
   │                                     │   └─ fail → 401 关闭
   │                                     │  ws.on_accept → tunnel_registry.register(node_id, conn)
   │                                     │  spawn 心跳 task(30s ping)
   │  ◄──── WebSocket 升级成功 ──────────│
   │                                     │
   │  (长连接维持,S2 负责发 Request 帧) │
```

**node_id 派生**:PC daemon 连接时在 query 传 `node_id`(S2 实现时从 hostname/MAC 派生稳定值)。remote 不主动派生 —— 接受 PC 自报。重复 node_id → 新连接踢旧(`tunnel_registry` 覆盖)。

### 2.2 手机请求流(反向代理骨架,S1 只做非流式)

```
手机                   remote                         PC daemon(S2 实现)
 │                       │                                │
 │ POST /api/v1/sessions/list                            │
 │ Authorization: Bearer <device_token>                  │
 │ ───────────────────► │                                │
 │                       │ auth 中间件:                   │
 │                       │  devices.get(token)            │
 │                       │   → node_id = "company-pc"     │
 │                       │ tunnel_registry.get(node_id)   │
 │                       │   → Some(conn)                 │
 │                       │ 生成 request_id = next_id()     │
 │                       │ pending.insert(id, oneshot)    │
 │                       │ 发 Frame::Request{             │
 │                       │   id, method: POST,            │
 │                       │   path: /api/v1/sessions/list, │
 │                       │   headers, body                │
 │                       │ }                              │
 │                       │ ─────────────────────────────► │
 │                       │                                │ (S2:打 localhost:7456)
 │                       │                                │
 │                       │ ◄──── Frame::Response{...} ─── │
 │                       │ pending.remove(id)             │
 │                       │   → oneshot.recv → Response    │
│ ◄──── HTTP 200 ────── │                                │
│   body: <sessions>    │                                │
```

**P2-1 修订(剥离 Authorization)**:构造 Request 帧时**剔除 `Authorization` header**(已消费;PC 本地无认证,且 device_token 不应出现在 PC 侧日志/代码路径)。这步在 remote 侧 `proxy.rs` 完成,见 §3.1。

**P1-2 修订(SSE 认证 = query token)**:浏览器 `EventSource` API **无法设置请求头**,手机 SSE 走 `${remoteDomain}/api/v1/proxy/api/v1/stream` 时无法带 `Authorization: Bearer`。auth 中间件必须接受 **"Bearer header 或 `?access_token=<token>` query 二选一"**;SSE 路径必然走 query。认证后从 path/query 剥掉 token,转发给 PC 的 Request 帧**不带 token**(P2-1 同一处逻辑)。契约写死 §3.1。

**S1 范围**:上面流程的非流式部分。`Stream` 帧(SSE 桥接)的 remote 侧处理在 S3。`pending` 结构 S1 直接按 P2-2 修订的 `PendingReply` enum 落地(为 SSE 预留),非流式只用 Oneshot 分支。

### 2.3 配对码流

```
PC daemon (S2)           remote                       手机 PWA
   │                       │                            │
   │ Frame::Request{       │                            │
   │   path:/internal/     │                            │
   │   pairing/generate    │                            │
   │ } ──────────────────► │                            │
   │                       │ code = random 6 digit      │
   │                       │ pairing_codes.insert(      │
   │                       │   code, node_id, +60s)     │
   │ ◄── Response{code}── │                            │
   │                                                       │
   │ (PC 前端展示码,用户念给手机)                          │
   │                                                       │ POST /api/v1/pairing/redeem
   │                                                       │ {code, device_name}
   │                                                       │ ──────────────────────► │
   │                       │ pairing_codes.get(code)      │
   │                       │   ├─ 过期/已用 → 400          │
   │                       │   └─ ok:                     │
   │                       │      token = random 32 byte  │
   │                       │      devices.insert(         │
   │                       │        token, node_id, ...)  │
   │                       │      pairing_codes.mark_used │
   │                       │ ◄── {device_token, node_id}─│
```

**配对码来源**:PC daemon 通过 WSS 请求 remote 生成(不是 PC 本地生成后上报)—— remote 是 single source of truth,避免双端状态不一致。这跟 PRD "PC daemon 端生成"表述有微调:生成**动作**由 PC 触发,但**落库**在 remote。语义一致(PC 发起,remote 记账)。

**P2-3 修订(暴力破解 + 撞码)**:
1. **redeem 限速**:6 位码 = 1M 空间 + 60s 窗口 + 公网可达(epic NFR 威胁模型是"扫描器每天扫"),无限制时可暴力扫。`/api/v1/pairing/redeem` 加 **per-IP 限速**(10 次/分钟,内存计数 `DashMap<IpAddr, (count, window_start)>`,MVP 不需 Redis),超限返 429 + `ErrorCategory::RateLimit`。
2. **撞码 retry**:两 node 同时生成可能撞 6 位码 → `INSERT pairing_codes PRIMARY KEY` conflict → 500。生成码时 catch conflict → retry 2-3 次重新随机;仍冲突(极小概率)返 500。

### 2.3.1 `/internal/` RPC 接收分派(P3-2)

`routes/ws.rs` 的帧接收循环分派规则(P3-2 修订):

```
ws recv loop:
  Frame::Request{path, ..} =>
    if path.starts_with(INTERNAL_PREFIX) {  // "/internal/"
      handle_internal_rpc(path, body)  // 本任务:/internal/pairing/generate
        → 生成配对码(§2.3)
    } else {
      // S1 阶段 PC 不发请求,log + 忽略
      // S2 后:手机→remote→PC 的转发,PC 的 Request 才会进这里(实际是 remote→PC 方向)
    }
  Frame::Response{..} | Frame::Stream{..} =>
    路由到 pending 表(§3.2.1)
```

**PRD 措辞统一(P3-2)**:PRD 写 `POST /api/v1/internal/pairing/generate`,design 写 `/internal/pairing/generate`(`INTERNAL_PREFIX`)。以 design 为准 —— 这是 **WS 内部 RPC**,不是 HTTP 路由,不带 `/api/v1` 前缀。

### 2.4 节点状态 / 心跳

```
PC daemon           remote
   │  (WSS 连接建立后)     │
   │                   │  spawn heartbeat task:
   │ ◄── ping(30s)─── │    每 30s 发 WebSocket Ping
   │ ─── pong ───────► │    收到 Pong → nodes.last_seen_at = now
   │                   │  超时检查 task:
   │                   │    每 30s 扫 nodes,now - last_seen_at > 90s
   │                   │    → status = offline + tunnel_registry.remove(node_id)
```

**离线判定**:基于 WebSocket ping/pong,不依赖应用层心跳。`axum::extract::ws` 的 `Ping`/`Pong` 帧够用。

---

## 3. 契约 / 接口

### 3.1 URL 命名

| 方法 | 路径 | 认证 | 用途 |
|---|---|---|---|
| GET | `/health` | 无 | nginx 健康检查 |
| GET | `/api/v1/health` | 无 | 同上(跟 daemon 对齐) |
| GET | `/ws` | query `secret` | PC daemon WSS 入口 |
| POST | `/api/v1/pairing/redeem` | body `{code, device_name}` | 手机配对 |
| GET | `/api/v1/nodes` | `Authorization: Bearer <token>` 或 `?access_token=<token>` | 节点列表(手机首页) |
| `*` | `/api/v1/proxy/*` | `Authorization: Bearer <token>` 或 `?access_token=<token>` | 手机反向代理入口(S1 非流式) |
| GET | `/`(fallback) | 无 | ServeDir 伺服 PWA 静态文件(P1-3) |

**认证双通道(P1-2 修订)**:auth 中间件接受:
1. `Authorization: Bearer <device_token>` header(普通 fetch 请求)
2. `?access_token=<device_token>` query(**EventSource 专用**,浏览器无法设 header)

认证后**立即从 path/query 剥掉 token**,转发给 PC 的 Request 帧 path 是纯净的 `/api/v1/...`,**不含 `access_token` query**(P2-1:Authorization header 同步剥离)。PC daemon 的 `/api/v1/stream` 不需要也不该看到手机 token。

**手机 API 反向代理的 URL 设计**:手机调的不是 PC daemon 的原始 URL,而是 remote 的 `/api/v1/proxy/<原始 path>`。例如手机要调 PC 的 `POST /api/v1/sessions/list` → 实际发 `POST /api/v1/proxy/api/v1/sessions/list` 到 remote。remote 中间件解 token → 找 node WSS → Request 帧 path 字段填 `/api/v1/sessions/list`(剥掉 `/api/v1/proxy` 前缀)。

> **决策 Q-P1**:`/api/v1/proxy/*` 前缀 vs 透传原始 path?选前缀。理由:① remote 自己的 `/api/v1/nodes`、`/api/v1/pairing`、`/api/v1/health` 跟 PC daemon 的 `/api/v1/*` 命名空间会冲突,前缀隔离干净;② 前端 PWA-remote transport 层统一加 `/api/v1/proxy` 前缀,transport 实现简单。

> **决策 Q-P2(P1-3)**:remote 加 ServeDir 伺服 PWA。理由:PC daemon 的 ServeDir 在 NAT 后,手机从 remote 域名加载前端必须由 remote 自己伺服;纯 Rust 不破坏零系统库依赖。与 PC daemon 同款 `ServeDir::new(dist).not_found_service(ServeFile(index.html))` SPA fallback,`resolve_dist_dir()` 查 `EVERLASTING_REMOTE_DIST_DIR` env 或默认路径。

### 3.2 帧定义(`everlasting-remote-protocol`,S1 本地先放 `frame.rs`)

```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    /// remote → PC:一次 HTTP 请求(手机反向代理或 remote 内部)
    Request {
        id: u64,
        method: String,        // "GET" / "POST" / ...
        path: String,          // "/api/v1/sessions/list"(已剥 proxy 前缀)
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// PC → remote:非流式响应
    Response {
        id: u64,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// 双向:流式(SSE chunked),按 id 关联 Request
    Stream {
        id: u64,
        event: StreamEvent,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamEvent {
    Chunk { bytes: Vec<u8> },  // SSE 的一段
    End,
    Error { message: String },
}

/// remote 内部 RPC(PC daemon → remote 的配对码生成等内部请求,也走 Request 帧)
pub const INTERNAL_PREFIX: &str = "/internal/";
```

**序列化**:JSON(S1/S3 MVP)。`serde(tag = "kind")` 内部 tagged,前向兼容(加新枚举变体不破老)。S3 可选 bincode 优化。

> **决策 Q-F1**:`headers` 用 `Vec<(String,String)>` 不用 `HashMap`。理由:HTTP header 有序 + 可重复(`Set-Cookie` 多个),HashMap 丢序丢重复。

### 3.2.1 `PendingReply`(P2-2 修订)

```rust
// remote 侧 pending 表,S1 直接按此结构落地(为 SSE 流式预留,S3 只加 Stream 分支使用路径)
enum PendingReply {
    Oneshot(oneshot::Sender<Frame>),        // 非流式:Response 一次
    Stream(mpsc::Sender<StreamEvent>),      // SSE:持续 chunk,End/Error 收尾(S3 用)
}
// DashMap<u64, PendingReply> + 每条挂 60s 超时 → 502/504 + 清理
```

**P2-2 修订**:原 design 的 `pending: DashMap<id, oneshot::Sender<Frame>>` 无超时(PC 静默 → 手机挂死 + 内存泄漏)+ 结构只装得下非流式(S3 上 SSE 时要重构)。S1 直接落地 `PendingReply` enum:非流式走 Oneshot 分支(每条 60s 超时,超时返 `ErrorCategory::Network` + 502),Stream 分支为 S3 预留但 S1 不实例化。超时值 60s 与配对码同量级。

### 3.3 数据模型(remote.db schema)

```sql
CREATE TABLE IF NOT EXISTS nodes (
    id TEXT PRIMARY KEY,              -- PC daemon 自报的稳定 node_id
    display_name TEXT NOT NULL,       -- "公司 PC" / "家里 PC"(PC 自报或默认)
    status TEXT NOT NULL,             -- "online" / "offline"
    last_seen_at INTEGER NOT NULL,    -- unix epoch ms
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS devices (
    token TEXT PRIMARY KEY,           -- 32 字节 hex(配对时签发)
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

**migration 风格**:跟 daemon 一致 —— `schema.rs::run_migrations(pool)` 一函数全 `CREATE TABLE IF NOT EXISTS` + `CREATE INDEX IF NOT EXISTS`,启动时无条件跑(`pool.rs::init_pool` 后调用)。不引入 timestamped SQL 文件。

### 3.4 配对 wire 示例

```jsonc
// POST /api/v1/pairing/redeem
// Request body:
{ "code": "123456", "device_name": "Carlos 的 iPhone" }

// Response 200:
{
  "device_token": "a1b2c3...(64 hex chars)",
  "node_id": "company-pc",
  "node_display_name": "公司 PC"
}

// Response 400(过期/已用/不存在):
{ "error": "invalid_or_expired_code" }
```

### 3.5 错误格式(跟 daemon `AppCommandError` 对齐,P3-1 修订)

```jsonc
// 所有 4xx/5xx 响应(category 用 PascalCase 序列化,与 daemon error.rs:33-39 一致):
{
  "category": "Auth" | "RateLimit" | "InvalidRequest" | "Server" | "Network",
  "message": "..."
}
```

**5 变体全列**(P3-1 修订:原 design 漏 `RateLimit`):前端按 category 路由(`Auth→Settings` / `RateLimit→toast` / `InvalidRequest→inline 错误` / `Server→toast` / `Network→toast`)。状态码:`Auth→401`, `RateLimit→429`, `InvalidRequest→400`, `Server→500`, `Network→502`(`node_offline` 也走 502 + `category: Network`)。remote 实现全 5 变体,避免前端遇 429 路由错乱。`#[serde(rename_all = "PascalCase")]` 跟 daemon。

### 3.6 CLI

```bash
everlasting-remote \
  --port 7457 \
  --db-path ~/.local/share/dev.everlasting.remote/remote.db \
  --shared-secret <SECRET>
# 或环境变量:
#   EVERLASTING_REMOTE_PORT=7457
#   EVERLASTING_REMOTE_DB_PATH=...
#   EVERLASTING_REMOTE_SECRET=...
```

优先级:CLI flag > env > default(`7457` / `~/.local/share/dev.everlasting.remote/remote.db` / **secret 无默认,必传**)。

> **决策 Q-S1**:secret 无默认必传 —— 启动时缺 secret 直接 panic + 明确报错。强制安全,防裸跑。

---

## 4. 兼容性 / 迁移

### 4.1 workspace 翻转的兼容期

翻 workspace 是**一次性破坏性改动**,但影响面可控:
- `Cargo.lock` 从 `app/src-tauri/` 迁到根 —— `cargo build` 重新生成,git diff 一次大改动
- `EVERLASTING_APP_IDENTIFIER` 注入:`everlasting-remote` 不需要它(自己用固定 `dev.everlasting.remote` 子目录),但 `everlasting`(daemon)的 `env!("EVERLASTING_APP_IDENTIFIER")` 不变 —— daemon build.rs 不动
- 现有 `scripts/daemon.sh`、CI workflow(`.github/workflows/`)的 `cd app/src-tauri && cargo ...` 路径**不变**(workspace member 内 cargo 命令照旧工作)
- **CI rust-cache `workspaces` 键要改(P3-3)**:`.github/workflows/ci.yml:57` 现是 `workspaces: 'app/src-tauri'`,Cargo.lock 迁到根后会缓存 miss(每 CI 跑全量编译,~60s → 分钟级)。改 `workspaces: '.'`。根 `Cargo.toml` 加 `default-members`(见 §1.2)避免根裸 `cargo build` 编 Tauri 重依赖;daemon 侧 CI 仍显式 `cargo build -p everlasting`/`--bin everlasting-daemon`(行为不变)。
- 前端 `pnpm`/vite 完全不受影响

### 4.2 回滚形状

S1 全是新文件 + workspace 根 `Cargo.toml`。回滚 = `git revert` 单 PR + 删 `crates/` 目录。daemon 零改动 → 回滚后 daemon 行为不变。

---

## 5. 关键权衡

| 决策 | 选择 | 理由 | 否决项 |
|---|---|---|---|
| crate 组织 | workspace + 独立 crate | 零依赖 daemon 重库,二进制最小 | 加 bin target:拉 libgit2,体积大 |
| 帧序列化 | JSON | 可调试(S3 联调时能 wscat 看帧) | bincode:省带宽但调试难,S3 优化期再换 |
| node_id 来源 | PC 自报 | daemon 知道自己身份(hostname 派生稳定) | remote 派生:remote 看不到 PC 的 MAC/hostname |
| 配对码生成位置 | remote 落库,PC 触发 | single source of truth,避免双端不一致 | PC 生成后上报:双写竞态 |
| 手机 API URL | `/api/v1/proxy/*` 前缀 | 隔离 remote 自身 API 命名空间 | 透传原始 path:与 remote 的 `/api/v1/nodes` 冲突 |
| 离线判定 | WS ping/pong | 协议层,零应用心跳代码 | 应用层心跳:多一层状态机 |
| secret 缺失 | panic 启动 | 强制安全 | 默认值:裸跑风险 |

---

## 6. 运营 / 回滚

### 6.1 部署形态

```bash
# 云服务器(ubuntu):
# 1. 编译(可本地 cross 或服务器上):
cargo build --release -p everlasting-remote
# 产物:target/release/everlasting-remote(ELF,ubuntu 直接跑)

# 2. 跑:
./everlasting-remote --port 7457 --shared-secret <SECRET> &

# 3. nginx 反代(用户自理,示例):
# server {
#   listen 443 ssl;
#   server_name remote.yourdomain.com;
#   location / { proxy_pass http://127.0.0.1:7457; }
#   location /ws { proxy_pass http://127.0.0.1:7457; proxy_http_version 1.1;
#                  proxy_set_header Upgrade $http_upgrade; proxy_set_header Connection "upgrade";
#                  proxy_read_timeout 300s; access_log off; }  # P3-5: 心跳倍数; P3-4: 防止 access log 记录 secret query
# }
```

**`/ws` 必须单独 nginx location** —— WebSocket 升级头要透传,普通 HTTP 反代会断 WSS。**`proxy_read_timeout 300s`** 必加(P3-5):心跳 30s × 倍数,避免空闲期被 nginx 默认 60s 掐断。**`access_log off`** 建议(P3-4):`/ws?secret=<S>` query 会被 nginx access log 记进 `$request`,导致服务器日志躺着 shared_secret。

### 6.2 日志

`tracing` + `tracing-subscriber`(跟 daemon 一致)。关键事件:
- `INFO tunnel_connected node_id=...`(PC 连上)
- `INFO tunnel_disconnected node_id=... reason=...`(PC 断开/超时)
- `INFO pairing_redeemed node_id=... device=...`(配对成功)
- `WARN shared_secret_rejected ip=...`(伪 daemon 尝试)
- `ERROR` 只用于不可恢复(panic 前的日志)

### 6.3 监控(最小)

S1 不做 metrics 端点。运维靠:日志 + `GET /health` + `GET /api/v1/nodes`(带 token 看在线状态)。V2 加 Prometheus 端点(若需要)。

---

## 7. 与现有代码的对齐

| 现有约定 | 本任务遵守 |
|---|---|
| daemon `AppCommandError` 错误格式 + status map | remote 用同款 `AppError`(复制,不依赖 daemon crate) |
| sqlx + WAL + busy_timeout pool 配置 | remote.db 同款 `init_pool` |
| `CREATE TABLE IF NOT EXISTS` 幂等 migration | remote schema.rs 同款 |
| snake_case body 字段 + camelCase 错误 category | remote 跟 |
| `tracing` + `EnvFilter` | remote 跟 |
| axum `State<Arc<AppState>>` 模式 | remote 用 `State<Arc<RemoteState>>`(自己定义) |

**不依赖 daemon 的东西**:agent core / tools / provider / Tauri / libgit2 —— 全不拉。remote 是干净的反向代理 + 认证网关。

---

## 附录 A:S1 验收 checklist(对应 PRD)

| PRD 验收项 | S1 实现位置 |
|---|---|
| `cargo build --release -p everlasting-remote` 产出二进制 | workspace + crates/everlasting-remote |
| `--port --shared-secret` 启动,`GET /health` 200 | config.rs + routes/health.rs |
| PC(wscat 模拟)带 secret 连 WSS → 注册成 node | routes/ws.rs + tunnel_registry + db/crud |
| 错误 secret → 401 | auth.rs |
| 配对码生成 → redeem → token | routes/pairing.rs + db/crud |
| token 请求透传到 PC(非流式) | routes/proxy.rs + pending DashMap |
| node 离线 → 502 `node_offline` | tunnel_registry + error.rs |
| 心跳 30s/90s | routes/ws.rs heartbeat task |

## 附录 B:前置 spike(实施前 30 min 验证)

- [ ] workspace 翻转最小验证:根 Cargo.toml + 空 member,`cargo build` 确认 build.rs/identifier 不破
- [ ] axum `ws` feature 验证:`axum = { version = "0.7", features = ["ws"] }` 加进 everlasting-remote Cargo.toml,WSS 升级能跑
- [ ] sqlx 在新 crate 独立 pool 验证:不依赖 daemon 的 `db` 模块,自己 `init_pool` 能开 remote.db
