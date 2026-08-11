# S2 PC daemon tunnel client:tunnel 模块 + WSS 长连接 + loopback 转发

> 架构决策见 [parent PRD](../08-11-remote-control-epic/prd.md)。本任务只细化执行。

## Goal

在现有 `everlasting-daemon` 加一个 `tunnel` 模块,让 PC daemon 能 opt-in 地连 remote daemon,维持 WSS 长连接,接收 remote 转发来的 HTTP 请求帧,用 reqwest 打自己 loopback(`localhost:7456`)处理后塞回 WSS。

**核心不变量:agent core 零改动 + 本地功能零依赖 remote。** 不配 remote_url 时,`tunnel` 模块根本不启动,daemon 行为与现状完全一致。

## Scope

### 新增 `tunnel` 模块(`app/src-tauri/src/daemon/tunnel/`)

- `tunnel/mod.rs`:模块入口,`spawn_tunnel_client(config) -> Option<JoinHandle>`
- `tunnel/client.rs`:WSS 客户端主体(连 remote + 维持 + 收发帧)
- `tunnel/dispatcher.rs`:收到 `Request` 帧 → reqwest 打 loopback → 包成 `Response`/`Stream` 帧塞回 WSS

### 启动集成(在 `lib.rs::run` 或 daemon main)

```rust
// 读 settings(已有的 config 表 / 新增 remote_config 表)
let remote_cfg = load_remote_config(&db).await?;  // {remote_url, shared_secret}
if let Some(cfg) = remote_cfg {
    if !cfg.remote_url.is_empty() {
        daemon::tunnel::spawn_tunnel_client(cfg, shutdown_token.clone());
    }
}
// 没配 remote → 不 spawn,纯本地模式
```

**关键**:tunnel client 是独立 tokio task,失败/断线不影响主 daemon。

### WSS 长连接维护

- 连接:`wss://<remote_url>/ws?secret=<shared_secret>&node_id=<ID>&display_name=<NAME>`(query 传参,见 design §2.1 P2-1 percent-encode)
- 心跳:**响应 remote 的 30s ping(回 pong)**;连接断开/90s 无帧 → 重连(P3-1:remote 主导 ping,客户端只回 pong,与 S1 契约一致)
- 断线重连:指数退避(1s → 2s → 4s → ... → cap 60s)
- 连接成功后:注册 node(派生稳定 node_id,首次连后 remote 记住)

### loopback 转发(dispatcher)

收到 remote 的 `Request { id, method, path, headers, body }` 帧:
1. 构造 reqwest 请求:`<method> http://localhost:7456<path>` + headers + body
2. 发出,等响应
3. 非流式响应 → 包成 `Response { id, status, headers, body }` 塞回 WSS
4. 流式响应(SSE,`Content-Type: text/event-stream`)→ **本任务先检测到就转 Stream 帧持续推 chunk**,完整 SSE 桥接协议联调留 S3

### settings 存储 + 实时生效

- 新增 DB 表 `remote_config`(单行,key-value 或固定列):`remote_url TEXT`, `shared_secret TEXT`
- 新增 IPC:
  - `get_remote_config() -> {remote_url, shared_secret}`
  - `set_remote_config(remote_url, shared_secret)` → 写 DB + IPC 通知 tunnel client 重连
- 重连逻辑:`set_remote_config` 写完 DB → 发 tokio `notify` / `mpsc` 消息 → tunnel client 收到 → 断开当前 WSS → 用新配置重连

### 配对码生成 API(本地 IPC,给 PC 前端用)

- `generate_pairing_code() -> {code, expires_in: 60}`:PC daemon 经 WSS 调 remote 的 `/api/v1/internal/pairing/generate` → 返回码给前端展示
- 依赖 tunnel client 在线(离线时返错误"remote 未连接")

## 依赖

- 无前置(可立即启动,与 S1 并行)
- 需要 S1 的帧类型定义(抽到共享 `everlasting-remote-protocol` crate,或先本地定义 S3 对齐)

## 验收标准

- [ ] DB 无 `remote_config` 或 `remote_url` 为空 → daemon 启动不 spawn tunnel,本地功能与现状完全一致(回归)
- [ ] 配上 `remote_url` + `shared_secret` → tunnel client 启动 → 连上 remote → 日志显示 "tunnel connected, node_id=..."
- [ ] remote 通过 WSS 发一个 `Request`(GET /api/v1/health 透传)→ PC dispatcher 打 loopback → 返回 remote → 链路通(非流式)
- [ ] remote 断开/重启 → PC tunnel client 指数退避重连,恢复后自动连上
- [ ] 改 `set_remote_config` → tunnel client 收到通知 → 用新 URL 重连(实时生效,不重启 daemon)
- [ ] `generate_pairing_code` IPC → 返回 6 位码(tunnel 离线时返明确错误)
- [ ] agent loop 全程不受 tunnel 影响:tunnel 断了,本地 chat/SSE/permission 全部正常

## Notes

- **agent core 零改动是硬约束**。tunnel client 只调 reqwest 打 loopback,不 import agent 模块。
- tunnel client 失败的容错:连接失败 → log + 后台重试,不 panic、不 crash daemon。
- 帧类型与 S1 共享:实施时定是否抽 `everlasting-remote-protocol` crate。倾向抽出来(S1/S2/S3 三方共用,避免重复定义)。
- SSE 流式转发:本任务先做到"检测到 SSE 响应 → 转 Stream 帧",完整协议(remote 侧 SSE 桥接 + 取消传播)在 S3 联调。
