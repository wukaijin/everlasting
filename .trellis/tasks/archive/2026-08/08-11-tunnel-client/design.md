# S2 Design — PC daemon tunnel client 模块

> **What/Why**:见 [prd.md](./prd.md)。本文是 **How**。
> **决策汇总**:[parent PRD](../08-11-remote-control-epic/prd.md)。
> **S1 契约**:[S1 design](../08-11-remote-daemon-core/design.md) 的 `Frame` 协议 + `/ws` 握手 + `/internal/pairing/generate` RPC。
> **代码库事实**:daemon main = `bin/everlasting-daemon.rs`(调 `server::load_daemon_state` + `server::serve_daemon`);AppState 17 字段;SSE 单全局 fan-out(`SseRegistry`,broadcast 给所有订阅者);config 存 `app_config` KV 表;新 IPC 改动清单见 §3.1(P3-2)。

---

## 1. 架构总览

### 1.1 进程内拓扑(S2 完成态)

```
everlasting-daemon 进程
├─ axum HTTP server :7456(本地 API,不动)
│   └─ /api/v1/stream(SSE,broadcast)
│
└─ tunnel client(新,独立 tokio task)  ◄── opt-in:有 remote_config 才 spawn
    ├─ WSS 长连接 → remote
    ├─ 心跳 task(等 remote ping,响应 pong)
    ├─ dispatcher loop:
    │     收 Frame::Request → reqwest 打 localhost:7456 → Frame::Response/Stream 回传
    │     (含 SSE 流式:检测 text/event-stream → 转 Stream 帧)
    └─ 重连 task(断线指数退避)
```

**关键不变量**(硬约束):
1. **agent core 零改动** —— tunnel 不 import `agent::` / `tools::` / `provider::` 任何模块
2. **本地功能零依赖 remote** —— remote_config 为空时,tunnel 模块根本不 spawn,daemon 行为 = 现状
3. **tunnel client 失败不 crash daemon** —— 连接失败只 log + 后台重试
4. **dispatcher 只用 reqwest 打 loopback** —— 不调 handler 函数,不绕过 axum(Q7 决策)

### 1.2 模块边界(新增)

```
app/src-tauri/src/daemon/tunnel/        [新]
├── mod.rs              pub spawn_tunnel_client(cfg, shutdown) -> JoinHandle<()>
│                       pub struct TunnelConfig { remote_url, shared_secret, node_id, display_name }
├── client.rs           WSS 连接 + 心跳 + 重连循环
├── dispatcher.rs       收 Frame::Request → reqwest localhost:7456 → 回 Frame
├── sse_bridge.rs       SSE 响应检测 → Stream 帧转换(SSE chunked → Stream::Chunk)
└── node_id.rs          hostname 派生稳定 node_id
```

**改动现有文件**:
- `bin/everlasting-daemon.rs`:main 里 `serve_daemon` 前 spawn tunnel client
- `commands/config.rs`:加 `get_remote_config_inner` / `set_remote_config_inner` + `#[tauri::command]`
- `lib.rs`:invoke_handler 注册新 command + Tauri GUI 也 spawn tunnel(对称)
- `daemon/routes/config.rs`:axum route(`get_remote_config` / `set_remote_config`)+ `daemon/routes/pairing.rs`(新,`generate_pairing_code` route)
- `db/migrations/schema.rs`:无改动(`app_config` KV 表复用,加新 key 不需 migration)
- `db/config.rs`:无改动(复用 `get_config_value` / `set_config_value`)
- `Cargo.toml`:加 `everlasting-remote-protocol` dep(S1 抽出的共享 crate)+ `tokio-tungstenite`(WSS 客户端)+ `reqwest`(打 loopback,可能已有)
- 前端 `transport/http.ts`:`CMD_TO_DOMAIN` 加 3 个新 cmd

### 1.3 帧协议引用(P1-2 修订:协议 crate 时序)

**协议 crate 由 S1 首个 commit 创建**(非 S3 抽出 —— 原 S1 design 排到 S3 是时序矛盾,S1 review P1-1 已修)。S2 的 dep 写法不变,实施前提 = S1 落地 crate;**S1 未落地前 S2 可先做 config/IPC/TunnelManager 等不依赖帧类型的部分**,帧类型一到即接。

复用 `everlasting-remote-protocol::Frame`(见 [S1 design §3.2](../08-11-remote-daemon-core/design.md))。S2 加 dep:

```toml
# app/src-tauri/Cargo.toml
[dependencies]
everlasting-remote-protocol = { path = "../../crates/everlasting-remote-protocol" }
tokio-tungstenite = { version = "0.23", features = ["rustls-tls-native-roots"] }  # WSS 客户端
hostname = "0.4"  # P2-1 修订:node_id 派生(原 design 漏列)
```

---

## 2. 数据流

### 2.1 启动 → 连接 → 注册

```
daemon main
   │
   │ let state = server::load_daemon_state(data_dir).await;
   │ let remote_cfg = load_remote_config(&state.db).await?;  // 读 app_config 两个 key
   │ if let Some(cfg) = remote_cfg.filter(|c| !c.remote_url.is_empty()) {
   │     daemon::tunnel::spawn_tunnel_client(cfg, node_id, shutdown_token.clone());
   │ }
   │ server::serve_daemon(state, port).await   // 不阻塞,tunnel 是独立 task
   │
spawn_tunnel_client:
   ├─ tokio::spawn(async {
   │     loop {  // 重连循环
   │       match connect_and_serve(&cfg, &node_id, shutdown).await {
   │         Ok(_) => break,  // 正常关闭
   │         Err(e) => { log; sleep(backoff.next()); }
   │       }
   │     }
   │   })
   │
connect_and_serve:
   ├─ ws = tokio_tungstenite::connect_async(
   │     build_ws_url(&cfg.remote_url, &cfg.shared_secret, &node_id, &display_name))  // P2-1: percent-encode
   │  .await?
   ├─ spawn 心跳 task(等 Ping → 回 Pong)
   └─ serve_loop:
        select! {
          msg = ws.recv() => match Frame::from_msg(msg) {
            Request{..} => spawn dispatch_one(req, ws_tx.clone()),
            _ => log/ignore
          }
          _ = shutdown => close ws
        }
```

**连接 URL(P2-1 修订:URL 编码)**:`wss://<remote_url>/ws?secret=<S>&node_id=<ID>&display_name=<NAME>`。query 值**必须 percent-encode**(`percent-encoding` crate 或 `urlencoding`)—— 中文 `display_name`("公司 PC")、含特殊字符的 secret 直接 `format!` 拼接会导致服务端 query 解析错乱或握手失败。原 design 的裸 `format!` 是 bug。

**query 面最小化(P2-1)**:重连时只带 `secret` + `node_id`(稳定值),`display_name` 只在首次注册带一次,变更走配置(避免每次重连都带中文 + 减小编码出错面)。或更简:display_name 干脆走首次 WSS 帧注册,不走 query。实施时定。

> **决策 Q-T1**:secret 走 query 还是 header?query。理由:WebSocket 握手时 header 自定义需额外 `connect_async_with_config` + `RequestBuilder`,query 简单。HTTPS(nginx 终结)下 query 不被中间人看到(加密)。S1 已按 query 设计。**注意 nginx access log 会记 query 含 secret,见 [S1 design §6.1 P3-4](../08-11-remote-daemon-core/design.md)(建议 `access_log off`)**。

### 2.2 dispatcher:Request → loopback → Response

```
remote                    tunnel client                 localhost:7456
  │                            │                              │
  │ Frame::Request{            │                              │
  │   id: 42,                  │                              │
  │   method: "POST",          │                              │
  │   path: "/api/v1/...",     │                              │
  │   headers: [...],          │                              │
  │   body: <bytes>            │                              │
  │ } ───────────────────────► │                              │
  │                            │ spawn dispatch_one:           │
  │                            │  reqwest::Client::new()       │
  │                            │   .request(method,           │
  │                            │     "http://localhost:7456"  │
  │                            │     + path)                   │
  │                            │   .headers(from_frame)        │
  │                            │   .body(body)                 │
  │                            │   .send().await ────────────►│
  │                            │                              │ (axum 处理)
  │                            │ ◄──── resp ──────────────────│
  │                            │                              │
  │                            │ if resp.headers.content_type │
  │                            │    == "text/event-stream":   │
  │                            │   → SSE 流式路径(§2.3)       │
  │                            │ else:                        │
  │                            │   body = resp.bytes().await  │
  │                            │   Frame::Response{           │
  │                            │     id: 42, status, headers, │
  │                            │     body                     │
  │                            │   }                          │
  │ ◄──────────────────────────│                              │
```

**reqwest client 复用**:`spawn_tunnel_client` 时建一个 `reqwest::Client`(连接池),所有 dispatch 共用,不每请求新建。

**header 透传规则**:
- `Authorization`(手机的 device token)透传 —— 不,等等。**重要**:`Authorization: Bearer <device_token>` 是手机给 remote 的,remote 验证后**剥掉**,转成 Request 帧时不带 device_token 给 PC(PC 不认识)。PC 的 axum 路由不需要 Authorization(本地无认证)。所以:**dispatcher 收到的 Request 帧 headers 不含 Authorization**(remote 侧剥),dispatcher 原样透传剩余 headers 给 localhost。
- `Content-Type` / `Accept` 透传
- `Last-Event-ID`(SSE 续传)透传 —— 这是 SSE 重连关键,必须带

> **决策 Q-T2**:remote 剥 Authorization。S1 的 proxy route 验证 token 后,转 Request 帧时移除 `Authorization` header。PC daemon 的 axum 无认证,不需要这 header。

> **P1-3 修订(SSE query token 跨任务契约)**:浏览器 `EventSource` 无法设 header,手机 SSE 走 remote 时 token 只能走 `?access_token=<token>` query。**S1 auth 中间件已定支持 "Bearer header 或 `?access_token=` 二选一"**(见 [S1 design §3.1 P1-2](../08-11-remote-daemon-core/design.md))。S2 dispatcher 侧:若收到的 Request 帧 path 仍带 `access_token` query,属 remote 未剥净的异常,**dispatcher 原样透传即可(不主动剥)**,以 S1 契约为单源 —— remote 负责认证 + 剥干净,S2 不重复实现剥离逻辑。

### 2.3 SSE 流式:Response → Stream 帧(SSE 全局流透传策略)

**关键设计**:SSE 是单全局 fan-out(`SseRegistry` broadcast 给所有订阅者),不按 session_id。grill 决策:**全局流透传,remote/client 过滤**。

```
remote                    tunnel client                 localhost:7456/stream
  │                            │                              │
  │ Frame::Request{            │                              │
  │   path: "/api/v1/stream",  │                              │
  │   headers: [               │                              │
  │     ("Last-Event-ID","50") │  ← SSE 续传                  │
  │   ]                        │                              │
  │ } ───────────────────────► │                              │
  │                            │ reqwest GET localhost:7456/  │
  │                            │   stream                     │
  │                            │   .header("Last-Event-ID",   │
  │                            │     "50") ──────────────────►│
  │                            │                              │ SseRegistry.subscribe
  │                            │                              │   (Some(50)) → replay + live
  │ ◄── Stream::Chunk ─────── │ ◄──── SSE chunk ─────────────│
  │   (SSE 原文 bytes)         │   "id:51\nevent:chat-event   │
  │                            │    \ndata:{...}\n\n"          │
  │ ◄── Stream::Chunk ─────── │ ◄──── SSE chunk ─────────────│
  │   ...                      │   ...                         │
  │                            │                              │
  │ (手机 EventSource 断开)    │                              │
  │ remote drop oneshot →      │                              │
  │   Frame::Stream{End} ───► │                              │
  │                            │ reqwest resp drop →          │
  │                            │   连接关闭 ◄─────────────────│ axum 检测客户端断开
  │                            │                              │ (但 SSE 是 broadcast,
  │                            │                              │  agent loop 不受影响)
```

**SSE chunk → Stream::Chunk 转换**:
- reqwest 的响应 body 是 `Stream<Item = Result<Bytes>>`(hyper chunked)
- 逐 chunk 读 → 每段包成 `Frame::Stream { id, event: Chunk { bytes } }` 塞回 WSS
- SSE 结束(server 关连接)→ `Stream::End`
- 读出错 → `Stream::Error { message }`

**注意**:SSE chunk 的边界**不一定对齐 SSE event 边界**(`id:51\nevent:...\ndata:...\n\n` 可能被拆到两个 chunk)。**这没关系** —— remote 侧把 Stream::Chunk 的 bytes 原样写给手机的 SSE HTTP body,手机 EventSource 自己按 `\n\n` 解析 event。隧道层不解析 SSE 语义,纯字节透传(见 S3 design Notes)。

**取消传播(SSE 特殊)**:
- 普通 Request(reqwest 打完拿 Response)→ oneshot 一次性,简单
- SSE Request → **持续流**,remote 侧要能"手机断了 → 通知 PC 停止转发"
- 机制:remote 侧为每个 SSE Request 维护一个 `Arc<AbortHandle>`(或 `CancellationToken`),手机 EventSource 断开 → remote 发 `Frame::Stream { id, End }`(或专用 `Cancel { id }` 帧)→ tunnel client 收到 → drop reqwest 的响应 future → reqwest 连接关闭 → localhost SSE 连接断开
- **但**:SSE 是 broadcast,PC daemon 的 agent loop 不会因为一个订阅者断开而停(这是 SseRegistry 的设计 —— 多订阅者)。所以"取消传播"在 SSE 场景**只断开这一个 tunnel 的 localhost SSE 订阅,不停 agent loop**。agent loop 由用户显式 cancel 走 `/api/v1/cancel`。

> **决策 Q-T3**:SSE 取消只断 tunnel 的本地订阅,不传播到 agent loop。理由:SseRegistry 是 broadcast 设计,单订阅者断开不应影响 agent。手机想停 agent → 显式调 `/api/v1/cancel`(经 tunnel 透传)。

### 2.4 配置变更 → 实时重连

```
前端 Settings → set_remote_config({remote_url, shared_secret})
   │
   │ IPC: transport.invoke("set_remote_config", {...})
   ▼
commands/config.rs::set_remote_config_inner(&state, remote_url, secret)
   ├─ P2-2 校验:scheme 必须 wss://(本地调试允许 ws://)
   │             去尾斜杠,失败返 InvalidRequest(前端 inline 提示)
   ├─ set_config_value(&state.db, "remote_url", &remote_url)
   ├─ set_config_value(&state.db, "shared_secret", &secret)
   └─ state.tunnel_manager.notify_config_changed()  ← 新字段
   │
AppState 加新字段:tunnel_manager: Arc<TunnelManager>
TunnelManager {
  current_handle: Mutex<Option<JoinHandle>>,
  config_rx: watch::Receiver<Option<TunnelConfig>>,  // None = 停止
}
notify_config_changed():send 新 config → 当前 tunnel task 自行 shutdown → spawn_tunnel_client 重新拉起
```

**`spawn_tunnel_client` 改为受 `watch::Receiver` 驱动**:config 变 → 旧 task 的 shutdown_token 触发 → 旧 task 退出 → manager spawn 新 task 用新 config。daemon 启动时也走这条路径(初始 config 从 DB 读,send 到 watch channel)。

> **决策 Q-T4**:tunnel 生命周期由 `TunnelManager`(挂 AppState)统一管,不裸 spawn。理由:① config 变更要能停旧 task;② shutdown 要能停 tunnel;③ 状态查询(前端展示"已连接/重连中")需要统一入口。

### 2.5 配对码生成(PC 经 WSS 调 remote 内部 RPC)

```
前端 → generate_pairing_code IPC
   │
   ▼
commands/pairing.rs::generate_pairing_code_inner(&state)
   ├─ tunnel_manager.get_conn() → Option<Frame channel>
   ├─ None → Err("remote 未连接")
   ├─ Some(tx) → 构造 Frame::Request {
   │     id: next_id(),
   │     method: "POST",
   │     path: "/internal/pairing/generate",  ← S1 INTERNAL_PREFIX
   │     body: empty
   │   }
   │   tx.send(request).await
   │   oneshot::wait for Response
   └─ Response body parse → { code, expires_in }
```

`/internal/pairing/generate` 是 remote 的内部 RPC —— 只接受经 WSS 连接的 PC daemon 调用(S1 在 ws 处理器里识别 `/internal/` 前缀),不走手机 HTTP 路由。

---

## 3. 契约 / 接口

### 3.1 新增 IPC 改动清单(P3-2 修订:6 条,含 2 条确认无改动)

| cmd | domain | body | 返回 |
|---|---|---|---|
| `get_remote_config` | `config` | 无 | `{remoteUrl, sharedSecret} | null` |
| `set_remote_config` | `config` | `{remoteUrl, sharedSecret}` | `void`(触发重连) |
| `generate_pairing_code` | `pairing`(新 domain) | 无 | `{code: "123456", expiresIn: 60}` |
| `get_tunnel_status` | `config` | 无 | `{connected: bool, remoteUrl: string, nodeId: string} | null` |

**改动清单(P3-2:原写"5 处"实际 6 条,含前端)**:
1. `commands/config.rs`:`get/set_remote_config_inner` + `get_tunnel_status_inner` + `#[tauri::command]` × 3
2. `commands/pairing.rs`([新]):`generate_pairing_code_inner` + `#[tauri::command]`
3. `lib.rs`:`invoke_handler` 加 4 个
4. `daemon/routes/config.rs`:3 个 axum route
5. `daemon/routes/pairing.rs`([新]):1 个 axum route + `router()`
6. 前端 `transport/http.ts`:`CMD_TO_DOMAIN` 加 4 条

> 注:`schema.rs` / `db/config.rs` **无改动**(复用 `app_config` KV 表 + `get/set_config_value`),不列入编号清单。

### 3.2 config 存储复用 `app_config` KV

```
key: "remote_url"      value: "wss://remote.yourdomain.com/ws"
key: "shared_secret"   value: "<明文或加密>"  ← 见 Q-T5
```

**零 migration** —— `app_config` 表已存在(schema.rs:240),复用 `get_config_value`/`set_config_value`。

> **决策 Q-T5**:shared_secret 存储加密?复用 RULE-D-001 的 AES-256-GCM + HKDF(machine-id)机制。理由:secret 跟 API key 同等敏感(DB 文件泄露不应暴露 secret)。`db::config::set_config_value` 存密文,读时解密。MVP 可先明文(标 follow-up),但 design 预留加密路径。

### 3.3 node_id 派生(`node_id.rs`)

```rust
pub fn derive_node_id() -> String {
    // 优先:hostname;fallback:随机 UUID 持久化到 app_config
    let hostname = hostname::get().ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| {
            // 持久化随机 id(首次生成存 app_config "tunnel_node_id")
            get_or_create_persistent_node_id()
        });
    sanitize(hostname)  // 只留 [a-z0-9-],小写
}
```

稳定:同一台机器重启 node_id 不变。display_name 默认 = hostname,前端 Settings 可改(存 `app_config "tunnel_display_name"`)。

### 3.4 dispatcher 的 loopback URL

```rust
const LOCAL_DAEMON_BASE: &str = "http://localhost:7456";
// 但!daemon 端口可配(--port / EVERLASTING_DAEMON_PORT)
// dispatcher 必须用 daemon 自己实际监听的端口,不能硬编码 7456
```

> **决策 Q-T6**:tunnel client 如何知道本地 daemon 端口?tunnel 模块在 daemon 进程内,`serve_daemon` 已经 bind 了端口。tunnel spawn 时传入 `local_port: u16`(main 里 parse_port_from_args 的结果)。dispatcher 用 `http://localhost:{local_port}`。

---

## 4. 兼容性 / 迁移

### 4.1 双模式并行(本地 vs tunnel)

- `app_config` 无 `remote_url` key → `load_remote_config` 返回 None → `spawn_tunnel_client` 不调用 → daemon 行为 = 现状(零回归)
- 配上 `remote_url` → tunnel task spawn → 后台连 remote → 本地 API 照常(axum server 不受影响)
- `set_remote_config({remoteUrl: ""})` → tunnel task shutdown → 回到纯本地
- **Full 模式(`?transport=tauri`)例外**(P3-3):无 daemon HTTP server,tunnel 不 spawn,remote 仅 Thin/sidecar-daemon 模式可用。详见 §4.2 P1-1 修订。

### 4.2 tunnel 只活在 daemon 进程(P1-1 修订)

> **P1-1 修订(原 §4.2 "Tauri GUI 也 spawn tunnel" 是设计缺陷)**:原 design 称 "daemon bin 和 Tauri GUI 都 spawn tunnel,否则 GUI 模式手机连不上"。经核验 `sidecar.rs` 代码事实,该前提**错误**,现重写。

**代码事实**(`sidecar.rs:8-30` + `lib.rs:150-180`):
- **Thin 模式(P2.4 默认)**:GUI 进程**故意不 load `AppState`、不开 `SqlitePool`、不跑 HTTP server**,只 spawn sidecar daemon 子进程。tunnel 要读 `app_config`(GUI 进程无 DB pool)、dispatcher 要打 loopback(GUI 进程无 server)—— **GUI 进程里 tunnel 无从启动**。
- **Full 模式(`?transport=tauri` 逃生通道)**:无 sidecar、无 HTTP server,前端走 Tauri IPC。tunnel 无 loopback 可代理。
- **真相**:Thin 模式 GUI 经 **sidecar daemon** 跑 agent core,sidecar 内 spawn 的 tunnel 天然覆盖 GUI 场景。"GUI 模式手机连不上"的担心不存在。

**双 spawn 的致命后果**(若按原 design):GUI 进程 + sidecar daemon 都连 remote,两进程 hostname 派生的 `node_id` 相同 → S1 `tunnel_registry` 的"重复 node_id → 新连接踢旧" → **两连接互相踢,永续 flapping**。

**修正后 tunnel 生命周期**:
- tunnel **只在 `bin/everlasting-daemon.rs::main` spawn**(`lib.rs` 零改动)
- `spawn_tunnel_client` 放 `everlasting_lib::daemon::tunnel`(供 daemon bin 调用,放 lib 只因 bin 不能 depend 自己的私有模块 —— 跟 `server::load_daemon_state` 同款 P2.1 决策)
- **Full 模式**(`?transport=tauri`)无 daemon HTTP server,tunnel 不 spawn,remote 不可用 —— 可接受,文档注明(Full 模式是逃生通道,非主路径)
- 若未来真要 GUI 进程直连 remote,node_id 必须与 daemon 区分(但那会产生"一台 PC 两个节点",不推荐)

### 4.3 回滚

删 `daemon/tunnel/` 模块 + revert IPC/config 改动。`app_config` 多两个 key 无害(不读不写)。daemon 行为回滚到现状。

---

## 5. 关键权衡

| 决策 | 选择 | 理由 | 否决项 |
|---|---|---|---|
| tunnel 触发方式 | opt-in(DB config) | 本地一等公民(Q8) | 默认连:破坏本地优先 |
| loopback 转发 | reqwest 打 localhost:7456 | agent core 零改动,API 路径统一(Q7) | 调 handler 函数:绕过 axum,路径分裂 |
| SSE 隔离 | 全局流透传,client 过滤 | 零改 agent core / SseRegistry | PC 加 session 过滤:动热路径 |
| SSE 取消 | 只断 tunnel 订阅,不传 agent loop | SseRegistry broadcast 设计 | 传播 cancel:破坏多订阅者语义 |
| tunnel 生命周期 | TunnelManager 挂 AppState | 统一管 config 变更/shutdown/状态 | 裸 spawn:无法停旧 task |
| node_id 来源 | hostname 派生 + 持久化 fallback | 稳定 + 无需配置 | 随机:重启变,remote 路由乱 |
| secret 传输 | WSS query param | HTTPS 下加密,简单 | header:握手复杂 |
| secret 存储 | 复用 RULE-D-001 加密(V2,先明文) | 同 API key 敏感度 | 明文长期:DB 泄露风险 |
| tunnel 进程归属 | 只在 daemon bin spawn,lib.rs 不改 | Thin 模式 GUI 无 DB/server,tunnel 无从启动;且双进程同 node_id 会互踢 flapping | GUI 也 spawn:基于错误前提(P1-1 修订) |
| query URL 编码 | percent-encode query 值 | 中文 display_name + 特殊字符 secret 不编码会断握手 | 裸 format!:实际 bug |
| remote_url 校验 | set_remote_config 阶段 scheme/尾斜杠校验 | 避免错误配置后台无限重连 | 直接写库:重连无意义消耗 |

---

## 6. 运营 / 回滚

### 6.1 日志

```
INFO tunnel_connecting remote=wss://... node_id=company-pc
INFO tunnel_connected node_id=company-pc
INFO tunnel_request id=42 method=POST path=/api/v1/sessions/list
INFO tunnel_response id=42 status=200
INFO tunnel_stream_start id=43 path=/api/v1/stream
WARN tunnel_disconnected reason=ws-error ... reconnecting in 2s
WARN tunnel_pairing_failed reason="remote not connected"
```

`tracing` target:`everlasting::daemon::tunnel`,可在 `RUST_LOG` 单独调级别。

### 6.2 失败模式

| 失败 | 行为 |
|---|---|
| remote 不可达(DNS/网络) | 指数退避重连(1s→2s→...→60s cap),不 crash daemon |
| remote_url 格式错(`https://`、尾斜杠、无 scheme) | **P2-2:`set_remote_config` 校验阶段拒绝**(返 `InvalidRequest`),不写库不 spawn,前端 inline 提示 —— 避免错误配置后台无限重连 |
| shared_secret 错误 | remote 拒绝(401),tunnel log + 停止重连(配置错误,重连无意义)→ 前端 status 显示 "auth_failed" |
| WSS 断线 | 重连;in-flight dispatcher 请求:reqwest 还在等 localhost,响应回来时 WSS 已断 → 响应丢弃(remote 侧已给手机返 502) |
| localhost:7456 不通(不该发生,同进程) | dispatcher 返 `Stream::Error` / Response 502 给 remote |
| generate_pairing_code 时 tunnel 未连 | IPC 返明确错误 "remote 未连接",前端提示用户先配 remote_url |

### 6.3 回滚

单 PR 回滚:删 `daemon/tunnel/` + revert IPC 改动(§3.1 清单)+ 删 Cargo.toml 的 `everlasting-remote-protocol` / `tokio-tungstenite` / `hostname` dep。`app_config` 残留 key 无害。

---

## 7. 与现有代码的对齐

| 现有约定 | 本任务遵守 |
|---|---|
| `_inner` + `#[tauri::command]` + axum route 三层 | 新 IPC 全跟 |
| `app_config` KV + `get/set_config_value` | remote_url/secret 存这里 |
| `axum::serve + graceful_shutdown` | tunnel task 接同一个 shutdown_token,shutdown 时先停 tunnel 再 drain |
| `tracing` + EnvFilter | tunnel 模块用 `everlasting::daemon::tunnel` target |
| reqwest 已是 dep(provider 调用用) | 复用,不新加 |
| sqlx pool 复用(state.db) | config 读写走 state.db |
| `CancellationGuard` RAII 模式 | 不复用(那是 agent loop 的),tunnel 自己的 shutdown_token |

**不改的东西**(硬约束):
- ❌ `agent/` 任何文件
- ❌ `tools/` 任何文件
- ❌ `provider/` 任何文件
- ❌ `SseRegistry` 实现(tunnel 只消费它,不改它)
- ❌ 现有 axum route(只加新的,不改老的)

---

## 附录 A:S2 验收 checklist(对应 PRD)

| PRD 验收项 | S2 实现位置 |
|---|---|
| 无 remote_config → 不 spawn tunnel,本地零回归 | main/lib.rs 的 `load_remote_config` 判空 |
| 配上 → tunnel 连 remote,日志 "connected" | tunnel/client.rs |
| remote 发 Request → PC 打 loopback → 返回 | tunnel/dispatcher.rs |
| remote 断/重启 → 指数退避重连 | tunnel/client.rs reconnect loop |
| set_remote_config → 实时重连 | TunnelManager + watch channel |
| generate_pairing_code IPC | commands/pairing.rs + tunnel 转发 `/internal/` |
| agent loop 不受 tunnel 影响 | tunnel 不 import agent 模块(硬约束) |

## 附录 B:前置 spike(实施前验证)

- [ ] `tokio-tungstenite` WSS 客户端连 axum WSS 服务端:最小握手能跑(本地两个进程对连)
- [ ] reqwest 打 `http://localhost:<自身端口>` 能走通(同进程内 HTTP 自连)
- [ ] `app_config` 加新 key 读写:确认 `set_config_value("remote_url", ...)` + `get_config_value` 工作
- [ ] workspace 翻转后,`everlasting` crate 加 `everlasting-remote-protocol` path dep 能编译

## 附录 C:S2 与 S1 的接口契约(联调前对齐)

S2 依赖 S1 的:
1. `everlasting-remote-protocol::Frame`(帧定义单源)
2. WSS 握手协议:`GET /ws?secret=&node_id=&display_name=`(query 参数顺序、编码)
3. `/internal/pairing/generate` RPC:Request path + Response body 格式 `{code, expires_in}`
4. Stream 帧的 `End`/`Error` 语义(谁发、何时发)

S2 提供给 S3 联调的:
- 真实 tunnel client 可连 S1 的 remote
- dispatcher 的非流式 + SSE 流式路径都已实现(S3 做端到端压测 + 取消传播联调)
