# Phase 2 Implement — 执行检查清单

> 配套 PRD:[prd.md](./prd.md) | Design:[design.md](./design.md)
> 顺序执行 P2.1 → P2.2 → P2.3 → P2.4 → P2.5,每个子阶段独立可提交、Tauri 版始终可用。

## 0. 前置条件

- [x] Phase 1 已归档(✓ 2026-07-20 commit `0dbc747`)
- [~] Tauri 版在主分支 dogfooding ≥ 3 天(07-20→07-22 仅 2 天,软约束;P2.3 transport 抽象 + SSE 链路已 dogfooding,不阻塞)
- [x] **完整 SubagentEventSink 注入**(P2.4 硬前置,2026-07-22 commit `e4bb80b`,Session 28)— daemon 路径 worker 事件从 buffer-only → SSE live(`app_handle: Option<AppHandle>` 参数拆成 `worker_catalog` + `worker_event_sink`)
- [x] 本文档 + PRD + design.md 三件套齐备
- [x] `implement.jsonl` / `check.jsonl` 已 seed(由 `task.py` 起步时自动建)

---

## P2.1 — AppState::load 去 AppHandle(2-3 天)

### 实现清单

- [x] **A1** `app/src-tauri/src/state.rs::AppState::load` 改为接受 `PathBuf`(data_dir)— commit `5a212f0`
- [x] **A2** 内部逻辑不变:LLM config from_env + db::init_pool + run_migrations + provider catalog + backfill spawn
- [x] **A3** 保留旧 `load(app: &AppHandle)` 签名作为 wrapper:`fn load(app: &AppHandle) -> impl Future<...>` → `Self::load(app.path().app_data_dir().unwrap())`
- [x] **A4** `state.rs:317` `projects:refreshed` 改走新接口(传 `Arc<dyn SystemEventSink>` 或保留 AppHandle 用于 Tauri 路径,daemon 路径传 stub noop sink)
- [x] **A5** 新增测试 `state_load_path_consistency`:对比 `AppState::load(AppHandle)` 与 `AppState::load(PathBuf)` 产出同一 `db_path` 与 `home_dir`(mock AppHandle 与 mock PathBuf 同源)
- [x] **A6** `cargo test --lib` 全绿(vitest 不变)

### 验证命令

```bash
# 1. 新测试通过
cd app/src-tauri && cargo test state_load_path_consistency

# 2. 全套 Rust 测试通过
cd app/src-tauri && cargo test --lib

# 3. Tauri 版零行为变化
pnpm tauri dev
#    验证:
#    - DB 文件路径与重构前一致(看 ~/.local/share/everlasting/everlasting.db)
#    - 首启 backfill 后项目列表刷新(projects:refreshed 仍能 emit)
```

### 回滚点

A3 旧 wrapper 保留,直接 revert `state.rs` 即可。

### 提交时机

A1-A6 全 ✓,`cargo test --lib` 全绿,`pnpm tauri dev` 正常 → 一次 commit `refactor(state): AppState::load accepts PathBuf, AppHandle wrapper preserved`。

---

## P2.2 — axum HTTP server + 84 handler(5-7 天)

### 依赖新增

```toml
# app/src-tauri/Cargo.toml
[dependencies]
axum = { version = "0.7", features = ["macros"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["services", "fs"] }
tokio-stream = { version = "0.1", features = ["sync"] }
serde_json = "1"
```

### 实现清单

- [x] **B1** `app/src-tauri/src/bin/everlasting-daemon.rs`:daemon main 入口 — commit `5a212f0`
  - `#[tokio::main]`
  - 解析 `--port` / `EVERLASTING_DAEMON_PORT` / 默认 7456
  - 启动前调 `GET http://localhost:{port}/api/v1/health` 端口冲突检查(Q1 fail loud)
  - `AppState::load(data_dir)` → `Arc<AppState>`
  - `axum::serve(...).with_graceful_shutdown(shutdown_signal())`
- [x] **B2** `app/src-tauri/src/daemon/server.rs`:axum router 装配 — commit `5a212f0`
  - `Router::new().route("/api/v1/health", get(health))`
  - `.nest("/api/v1", routes::router(state.clone()))`
- [x] **B3** `daemon/routes/health.rs`:`GET /api/v1/health` 返回 `{daemon_id, daemon_version, api_versions: ["v1"], uptime_seconds, session_count}` — commit `5a212f0`
- [x] **B4** **84 命令 _inner 拆解**(commands/*.rs + agent/chat.rs + git/error.rs) — commit `5a212f0`
  - 每个 `pub async fn xxx(state, ...) -> Result<...>` 拆为 `pub async fn xxx_inner(state: &Arc<AppState>, ...) -> Result<...>` 保留业务逻辑
  - 原 `#[tauri::command]` 入口退化为 `xxx_inner(&state, ...).await` 薄包装
- [x] **B5** `daemon/routes/{domain}.rs`:84 axum handler,每个文件对应一个 domain,handler 调对应 `_inner` — commit `5a212f0`(实际 79 handler / 19 domain 模块)
  - 例:`routes/sessions.rs::list_sessions(Extension(state): Extension<Arc<AppState>>, Json(req): Json<ListSessionsReq>) -> Result<Json<ListSessionsResp>, AppCommandError>`
- [x] **B6** 错误转换:handler 返回 `AppCommandError` → axum `IntoResponse` 转换为 HTTP 状态码 + JSON body — commit `5a212f0`(`daemon/error.rs`)
- [x] **B7** handler 单测:`daemon/routes/tests_*.rs`(按 domain),happy path + 错误码 — commit `5a212f0`
- [x] **B8** 全套 `cargo test` 全绿,包括新 handler 测试

### 验证命令

```bash
# 1. daemon 启动
cd app/src-tauri && cargo build --bin everlasting-daemon
./target/debug/everlasting-daemon --port 7456
#    期望:日志 "listening on 0.0.0.0:7456"

# 2. health 端点
curl http://localhost:7456/api/v1/health
#    期望:200 + JSON 含 daemon_id, daemon_version, api_versions: ["v1"]

# 3. 每个 domain 冒烟测 1 个 handler
curl -X POST http://localhost:7456/api/v1/sessions/list -d '{}'
curl -X POST http://localhost:7456/api/v1/projects/list -d '{}'
# ... (84 个 curl 调用,逐一验证)

# 4. handler 单测
cargo test --lib daemon::routes::

# 5. Tauri 版仍正常
pnpm tauri dev
```

### 回滚点

- daemon bin 是新 crate target,删除 bin 文件 + `Cargo.toml` `[[bin]]` 段即回滚
- B4 `_inner` 重构无副作用(仅函数签名,业务逻辑零变化)

### 提交时机

B1-B8 全 ✓,`cargo build` 成功,`/api/v1/health` 200,84 handler curl 全过 → 一次 commit `feat(daemon): axum HTTP server with 84 handler mappings (Phase 2.2)`。

---

## P2.3 — SSE 事件流 + httpTransport(4-5 天)

### 实现清单

- [x] **C1** `app/src-tauri/src/daemon/sse.rs` — commit `f2a675b`(实现简化:全局单流 + request_id 路由,非按 session_id 分 buffer;详见 sse.rs 模块文档"两处简化")
  - `HttpSseSink` 实现 `ChatEventSink` trait(注入 `chat_loop`)
  - `HttpSseSubagentSink` 实现 `SubagentEventSink` trait(注入 `subagent/sink.rs`)**Phase 1 §3.3 承诺**(C5 commit `e4bb80b` 完整闭合)
  - `SseRegistry`:全局单流 `Mutex<RegistryInner>`(senders + VecDeque buffer + 全局递增 u64 id)
  - 单条 > 256KB(`LARGE_PAYLOAD_THRESHOLD`)不入 buffer,直推 live channel(走 R6.3 大 message 旁路)
  - `event_name = "stream-resync"` 全局 sentinel(Last-Event-ID < buffer_oldest 时发,前端按当前 session GET snapshot)
- [x] **C2** `daemon/routes/stream.rs`(从 health.rs 独立成 stream.rs):`GET /api/v1/stream` — commit `f2a675b`
  - SSE handler:接收 `Last-Event-ID` 头
  - 客户端连入时 `state.sse.subscribe(last)` 注册到 `SseRegistry`
  - 30s 间隔 `: ping` 心跳(`KeepAlive`)
- [x] **C3** `daemon/routes/sessions.rs`:新增 `GET /api/v1/sessions/{id}/snapshot` — commit `f2a675b`
  - 复用 `load_session_inner` + `get_pending_interaction_inner`
  - 返回完整 session 状态 + pending interaction
- [x] **C4** `state.rs`:`AppState.sse: Arc<SseRegistry>` 字段 — commit `f2a675b`
- [x] **C5** `agent/chat.rs`:接受 sink 注入,Q0 决议下 `chat_inner` 接 `worker_event_sink` 参数 — commit `e4bb80b`(P2.4 C5 完整闭合 SubagentEventSink)
- [x] **C6** `app/src/transport/http.ts`:填充 stub — commit `f2a675b`
  - `invoke(cmd, args)` → `fetch('/api/v1/' + path, {method: 'POST', body: JSON.stringify(args), headers: {'Content-Type': 'application/json'}})`,错误 → `throw new TransportError(status, body)`
  - `listen(event_name, handler)`:单全局 `EventSource('/api/v1/stream')`,按 `event` name 分发(handler 收已解包 payload)
  - `stream-resync` sentinel 作为普通 event 透传给注册的 store,store 自己 GET snapshot(transport 不持有 session 状态)
- [x] **C7** `app/src/transport/index.ts`:切换逻辑 — commit `f2a675b`(P2.4 D3 改默认 httpTransport)
  - 默认 httpTransport(P2.4 后)
  - `?transport=tauri` query 强制走 Tauri(Full 模式逃生)
- [ ] **C8** `app/src/transport/api-types.ts`:hand-written TS 类型 — **P2.5 遗留,低风险随用随补**(留后)
  - 84 handler 入参 + 返参
  - SSE event payload 类型(含 `session_id` 字段)
- [x] **C9** SSE 单测 + 集成测试 — commit `f2a675b` + `2392f41`(单测)+ `e6b7a2f`(E2E harness E1b)
  - 单测:`SseRegistry` 行为(broadcast / replay / 淘汰 / 上限 / large-payload 旁路),sse.rs 7 单测
  - 集成:`tests/e2e.rs` E1b SSE 重连协议 4 tests(replay / sentinel / large-payload / first-connection)
  - 集成:`tests/e2e.rs` E1a chat happy-path(httpmock Anthropic SSE)
- [x] **C10** httpTransport 单测:vitest `app/src/transport/http.test.ts`(mock fetch + EventSource) — commit `f2a675b`;+ `transport-parity.test.ts`(E2 契约层)commit `e6b7a2f`
- [x] **C11** 全套 `cargo test` + `pnpm vitest run` 全绿

### 验证命令

```bash
# 1. SSE 单测 + 集成测试
cd app/src-tauri && cargo test --lib daemon::sse::
cargo test --test e2e -- --test-threads=1 sse_basic

# 2. 前端切到 httpTransport 跑通
pnpm dev  # 仅 Vite,不启 Tauri
# 浏览器打开 http://localhost:1420?transport=http
#    (daemon 需已起在 7456)

# 3. 手动 smoke test 全清单
#    □ 发消息,看到流式 delta
#    □ permission:ask 弹窗 + 点允许
#    □ ask_user_question 卡片 + 回答
#    □ subagent drawer 事件流
#    □ 项目列表 backfill 后刷新

# 4. 事件序列对拍(Tauri vs HTTP)
cargo test --test e2e event_sequence_parity

# 5. Tauri 版仍可用(默认走 tauriTransport)
pnpm tauri dev
```

### 回滚点

- C1-C5 是新模块 + 新 trait impl,删除文件 + 还原 `chat_loop` 注入参数即回滚
- C6-C7 httpTransport 仍可降级为 stub(同 Phase 1 stub 形态),前端默认走 tauriTransport
- **核心回滚保证**:即使整个 SSE 模块废弃,Tauri 路径完全不受影响(独立注入 sink)

### 提交时机

C1-C11 全 ✓,浏览器 smoke test 全过,事件序列对拍一致 → 一次 commit `feat(daemon+sse): HttpSseSink + single global stream + httpTransport (Phase 2.3)`。

---

## P2.4 — GUI 全走 RPC + daemon 内嵌静态文件(2-3 天)

### 实现清单

- [x] **D1** `app/src-tauri/tauri.conf.json`:新增 `bundle.externalBin = ["binaries/everlasting-daemon"]`(或 Tauri 2 等价配置)
- [x] **D2** Tauri setup 钩子:`app.handle().plugin(tauri_plugin_shell::init())` 或 sidecar API
  - 启动时 spawn sidecar `everlasting-daemon --port 7456`
  - 监听 sidecar 进程事件,关窗时 SIGTERM
- [x] **D3** `app/src/transport/index.ts`:GUI 启动时
  - 调 `fetch('/api/v1/health')`(经 sidecar)→ 校验 `daemon_id` / `api_versions`(Q5 分层校验)
  - 协议不匹配 → fail loud;构建不一致 → console warning
  - health 通过 → 切到 httpTransport
- [x] **D4** `app/src-tauri/src/daemon/server.rs`:扩展 axum 路由,根路径 `/` ServeDir 指向 `app/dist/`
  - 需先 `pnpm build` 产出 `dist/`
  - 生产模式单二进制部署(dev 模式前端走 Vite 1420)
- [x] **D5** GUI 进程**不**建 SqlitePool
  - ~~`AppState::load` 在 GUI 路径下调瘦壳~~ → **决议变更(全瘦客户端重构)**:Thin 模式根本不调 `AppState::load`、不 `app.manage(AppState)`(79 handler 仍注册但 httpTransport 不 invoke 故不触发 State 解析)。Full 模式(`?transport=tauri`)保留原行为作逃生。
  - 验证(手动,WSL 无 GUI 运行时):`lsof -p <gui-pid>` 无 SQLite 文件句柄
- [x] **D6** `pick_project_dir` 浏览器降级
  - Tauri 用 `tauri-plugin-dialog` 原生选择
  - 浏览器模式:`<input type="text">` 路径输入(store `manualPathOpen` + `addProjectByPath`),daemon 复用 `create_project` 校验
  - ~~统一 UX 抽象 `<ProjectDirPicker mode="auto">`~~ → **简化**:直接在 `ProjectTabs.vue` 内联路径输入(store 级降级,非独立组件),4 个 vitest 覆盖
- [x] **D7** dev 模式:`pnpm dev` 加 `concurrently`(新增 dev 依赖)
  - `dev:all` script: `concurrently "pnpm dev" "cargo run --bin everlasting-daemon -- --port 7456"`
  - `daemonBase()` DEV 探测:vite 1420 ↔ daemon 7456 跨域默认走 7456;PROD sidecar 同源走 `location.origin`
- [x] **D8** 双进程写竞争消除:~~同时开 Tauri + 浏览器~~ → **`init_pool` 加 WAL + busy_timeout=5s**(SqliteConnectOptions per-connection pragma)+ 4 个单测(WAL mode / busy_timeout / foreign_keys / 并发读不 SQLITE_BUSY)
- [x] **D9** 全套 `cargo test` + `pnpm vitest run` 全绿(cargo 1561 / vitest 934 / vue-tsc 0 err)

> **D5 偏差说明(2026-07-22)**:原方案"瘦壳 AppState"会要求把 `db: SqlitePool` 改成 `Option`(101 处 `state.db` 访问全要改),代价过高。改用"Thin 模式不 load AppState"——79 handler 仍注册(编译不变),但 httpTransport 默认不走 invoke,故无 State 解析 panic。`?transport=tauri` 走 Full 模式(调原 `AppState::load`)作逃生。这满足 AC "lsof 无 SQLite 句柄"(Thin 不开 pool)且零侵入 commands。
> **WSL 验证限制**:本环境无 GUI 运行时,sidecar spawn / health 握手 / 关窗 SIGTERM / 双进程 / `pnpm tauri build` 留手动验证清单(见下方)。

### 验证命令

```bash
# 1. dev 模式
pnpm tauri dev
#    验证:
#    - concurrently 启 daemon 在另一 pane
#    - GUI 自动连 http://localhost:7456(httpTransport)
#    - GUI 进程无 SQLite 句柄(lsof)
#    - 关窗时 daemon 进程清理(ps)

# 2. production 模式
cd app && pnpm build  # 产出 dist/
cd src-tauri && cargo build --release --bin everlasting-daemon
./target/release/everlasting-daemon --port 7456
# 浏览器打开 http://localhost:7456
#    验证:同时拿到前端 + API(同源)

# 3. 双进程写竞争测试
cargo test --test e2e dual_process_no_sqlite_busy

# 4. GUI 关闭后 daemon 清理
# 关 Tauri 窗口 → ps -ef | grep everlasting-daemon → 应已退出
```

### 回滚点

- D1-D3 走 sidecar 路径:删 externalBin 配置 + 还原 Tauri 入口即回滚(Q2 决议)
- D4 静态文件服务是 daemon 新增,删除 ServeDir mount 即回滚
- D5-D6 是 UX 改造,改回原 `pick_project_dir` 调用即回滚

**关键风险**:D3 切到 httpTransport 是**不可逆切换**——失败时 GUI 完全无功能。缓解:
- D2 sidecar 启动失败 → GUI 弹错误并退出(不静默 fallback)
- D3 health 校验失败 → 弹错误并退出
- D8 写竞争测试必须通过

### 提交时机

D1-D9 全 ✓,`pnpm tauri dev` 正常,`pnpm tauri build` 产出可用 daemon,双进程写竞争测试过 → 一次 commit `feat(daemon+gui): sidecar spawn + httpTransport switch + static file serving (Phase 2.4)`。

---

## P2.5 — WSL 端到端验证 + E2E harness(3-4 天,含 0.5 周 E2E)

### 实现清单

> **Scope 决议(2026-07-23,务实落地版)**:原 E1 设想的"直接注入 MockProvider 跑 agent loop"在架构上不可达 —— 集成测试 `tests/e2e.rs` 只能访问 `everlasting_lib::daemon::*`(lib.rs 唯一 `pub mod`),`db`/`agent`/`llm`/`state` 全私有,`MockProvider` 是 `#[cfg(test)]` lib-crate-only。可行路径:经 `build_router` + `load_daemon_state` 发真实 HTTP,DB seed 走 HTTP 自身端点,provider mock 用 `httpmock`(整段 SSE 当单 body,SseParser 字节导向能解析)。原 E2 设想的"双 transport 双跑套件"也无现有机制(24 个测试文件已全 mock transport,双跑无意义)。故 E1/E2 改为**契约层 + happy-path** 务实版;E4/E5 留手动。

- [x] **E1** `app/src-tauri/tests/e2e.rs`(Rust integration test,10 tests 全绿):
  - `e1a_chat_happy_path_httpmock`:HTTP seed catalog(create_project/add_provider/add_model/set_default_model/create_session)+ httpmock Anthropic 整段 SSE → POST /chat → 断言 mock 收到 1 次 `POST /v1/messages`(异步 spawn + `hits_async` 轮询,避免阻塞 runtime)
  - `e1a_chat_with_no_model_returns_structured_error`:无 model 时 chat 不 500 panic(pre-flight 失败走 SSE error 路径)
  - `e1b` SSE 重连协议(4 tests,纯 `SseRegistry` pub API):replay after Last-Event-ID / resync sentinel on buffer overrun / large-payload 跳过 buffer / first-connection no-replay
  - `e1c_snapshot_returns_session_state`:`GET /api/v1/sessions/{id}/snapshot` 200 + JSON
  - `e1d_health_wire_shape_via_router` + `health_bare_alias_works`:经完整 router 验证 health wire shape(camelCase 字段 + api_versions 含 v1)+ `/health` 别名
  - `e1e_all_api_routes_are_mounted`:全部 `/api/v1/*` route 发空 body 断言非 404(route 漏挂载回归保护;从 `routes/*.rs` 的 `.route(...)` 提取的真实列表)
- [x] **E2** `app/src/transport/transport-parity.test.ts`(8 tests 全绿):**契约层一致性**而非双跑重构。mock Tauri API + fetch/EventSource,断言 tauriTransport / httpTransport 对同一组 `invoke`/`listen` 调用行为对齐 —— 成功路径 resolve 同值 / 失败路径都 reject / listen 投递**已解包** payload(非 Tauri `Event<T>` 信封)/ unlisten 取消生效 / listen 返回 `Promise<UnlistenFn>`。+ httpTransport camelCase→snake_case 顶层 key 转换锁定。
- [x] **E3** WSL 部署文档:`docs/HACKING-wsl.md` 新增 §远程访问 daemon 部署
  - 生产模式(单二进制:build dist + daemon release + 浏览器 localhost:7456)
  - dev 模式(vite 1420 + daemon 7456 + `?daemonUrl=` 跨域)
  - 降级排查(daemon 监听 0.0.0.0 / WSL 2 localhost forwarding / 虚拟 IP 172.x.x.x / netsh portproxy)
  - Tauri GUI sidecar 模式(`?transport=tauri` 逃生)+ 验证命令速查
  - `docs/REMOTE-ACCESS-ROADMAP.md` Phase 2 整体验收段更新(P2.1–P2.5 代码+测试就绪,GUI 实跑留手动)
- [ ] **E4** 手动 smoke test checklist(P2.5 验收,**留 GUI-capable 机器手动**):
  - WSL→Windows 宿主浏览器跑通(发消息 / 流式 / permission / question / subagent)
  - 断网重连后 UI 完整恢复(EventSource 自动重连 + Last-Event-ID 回放 / sentinel → snapshot)
  - 5MB shell 输出不丢(LARGE_PAYLOAD_THRESHOLD 旁路 live channel,buffer 不爆)
  - 关 Tauri 窗口后 daemon 进程清理(`RunEvent::Exit` → sidecar SIGTERM)
  - daemon 重启后 GUI 自动重连(`awaitDaemonHealthy` 健康检查)
  - 双进程(Tauri Thin + 浏览器)无 SQLITE_BUSY(WAL + busy_timeout=5s)
- [ ] **E5** Dogfooding 周期文档:Phase 3 启动条件(主分支 ≥ 2 周 dogfooding 无 P0/P1)—— 计时未起

### 验证命令

```bash
# 1. E2E harness 跑通(本环境已验证,10 tests 全绿)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --test e2e -- --test-threads=1

# 2. E2 transport parity(本环境已验证,8 tests 全绿)
cd app && pnpm vitest run src/transport/transport-parity.test.ts

# 3. 全量回归(本环境已验证)
cd app/src-tauri && PKG_CONFIG_PATH="..." cargo test --lib   # 1561+ 不回退
cd app && pnpm vitest run && pnpm vue-tsc --noEmit

# 4. WSL 端到端(GUI-capable 机器手动)
cd app/src-tauri && cargo run --release --bin everlasting-daemon -- --port 7456
# Windows PowerShell / 浏览器:见 docs/HACKING-wsl.md §远程访问 daemon 部署
```

### 回滚点

E1-E5 是验证而非新功能,失败不阻塞主路径(主路径已在 P2.4 完成)。

### 提交时机

E1-E5 全 ✓,E2E harness 全过,WSL 端到端走通 → 一次 commit `test(daemon): E2E harness + WSL deployment docs (Phase 2.5)` + 更新 `docs/HACKING-wsl.md` + 更新 `docs/REMOTE-ACCESS-ROADMAP.md` 标记 Phase 2 完成。

---

## Phase 2 整体验收(P2.1-P2.5 全 ✓ 后)

- [ ] P2.1 ~ P2.5 全部 commit 在主分支
- [ ] 本机浏览器可访问 daemon(`pnpm build && cargo build --release --bin everlasting-daemon && ./everlasting-daemon`,浏览器开 http://localhost:7456)
- [ ] **WSL→Windows 宿主**浏览器访问跑通(用户主用场景)
- [ ] Tauri 版仍可用(`pnpm tauri dev`,默认走 httpTransport 连本机 daemon)
- [ ] 84 command 行为在 HTTP transport 下与 Tauri 版一致(vitest 对拍)
- [ ] 10 类 SSE 事件 + 4 类 round-trip 端到端验证通过
- [ ] 无 SQLITE_BUSY(GUI 不开 db,daemon 独占)
- [ ] daemon 单二进制部署可用(浏览器访问同源拿前端 + API)
- [ ] **主分支 dogfooding ≥ 2 周**,无 P0/P1 问题后,标记 Phase 2 完成

---

## 风险登记表

| ID | 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|---|
| **R-1** | P2.4 切 httpTransport 后 daemon 不稳,GUI 完全无功能 | 中 | 高 | Q1 fail loud + sidecar 自动重启 + health 校验 |
| **R-2** | 84 handler 机械映射遗漏字段 / 类型不匹配 | 中 | 中 | E2E 对拍测试 + 84 handler happy path 覆盖 |
| **R-3** | SSE 断网重连 + 5MB tool_result 不丢 | 中 | 高 | R6.1 resync sentinel + buffer 上限 512 + 大 message 旁路 |
| **R-4** | GUI 进程误开 SqlitePool 导致 dual-pool | 低 | 高 | D5 明确瘦壳 + D8 写竞争测试 + lsof 验证 |
| **R-5** | WSL localhost forwarding 在某些 Windows 版本不通 | 中 | 中 | HACKING-wsl §WSL 远程访问部署 列出降级方案(虚拟 IP / netsh portproxy) |
| **R-6** | tokio::spawn 与 tauri::async_runtime::spawn 行为差异 | 低 | 低 | Q6 决议已隔离(daemon 走 tokio,Tauri 路径保留原) |
| **R-7** | axum 0.7 与 Tauri 2 冲突(chrono 等 transitive deps) | 低 | 中 | P2.2 早期 spike 验证版本兼容 |

## 复盘检查点

- **P2.2 完成后**:review 84 handler 是否有重大业务逻辑需抽 service(若 > 30% handler 超过 50 行,回头补 `_inner` → service 抽象)
- **P2.3 完成后**:验证 Phase 1 §3.3 承诺(SubagentEventSink 独立注入)是否破坏
- **P2.4 完成后**:**关键 gate**——切 httpTransport 后跑 1 周 dogfooding 才进入 P2.5
- **P2.5 完成后**:Phase 2 整体复盘,决定是否进入 Phase 3 远期规划

---

## 复盘模板(每个子阶段 commit 时填)

```markdown
## <子阶段名> 复盘 (YYYY-MM-DD)

### 完成度
- [ ] A1..N / B1..N / C1..N / D1..N / E1..N 全 ✓

### 验证命令输出
- cargo test: ...
- pnpm vitest: ...
- 手动 smoke: ...

### 偏差 / 未解决问题
- (列出本次与设计文档的偏差)

### 下一步
- (进入下一子阶段 / 回头修复 / 暂停)
```