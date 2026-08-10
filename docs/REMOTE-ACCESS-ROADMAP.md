# 远程访问 / 多通道改造 — 实施路线图

> **状态**:实施路线图(2026-07-20)。本文档是 [REMOTE-ACCESS-RESEARCH.md](./_archive/2026-07-20-remote-access-research.md) 调研评估的**可执行版本**,把 Phase 1/2/3 拆成可独立验证、独立交付的子阶段。
> **定位**:RESEARCH.md 是"为什么这么做",本文是"具体怎么做、怎么验证"。每个子阶段都满足三个条件:① 能独立提交 ② 有明确的验证标准 ③ Tauri 版始终可用(不破坏现状)。
> **关联**:[ARCHITECTURE §4/§5](./ARCHITECTURE.md#4-决策agent-daemon-化) / [ROADMAP B10](./ROADMAP.md) / [REVIEW-remote-access-research-*](./_reviews/)(2026-07-20 两份 review 已吸纳修正)

---

## 0. 总览

```
近期(可立即启动)                          远期(协议稳定后)
┌─────────────────────────────────┐     ┌──────────────────────┐
│  Phase 1: transport 抽象         │     │  Phase 3: 认证 + 远程 │
│  ├─ P1.1 Transport interface     │     │  (设计草稿见         │
│  ├─ P1.2 TauriTransport + 迁移   │     │   RESEARCH §4.4)     │
│  └─ P1.3 后端 emit 散点收敛      │     └──────────────────────┘
├─────────────────────────────────┤
│  Phase 2: daemon 拆分            │     可选
│  ├─ P2.1 AppState::load 去 AppHandle│   ┌──────────────────────┐
│  ├─ P2.2 axum HTTP server + 79 handler│ │  Phase 4: Electron   │
│  ├─ P2.3 SSE + HttpTransport     │     │  thin client         │
│  ├─ P2.4 GUI 全走 RPC + 静态文件 │     └──────────────────────┘
│  └─ P2.5 WSL 端到端验证 + E2E    │
└─────────────────────────────────┘
```

**核心约束(每个子阶段都必须满足)**:
1. **独立可提交**:每个子阶段是一个独立的 PR/commit,不依赖未完成的后续子阶段
2. **独立可验证**:有明确的验证命令/标准(不是"感觉对了")
3. **Tauri 版始终可用**:`pnpm tauri dev` 在任何子阶段完成后都能正常工作
4. **回归可控**:每个子阶段完成后,既有测试套件全绿

---

## Phase 1:transport 抽象 + emit 散点收敛(2-3 天)

**目标**:把 Tauri IPC 收敛到 `Transport` 接口背后,行为零变化;同时把后端 6 处 emit 散点收到 sink 抽象。为 Phase 2 换 transport 铺路。

### P1.1 Transport interface 定义(0.5 天)

**工作内容**:
- 新增 `app/src/transport/types.ts`:`Transport` interface(`invoke` + `listen`)
- 新增 `app/src/transport/index.ts`:`isTauri()` 判断 + 默认 transport 选择(此时只有 tauriTransport,httpTransport 留 stub)
- **不改任何调用点**(留给 P1.2)

**验证标准**:
```bash
# 1. 类型检查通过
cd app && pnpm vue-tsc --noEmit

# 2. 新增 transport 模块有单元测试
pnpm vitest run src/transport/

# 3. 现有功能完全不受影响(因为还没改调用点)
pnpm tauri dev  # 手动确认聊天、session 切换、permission 弹窗都正常
```

**交付物**:`app/src/transport/{types.ts, tauri.ts, http.ts(stub), index.ts}` + 单元测试

---

### P1.2 TauriTransport 实现 + 21 文件迁移 + 22 测试文件 mock(1-1.5 天)

**工作内容**:
- 实现 `app/src/transport/tauri.ts`:`tauriTransport` 完整实现(转发 `invoke` / `listen`)
- **机械替换 21 个非测试文件**:`import { invoke, listen } from '@tauri-apps/...'` → `import { transport } from '@/transport'`,调用点 `invoke(...)` → `transport.invoke(...)`
- **同步改 22 个 `*.test.ts` 文件**的 mock:`vi.mock('@tauri-apps/api/core')` → `vi.mock('@/transport', () => ({ transport: mockTransport }))`
- `httpTransport` 保持 stub(Phase 2 填充)

**迁移清单**(21 个非测试文件,按类别):
- **stores(13)**:streamController / chat / permissions / projects / subagentRuns / audit / config / memory / models / permissionGrants / providers / subagents / traceStore
- **utils(5)**:toolModeChange / toolQuestion / toolTaskStateTransition / uiDiffApply / useErrorBus
- **components(4)**:ChatInput.vue / ModelSelect.vue / AskUserQuestionCard.vue / ModelsTab.vue
- **layout(1)**:TitleBar.vue(只改 window/os,不改 invoke)
- **entry(1)**:main.ts(仅 import 类型,可不动)

**验证标准**:
```bash
# 1. 全套 vitest 跑通(22 个测试文件 mock 改造后)
cd app && pnpm vitest run

# 2. 类型检查
pnpm vue-tsc --noEmit

# 3. 零行为变化 —— 手动 smoke test
pnpm tauri dev
#    验证清单:
#    □ 发送一条消息,看到流式 delta
#    □ 触发 permission:ask,点允许,agent 继续
#    □ 触发 ask_user_question,回答后 agent 继续
#    □ 切换 session,LRU 缓存正常
#    □ subagent drawer 看到事件流
#    □ 项目列表加载正常

# 4. grep 确认无残留(除 transport 模块本身和测试 mock)
grep -rn "from '@tauri-apps/api/core'" app/src --include="*.ts" --include="*.vue" | grep -v '\.test\.' | grep -v 'transport/'
# 期望:空(所有调用点都改完了)
```

**交付物**:21 文件迁移完成 + 22 测试 mock 改造 + `tauriTransport` 实现

---

### P1.3 后端 emit 散点收敛(0.5-1 天)

**工作内容**:把 §1.2b 盘点的 6 处直调 `app.emit` 收到 sink 抽象,为 Phase 2 换 transport 扫清障碍。

**6 处散点处理方案**:

| 散点位置 | 处理方式 |
|---|---|
| `agent/chat.rs:187` pre-flight error 直调 `chat-event` | 改走 `AppHandleSink`(构造一个临时 sink 或复用传入的) |
| `agent/helpers.rs:160,170` `emit_chat_event` / `emit_tool_result` | **删除这两个 helper 函数**,调用方改用 sink(疑似早期遗留,sink trait 已覆盖) |
| `agent/subagent/sink.rs::record` + `sink/events.rs::emit_permission_ask` subagent 事件直调 | **保留 collector 双通道语义**,但把 `tauri::AppHandle` 抽象成 `dyn SubagentEventSink` trait,新增 `AppHandleSubagentSink` 实现(包装现有逻辑) |
| `agent/subagent/dispatch/finalize.rs::collect_outcome` `subagent:finished` 直调 | 同上,走 `SubagentEventSink` trait |
| `state.rs:317` `projects:refreshed` 直调 | 这个在 `AppState::load` 后台任务里,新增轻量 `SystemEventSink` trait 或直接保留(Phase 2 daemon 化时再处理,因为它不在 agent loop 热路径) |

**关键**:subagent 路径**不能简单合并到 `AppHandleSink`**——subagent 事件注入走 collector 路径,与父 agent loop 的 sink 是两套语义(注释明确 "runs in place of app_handle.emit")。只抽象类型,不改语义。

**验证标准**:
```bash
# 1. cargo 全套测试
cd app/src-tauri && cargo test

# 2. 行为零变化 —— 手动 smoke test(同 P1.2 清单,重点验证 subagent 场景)
pnpm tauri dev
#    验证清单:
#    □ 普通 chat 流式正常(chat-event 经 sink)
#    □ agent error 时前端收到错误事件(chat.rs:187 散点)
#    □ 触发 subagent,drawer 看到完整事件流(subagent:event / subagent:finished)
#    □ subagent 内触发 permission:ask,弹窗正常(subagent/sink/events.rs::emit_permission_ask 散点)
#    □ 首次启动,项目列表 backfill 后刷新(projects:refreshed)

# 3. grep 确认散点收敛(除允许保留的)
grep -rn '\.emit(' app/src-tauri/src/agent/ | grep -v 'test'
# 期望:所有 emit 都经 sink trait 或 SubagentEventSink trait
```

**交付物**:6 处散点收敛 + `SubagentEventSink` trait 抽象 + cargo test 全绿

---

### Phase 1 整体验收

- [ ] P1.1 / P1.2 / P1.3 全部完成
- [ ] `pnpm vitest run` 全绿(22 测试文件 mock 改造)
- [ ] `cargo test` 全绿(后端散点收敛)
- [ ] `pnpm tauri dev` 零行为变化(手动 smoke test 全过)
- [ ] grep 确认:前端无非 transport 模块的 `@tauri-apps/api/core` import;后端 agent 模块无裸 `.emit(` 调用
- [ ] **此时 Tauri 版完全可用,Phase 2 可启动**

---

## Phase 2:daemon 拆分 + 本地 HTTP server(2-3 周 + 0.5 周 E2E)

**目标**:把 agent core 拆到独立 daemon 进程,本机浏览器(含 Windows 宿主访问 WSL daemon)可访问。

> ⚠️ **子阶段顺序严格**:P2.1 → P2.2 → P2.3 → P2.4 → P2.5,每个依赖前一个。但每个完成后 Tauri 版仍可用(走 tauriTransport)。

### P2.1 `AppState::load` 去 AppHandle 依赖(2-3 天)

**目标**:让 `AppState::load` 能在无 Tauri `AppHandle` 的环境下初始化(daemon main 的前置条件)。

**工作内容**:
- 重构 `AppState::load`:接受 `PathBuf`(data_dir)而不是 `AppHandle`,内部用 `dirs` crate 取 `home_dir()` / `data_dir()`
- 保留 `AppHandle` 版本的 wrapper(供 Tauri GUI 用,转发到纯 PathBuf 版本)
- **验证路径一致性**:`AppState::load(AppHandle)` 和 `AppState::load(PathBuf)` 产出**同一个** SQLite 文件路径、同一个 home_dir

**验证标准**:
```bash
# 1. 路径一致性测试(新增)
cd app/src-tauri && cargo test state_load_path_consistency

# 2. cargo test 全绿
cargo test

# 3. Tauri 版零行为变化
pnpm tauri dev
#    验证:DB 还是同一个文件(检查 ~/.local/share/dev.everlasting.app/everlasting.db 或 WSL 对应路径)
#    注:data dir 子目录是 tauri.conf.json 的 identifier(dev.everlasting.app),
#    非 crate 名 everlasting —— daemon bin 经 build.rs 注入 EVERLASTING_APP_IDENTIFIER 对齐。
#    验证:get_home_dir 返回值与重构前一致(用 StatusBar 路径短化验证)
```

**交付物**:重构后的 `AppState::load` + 路径一致性测试

---

### P2.2 axum HTTP server + 79 handler 机械映射(5-7 天)

**目标**:daemon main 能启动 HTTP server,79 个 command 全部映射成 HTTP handler,Tauri 版仍走原 IPC。

**工作内容**:
- 新增 `src-tauri/src/bin/everlasting-daemon.rs`:daemon main 入口(调 `AppState::load(PathBuf)` + 启动 axum)
- 新增 `src-tauri/src/daemon/{mod.rs, server.rs, routes/}`:axum server + 79 个 HTTP handler
- handler 映射规则:每个 `#[tauri::command]` 对应一个 axum handler,`State<'_, Arc<AppState>>` → axum `Extension<Arc<AppState>>`
- 协议:REST 风格,body 字段保持现有 snake_case(与 `AppCommandError` 一致);URL `/api/v1/...`(加版本号)
- **此阶段前端不改**(httpTransport 仍是 stub,前端走 tauriTransport)
- 新增 `src-tauri/src/daemon/auth.rs` 的 stub(Phase 2 本地无认证,Phase 3 填充)

**关键**:`ts-rs` codegen 验证 —— 确认能覆盖 `ChatEvent` 这种 `#[serde(tag = "type")]` 的内部 tagged enum,生成 TS 类型供前端用。

**验证标准**:
```bash
# 1. daemon 能启动
cd app/src-tauri && cargo run --bin everlasting-daemon -- --port 7456
# 期望:日志显示 "listening on 0.0.0.0:7456"

# 2. handler 单元测试(新增,覆盖 79 个 handler 的 happy path + 错误码)
cargo test --package everlasting-daemon --lib routes::

# 3. 冒烟测试:curl 每类 handler
curl http://localhost:7456/api/v1/health                          # 健康检查
curl -X POST http://localhost:7456/api/v1/sessions/list -d '{...}' # session 列表
curl -X POST http://localhost:7456/api/v1/chat -d '{...}'          # 触发 agent(验证 spawn)
# ... 覆盖所有 79 个

# 4. Tauri 版完全不受影响(前端没改)
pnpm tauri dev  # 正常工作
```

**交付物**:`everlasting-daemon` bin + 79 个 HTTP handler + ts-rs 类型生成 + handler 单元测试

---

### P2.3 SSE 事件流 + HttpTransport 实现(4-5 天)

**目标**:daemon 推送流式事件,前端 httpTransport 能订阅,完整 agent loop 走 HTTP/SSE 通道。

**工作内容**:
- 实现 `HttpSseSink`(实现 `ChatEventSink` trait):emit 事件 → 写入 session 对应的 SSE channel
- daemon 新增 `GET /api/v1/stream/{session_id}` SSE endpoint:维护 `session_id → Vec<SseSender>` 路由表
- daemon SSE backpressure 设计:mpsc bounded buffer(64-256,实施时实测),`await send` 反压 agent loop;定期发 `: ping` 心跳
- 填充前端 `app/src/transport/http.ts`:
  - `invoke` → `fetch('/api/v1/...', {...})`
  - `listen` → 内部维护**单个全局 `EventSource('/api/v1/stream/all')`** + 事件名→handler 分发表(方案 B,见 RESEARCH §4.2)
- 前端新增 transport 切换逻辑:`isTauri()` ? tauriTransport : httpTransport

**关键难点**:`projects:refreshed` / `subagent:event` / `subagent:finished` 这 3 个 P1.3 收敛的事件,在 SSE 通道下要确保不漏(它们不走 agent loop 的 sink,走 P1.3 新增的 trait)。

**验证标准**:
```bash
# 1. SSE 单元/集成测试(mock provider 跑 1 轮 agent loop,验证事件序列)
cd app/src-tauri && cargo test --package everlasting-daemon --lib sse::

# 2. 前端切到 httpTransport 跑通(临时改 isTauri() 返回 false,或加 ?transport=http query param)
#    手动验证:启动 daemon,浏览器打开前端,完整跑一轮对话
#    验证清单:
#    □ 发消息,看到流式 delta(SSE chat-event)
#    □ tool:call / tool:result 卡片正常(SSE)
#    □ permission:ask 弹窗 + 点允许,agent 继续(POST /permission/respond + oneshot resolve)
#    □ ask_user_question 卡片 + 回答(SSE tool:question + POST resolve)
#    □ subagent drawer 事件流(SSE subagent:event)

# 3. 事件序列对拍:SSE 端 vs Tauri 端,同一轮对话产生的事件序列一致
#    (用 mock provider 固定 prompt,对比两端 EventLog)

# 4. Tauri 版仍可用(切回 tauriTransport)
pnpm tauri dev  # 正常
```

**交付物**:`HttpSseSink` + SSE endpoint + `httpTransport` 实现 + SSE 集成测试

---

### P2.4 GUI 全走 RPC + daemon 内嵌静态文件 server(2-3 天)

**目标**:Tauri GUI 切到 httpTransport(连本机 daemon),不再开本地 db pool;daemon 内嵌前端静态文件,单二进制部署。

**工作内容**:
- Tauri GUI 启动逻辑改为:① 启动时 spawn daemon 子进程 ② 等 daemon ready(轮询 `/api/v1/health`)③ 切到 httpTransport ④ 关闭 Tauri 时清理 daemon 子进程
- **GUI 进程不再调 `AppState::load`**(或调瘦版,只拿 transport 句柄,不拿 db/catalog/cancellations)—— 消除 dual-pool 写竞争
- daemon 内嵌静态文件 server:`tower-http::services::ServeDir` 指向 `app/dist/`(生产模式);开发模式前端走 Vite dev server
- `pick_project_dir` 浏览器降级:新增"手动输入项目路径"UX(浏览器拿不到绝对路径)

**验证标准**:
```bash
# 1. Tauri GUI 连本机 daemon 跑通
pnpm tauri dev
#    验证:GUI 启动时自动 spawn daemon,关闭时清理
#    验证:GUI 走 httpTransport,功能与 P2.3 浏览器版一致
#    验证:GUI 进程不开 db(用 lsof / strace 确认无 SQLite 文件句柄)

# 2. daemon 单二进制部署
cd app && pnpm build  # 产出 dist/
cd src-tauri && cargo build --release --bin everlasting-daemon
./target/release/everlasting-daemon --port 7456
# 浏览器打开 http://localhost:7456 —— 同时拿到前端 + API(同源)

# 3. 双进程写竞争消除测试
#    同时开 Tauri GUI + 浏览器,两边都往同一 session 发消息
#    验证:无 SQLITE_BUSY 错误(daemon 独占 db,GUI 全走 RPC)

# 4. GUI 关闭后 daemon 进程清理
#    关 Tauri 窗口,ps 确认 daemon 子进程已退出
```

**交付物**:GUI httpTransport 切换 + daemon 进程管理 + 静态文件 server + `pick_project_dir` 降级 UX

---

### P2.5 WSL 端到端验证 + E2E harness(3-4 天,含 0.5 周 E2E)

**目标**:验证 WSL→Windows 宿主浏览器的完整链路;建立端到端测试 harness,作为 Phase 2 的最终验收。

**工作内容**:
- **WSL 部署验证**(本项目的核心场景):
  - daemon 跑 WSL 2,监听 `0.0.0.0:PORT`
  - Windows 宿主浏览器访问 `http://localhost:PORT`(利用 WSL 2 默认 localhost forwarding)
  - 验证不通时的降级:`172.x.x.x:PORT`(WSL 虚拟 IP)或 `netsh portproxy`
- **E2E harness**(Playwright 或自定义 Rust integration test):
  - 启 daemon + 启 mock HTTP client
  - 模拟 10 类 SSE 事件订阅 + 4 类 round-trip(permission / question / mode_change / task_state_transition)
  - mock provider(`llm/provider/mock.rs`,已存在)跑 1 轮 agent loop,验证事件序列与 Tauri 端一致
  - 断网重连测试:EventSource 断开重连后不漏事件
  - 大 message 测试:`tool_result` 5MB shell 输出在 SSE chunked transfer 下的边界
- **回归测试套件**:用同一套 vitest(走 httpTransport)确保 79 个 command 行为与 Tauri 版一致

**验证标准**:
```bash
# 1. WSL 端到端(在 WSL 内启动 daemon,Windows 宿主浏览器访问)
cd app/src-tauri && cargo run --release --bin everlasting-daemon -- --port 7456
# Windows PowerShell:
curl http://localhost:7456/api/v1/health  # 期望 200 OK
# Windows 浏览器打开 http://localhost:7456 —— 完整功能验证

# 2. E2E harness 跑通
cargo test --package everlasting-daemon --test e2e -- --test-threads=1
# 期望:10 类事件 + 4 类 round-trip 全过,事件序列与 Tauri 端对拍一致

# 3. 回归测试(浏览器版 vs Tauri 版行为一致)
cd app && pnpm vitest run --transport=http  # 走 httpTransport 跑全套
pnpm vitest run                              # 走 tauriTransport 跑全套
# 期望:两套结果一致

# 4. 断网重连 + 大 message 边界测试
cargo test --package everlasting-daemon --test e2e sse_resend
cargo test --package everlasting-daemon --test e2e large_payload
```

**交付物**:WSL 部署文档 + E2E harness + 回归测试套件

---

### Phase 2 整体验收

> **状态(2026-07-23,P2.5)**:P2.1–P2.5 的**代码 + 自动化测试**全部就绪并提交。下方 GUI 运行时验证项(dogfooding、WSL→Windows 宿主浏览器实跑)需在 GUI-capable 机器手动验证后才能勾选 —— WSL 无头环境无法跑 Tauri 窗口 / 无真实 LLM 凭据。手动 smoke 清单见 `.trellis/tasks/archive/2026-07/07-20-remote-access-daemon-split/implement.md` P2.5 §E4。

- [x] P2.1 ~ P2.5 **代码 + 自动化测试**全部完成(commit `84d4689` + P2.5)
- [ ] **本机浏览器可访问 daemon**(Windows 宿主访问 WSL daemon 跑通)—— 留手动,见 [HACKING-wsl §远程访问 daemon 部署](./HACKING-wsl.md#远程访问-daemon-部署phase-22026-07-23)
- [ ] **Tauri 版仍可用**(走 httpTransport 连本机 sidecar daemon)—— 留手动
- [x] 79 command 行为在 HTTP transport 下挂载就绪(`tests/e2e.rs` E1e router smoke:全部 route 非 404)+ transport 契约一致(`transport-parity.test.ts` E2)
- [x] SSE 重连 / sentinel / resync / large-payload 协议单测(`tests/e2e.rs` E1b + `daemon/sse.rs` 7 单测)
- [ ] 10 类 SSE 事件 + 4 类 round-trip 端到端验证通过 —— E1a chat happy-path 就绪;完整 10 类事件序列留 GUI 运行时
- [x] 无 dual-pool 写竞争(GUI 瘦客户端不开 db,WAL + busy_timeout=5s 就绪;`lsof` 验证留手动)
- [x] daemon 单二进制部署可用(ServeDir 兜底挂 `/`,前端 + API 同源)—— 代码就绪,实跑留手动
- [ ] **dogfooding ≥ 2 周无 P0/P1** —— Phase 3 启动前置条件,计时未起

**Phase 3 仍定为远期**:前置条件是 Phase 2 实跑稳定(至少 dogfooding 1 个月)。当前仅代码就绪,不启动 Phase 3。

---

## Phase 3:认证 + 跨设备远程(远期,1 周+)

> 📌 **本阶段定为远期规划**,不在近期实施范围。前置条件:Phase 2 本机访问跑通 + HTTP/SSE 协议经实际使用稳定(至少 dogfooding 1 个月)。设计草稿见 [RESEARCH §4.4](./_archive/2026-07-20-remote-access-research.md#44-phase-3远期规划认证--跨设备远程访问)。

**启动时再拆子阶段**(参考要点):
- P3.1 配对码流程 + `devices` 表 + token 校验中间件
- P3.2 HTTPS(自签/Let's Encrypt/Cloudflare Tunnel)
- P3.3 读写不对称(远程 client 默认只读,写操作需 grant;`Transport.isLocal` 属性区分)
- P3.4 token 存储 XSS 防护评估(localStorage vs httpOnly cookie)
- P3.5 Cloudflare Tunnel / Tailscale Funnel 部署文档(两套并存)

---

## Phase 4:Electron thin client(可选,3-5 天)

> 📌 **可选**,Tauri + Web 浏览器访问已覆盖全部场景。除非有强烈的原生通知/托盘/自动更新需求,否则不投入。

复用 Phase 2 的 httpTransport,Electron 主进程 = 浏览器 + 原生能力(托盘、通知、自动启动)。

---

## 附录:子阶段依赖图

```
P1.1 ─┬─> P1.2 ──> P1.3 ──┬─> P2.1 ──> P2.2 ──> P2.3 ──> P2.4 ──> P2.5
      │                    │
      └─(独立,可并行)      └─(Phase 1 验收后才能启动 Phase 2)

Phase 3 ── 依赖 Phase 2 协议稳定(dogfooding 1 个月+) ── 远期
Phase 4 ── 依赖 Phase 2 httpTransport ── 可选
```

**关键路径**:P1.1 → P1.2 → P1.3 → P2.1 → P2.2 → P2.3 → P2.4 → P2.5(串行,无并行空间)

**可并行点**:
- P1.1(前端 interface)和 P1.3(后端散点)可与 P1.2 部分并行(但 P1.2 依赖 P1.1 的 interface 定义)
- P2.5 的 E2E harness 可在 P2.3 完成后提前编写(P2.4 / P2.5 实施时直接用)
