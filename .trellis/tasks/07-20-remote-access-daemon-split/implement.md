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

- [ ] **A1** `app/src-tauri/src/state.rs::AppState::load` 改为接受 `PathBuf`(data_dir)
- [ ] **A2** 内部逻辑不变:LLM config from_env + db::init_pool + run_migrations + provider catalog + backfill spawn
- [ ] **A3** 保留旧 `load(app: &AppHandle)` 签名作为 wrapper:`fn load(app: &AppHandle) -> impl Future<...>` → `Self::load(app.path().app_data_dir().unwrap())`
- [ ] **A4** `state.rs:317` `projects:refreshed` 改走新接口(传 `Arc<dyn SystemEventSink>` 或保留 AppHandle 用于 Tauri 路径,daemon 路径传 stub noop sink)
- [ ] **A5** 新增测试 `state_load_path_consistency`:对比 `AppState::load(AppHandle)` 与 `AppState::load(PathBuf)` 产出同一 `db_path` 与 `home_dir`(mock AppHandle 与 mock PathBuf 同源)
- [ ] **A6** `cargo test --lib` 全绿(vitest 不变)

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

- [ ] **B1** `app/src-tauri/src/bin/everlasting-daemon.rs`:daemon main 入口
  - `#[tokio::main]`
  - 解析 `--port` / `EVERLASTING_DAEMON_PORT` / 默认 7456
  - 启动前调 `GET http://localhost:{port}/api/v1/health` 端口冲突检查(Q1 fail loud)
  - `AppState::load(data_dir)` → `Arc<AppState>`
  - `axum::serve(...).with_graceful_shutdown(shutdown_signal())`
- [ ] **B2** `app/src-tauri/src/daemon/server.rs`:axum router 装配
  - `Router::new().route("/api/v1/health", get(health))`
  - `.nest("/api/v1", routes::router(state.clone()))`
- [ ] **B3** `daemon/routes/health.rs`:`GET /api/v1/health` 返回 `{daemon_id, daemon_version, api_versions: ["v1"], uptime_seconds, session_count}`
- [ ] **B4** **84 命令 _inner 拆解**(commands/*.rs + agent/chat.rs + git/error.rs)
  - 每个 `pub async fn xxx(state, ...) -> Result<...>` 拆为 `pub async fn xxx_inner(state: &Arc<AppState>, ...) -> Result<...>` 保留业务逻辑
  - 原 `#[tauri::command]` 入口退化为 `xxx_inner(&state, ...).await` 薄包装
- [ ] **B5** `daemon/routes/{domain}.rs`:84 axum handler,每个文件对应一个 domain,handler 调对应 `_inner`
  - 例:`routes/sessions.rs::list_sessions(Extension(state): Extension<Arc<AppState>>, Json(req): Json<ListSessionsReq>) -> Result<Json<ListSessionsResp>, AppCommandError>`
- [ ] **B6** 错误转换:handler 返回 `AppCommandError` → axum `IntoResponse` 转换为 HTTP 状态码 + JSON body
- [ ] **B7** handler 单测:`daemon/routes/tests_*.rs`(按 domain),happy path + 错误码
- [ ] **B8** 全套 `cargo test` 全绿,包括新 handler 测试

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

- [ ] **C1** `app/src-tauri/src/daemon/sse.rs`:
  - `HttpSseSink` 实现 `ChatEventSink` trait(注入 `chat_loop`)
  - `HttpSseSubagentSink` 实现 `SubagentEventSink` trait(注入 `subagent/sink.rs`)**Phase 1 §3.3 承诺**
  - `SseRegistry`:全局 `Arc<RwLock<HashMap<event_name, Vec<SseSender>>>>`
  - `SseBuffer`:按 `session_id` 维护 `VecDeque<{id, event}>`(每 session 独立 `AtomicU64`)
  - 单条 > 256KB 不入 buffer,直推(走 R6.3 大 message 旁路)
  - `event_name = "stream-resync-{session_id}"` sentinel(Last-Event-ID < buffer_oldest 时发)
- [ ] **C2** `daemon/routes/health.rs` 同文件扩展:`GET /api/v1/stream`
  - SSE handler:接收 `Last-Event-ID` 头
  - 客户端连入时注册 `SseSender` 到 `SseRegistry`
  - 30s 间隔 `: ping` 心跳;60s 无响应主动断开
- [ ] **C3** `daemon/routes/sessions.rs`:新增 `GET /api/v1/sessions/{id}/snapshot`
  - 复用 `load_session_inner` + `get_pending_interaction_inner`
  - 返回完整 session 状态 + pending interaction
- [ ] **C4** `daemon/state.rs`(`AppState` 扩展或新结构):注入 `HttpSseSink` 实例 + `SseRegistry` 句柄
- [ ] **C5** `agent/chat.rs`:接受 sink 注入,Q0 决议下 `chat_inner` 不变(sink 通过参数传)
- [ ] **C6** `app/src/transport/http.ts`:填充 stub
  - `invoke(cmd, args)` → `fetch('/api/v1/' + path, {method: 'POST', body: JSON.stringify(args), headers: {'Content-Type': 'application/json'}})`,错误 → `throw new TransportError(status, body)`
  - `listen(event_name, handler)`:单全局 `EventSource('/api/v1/stream')`,按 `event.data.session_id` 与 `event.type` 双重过滤分发到 handler
  - 收到 `stream-resync-{session_id}` → 自动 GET snapshot,store 替换
- [ ] **C7** `app/src/transport/index.ts`:切换逻辑
  - 默认 `isTauri()` 判定
  - 新增 `?transport=http` query 强制切流(测试用)
  - 新增 `?transport=tauri` 强制走 Tauri(debug)
- [ ] **C8** `app/src/transport/api-types.ts`:hand-written TS 类型
  - 84 handler 入参 + 返参
  - SSE event payload 类型(含 `session_id` 字段)
- [ ] **C9** SSE 单测 + 集成测试
  - 单测:`SseBuffer` 行为(增/淘汰/上限)
  - 集成:mock provider 跑 1 轮 agent loop,断言 10 类事件序列到 SSE 客户端
  - 集成:Last-Event-ID 重连 + resync sentinel 路径
- [ ] **C10** httpTransport 单测:vitest `app/src/transport/http.test.ts`(mock fetch + EventSource)
- [ ] **C11** 全套 `cargo test` + `pnpm vitest run` 全绿

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

- [ ] **D1** `app/src-tauri/tauri.conf.json`:新增 `bundle.externalBin = ["binaries/everlasting-daemon"]`(或 Tauri 2 等价配置)
- [ ] **D2** Tauri setup 钩子:`app.handle().plugin(tauri_plugin_shell::init())` 或 sidecar API
  - 启动时 spawn sidecar `everlasting-daemon --port 7456`
  - 监听 sidecar 进程事件,关窗时 SIGTERM
- [ ] **D3** `app/src/transport/index.ts`:GUI 启动时
  - 调 `fetch('/api/v1/health')`(经 sidecar)→ 校验 `daemon_id` / `api_versions`(Q5 分层校验)
  - 协议不匹配 → fail loud;构建不一致 → console warning
  - health 通过 → 切到 httpTransport
- [ ] **D4** `app/src-tauri/src/daemon/server.rs`:扩展 axum 路由,根路径 `/` ServeDir 指向 `app/dist/`
  - 需先 `pnpm build` 产出 `dist/`
  - 生产模式单二进制部署(dev 模式前端走 Vite 1420)
- [ ] **D5** GUI 进程**不**建 SqlitePool
  - `AppState::load` 在 GUI 路径下调瘦壳(`Arc<AppState>` 仅持有 transport 句柄 + 无 db pool)
  - 验证:`lsof -p <gui-pid>` 无 SQLite 文件句柄
- [ ] **D6** `pick_project_dir` 浏览器降级
  - Tauri 用 `tauri-plugin-dialog` 原生选择
  - 浏览器模式:`<input type="text">` 路径输入,daemon 调 `db::projects::get(id)` 校验存在性
  - 统一 UX 抽象 `<ProjectDirPicker mode="auto">`
- [ ] **D7** dev 模式:`pnpm dev` 加 `concurrently`(新增 dev 依赖)
  - `concurrently "pnpm vite" "cargo run --bin everlasting-daemon -- --port 7456"`
  - GUI 启动时 `pnpm tauri dev` 只连已起 daemon
- [ ] **D8** 双进程写竞争消除测试:同时开 Tauri + 浏览器往同 session 发消息,断言无 SQLITE_BUSY
- [ ] **D9** 全套 `cargo test` + `pnpm vitest run` 全绿

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

- [ ] **E1** `app/src-tauri/tests/e2e.rs`(Rust integration test):
  - `e2e_basic_chat`:daemon + mock provider + SSE client,跑 1 轮 chat,断言 10 类事件序列
  - `e2e_84_handlers`:84 handler 逐一调用 happy path
  - `e2e_error_codes`:84 handler 逐一注入错误码,断言 HTTP 状态码 + 错误体
  - `e2e_sse_reconnect`:断开 EventSource + 重连带 Last-Event-ID,断言 resend 正确
  - `e2e_sse_resync_sentinel`:Last-Event-ID < buffer_oldest 场景,断言 sentinel 触发 + snapshot 路径
  - `e2e_large_payload_5mb`:tool_result 5MB shell 输出通过 SSE 推送,断言不截断
  - `e2e_dual_process`:同时启两个 client(Tauri mock + HTTP),断言无 SQLITE_BUSY
- [ ] **E2** 回归测试套件扩展:`pnpm vitest run --transport=http` + `pnpm vitest run --transport=tauri`,结果应一致
- [ ] **E3** WSL 部署文档:`docs/HACKING-wsl.md` 新增 §WSL 远程访问部署
  - daemon 跑 WSL 监听 0.0.0.0:7456
  - Windows 宿主浏览器 http://localhost:7456(WSL 2 localhost forwarding)
  - 降级:WSL 虚拟 IP 172.x.x.x:7456 / netsh portproxy
- [ ] **E4** 手动 smoke test checklist(P2.5 验收):
  - WSL→Windows 宿主浏览器跑通(发消息 / 流式 / permission / question / subagent)
  - 断网重连后 UI 完整恢复
  - 5MB shell 输出不丢
  - 关 Tauri 窗口后 daemon 进程清理
  - daemon 重启后 GUI 自动重连(httpTransport 健康检查)
- [ ] **E5** Dogfooding 周期文档:Phase 3 启动条件(2 周 dogfooding)

### 验证命令

```bash
# 1. E2E harness 跑通
cd app/src-tauri && cargo test --test e2e -- --test-threads=1
#    期望:所有 e2e_* 测试全过

# 2. WSL 端到端(在 WSL 内)
cd app/src-tauri && cargo run --release --bin everlasting-daemon -- --port 7456
# Windows PowerShell:
curl http://localhost:7456/api/v1/health
# Windows 浏览器:http://localhost:7456 —— 完整功能验证

# 3. 回归测试套件
cd app && pnpm vitest run --transport=http
pnpm vitest run --transport=tauri
#    期望:两套结果一致(除 transport-specific 测试)

# 4. 大 message 边界
cargo test --test e2e -- --test-threads=1 large_payload
cargo test --test e2e -- --test-threads=1 sse_resync
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