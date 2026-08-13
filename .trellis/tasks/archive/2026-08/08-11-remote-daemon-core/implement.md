# S1 Implement — everlasting-remote crate 实施顺序

> **What/Why**:[prd.md](./prd.md) · **How**:[design.md](./design.md)
> **Review 修订**:P1-1~P3-5 全部已吸纳进 design,本文按修订后的 design 执行。
> **执行原则**:每个 step 是独立 commit,有明确验证命令。Step 1-2 是破坏性 workspace 翻转,必须先验证不破现状。

---

## Step 0:Workspace 翻转 spike(0.5h,验证可行性)

**目的**:确认 workspace 翻转不破 daemon/GUI build,再正式动手。

**动作**:
1. 创建根 `Cargo.toml`(`[workspace]` + `members` + `default-members` + `resolver="2"`)
2. 把 `app/src-tauri/Cargo.lock` 移到根(`git mv` + `cargo build` 重新生成确认)
3. `cd app/src-tauri && cargo build --lib` 确认 daemon + GUI 编译照旧
4. `cargo build --bin everlasting-daemon` 确认 daemon bin 产出

**验证**:
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo build --lib
# 期望:0 error,跟翻转前一致
cargo build --bin everlasting-daemon
# 期望:target/release or debug/everlasting-daemon 产出
```

**回滚**:若破现状 → `git checkout Cargo.toml app/src-tauri/Cargo.lock` + 删根 Cargo.toml。

**commit**:`build(workspace): 翻 Cargo workspace,根 Cargo.toml + default-members`(单独 commit,可独立 revert)

---

## Step 1:协议 crate `everlasting-remote-protocol`(P1-1 修订,S1 首个 commit)

**目的**:S1/S2/S3 共享的帧类型 crate,零依赖纯类型。P1-1 修订要求 S1 首个 commit 即创建。

**动作**:
1. `mkdir -p crates/everlasting-remote-protocol/src`
2. `crates/everlasting-remote-protocol/Cargo.toml`:`[package]` name/version/edition,deps 只 `serde` + `serde_json`(序列化用)
3. `src/lib.rs`:`Frame` enum + `StreamEvent` enum + `INTERNAL_PREFIX` const(见 design §3.2)
4. 根 `Cargo.toml` `members` 加 `crates/everlasting-remote-protocol`
5. 单元测试:序列化 round-trip(`Frame::Request{...}` → JSON → 反序列化 → 比对)

**验证**:
```bash
cargo test -p everlasting-remote-protocol
# 期望:round-trip 测试全过
cargo build -p everlasting-remote-protocol
# 期望:零系统库依赖(只 serde)
```

**commit**:`feat(remote): everlasting-remote-protocol crate(Frame/StreamEvent 帧类型)`

---

## Step 2:everlasting-remote crate 骨架(CLI + health + ServeDir)

**目的**:能编译能启动的二进制,加 health 端点 + ServeDir 静态托管(P1-3)。

**动作**:
1. `crates/everlasting-remote/` 完整目录结构(design §1.2)
2. `Cargo.toml`:deps(axum+ws / tokio / tokio-tungstenite / sqlx / serde / dashmap / anyhow / tracing / tracing-subscriber / tower-http[ServeDir] / subtle / clap / hostname)+ depend `everlasting-remote-protocol`
3. `main.rs`:clap CLI(`--port`/`--db-path`/`--shared-secret`)+ tracing init + secret 缺失 panic(Q-S1)
4. `server.rs`:`build_router` + `serve_remote`(仿 daemon `serve_daemon`,但 graceful shutdown 只关 WSS 连接 + 停心跳 task,无 SSE/agent loop)
5. `routes/health.rs`:`GET /health` + `GET /api/v1/health`
6. `routes/mod.rs`:router 装配(此 step 只 health + ServeDir fallback)
7. ServeDir:`resolve_dist_dir()` 查 `EVERLASTING_REMOTE_DIST_DIR` env 或默认 `./dist`,SPA fallback index.html(P1-3)
8. `error.rs`:`AppError` + 5 变体 `ErrorCategory`(P3-1 全)+ `IntoResponse`

**验证**:
```bash
cargo build -p everlasting-remote --release
# 期望:ubuntu 二进制产出
./target/release/everlasting-remote --port 7458 --shared-secret test123 &
curl http://localhost:7458/health  # 200
curl http://localhost:7458/api/v1/health  # 200
# 缺 secret:
./target/release/everlasting-remote --port 7458  # panic + 明确报错
kill %1
```

**commit**:`feat(remote): everlasting-remote crate 骨架(CLI + health + ServeDir + error)`

---

## Step 3:DB 层(nodes / devices / pairing_codes)

**目的**:remote 自己的 SQLite pool + schema + CRUD。

**动作**:
1. `db/pool.rs`:`init_pool(db_path)` —— sqlx,WAL + busy_timeout(仿 daemon `migrations/pool.rs`)
2. `db/schema.rs`:`run_migrations(pool)` —— `CREATE TABLE IF NOT EXISTS` nodes/devices/pairing_codes + index(design §3.3),幂等
3. `db/crud.rs`:`insert_node` / `update_node_status` / `get_node` / `insert_device` / `get_device_by_token` / `insert_pairing_code`(含 conflict retry P2-3)/ `redeem_pairing_code`(事务:校验未过期未用 + insert device + mark used)
4. `config.rs`:`RemoteState { db, shared_secret, node_connections, pending }` —— axum 共享 state(对应 daemon AppState 角色)

**验证**:
```bash
cargo test -p everlasting-remote --lib db::
# 期望:CRUD + migration 测试全过
```

**commit**:`feat(remote): DB 层(nodes/devices/pairing_codes + CRUD + conflict retry)`

---

## Step 4:auth 中间件(shared_secret + device_token 双通道,P1-2)

**目的**:WSS 握手 secret 校验 + 手机 HTTP token 校验(header 或 query 二选一)。

**动作**:
1. `auth.rs`:
   - `verify_shared_secret(query_secret) -> bool` —— `subtle::ConstantTimeEq` 常时比较(P3-4)
   - `device_token_from_request(headers, query) -> Option<String>` —— 先查 `Authorization: Bearer`,无则查 `?access_token=`(P1-2)
   - axum middleware `require_device_token`:提取 token → 查 devices 表 → 有效则注入 `AuthenticatedDevice { token, node_id }` 到 request extension;无效 401
2. token 剥离逻辑(P2-1):proxy route 在构造 Request 帧前从 headers 移除 `Authorization`(在 Step 6 proxy 实现)

**验证**:
```bash
cargo test -p everlasting-remote --lib auth::
# 期望:secret 常时比较 + token 双通道提取 + 无效 token 401 测试全过
```

**commit**:`feat(remote): auth 中间件(shared_secret 常时比较 + device_token header/query 双通道)`

---

## Step 5:WSS 服务端(routes/ws.rs + tunnel_registry)

**目的**:收 PC daemon outbound 连接,注册 node,心跳判离线。

**动作**:
1. `tunnel_registry.rs`:`DashMap<String, TunnelConn>` + `register` / `remove` / `get` + 在线判定
2. `routes/ws.rs`:
   - `GET /ws` handler:`WebSocketUpgrade` → 握手时校验 `?secret=`(常时比较)+ 读 `?node_id=` `?display_name=`(percent-decode,P2-1 编码反向)
   - 升级后:`tunnel_registry.register(node_id, conn)` + upsert `nodes` 表(online)
   - 心跳 task:30s ping,90s 无 pong → `tunnel_registry.remove` + `nodes.status=offline`
   - 接收循环:Frame::Request path 以 `INTERNAL_PREFIX` 开头 → `handle_internal_rpc`(Step 7);否则 S1 阶段 log + 忽略(S2 后才有手机转发方向)(P3-2)
3. `TunnelConn`:持 `SplitSink`(发 Frame) + `SplitStream`(收 Frame)

**验证**:
```bash
cargo test -p everlasting-remote --lib ws:: tunnel_registry::
# 期望:握手 secret 校验 + node 注册 + 心跳超时离线 测试全过
# 手动 wscat 验证:
./target/release/everlasting-remote --port 7458 --shared-secret test123 &
# wscat -c "ws://localhost:7458/ws?secret=test123&node_id=test-pc&display_name=TestPC"
# 期望:连接成功;curl 错误 secret → 连接被拒
```

**commit**:`feat(remote): WSS 服务端 + tunnel_registry + 心跳判离线`

---

## Step 6:手机反向代理骨架(routes/proxy.rs + pending,P2-2)

**目的**:手机 HTTP 请求 → token 认证 → 找 node WSS → Request 帧转发 → Response 回传(非流式,SSE 留 S3)。

**动作**:
1. `PendingReply` enum(P2-2):`Oneshot(oneshot::Sender<Frame>)` / `Stream(mpsc::Sender<StreamEvent>)`(Stream 分支 S1 不实例化,为 S3 预留)
2. `pending: DashMap<u64, PendingReply>` + 每条 60s 超时(P2-2,超时返 502 Network)
3. `routes/proxy.rs`:
   - 路由 `* /api/v1/proxy/*path`(catch-all,带 `require_device_token` middleware)
   - 从 extension 拿 `AuthenticatedDevice.node_id` → `tunnel_registry.get(node_id)`
   - node 离线 → 502 `{"category":"Network","message":"node_offline"}`
   - node 在线 → 生成 request_id → `pending.insert(id, Oneshot(tx))` → 构造 `Frame::Request`(path 剥 `/api/v1/proxy` 前缀 + **剥 Authorization header** P2-1)→ `TunnelConn.send(Request)` → `oneshot::rx.await`(60s 超时)→ 解 Response → 返手机 HTTP
4. request_id 生成器:原子递增 u64

**验证**:
```bash
cargo test -p everlasting-remote --lib proxy:: pending::
# 期望:转发骨架 + 剥 token + 离线 502 + 超时 502 测试全过
```

**commit**:`feat(remote): 手机反向代理骨架(PendingReply + 非流式转发 + token 剥离 + 超时)`

---

## Step 7:配对码生命周期(routes/pairing.rs + internal RPC,P2-3)

**目的**:PC 经 WSS 请求生成码 + 手机 HTTP redeem + per-IP 限速。

**动作**:
1. `routes/pairing.rs`:
   - `POST /api/v1/pairing/redeem`(无 device_token middleware,配对时还没 token):body `{code, device_name}` → per-IP 限速(P2-3,10/min,超限 429 RateLimit)→ `db::redeem_pairing_code`(事务)→ 签发 32 字节 hex token + insert device → 返 `{device_token, node_id, node_display_name}`
2. `handle_internal_rpc(path, body)`(Step 5 ws.rs 调用):
   - `/internal/pairing/generate` → 生成 6 位码(conflict retry 2-3 次 P2-3)→ `insert_pairing_code`(60s 过期)→ 返 `{code, expires_in: 60}`
   - 通过 WSS 以 `Frame::Response` 回给 PC daemon

**验证**:
```bash
cargo test -p everlasting-remote --lib pairing::
# 期望:生成 + redeem + 过期 + 重复用 + 限速 + 撞码 retry 测试全过
```

**commit**:`feat(remote): 配对码生命周期(internal RPC 生成 + HTTP redeem + per-IP 限速)`

---

## Step 8:节点状态 API(routes/nodes.rs)

**目的**:手机首页看已配对节点 + 在线状态。

**动作**:
1. `routes/nodes.rs`:`GET /api/v1/nodes`(带 `require_device_token` middleware)→ 查 devices 表(该 token 绑定的 node)+ join nodes 表拿 status → 返 `[{node_id, display_name, status, last_seen_at}]`

**验证**:
```bash
cargo test -p everlasting-remote --lib nodes::
# 期望:token → 对应 node 列表 + status 反映在线状态
```

**commit**:`feat(remote): 节点状态 API(GET /api/v1/nodes)`

---

## Step 9:端到端冒烟(用 wscat 模拟 PC daemon)

**目的**:无 S2 时,手动 wscat 模拟 PC daemon 验证完整链路。

**动作**:
1. 启 remote:`./everlasting-remote --port 7458 --shared-secret test123`
2. wscat 连:`wscat -c "ws://localhost:7458/ws?secret=test123&node_id=test-pc&display_name=TestPC"`
3. wscat 发配对码生成请求:`{"kind":"request","id":1,"method":"POST","path":"/internal/pairing/generate","headers":[],"body":[]}`
4. wscat 收到 Response 含 6 位码
5. curl redeem:`curl -X POST localhost:7458/api/v1/pairing/redeem -d '{"code":"XXXXXX","device_name":"test"}'` → 拿 token
6. curl nodes:`curl -H "Authorization: Bearer <token>" localhost:7458/api/v1/nodes` → 看 test-pc 在线
7. wscat 模拟 PC 响应手机请求:手机 curl `curl -H "Authorization: Bearer <token>" localhost:7458/api/v1/proxy/api/v1/health` → remote 发 Request 帧 → wscat 手动敲 Response 帧 → 手机收到

**验证**:上面 7 步全过 = S1 骨架端到端通。

**commit**:`test(remote): S1 端到端冒烟测试(wscat 模拟 PC daemon)`

---

## Step 10:文档 + scripts

**目的**:部署文档 + nginx 示例 + scripts/remote.sh(仿 daemon.sh)。

**动作**:
1. `scripts/remote.sh`:仿 `scripts/daemon.sh`(start/stop/restart/status/logs),管理 everlasting-remote 进程
2. S1 design §6.1 nginx 示例(P3-5 proxy_read_timeout + P3-4 access_log off)整理进 docs
3. README 段落(可选)

**commit**:`chore(remote): scripts/remote.sh + 部署文档(nginx 示例)`

---

## S1 完成标准(对应 PRD 验收)

- [ ] Step 0-10 全部 commit
- [ ] `cargo build --release -p everlasting-remote` 产出 ubuntu 二进制
- [ ] `cargo test -p everlasting-remote` 全绿
- [ ] Step 9 端到端冒烟 7 步全过
- [ ] 现有 daemon + GUI build 不破(Step 0 回归验证)
- [ ] CI rust-cache `workspaces: '.'`(P3-3,改 `.github/workflows/ci.yml`)

## 实施顺序总结

```
Step 0 (workspace spike) → Step 1 (protocol crate) → Step 2 (骨架)
  → Step 3 (DB) → Step 4 (auth) → Step 5 (WSS) → Step 6 (proxy)
  → Step 7 (pairing) → Step 8 (nodes) → Step 9 (e2e 冒烟) → Step 10 (docs)
```

Step 0-2 是地基(破坏性 workspace + 二进制能跑),Step 3-8 是功能层(可并行/分人),Step 9-10 收尾。每个 step 独立 commit + 独立验证。
