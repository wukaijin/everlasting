# S2 Implement — PC daemon tunnel client

> 执行顺序 + 验证命令 + 评审门 + 回滚点。需求见 [prd.md](./prd.md),设计见 [design.md](./design.md)(评审已过,review.md 结论"可批准进入实施")。

## 前置条件(已核验)

- [x] S1 已落地 `everlasting-remote-protocol` crate(`crates/everlasting-remote-protocol/src/lib.rs`,Frame / StreamEvent / INTERNAL_PREFIX 齐备,帧定义单源)
- [x] S1 remote daemon 可本地起(默认端口 **7457**,`scripts/remote.sh` + `scripts/remote-e2e-smoke.mjs` 已有)
- [x] reqwest 已是 daemon dep(0.13,含 stream feature);`get_config_value` / `set_config_value` 可直接复用(零 migration)
- [ ] 前置 spike(design 附录 B):tokio-tungstenite 客户端↔服务端最小握手;同进程 HTTP 自连;workspace path dep 编译 —— **第 1 步完成时一并验证**

## 硬约束(每步都复查)

1. `tunnel` 模块**不得 import** `agent::` / `tools::` / `provider::`(agent core 零改动)
2. 不配 `remote_url` → **不 spawn**,daemon 行为与现状一致(零回归)
3. tunnel 只由 `bin/everlasting-daemon.rs::main` spawn;`lib.rs` 只加 invoke_handler 注册,**不改 sidecar/双模式逻辑**(P1-1)
4. 现有 axum route 只增不改;`SseRegistry` 不动
5. dispatcher 只打 `http://localhost:{local_port}`,不调 handler 函数(Q7)

## 执行顺序(每步独立可编译 + 可测,逐步 commit)

### 第 1 步:Cargo 依赖 + 模块骨架
- `app/src-tauri/Cargo.toml` 加:
  - `everlasting-remote-protocol = { path = "../../crates/everlasting-remote-protocol" }`
  - `tokio-tungstenite = { version = "0.23", features = ["rustls-tls-native-roots"] }`(WSS 客户端)
  - `hostname = "0.4"`(P2-1 漏列补)+ `percent-encoding`(query URL 编码)
- `daemon/mod.rs` 加 `pub mod tunnel;`
- `daemon/tunnel/mod.rs`:`TunnelConfig { remote_url, shared_secret, node_id, display_name }` + `spawn_tunnel_client(...)` 签名(实现可后补)
- `daemon/tunnel/node_id.rs`:`derive_node_id()`(hostname → sanitize `[a-z0-9-]` → fallback 随机 UUID 持久化 `app_config "tunnel_node_id"`)

**验证**:`cargo check` + `cargo fmt --check`;顺便验证 workspace path dep 编译(spike ③)

### 第 2 步:TunnelManager + config IPC(不依赖帧类型,可先行)
- `state.rs` AppState 加 `pub tunnel_manager: Arc<TunnelManager>`(design §2.4 Q-T4)
- `daemon/tunnel/manager.rs`:`watch::Receiver<Option<TunnelConfig>>`(None = 停止)+ current_handle + `notify_config_changed`;daemon 启动走同一条路径(初始 config 从 DB 读,seed 到 watch channel)
- `commands/config.rs`:`get_remote_config_inner` / `set_remote_config_inner` / `get_tunnel_status_inner` + `#[tauri::command]` × 3
  - P2-2 校验:scheme 必须 `wss://`(本地调试允许 `ws://`)、去尾斜杠,失败返 InvalidRequest(前端 inline 提示),不写库
- `daemon/routes/config.rs`:3 个 axum route(跟现有 `_inner` 委托模式)
- `lib.rs`:`invoke_handler` 加 3 个
- 前端 `app/src/transport/http.ts`:`CMD_TO_DOMAIN` 加 `get_remote_config` / `set_remote_config` / `get_tunnel_status` → `config` 域(design §3.1 清单第 6 条)

**验证**:`cargo test --lib`(现有测试不破)+ `pnpm test`(http.ts 类型)

### 第 3 步:WSS client + 心跳 + 重连(client.rs,依赖协议 crate)
- `build_ws_url`:`wss://{remote_url}/ws?secret=&node_id=&display_name=`—— **query 值必须 percent-encode**(P2-1,中文 display_name/特殊字符 secret)
- `connect_and_serve`:connect_async → spawn 心跳 task(等 remote Ping → 回 Pong,remote 主导)→ serve_loop(`select!`:收帧 vs shutdown)
- 收到 `Frame::Request` → spawn `dispatch_one`(第 4 步)
- 断线重连:指数退避 1s→2s→4s→…→cap 60s;**secret 校验失败(auth 类错误)停止重连**(§6.2)
- 日志 target `everlasting::daemon::tunnel`:`tunnel_connecting` / `tunnel_connected` / `tunnel_disconnected`(§6.1)

**验证**:本地起 S1 remote(`scripts/remote.sh`)+ daemon 配 `wss://localhost:7457/ws` → 日志出现 connected;remote 重启 → 指数退避重连

### 第 4 步:dispatcher + SSE 桥(dispatcher.rs + sse_bridge.rs)
- spawn 时建一个 `reqwest::Client`(连接池)复用,不每请求新建
- `dispatch_one`:`Request{id, method, path, headers, body}` → `http://localhost:{local_port}`(Q-T6:main 传 `parse_port_from_args` 结果,不硬编码 7456)→ 非流式 → `Frame::Response`
- header 透传规则(design §2.2):`Authorization` 已被 remote 剥(PC 侧不再处理);`Content-Type` / `Accept` / `Last-Event-ID` 透传;`access_token` query 若还在则原样透传(S1 契约为单源,不主动剥)
- SSE 检测(`Content-Type: text/event-stream`)→ `sse_bridge.rs` 逐 chunk 转 `Stream::Chunk` → 结束 `Stream::End` → 读错 `Stream::Error`;chunk 边界不对齐 SSE event 边界没关系,纯字节透传(Q-T3)
- localhost 不通(不该发生)→ 返 502 Response / `Stream::Error`

**验证**:本地 remote + daemon 双起,curl remote 的 `/api/v1/proxy/api/v1/health` 走通(非流式);`/api/v1/stream` 走 SSE 路径冒烟

### 第 5 步:daemon main 集成
- main:`load_remote_config(&state.db)`(读 `app_config` 两 key)→ 有值则 `tunnel_manager` 拉起(`spawn_tunnel_client`)
- 传 `local_port`(第 4 步 Q-T6)
- shutdown:`shutdown_signal`(server.rs:298,目前私有)需小改使其同时通知 tunnel 先停、再 drain —— 注意**不能**改现有 serve 行为,只加 tunnel 通知

**验证**:
- 无配置启动 = 现状回归(不 spawn,日志无 tunnel)
- 配 `remote_url` → 连上 → `set_remote_config` 改 URL → 旧连接断、新 URL 重连(实时生效)
- `set_remote_config({remoteUrl: ""})` → tunnel 停止 → 纯本地

### 第 6 步:配对码 IPC(pairing)
- `commands/pairing.rs`(新):`generate_pairing_code_inner` → `tunnel_manager.get_conn()` → None 返明确错误"remote 未连接";Some(tx) → `Frame::Request{path: "/internal/pairing/generate"}` → 等 Response → parse `{code, expires_in}`(S1 契约:`expires_in` 单位秒 = 60)
- `daemon/routes/pairing.rs`(新):1 个 axum route + `router()`
- `lib.rs` + 前端 `http.ts`(新 domain `pairing`,design §3.1)

**验证**:remote 起着 → IPC 返回 6 位码;remote 停 → 返回明确错误

## 验证命令

```bash
# 后端全量(AGENTS.md 约定,PKG_CONFIG_PATH 在 WSL 必需)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && cargo fmt --check
# 前端 http.ts 改动
cd app && pnpm test
# 端到端冒烟(S1 脚本可复用)
bash scripts/remote.sh          # 起 remote(7457)
node scripts/remote-e2e-smoke.mjs   # S1 冒烟(node 模拟 PC daemon)
```

## 评审门

- 每步完成 → `cargo check` / 相关测试绿
- 全部完成 → `trellis-check` 全量核验:design §3.1 清单逐条(6 条)、硬约束复查、SSE 桥接是否留了 S3 联调入口(不追求 S3 的取消传播)
- 回归:无 `remote_url` 配置时 `cargo test` + 手工起 daemon 行为与现状一致

## 回滚点

- 每步独立 commit;回滚 = revert 对应 commit(design §6.3)
- 整体回滚:删 `daemon/tunnel/` + revert IPC 改动(§3.1 清单)+ 删 Cargo.toml 三个新 dep;`app_config` 残留 key 无害
