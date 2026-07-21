# Phase 2 Design — daemon 拆分 + HTTP/SSE

> 配套 PRD:[prd.md](./prd.md)。本文档回答"具体怎么做";PRD 回答"做什么/为什么"。
> 实施检查清单:[implement.md](./implement.md)。

## 1. 架构总览

### 1.1 进程拓扑(Phase 2 完成态)

```
┌─────────────────────┐              ┌──────────────────────────────┐
│  Tauri GUI 进程      │              │  everlasting-daemon 进程       │
│  ────────────        │              │  ──────────────────          │
│  • webview(Vue)     │  httpTransport│  • axum HTTP server (0.0.0.0)│
│  • transport =       │ ◄──────────► │  • 84 handlers (+ inner)    │
│    httpTransport     │  /api/v1/*    │  • HttpSseSink → SSE 分发    │
│  • sidecar spawn     │  /api/v1/stream│  • AppState (db pool + ...) │
│  • 关窗 SIGTERM      │  EventSource │  • tokio::main runtime       │
└─────────────────────┘              └──────────────────────────────┘
                                              │
                                              │ tokio::spawn → agent loop
                                              ▼
                                    ┌──────────────────────┐
                                    │ SQLite + LLM provider │
                                    └──────────────────────┘

WSL 部署形态:daemon 监听 0.0.0.0:7456(WSL 内),Windows 宿主浏览器 →
http://localhost:7456(WSL 2 默认 localhost forwarding)→ 直连
```

### 1.2 模块边界(新增)

```
app/src-tauri/src/
├── bin/
│   └── everlasting-daemon.rs        # NEW: daemon main 入口(#[tokio::main])
├── daemon/                          # NEW
│   ├── mod.rs                        # public re-exports
│   ├── server.rs                     # axum router 装配 + 启动 + graceful shutdown
│   ├── sse.rs                        # HttpSseSink + HttpSseSubagentSink + SseRegistry + SseBuffer
│   └── routes/                       # 84 axum handlers(每个文件 = 一个 domain)
│       ├── sessions.rs / projects.rs / providers.rs / models.rs
│       ├── subagents.rs / subagent_runs.rs / permissions.rs / question.rs
│       ├── memory.rs / config.rs / audit.rs / trace.rs / checklist.rs
│       ├── task.rs / cancel.rs / files.rs / worktree.rs / panel.rs
│       ├── command_palette.rs / ui.rs
│       └── health.rs                 # GET /api/v1/health
├── commands/                        # 既有,Tauri 入口(保留,过渡期)
│   └── sessions.rs                   # 业务逻辑 create_session_inner(单份,见 Q0 决议)
└── ...(其余不变)
```

### 1.3 关键不变量

- **单 SQLite 实例**:daemon 独占 SqlitePool,GUI 全走 RPC(D1 决议)。GUI 进程**不**持有 SqlitePool(`AppState::load` 在 GUI 进程仍走瘦壳,不建 pool)。
- **SSE 单全局流**:`GET /api/v1/stream`,按 `event_name` 分发;`payload` 含 `session_id`,前端 listen handler 过滤。
- **HttpSseSink ≠ HttpSseSubagentSink**:Phase 1 §3.3 承诺保持。两者各自实现对应 sink trait,各自独立注入(`chat_loop.rs` 注入 `HttpSseSink`,`subagent/sink.rs` 注入 `HttpSseSubagentSink`)。
- **业务逻辑单份**:command 文件 `xxx_inner(state: &Arc<AppState>, ...)` 保留业务实现,Tauri/axum 入口都调 `_inner`,无 duplication。

## 2. 数据流

### 2.1 命令调用流(POST /api/v1/chat 范例)

```
GUI httpTransport.invoke("chat", {requestId, sessionId, messages})
   │
   ▼
POST /api/v1/chat   body: {request_id, session_id, messages}
   │
   ▼ axum handler (daemon/routes/sessions.rs::chat)
Extension(Arc<DaemonState>) → state.app_state
   │
   ▼ state.app_state.chat_inner(request_id, session_id, messages).await
   │
   ▼ 业务:
1. providers::resolve_model(session_id) → Arc<dyn Provider>
2. agent::chat_loop::run(...) tokio::spawn
3. run 内通过 HttpSseSink.emit_chat_event(...) 推到 SSE channel
4. handler 立即返回 Ok({accepted: true})  → GUI 进入 LRU 缓存等待事件
   │
   ▼ HTTP 响应:202 Accepted
```

### 2.2 SSE 推送流

```
agent_loop
   │
   ▼ ChatEventSink::emit_chat_event(payload)
HttpSseSink
   │
   ├──► 1. 序列化 payload → SSE frame (id: <u64>, event: <name>, data: <json>)
   ├──► 2. 按 session_id 推入环形 buffer (VecDeque, 上限 512)
   │      └─> 单条 > 256KB? 跳过 buffer,直推,不参与重连回放
   ├──► 3. 查分发表 event_name → Vec<SseSender>
   └──► 4. tokio::spawn 推送到每个 SseSender
              │
              ▼
         EventSource client → listen handler by event name
```

### 2.3 Round-trip 协议(4 类 oneshot)

```
GUI 触发 permission:ask(agent 在等用户点允许)
   │
   ▼ agent_loop → PermissionStore::ask() → oneshot::Sender<Decision> parked
GUI listen "permission:ask" handler 弹窗
   │
   ▼ 用户点允许
POST /api/v1/permission/respond   body: {request_id, allow: true}
   │
   ▼ handler → PermissionStore::resolve(request_id, Decision::Allow)
   │
   ▼ oneshot::Sender.send(decision).unwrap() 唤醒 agent_loop
```

**关键**:`request_id` 是 GUI 维护的 LRU key,daemon 通过 `PermissionStore` (已在内存)的 `HashMap<request_id, oneshot::Sender>` 解析。**无状态外溢**——session_id 不是 round-trip 必需,避免 session 切换导致的悬空 oneshot。

### 2.4 断网重连(resync 协议)

```
GUI EventSource 断开(网络抖动)
   │
   ▼ EventSource 自动重连 → GET /api/v1/stream + Last-Event-ID: 1234
daemon 查环形 buffer
   │
   ├──► 1234 >= buffer_oldest_id → 重发 1234-当前最新 之间的所有事件
   │
   └──► 1234 < buffer_oldest_id → 发 sentinel:
            event: stream-resync-{session_id}
            data: {"reason":"buffer_overrun","session_id":"..."}
        │
        ▼ GUI listen "stream-resync-{session_id}" handler
           GET /api/v1/sessions/{session_id}/snapshot
              │
              ▼ load_session_inner + get_pending_interaction_inner
              返回完整 session 状态 + pending interaction
           │
           ▼ GUI 替换 store.session 内容 + 重画 UI
```

**R6.1.a snapshot 复用现有 endpoint**:不新建 service,直接复用 `load_session_inner` + `get_pending_interaction_inner`,二者已在 Phase 1 完成。

## 3. 契约 / 接口

### 3.1 URL 命名

- REST:`POST /api/v1/{domain}/{action}`(e.g. `/api/v1/sessions/list`)
- SSE:`GET /api/v1/stream`(单全局流)
- Snapshot:`GET /api/v1/sessions/{id}/snapshot`
- Health:`GET /api/v1/health`

### 3.2 Body 字段命名

- Rust serde 默认 `snake_case`(已对齐现有 `AppCommandError` 格式)
- TS 侧定义 `interface` 时同样 `snake_case`,Pinia store 反序列化直接对接

### 3.3 TS 类型来源

**不引入 ts-rs**(Q4 决议)。`app/src/transport/api-types.ts` 手写,类型同步靠:
1. `pnpm vue-tsc --noEmit` 编译期检查
2. E2E 对拍测试:同输入两端(Tauri + HTTP)行为一致

### 3.4 Health 端点

```json
GET /api/v1/health → 200 OK
{
  "daemon_id": "uuid-v4",          // 进程级唯一,GUI 启动时校验(端口复用判定)
  "daemon_version": "0.1.0",
  "api_versions": ["v1"],           // 协议版本,GUI 期望 "v1" 必须存在
  "uptime_seconds": 3600,
  "session_count": 5
}
```

### 3.5 错误格式

复用 `AppCommandError`(`#[derive(Serialize)]`):

```json
HTTP 4xx/5xx
{
  "kind": "NotFound" | "InvalidInput" | "PermissionDenied" | "Internal" | ...,
  "message": "中文用户消息",
  "request_id": "..." (optional)
}
```

GUI httpTransport 解析:`response.status >= 400` → throw `TransportError`,前端 handler 形态与 Tauri 一致(包装错误消息)。

## 4. 兼容性 / 迁移

### 4.1 双入口并行期(P2.1 → P2.3)

- **P2.1**:AppState::load 接受 PathBuf;Tauri 入口保留旧签名(包装到 PathBuf 版本);`pnpm tauri dev` 仍可用。
- **P2.2**:daemon 启动,84 handler 全部可达;**前端未切 httpTransport**(仍是 stub),Tauri 入口是唯一前端通道;`pnpm tauri dev` 正常。
- **P2.3**:HttpSseSink 实现 + httpTransport 填充;前端新增 `?transport=http` query 强制切流,验证浏览器路径;`pnpm tauri dev` 仍走 tauriTransport。
- **P2.4**:Tauri GUI 默认走 httpTransport,sidecar spawn daemon;Tauri 入口**保留**(D1 决议"不立即废弃",直到 Phase 2 dogfooding 稳定再 archive)。

### 4.2 字段兼容性

- 现有 84 command 的 snake_case 字段**不动**。HTTP body 与 Tauri invoke payload 字段名一致,store 反序列化无 breaking change。
- 新增 endpoint(body: snapshot)走 `app/src/transport/api-types.ts`,与现有 Pinia store 类型对齐。

### 4.3 客户端版本兼容

- **协议版本不匹配** → fail loud(Q5 决议)。
- **构建版本不一致** → 仅 warning(可能 dev/edge 混用)。
- **breaking change** = /api/v2(GUI 必须升级)。

## 5. 关键权衡

| 决策 | 选择 | 否决方案 | 理由 |
|---|---|---|---|
| **handler vs service** | 命令文件内 `_inner` 函数(Q0) | 强制抽 `crate::service::*` | 拿复用、不付跨模块搬迁 + 多一层抽象成本 |
| **SSE 拓扑** | 单全局流 `/api/v1/stream`(Q3 决议) | per-session 多流 | 单 EventSource 简化前端 listen handler(无需为每个 session 建连接);事件 payload 含 `session_id` 字段供按 session 过滤 |
| **event id** | 递增整数 u64 | UUID | 客户端重连时可直接比较 Last-Event-ID 与 buffer_oldest;UUID 无序 |
| **lost segment** | resync sentinel + snapshot 复用 | 410 Gone | 5MB tool_result 期间断网不丢,sentinel 通知走轻量重拉路径 |
| **端口冲突** | fail loud + health 区分 | 自动跳端口 | 避免多 daemon 数据分裂;复用路径显式 allow |
| **ts-rs** | 不引入 | cargo build hook 生成 | 注解 + build hook + 与手写类型冲突,Phase 2 不成正比 |
| **GUI spawn** | sidecar(prod) + concurrently(dev) | GUI 运行时 cargo run | 编译延迟;WSL/Win 路径差异 |
| **E2E harness** | Rust integration test | Playwright | WSL 浏览器成本;mock provider 复用现成 |

## 6. 运营 / 回滚

### 6.1 Rollback 形状

每个子阶段 P2.1-P2.5 完成后,**Tauri 入口仍可用**(双入口并行期 4.1)。回滚策略:

- **P2.1 失败**:不破坏现有 Tauri;撤销 PathBuf 重构,保留 AppHandle 版本。
- **P2.2 失败**:daemon 实现不影响 Tauri;daemon bin 可不构建,Tauri 路径完整。
- **P2.3 失败**:httpTransport 是 stub 之外的实现;前端默认仍走 tauriTransport,httpTransport 仅 ?transport=http 触发。
- **P2.4 失败**:**关键回滚点**——GUI 默认切到 httpTransport 后,若 daemon 不稳,GUI 退化为无功能。缓解:Q5 health 校验 + Q1 端口 fail loud + sidecar 自动重启(launchd/systemd)。
- **P2.5 失败**:E2E harness 失败不阻塞主路径,留待后续补。

### 6.2 Dogfooding 周期

P2.4 完成后需在主分支上至少 dogfooding **2 周**(确认 daemon 在长期使用中稳定),才进入 Phase 3(parent 远期)。期间:

- 每日检查 daemon 进程是否存活
- 验证 SSE 断网重连后 UI 完整恢复
- 验证 5MB tool_result 推送不丢
- 验证 WSL→Windows 宿主浏览器稳定可达

### 6.3 监控 / 日志

- daemon 启动时 `tracing` 初始化(env `RUST_LOG=info,everlasting=debug`)
- 所有 HTTP handler 进入/退出 trace span(`#[tracing::instrument]`)
- SSE 连接数 / buffer 使用率 / emit 速率作为 health 端点 metric(Q5 health 端点可扩展 `metrics` 字段)

## 7. 与 Phase 1 契约对齐

| Phase 1 承诺 | Phase 2 兑现位置 |
|---|---|
| `SubagentEventSink` trait 已存在 | `daemon/sse.rs::HttpSseSubagentSink` 实现 |
| `HttpSseSubagentSink` ≠ `HttpSseSink` | 各自独立 trait 实现,各自独立注入 |
| `AppHandleSink` / `AppHandleSubagentSink` 保留(Tauri 路径) | Tauri 入口仍用现有 sink,不动 |
| `streamController.listen` 接收 payload(非 Event<T>) | httpTransport.listen 同语义,R6 单全局流后分发表按 event_name 路由 |

---

## 附录 A:子阶段映射表

| 子阶段 | 工作内容 | 关键交付物 | 验证命令 |
|---|---|---|---|
| P2.1 | AppState::load 去 AppHandle | `state.rs::load(PathBuf)` + wrapper | `cargo test state_load_path_consistency` |
| P2.2 | axum server + 84 handler | `bin/everlasting-daemon.rs` + `daemon/routes/*` | `cargo run --bin everlasting-daemon` + `curl /api/v1/health` |
| P2.3 | SSE + httpTransport | `daemon/sse.rs` + `app/src/transport/http.ts` | `cargo test sse::` + 浏览器手动 smoke |
| P2.4 | GUI sidecar + 静态文件 | `tauri.conf.json externalBin` + `tower-http ServeDir` | `pnpm tauri build` + 双进程启动验证 |
| P2.5 | WSL E2E | `app/src-tauri/tests/e2e.rs` | `cargo test --test e2e -- --test-threads=1` |

## 附录 B:文件清单(预测)

新增 25 文件 + 改动 25 文件(粗估):

```
新增:
  src-tauri/src/bin/everlasting-daemon.rs
  src-tauri/src/daemon/{mod.rs, server.rs, sse.rs, auth.rs}
  src-tauri/src/daemon/routes/{mod.rs, sessions.rs, projects.rs, providers.rs, models.rs,
                              subagents.rs, subagent_runs.rs, permissions.rs, question.rs,
                              memory.rs, config.rs, audit.rs, trace.rs, checklist.rs,
                              task.rs, cancel.rs, files.rs, worktree.rs, panel.rs,
                              command_palette.rs, ui.rs, health.rs}
  src-tauri/tests/e2e.rs
  app/src/transport/http.ts (Phase 1 已 stub,Phase 2 填充)
  app/src/transport/api-types.ts (新)

改动:
  src-tauri/src/state.rs (P2.1 load 重构)
  src-tauri/src/commands/*.rs (P2.2 拆 _inner)
  src-tauri/Cargo.toml (新增 axum/tower-http/tokio-stream/SSE deps)
  src-tauri/tauri.conf.json (P2.4 externalBin)
  app/package.json (新增 concurrently dev 依赖)
  app/src/transport/{index.ts, types.ts, transport.test.ts} (P2.3 切换逻辑)
  + 各 stores/utils 微调(若 84 handler 的 body shape 有微差)
```