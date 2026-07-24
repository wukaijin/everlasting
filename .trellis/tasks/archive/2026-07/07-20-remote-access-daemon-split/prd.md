# 远程访问 Phase 2:daemon 拆分 + 本地 HTTP server + SSE

> Parent: [07-20-remote-access-multi-channel](../07-20-remote-access-multi-channel/)
> 实施路线: [docs/REMOTE-ACCESS-ROADMAP.md Phase 2](../../../docs/REMOTE-ACCESS-ROADMAP.md#phase-2daemon-拆分--本地-http-server2-3-周--05-周-e2e)
> 配套设计:[design.md](./design.md) | 实施清单:[implement.md](./implement.md)
> **状态:Phase 2 规划完成(2026-07-21),等待主分支 dogfooding 后启动实施。**

## Goal

把 agent core 从 Tauri GUI 进程拆到独立 daemon 进程,启动 axum HTTP server,本机浏览器(含 **Windows 宿主访问 WSL daemon**)可完整使用 agent 功能。

## Background

- **Phase 1 已归档**(commit `0dbc747`,2026-07-20):前端 Transport 接口 + 后端 emit sink 收敛。
- Phase 2 在此基础上:
  - 新增 `everlasting-daemon` cargo bin,持有 `AppState` + agent core
  - **84** 个 `#[tauri::command]`(grep 实测)映射为 axum HTTP handler
  - 10 类 emit 事件经 `HttpSseSink` 推到浏览器(Phase 2 选定单全局 EventSource + 事件分发表,Q3 决议)
- **架构决策 D1(单一协议统一)**:Phase 2 完成后 Tauri GUI 也切 httpTransport,Tauri 内嵌 IPC 入口废弃为死代码。
- **承接 Phase 1 §3.3 承诺**:`HttpSseSubagentSink` 必须独立于 `HttpSseSink` 实现(各自对应 sink trait,各自独立注入),**禁止**合并为单一 trait(详见 [transport-abstraction design.md §3.3](../07-20-remote-access-transport-abstraction/design.md#33-subagenteventsink-trait-设计))。

## Technical Notes(2026-07-21 代码盘点 + brainstorm 决议)

### 代码现状

- **C1** grep 计数 `#[tauri::command]` 共 **84 处**,分布:`commands/` 下 21 个文件 + `agent/chat.rs` + `git/error.rs` + `commands/tests_*`(2) + `lib.rs`。PRD 早期估计 79 是低估,实施清单按 84 走。
- **C2** `AppState::load(app: &AppHandle)`(`state.rs:215`)当前依赖 `app.path().app_data_dir()`,daemon 化需重构成接受 `PathBuf`。
- **C3** `state.rs:317` `projects:refreshed` 直调 `app.emit`,Phase 1 已保留(daemon 化时统一收口走 R6)。
- **C4** 当前依赖:**未引入** axum / tower-http / tokio-stream SSE(EventSource server 端)。Cargo.toml 需新增 `axum`、`tower-http`、`tokio-stream`。
- **C5** `Cargo.toml` 当前 `[[bin]]` 只有 `everlasting`(lib.rs),Phase 2 需新增 `[[bin]] name = "everlasting-daemon" path = "src/bin/everlasting-daemon.rs"`。
- **C6** `lib.rs` 当前用 `tauri::async_runtime::block_on(AppState::load(&app.handle()))`,daemon 化不影响此路径(Phase 2 保留 Tauri 入口作为过渡)。
- **C7** `HttpSseSink` 实现 `ChatEventSink` trait(由 `agent/chat.rs` 持有);`HttpSseSubagentSink` 实现 `SubagentEventSink` trait(由 `agent/subagent/sink.rs` 持有)。**两者并列独立**。

### 已决议设计决策(2026-07-21 brainstorm)

| ID | 决策 | 否决方案 | 关键理由 |
|---|---|---|---|
| **Q0** | handler 内联 `_inner` 模式(command 文件保留业务逻辑单份,Tauri/axum 双入口都调 `_inner`) | 强制抽 `crate::service::*` | 拿复用、不付跨模块搬迁 + 多一层抽象成本 |
| **Q1** | daemon 端口:`--port` > `EVERLASTING_DAEMON_PORT` > 默认 7456;**端口占用绝不自动跳**;启动前 health-check 区分(自己 daemon→复用 / 别的程序→fail loud 中文提示) | 自动跳端口 | 避免多 daemon 数据分裂;复用路径显式 allow |
| **Q2** | GUI spawn:production 走 Tauri `bundle.externalBin` sidecar,dev 走 `concurrently` 常驻 daemon + GUI 只连 | GUI 运行时 cargo run | 编译延迟 / 跨平台路径差异 |
| **Q3** | `projects:refreshed` 走统一全局 SSE channel(`event: "system-projects-refreshed"`),**不**抽 `SystemEventSink` trait;**R6 拓扑决策:单全局流**(`/api/v1/stream`)+ 事件分发表(替代 R6 原案 per-session 多流) | 抽 SystemEventSink trait / 走轮询 | 单 EventSource 简化前端 listen handler(避免按 session 起多连接);events payload 含 `session_id` 字段供前端按 session 过滤;避免 trait 体系膨胀 |
| **Q4** | **不引入 ts-rs**,Phase 2 新增 endpoint 的 TS 类型在 `app/src/transport/api-types.ts` 手写;类型同步靠 vue-tsc + E2E 对拍 | cargo build hook 生成 | 注解成本 + build hook 与手写类型冲突,Phase 2 不成正比 |
| **Q5** | daemon `/api/v1/health` 返回 `{daemon_id, daemon_version, api_versions}`;GUI 启动时**分层校验**:协议版本不匹配 fail loud,构建版本不一致仅 warning | 仅 warning(协议不兼容静默击穿) | 区分协议 / 构建版本,前者必须严格,后者宽容 |
| **Q6** | daemon 入口 `#[tokio::main]`,后续 `tokio::spawn(...)`;Tauri 路径保留 `tauri::async_runtime::spawn` | 统一 runtime | 隔离差异,D1 决议下 Tauri 路径未来废弃 |
| **Q7** | **Rust integration test**(`app/src-tauri/tests/e2e.rs`)+ 复用现有 `llm/provider/mock.rs`;**不引入** Playwright | Playwright | WSL 浏览器启动成本;mock provider 复用现成 |

### 与 Phase 1 的契约对齐

- `SubagentEventSink` trait 已存在(Phase 1 产出),Phase 2 兑现:新增 `HttpSseSubagentSink: SubagentEventSink`,与 `HttpSseSink: ChatEventSink` 并列,**禁止**合并(否则反向破坏 Phase 1 散点收敛成果)。
- `AppHandleSubagentSink` / `HttpSseSubagentSink` 与 `AppHandleSink` / `HttpSseSink` 各自独立注入,Tauri 入口仍用 `AppHandleSink` 系。
- `streamController.listen` 接收 payload(非 Event<T>)—— Phase 2 `httpTransport.listen` 同语义,按 R6 单全局流后分发表按 `event_name` 路由。

## Requirements

### daemon 进程

- **R1** 新增 `src-tauri/src/bin/everlasting-daemon.rs`:daemon main 入口,`#[tokio::main]` 启动 axum runtime,调 `AppState::load(PathBuf)` 启动 axum HTTP server,监听 `0.0.0.0:PORT`(默认 7456,Q1 决议)。启动前 health-check(Q1 端口冲突 fail loud)。
- **R2** `AppState::load` 重构:接受 `PathBuf`(data_dir)而非 `AppHandle`;**保留 AppHandle wrapper** 供过渡期 Tauri 用(Q0 决议 handler 内联 `_inner` 模式);**验证两种入口产出同一 SQLite 文件路径**(新增 `state_load_path_consistency` 测试)。
- **R3** daemon 内嵌静态文件 server(`tower-http::services::ServeDir` 指向 `app/dist/`),实现单二进制部署(前端 + API 同源,免 CORS)。

### HTTP 协议

- **R4** **84** 个 command 机械映射为 `POST /api/v1/<domain>/<command>` handler(Q0 `_inner` 模式,无 duplication),JSON body 字段保持 snake_case(与 `AppCommandError` 一致)。URL 形态简单:每个 handler 接受一个 JSON body 调对应 `_inner`,**不**做 REST 风格的资源嵌套(无 `GET /api/v1/sessions/{id}/messages/{seq}` 之类)——与 Tauri `invoke(cmd, args)` 语义一致,降低 httpTransport shim 复杂度(机械映射,无需 URL 模板)。
- **R5** TS 类型来源(Q4 决议):不引入 ts-rs,`app/src/transport/api-types.ts` 手写;类型同步靠 `pnpm vue-tsc --noEmit` 编译期检查 + E2E 对拍测试。
- **R6** SSE endpoint `GET /api/v1/stream`(Q3 决议单全局流,非 per-session 多流):
  - daemon 维护 `Arc<RwLock<HashMap<event_name, Vec<SseSender>>>>` 分发表 + 按 `session_id` 维度 `VecDeque<{id, event}>` 环形 buffer(Q-SSE-1 决议:event id 递增整数 u64、buffer 上限 512、单条 > 256KB 不入 buffer 走大 message 旁路)。
  - `HttpSseSink` emit 时按 `event_name` 路由到所有订阅者,事件 payload 含 `session_id` 字段供前端 listen handler 过滤。
  - `event: "stream-resync-{session_id}"` sentinel(Q-SSE-1 决议:不用 `410 Gone`):Last-Event-ID < buffer_oldest 时发,通知 client 走 R6.1.a resync。
  - **R6.1.a resync 协议**:client 收到 resync sentinel 后调 `GET /api/v1/sessions/{id}/snapshot`(新增 endpoint),daemon 复用 `load_session_inner` + `get_pending_interaction_inner` 返回完整会话状态 + pending interaction(**不新建 service**)。
  - **R6.2** 客户端在线检测:daemon 端 30s 间隔 `:\n\n` ping;每条 SSE 连接 60s 无响应关闭;client 端 60s 无 frame 触发 reconnect。
  - **R6.3** 大 message 阈值:单条 SSE 上限 8MB,超过则走 R6.1 buffer 旁路(不进 buffer,断网重连时丢失靠 client resync 拉取历史 tool_result DB 最新值)。
- **R7** 4 类 round-trip(permission / question / mode_change / task_state_transition)的 oneshot 跨进程转换:axum handler 通过 `Extension<Arc<AppState>>` 调 `permission_store.resolve()` / `question_store.resolve()`,命中 daemon 进程内 oneshot sender。**关键**:`request_id` 是 GUI 维护的 LRU key,daemon 通过 `PermissionStore` 等内存结构解析,session_id 不是 round-trip 必需。

### 前端 + GUI

- **R8** 填充 `httpTransport` 实现(Q3 + Q7 决议):`invoke` → `fetch('/api/v1/' + path, POST, body)`;`listen` → **单全局** `EventSource('/api/v1/stream')` + 事件分发表(按 event_name 路由 + session_id 过滤);收到 resync sentinel → 自动 GET snapshot,store 替换。
- **R9** Tauri GUI 改为 thin client(Q2 决议):
  - dev:`concurrently` 启 daemon 在独立 pane,GUI 启动后只连 `http://localhost:7456`(不主动 spawn)。
  - production:Tauri `bundle.externalBin` 配置 sidecar,自动 spawn + 关窗 SIGTERM。
  - **GUI 不开本地 SqlitePool**(消除 dual-pool 写竞争,符合 D1)。
- **R10** `pick_project_dir` 浏览器降级 UX:手动输入项目路径文本框(daemon 校验路径存在性),浏览器拿不到绝对路径;统一 `<ProjectDirPicker mode="auto">` 抽象。

### WSL 部署

- **R11** WSL 2 localhost forwarding 验证:daemon 跑 WSL 监听 `0.0.0.0:7456`,Windows 宿主浏览器访问 `http://localhost:7456` 可用。降级:WSL 虚拟 IP(`172.x.x.x:7456`)或 `netsh portproxy`。部署文档写入 `docs/HACKING-wsl.md` §WSL 远程访问部署。

### 测试(Q7 决议:Rust integration test + 手动 smoke)

- **R12** axum handler 单元测试:**由 P2.5 integration test 覆盖(Q7 决议)**——P2.2 不强制 84 handler happy-path 单测;`app/src-tauri/tests/e2e.rs` 在 P2.5 一并覆盖 happy path + 错误码,避免重复维护单测 + 集成测试两套断言。
- **R13** SSE 集成测试 + 单测:mock provider 跑 1 轮 agent loop,断言 10 类事件序列到 SSE 客户端;Last-Event-ID 重连;resync sentinel 路径;大 message 旁路。
- **R14** E2E harness(`app/src-tauri/tests/e2e.rs`):10 类 SSE 事件 + 4 类 round-trip;断网重连;5MB shell 输出走 resync snapshot;双进程写竞争消除。
- **R15** 回归测试:同一套 vitest 走 httpTransport vs tauriTransport,84 command 行为对拍一致(除 transport-specific 测试)。

## Acceptance Criteria

### 功能(每项均需独立可验证)

- [ ] daemon 能独立启动(`cargo run --bin everlasting-daemon`),`curl /api/v1/health` 返回 200 + JSON 含 daemon_id / daemon_version / api_versions
- [ ] **84** 个 HTTP handler 覆盖完整(handler 单测全绿 + curl 冒烟)
- [ ] 本机浏览器可完整使用 agent(发消息 / 流式 delta / permission 弹窗 / ask_user_question / subagent drawer)
- [ ] **WSL→Windows 宿主**浏览器访问跑通(用户主用场景)
- [ ] daemon 单二进制部署可用(浏览器访问 `http://localhost:7456` 同时拿到前端 + API)

### 一致性(回归)

- [ ] **84** 个 command 在 HTTP transport 下的行为与 Tauri 版一致(vitest 对拍)
- [ ] 10 类 SSE 事件序列与 Tauri 端 emit 序列一致(集成测试对拍)
- [ ] 无 `SQLITE_BUSY`(GUI 不开 db,daemon 独占)
- [ ] GUI 进程 `lsof` 无 SQLite 文件句柄

### 稳定性

- [ ] SSE 断网重连后不漏事件(resync sentinel 路径触发)
- [ ] 5MB `tool_result` shell 输出在 SSE chunked transfer 下不截断
- [ ] GUI 关闭后 daemon 子进程清理(无泄漏)
- [ ] **主分支 dogfooding ≥ 2 周**后无 P0/P1 问题

### 架构不变量

- [ ] `HttpSseSubagentSink: SubagentEventSink` 与 `HttpSseSink: ChatEventSink` **独立**实现,各自独立注入(Phase 1 §3.3 承诺兑现)
- [ ] AppState::load 接受 PathBuf;Tauri wrapper 保留;两条路径产出同一 SQLite 文件路径
- [ ] 84 handler 业务逻辑单份(`_inner` 函数),Tauri/axum 双入口复用

## Dependencies

- **前置**:[07-20-remote-access-transport-abstraction](../07-20-remote-access-transport-abstraction/) 完成(commit `0dbc747`,2026-07-20)—— Transport interface + emit sink trait + `_inner` 模式雏形。
- **后置**:无(Phase 3 认证 + 远程是远期,不建 task)。
- **启动条件**:Phase 1 已 archive + **主分支 dogfooding ≥ 3 天**(确认 Phase 1 抽象无 regression)。

## Notes

- **不实现**:认证 / HTTPS / 配对码(Phase 3 远期);Electron(Phase 4 可选)。
- **工作量估**:2-3 周 + 0.5 周 E2E(`docs/REMOTE-ACCESS-ROADMAP.md §Phase 2`)。
- **关键 gate(P2.4 后)**:切 httpTransport 是**不可逆切换**,daemon 不稳时 GUI 完全无功能。缓解:Q1 fail loud + sidecar 自动重启 + Q5 health 校验 + 双进程写竞争测试。
- **dogfooding 周期**:P2.4 完成后需在主分支上至少 dogfooding **2 周**,确认 daemon 在长期使用中稳定,才标记 Phase 2 完成。