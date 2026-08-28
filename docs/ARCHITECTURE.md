# ARCHITECTURE — 架构设计

> Everlasting 的"整体怎么搭、关键流程怎么走"。包括系统架构图、请求生命周期的 16 道关卡、以及核心架构决策。
> 需求见 [DESIGN.md](./DESIGN.md),技术选型见 [TECH.md](./TECH.md),决策档案见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),技术路线图见 [ROADMAP.md](./ROADMAP.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 系统架构

> ✅ **当前状态(2026-08-20)**:**daemon 化(2026-07-23)+ remote-control epic S1~S6b(2026-08-11~13 收官,merge `94828cb`)+ 6 个跨层特性(2026-08-14~18:C7 tools token / C7D stub 注册 / memory-gov 指令块治理 / B1 image multimodal / D2 跨 session 全文搜索 / C3+ LLM 摘要式压缩)+ 5 个续接特性(2026-08-19~20:unified-context-budget 统一 token 预算 + 关卡⑤硬卡 / MAX_TURNS 软卡 / 手动 /compact / 跨 session 接力 handoff / worker per-turn 度量 + turn_trace 表重建)**。agent core 跑在独立 `everlasting-daemon` 进程(axum HTTP server,见 `app/src-tauri/src/daemon/` + `bin/everlasting-daemon.rs`)。Tauri GUI 进程作为瘦客户端,经 `sidecar.rs::spawn_and_manage` spawn daemon 为子进程,前端默认走 `httpTransport`(同源 HTTP + SSE)与 daemon 通信;daemon 用 `tower-http::ServeDir` 同源服务前端 SPA,故也支持纯浏览器访问(浏览器模式)。`?transport=tauri` + Full 模式(`EVERLASTING_GUI_FULL_STATE=1`)是 daemon 故障时的逃生舱,回退到一体化 Tauri IPC(legacy in-process)。编排放 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md),决策见 [§4](#4-决策agent-daemon-化) + [IMPLEMENTATION/decisions-2026-07.md](./IMPLEMENTATION/decisions-2026-07.md) + [IMPLEMENTATION/decisions-2026-08.md](./IMPLEMENTATION/decisions-2026-08.md)(按月分卷,入口索引在 `decisions.md`)。
>
> 📜 **历史脉络**:2026-06-07 初版本文档时,daemon 化还是"目标态",且当时设想用 `Channel Router` + `TauriGuiChannel`/`FeishuChannel`/`CliChannel` 抽象(见 [§5](#5-决策channel-adapter-抽象早期设想未实施))承载多入口。**实际落地(2026-07)走的是更简单的 axum HTTP 单端点路线**,没有引入 Channel trait —— 该抽象降级为「早期设想,未实施」,保留在 §5 供历史参考。§2 16 关卡中残留的 "Channel Router" 字样是当时叙事载体,实际对应 daemon 的 axum 路由 + `HttpSseSink`。

### 1.1 进程拓扑(daemon 化后,2026-07 落地)

```
三种运行形态,共享同一份 agent core 代码(AppState + agent loop)。

╔══ 形态 A:Tauri GUI + sidecar daemon(默认,Thin 模式)══════════════╗
║                                                                        ║
║  ┌─ Tauri GUI Process(瘦客户端)──────────────┐                       ║
║  │  Vue UI (SPA)   TitleBar (window 控)        │  sidecar.rs::        ║
║  │                                              │  spawn_and_manage    ║
║  │  transport.invoke()  ── httpTransport(默认)  │  (tauri-plugin-shell)║
║  │    fetch POST + SSE                         │                       ║
║  │  (逃生:?transport=tauri → Tauri IPC,Full)   │── spawn args:        ║
║  │                                              │   --port 7456        ║
║  └──────────────────────────────────────────────┘   --data-dir <dir>   ║
║                           │                                            ║
║                           │ 同源 HTTP/SSE(0.0.0.0:7456)              ║
║                           ▼                                            ║
║  ┌─ everlasting-daemon Process (tokio + axum)──────────────┐          ║
║  │  axum router (daemon/server.rs::build_router)            │          ║
║  │   · 118 个 #[tauri::command] 镜像为 REST 路由(2026-08-28 │          ║
║  │     实测)                                              │          ║
║  │   · /api/v1/stream (SSE) — HttpSseSink 广播事件          │          ║
║  │   · /api/v1/attachments/<id> GET 二进制(B1 08-16,首个    │          ║
║  │     非 JSON REST 路由,手机 PWA 看图路径)                │          ║
║  │   · ServeDir fallback(同源服务 dist/ SPA)              │          ║
║  │  ──────────────────────────────────────────────────────  │          ║
║  │  AppState (Arc,axum 每个 handler clone 一份)             │          ║
║  │   · SQLite pool(持有 WAL writer;Thin 模式 GUI 不开)   │          ║
║  │   · agent core(Agent Loop / Tool Registry 25 builtin     │          ║
║  │     + 1 stub 元工具 load_tool_schemas + 1 动态 dispatch  │          ║
║  │     dispatch_subagent = 27 注册名;                         │          ║
║  │     / Workflow Engine / Resource Loaders /              │          ║
║  │     PermissionStore / SessionManager)                   │          ║
║  │   · 自研 LLM Provider trait(Anthropic/OpenAI)          │          ║
║  └──────────────────────────────────────────────────────────┘          ║
║                                                                        ║
╚════════════════════════════════════════════════════════════════════════╝

╔══ 形态 B:纯浏览器模式(同一 daemon,无 Tauri)════════════════════╗
║                                                                        ║
║  ┌─ Browser(任意浏览器)──────────────┐                                ║
║  │  isTauriWebview() = false           │  http://localhost:7456/       ║
║  │  → BrowserHeader(替代 TitleBar)    │ ◄── ServeDir 返回 dist/ SPA   ║
║  │  transport 仍走 httpTransport       │    (transport 载体不变)       ║
║  └─────────────────────────────────────┘                                ║
║                           │                                            ║
║                           │ 同源 HTTP/SSE                              ║
║                           ▼                                            ║
║              (连同一份 everlasting-daemon,见形态 A)                   ║
║                                                                        ║
╚════════════════════════════════════════════════════════════════════════╝

╔══ 形态 C:手机 PWA / 远程浏览器 → 云 everlasting-remote(2026-08 epic)══╗
║                                                                          ║
║  ┌─ 手机 PWA / 远程浏览器 ─────────────────────────┐                    ║
║  │  pwa-remote 模式(httpTransport 第三态):          │  HTTPS + WSS       ║
║  │  device_token → /api/v1/proxy 前缀 +             │  (nginx 反代,      ║
║  │  Authorization: Bearer + SSE ?access_token=      │  HTTPS 用户自理)    ║
║  │  (transport/auth.ts + http.ts)                   │                    ║
║  └───────────────────────────────────────────────────┘                    ║
║                           │                                               ║
║                           ▼                                               ║
║  ┌─ everlasting-remote Process(云上,国内 2C2G 服务器)────────────┐       ║
║  │  axum 云服务端(crates/everlasting-remote/)                     │       ║
║  │   · shared_secret auth(防伪 daemon)+ device_token 认证         │       ║
║  │   · 配对码 60s 一次性 + per-IP 限速(ratelimit.rs 10 次/分)     │       ║
║  │   · WSS 隧道服务端 + 反向代理 + SSE 桥                         │       ║
║  │   · DB:nodes / devices / pairing_codes 三表                    │       ║
║  │   · 只存 token/devices/配对码,不存 agent 数据                   │       ║
║  └─────────────────────────────────────────────────────────────────┘       ║
║                           │  WSS 长连接                                    ║
║                           ▼                                                ║
║  ┌─ PC daemon 的 tunnel client(daemon/tunnel/)───────────────┐           ║
║  │  client / config / dispatcher / manager / node_id /        │           ║
║  │  sse_bridge;WSS 长连接 + loopback 转发;                    │           ║
║  │  取消只停转发(sse_bridge select!),不终止本地会话           │           ║
║  └────────────────────────────────────────────────────────────┘           ║
║                           │  loopback                                      ║
║                           ▼                                                ║
║              (连同一份 everlasting-daemon,见形态 A)                        ║
║                                                                          ║
╚══════════════════════════════════════════════════════════════════════════╝

   daemon 进程外部依赖(三种形态共用):
         ↓ LLM API                  ↓ Local FS / Git
    (Anthropic / OpenAI)         (WSL 内 $HOME/projects)
```
**进程边界说明**:
- **Tauri GUI Process(Thin 模式)**:只渲染 SPA + 经 `httpTransport` 转发请求,**不**加载 `AppState`、**不**开 DB pool、**不**跑 sweep/hygiene 后台任务。spawn daemon 子进程,`RunEvent::Exit` 钩子回收 sidecar(无孤儿进程)。
- **everlasting-daemon Process**:跑所有 agent 逻辑 + 持有 SQLite pool(WAL writer)。axum router 把 118 个原 `#[tauri::command]` handler 镜像为 REST 路由(2026-08-28 实测),前端同一份 handler 代码服务 IPC 与 HTTP。
- **通信**:同源 HTTP(POST `/api/v1/...`)+ SSE(`/api/v1/stream`)。sidecar 模式下 daemon 监听 `0.0.0.0:7456`(WSL-first:Windows 宿主浏览器经 WSL2 localhost 转发可达),GUI 同源访问无 CORS。**不是** Unix socket / WebSocket —— 早期设想的本地 IPC 已被同源 HTTP 取代(见 [§5](#5-决策channel-adapter-抽象早期设想未实施))。
- **逃生舱**:`?transport=tauri` + Full 模式(`EVERLASTING_GUI_FULL_STATE=1`)回退到 legacy in-process —— GUI 加载 `AppState` + 走 Tauri IPC,不 spawn sidecar。daemon 故障时用。
- **daemon 化动机**:远程/浏览器访问;agent core 与 GUI 解耦;多 client(GUI + 浏览器 + 经 remote daemon 的远程 PWA client)共用同一 agent core。详见 [§4 决策:Agent Daemon 化](#4-决策agent-daemon-化)。
- **everlasting-remote Process(云上,2026-08 remote epic)**:axum 云服务端(`crates/everlasting-remote/`,国内 2C2G 服务器):shared_secret auth(防伪 daemon)+ device_token 认证、配对码 60s 一次性 + per-IP 限速(`ratelimit.rs` 10 次/分)、WSS 隧道服务端 + 反向代理 + SSE 桥。DB 只存 `nodes` / `devices` / `pairing_codes` 三表(节点身份 / device_token / 配对码),**不存 agent 数据**。
- **PC daemon 的 tunnel client(`daemon/tunnel/`)**:出站 WSS 长连接连云上 remote,把远程请求 loopback 转发到本地 agent core(子模块 client / config / dispatcher / manager / node_id / sse_bridge)。取消只停转发(`sse_bridge` `select!`),不终止本地会话。**PC daemon 本地功能零依赖 remote** —— 云上 remote 或隧道断线不影响本地 GUI / 浏览器使用。

### 1.2 关键数据流:用户发一条消息(daemon 化后,默认 httpTransport)

> 📌 **当前默认路径(Thin 模式)**:Frontend → `transport.invoke('chat', ...)`(`httpTransport`:fetch POST 到 daemon `/api/v1/chat`)→ daemon 进程的 axum 路由调同一份 `chat` handler → `chat_stream_with_tools()`(reqwest + 手写 SSE)→ `HttpSseSink`(`daemon/sse.rs`)经 `/api/v1/stream` 同源 SSE 广播 `chat-event` / `tool:call` / `tool:result` → Frontend 单 SSE listener(`streamController.ts`,按 `request_id` 路由到对应 session 的 streamController,`chat-event` payload 自 2026-08-27 起回填 **`session_id`**,支持跨客户端(remote PWA)按 session 认领)→ Pinia store 增量更新。
> 逃生路径(Full 模式 `?transport=tauri`):`tauriTransport` 走 Tauri IPC,handler 在 GUI 进程内,事件经 Tauri event emit。两条路径共享同一 `#[tauri::command]`/REST 双暴露 handler。
>
> 📌 **远程 PWA 语境(2026-08 remote epic)**:`httpTransport` 内部有第三态 **pwa-remote** —— 前端持有 `device_token` 时(`transport/auth.ts` 的 `isRemoteContext()`),请求自动加 `/api/v1/proxy` 前缀 + `Authorization: Bearer <device_token>`(`http.ts`),SSE 经 `/api/v1/stream?access_token=...`;请求先到云上 `everlasting-remote`,由它经 WSS 隧道反代到 PC daemon,远程 PWA 与本地 GUI / 浏览器共用同一 agent core(拓扑见 §1.1 形态 C)。

```
[1] Frontend (Vue 3)
    用户输入消息 → transport.invoke('chat', { requestId, messages })
      └ 默认 httpTransport:fetch POST /api/v1/chat(同源 daemon)
      └ 逃生 tauriTransport:tauri.invoke('chat', ...)(Full 模式,GUI 进程内)

[2] everlasting-daemon Process(axum)  /  Full 模式下的 Tauri GUI Process
    axum 路由 / Tauri command 收到请求 → spawn 异步任务处理
    invoke/fetch resolve 立即返回("已受理",非"已完成")

[3] agent core(同一份 handler 代码,两种入口)
    chat_inner 路由临界区(F1 输入侧 gate,2026-08-25 落地;单一 Mutex,锁序 queues → active,区内零 await):
      所有发送一律入队( AppState.session_message_queues,per-session FIFO,上限 20,uuid 寻址)
      ├─ 忙(session_active_request 命中)→ 返回 {queued:true, id, position}(本次 RPC 无流,事件仍按 session 广播)
      └─ 闲 → 同临界区内注册 rid + spawn 队列驱动器,响应形状与现状一致(unit)
    队列驱动器 loop(原 tokio::spawn 体):
      run_chat_loop → cancelled 清队 break / 错误·续轮触顶(50) 保留队列 break
      → drain 非空 → emit ChatEvent::TurnContinuation(前端续轮渲染边界,先于新 run 任何 delta)
      → persist(drained) 为下一轮初始 user 输入(每条独立 APPEND,cache 断点不变量保持)→ 再进 run
      退出协议:拿路由锁,队列空才注销 slot(反搁浅);DriverSink 单 rid 跨内层轮保活,只在真结束 emit Done
    SessionManager::handle_message(session_id, content)
      → 写入 SQLite (user message)
      → 触发 agent core
    agent core:
      构造 messages: [system_prompt + role + memory, ...history, new_user_msg]
      // Skill 按 use_skill 触发时按需加载(详见 [ARCHITECTURE §2.5.12](#2512-⑤-memory-gov-指令块窗口治理2026-08-15-落地))
      while !done {
        stream = llm.stream(messages, tools)
        for chunk in stream {
          match chunk {
            TextDelta(t)  => sink.send(ChatToken(t)),       // HttpSseSink(daemon)/Tauri emit(Full)
            ToolUse(...)  => 权限检查(per-mode) → 执行 → 构造 tool_result 回填,
            UiRender(...) => sink.send(UiCard(...)),
          }
        }
      }
      sink.send(ChatDone)

[4] Frontend
    transport.listen("chat-event") → payload.type 分发:
      "delta"  → 追加 token 到 UI
      "done"   → 解禁输入框
      "error"  → 显示错误提示
    (另有 "tool:call" / "tool:result" / "permission:ask" 独立事件)
```

### 1.3 关键数据流:session 切换(daemon 化后)

> 📌 **当前默认路径(daemon 化后)**:`switchSession(id)` → `chatStore` 委托 `streamController.ensureLoaded(id)` → LRU 命中则从 `messagesBySession` Map 拿;未命中则 `transport.invoke('load_session', { sessionId })`(默认 httpTransport → daemon,Full 模式 → Tauri IPC)从 SQLite 读 → 写入 Map → `currentSessionId.value = id` → `currentCwd` 更新 → UI 重新渲染。**前 session 的 in-flight SSE 流不受影响**(流指示器在 SessionList 蓝点继续 pulse 直到 `done` 到达)。详细架构见 `.trellis/spec/frontend/state-management.md` §"Stream Controller Pattern"。
>
> 📌 **远程 PWA 语境**:session 加载走同一 `load_session` 路径 —— pwa-remote 态下 transport 请求经 remote daemon 反代到 PC daemon(pwa-remote 三态见 §1.2),对 agent core 语义与本地一致。

```
[1] User clicks project A → session B
[2] Frontend: transport.invoke('load_session', { sessionId: B })
[3] daemon / Tauri backend: 从 SQLite 读 messages → 返回 SessionSnapshot
```

### 1.4 群聊模式(group chat,2026-07-29 落地,08-04~08-07 迭代加固)

> 📌 **session_type 区分两种循环**:`sessions.session_type = 'chat'`(默认)走 `agent/chat_loop.rs`(经典单 agent);`'group_chat'` 走 `agent/group_chat_loop.rs` 编排 + `agent/group_chat_prompts.rs`(prompt/history 纯函数)(多参与者 turn-taking)。
>
> **群聊循环**(`group_chat_loop.rs`,prompt/历史纯函数在 `group_chat_prompts.rs`)由一个 **moderator**(主持人)agent 协调多个 **参与者** agent 轮流发言:moderator 用 `nominate_speaker` 点名下一发言者(**唯一调度机制**,`nominate_speaker` / `end_discussion` 均 moderator-only,参与者不得调用),参与者发言后回到 moderator,moderator 调 `end_discussion(summary)` 终止并给出全场总结。每条 message 落库带 `speaker` 列(参与者标识),前端按 speaker 渲染独立气泡 + 实时发言人 chip,`end_discussion` 的 summary 由 `DiscussionSummaryCard.vue` 渲染为"讨论总结"卡片。
>
> **上下文构建 — per-role history 隔离(08-07)**:每个角色从共享 DB transcript 经 `role_history(full, current_role)` 组装**独立** LLM 上下文:只保留自己的 assistant 行(verbatim,含 thinking + signature),他人发言改写为 `role:user`(归属由 wire 层插 `@name:` 前缀),他人 thinking / 工具对(工具结果不共享)与 moderator 仲裁对被剥离。取代早期 `participant_view`(多身份 assistant 共存 = 同模型串台根因)。
>
> **工具白名单(08-07;08-25 F4 起 + `web_search`)**:moderator 与参与者只拿调研类工具 `read_file`/`grep`/`glob`/`list_dir`/`web_fetch`/`web_search`,moderator 额外持有仲裁工具(白名单取代黑名单,新增 builtin 工具默认不进群聊);参与者 `max_turns=20`(可取材实证)、moderator `max_turns=1`。
>
> **入口与事件(08-04~08-07)**:入口持久化去重 + 参与者身份护栏(防 LLM 自名开头)+ 终止/发言人事件 + 逐轮流式 + 人类抢占插话;moderator 未调 `nominate_speaker` 时**重试 moderator turn**(08-06 废弃 round-robin 机械派人)+ wire 层孤儿 tool_use 自愈;编排器静默路径变可见 `Done{stop_reason}` 事件,非终态挂 notice 不 finalize;identity_contract 契约测试守身份不变量。Phase 1-4 见 `.trellis/tasks/archive/2026-07/07-29-group-chat/`,08-04~08-07 迭代见 `.trellis/tasks/archive/2026-08/08-0{4,6,7}-group-chat-*`。

### 1.5 远程访问形态(2026-08 remote-control epic S1~S6b 落地,merge 94828cb)

> 完整 E2E 部署 / 验收手册见 [REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md);运维 / systemd / nginx 见 [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md)。本节只讲 §1 拓扑层的角色边界与"本地零依赖 remote"不变量。

**三个角色,三台机器**:

| 角色 | 跑什么 | 端口 | 谁访问 |
|---|---|---|---|
| **云服务器** | `everlasting-remote`(独立二进制,`crates/everlasting-remote`)+ nginx + 前端 dist | 443(nginx)→ 7457(remote) | 手机 + PC 都连它 |
| **PC** | `everlasting-daemon`(agent core + tunnel client + 前端) | 7456 | PC 自己 + 经 remote 被手机访问 |
| **手机** | 浏览器(Safari/Chrome PWA) | — | 访问 remote 域名 |

**关键不变量**:PC daemon 本地功能**完全不依赖 remote**。remote 挂了/隧道断了/没配,PC 本地照常工作,只是手机暂时连不上。agent 进程 100% 在 PC,数据不出本机;云上 remote **不持文件 / 不存 agent 数据**,只存 `nodes` / `devices` / `pairing_codes` 三表。

**`/health` 端点**:`GET /health` 返回 `{remoteId, ...}`(`remoteId` 字段是手机 PWA `isRemoteContext()` 判定的信号,无此字段手机不跳 `/pairing`)。

**首个非 JSON REST 路由(B1 08-16)**:`GET /api/v1/attachments/<id>` 返回二进制(图片 / 附件),同源 daemon / 手机 PWA 都可达,用于 inline 预览 `messages.metadata.attachments`(B1 image multimodal 详见 §1.6)。

> 拓扑 ASCII + 三形态(形态 A Tauri GUI + sidecar / 形态 B 纯浏览器 / 形态 C 手机 PWA + 云 remote)见 §1.1 图示;**形态 C 即 §1.1 形态 C**(2026-08 epic 引入,详见 §1.1)。

> 📌 手机 PWA / 远程浏览器经 HTTPS 访问云上 `everlasting-remote`,由它经 WSS 长连接接到 PC daemon 的 tunnel client,loopback 打到本地 agent core(拓扑见 §1.1 形态 C)。**remote 只存 token/devices/配对码,不存 agent 数据;PC daemon 本地功能零依赖 remote。** 中继方案变更:Cloudflare Workers + D1 → 国内 2C2G 服务器 + 自研 Rust remote daemon(HTTPS 用户自理,nginx 反代,非 Cloudflare Tunnel)。部署见 [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md),端到端验证见 [REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md)。

**配对码 bootstrap 流程**(`crates/everlasting-remote/` + `app/src-tauri/src/daemon/tunnel/`):
1. PC 端 Remote tab(`app/src/components/settings/RemoteTab.vue`)生成 6 位配对码(60s 一次性)
2. 手机 PWA `redeem` 配对码 → 换取 64-hex `device_token`(per-IP 限速,`ratelimit.rs` 10 次/分)
3. 绑定后的 PC 出现在 nodes 列表(`app/src/views/NodeListView.vue`),此后 PWA 经 `/api/v1/proxy` + `Authorization: Bearer <device_token>` 访问

**vue-router 守卫**:`app/src/router/index.ts` 带 `isRemoteContext()` 守卫 —— 仅 remote-served 语境 gate 配对页(先配对再进 `/chat`);daemon / Tauri 语境直进 `/chat`(现状不变)。前端页面:`PairingView`(配对码兑换)/ `NodeListView`(节点列表)/ `ChatView`(聊天)。

**PWA 壳**:vite-plugin-pwa + `public/icons/`,手机浏览器可安装为 PWA。脚本:`scripts/remote.sh`(本地隧道)/ `deploy-remote.sh`(云端部署)/ `remote-e2e-smoke.mjs`(端到端冒烟)。

**决策偏差记录**:Phase 3(dogfooding)在 dogfooding 前置条件未满足时由 epic 直接启动完成 —— 见决策日志对应条目。

### 1.6 近期落地特性(2026-08-14~28)

> 2026-08-14~28 批量落地:C7 tools token 治理 / C7D stub 注册 / memory-gov 指令块治理 / B1 image multimodal / D2 跨 session 搜索 / C3+ 摘要式压缩 / unified-context-budget 统一预算 / MAX_TURNS 软卡 / 手动 /compact / handoff 接力 / worker per-turn 度量 / F1 消息队列 / F4 web_search / F5 文档提取 / F6 异步可观测性 + F3 并发闸 / F2·F2b 定时任务 / stream session_id。
>
> **完整逐项记录(一句 + 链接)见 [ROADMAP.md §1.2](./ROADMAP.md#12-路线图外完成)**——本节不再重复;横切关注点(C7 / C7D / memory-gov / C3+ / budget 硬卡 / softcap)的关卡级设计见 §2.5.10~15。F2 定时任务调度内核与 F6/F3 异步编排的关键架构点见下两小节(其余同见 ROADMAP)。

**F2·F2b 定时任务调度内核(2026-08-28,架构级关键设计)**

- `scheduler/` 模块,daemon 常驻 30s tick + CancellationToken 停机(GUI Full 零 timer 硬约束,调度仅 daemon 进程);单一扫描算法——每 tick 重算「自 `max(created_at, last_fired_at)` 以来最近到期点」,catch-up 与常规触发同一判定,落账记理论到期点 `due` 防相位漂移;同 session 每 tick 至多一 fire
- origin 载体链:fire = 构造带 origin 的 user message 走 `chat_inner` 同源路径,`ChatEntry → QueuedMessage.origin → ChatLoopRequest → persist 门控` 落 `messages.metadata.scheduled`(additive)——F1 队列「闲也入队」路由统一
- F2b:6 档 preset + `max_runs`/`ends_at` 结束条件 + `completed` 审计;`ScheduledTaskFired` 六动作;`scheduled_tasks_enabled` kill switch fail-open
- 完整设计:见 [ROADMAP.md §1.2 F2/F2b](./ROADMAP.md) 行 + spec [backend/scheduled-tasks.md](../.trellis/spec/backend/scheduled-tasks.md)

**F6 异步任务可观测性 + F3 全局并发闸(2026-08-27)**

- `SessionSummary.busy` 运行时 enrich(daemon 层单点 `list_sessions_inner`,双 transport 一致)+ 轮次终结跨 session toast + Tauri 壳关闭确认(仅 `isTauriWebview`)
- F3 最小档全局信号量 `max_concurrent_loops`(缺省 4):spawn 闭包头 acquire 排队不拒绝,等闸取消完整回滚 claim 注册
- 零新表零 migration(跨重启终态复用 messages.status 恢复链);F1-C 移出归 F2
- 完整设计:见 [ROADMAP.md §1.2 F6](./ROADMAP.md) 行 + spec [pattern-global-loop-semaphore](../.trellis/spec/backend/agent-loop-architecture/pattern-global-loop-semaphore.md)

---

## 2. Harness 设计:从用户输入到文件变更的 16 道关卡

这一节把架构图展开成**具体的请求生命周期**。理解了这 16 关,就理解了 harness engineering 在做什么。

> **演进说明**:早期版本是 14 道关卡,daemon 化(见 [§4](#4-决策agent-daemon-化))和资源加载系统(见 [TECH.md §5](./TECH.md#5-决策skill--memory--role-共用-frontmatter-loader))扩展后变成 16 关。

### 2.1 全景图

```
        你按回车
           ↓
   ① 前端校验 ──────── 拒
           ↓
   ② transport 边界(httpTransport/tauriTransport) ──── 拒
           ↓
   ③ daemon 路由入口(axum / Tauri command)
       │  ├ 请求去重(request_id)
       │  └ session 路由
           ↓
   ④ Session Manager
       │  ├ session 状态检查
       │  ├ 持久化 user msg
       │  └ 构造 AgentContext
           ↓
   ⑤ Context 构造
       │  ├ 5a 加载 4 层 Memory
       │  ├ 5b 注入 Role prompt
       │  ├ 5c 列出可用 Skill 描述
       │  ├ token 预算检查
       │  └ tool 白名单过滤
           ↓
   ⑥ LLM 请求
       │  ├ 超时 / 错误? 重试
       │  └ 鉴权失败? 终止
           ↓
   ⑦ SSE 解析
       │  └ token 边收边 emit
           ↓
   ⑧ 决策分叉
       │  ├ 8a Mode 检查(plan 模式拒绝 tool)
       │  └ 8b 内容类型(text / tool / ui_render)
       │
       ├─ text ───────────────────────┐
       │                              ↓
       └─ tool_use →  ⑨ 权限检查  ←──┐
                       │              │
                  ┌────┴────┐         │
                允许    拒绝(回 LLM)   │
                  ↓                   │
              ⑩ Tool 执行             │
                  │                   │
              ⑪ Git 联动               │
                  ↓                   │
              ⑫ 结果回填 ─────────────┘
                  │
              ⑬ 循环检测
                  │
                  ↓
              ⑥ ⑥ ⑥ (回到 LLM)
                  │
              (LLM 决定结束)
                  ↓
              ⑭ 流式 token 输出(text / ui 走不同 channel)
                  ↓
              ⑮ Channel 输出(daemon → 对应 client)
                  ↓
              ⑯ 结束 / 解禁 / 统计
```

### 2.2 16 关详解

> 📜 **叙事载体说明(daemon 化后)**:以下 16 关最初用"目标态 + Channel Router"语言写就。2026-07 daemon 化落地后,实际没有 `Channel` trait / `Channel Router` —— 关卡③的"Channel 入口"实际是 daemon 的 axum HTTP 路由(`daemon/routes/`),关卡⑮的"Channel 输出"实际是 `HttpSseSink`(`daemon/sse.rs`)经同源 SSE 广播。Full 模式逃生时则对应 Tauri command / Tauri event emit。关卡本身的**逻辑顺序与职责划分不变**,只是载体从"多 channel 抽象"收敛为"HTTP/SSE 单端点(+ Tauri IPC 逃生)"。

#### ① 前端校验(Vue 3)

```
输入框 → onSend(prompt)
  ├─ 非空?截断超长文本?
  ├─ 是否有未完成的 tool call?(防双发)
  └─ 当前 session 状态是否 idle?
```

- **关卡点**:空消息、过长输入、并发请求、session 锁定
- **失败后果**:UI 拦截,不发请求

#### ② transport 边界(httpTransport 默认 / tauriTransport 逃生)

```ts
await transport.invoke("chat", { requestId, messages })
// 默认 httpTransport:fetch POST /api/v1/chat(同源 → daemon axum 路由)
// 逃生 tauriTransport:tauri.invoke('chat', ...)(Full 模式,GUI 进程内)
```

```
  ├─ 参数反序列化(JSON → Rust struct;axum extractor / Tauri command 两路共享同一 handler)
  ├─ 命令是否在白名单?(Tauri capability 限制 — Full 模式;daemon 模式无 capability 层)
  ├─ rate limit?(每 session 每分钟 N 条)
  └─ spawn 异步任务处理 LLM stream
       └─ invoke/fetch resolve 立即返回("已受理")
```

- **关卡点**:参数类型校验、Tauri 2 capability 权限(默认拒绝,仅 Full 模式)、简单限流、transport 转发
- **失败后果**:返回错误,前端 toast 提示
- **重要**:invoke resolve **不代表** "已处理",只代表"已转发到 agent core"。结果走 ⑮ 通道(SSE / Tauri event)回来

#### ③ daemon 路由入口(axum / Tauri command 接收)

```
daemon axum 路由 / Tauri command handler(同一份代码):
  ├─ 收到请求 { session_id, request_id, messages, mode, ... }
  ├─ 去重:同一个 request_id 短时间内重复 → 丢弃(防网络重发)
  ├─ 权限/鉴权:两层(2026-08 起)
  │    ├─ 云端 remote daemon:shared_secret(防伪 daemon)+ device_token 认证(已落地)
  │    └─ 本地 daemon:单用户场景,仍无多用户鉴权
  └─ 路由:按 session_id 选对应的 Session
       └─ 多 client 连同一 daemon 时共享同一 session 池(从 SQLite 读)
```

- **关卡点**:请求去重、session 路由
- **失败后果**:静默丢弃重复请求
- **设计动机**:见 [§4 决策:Agent Daemon 化](#4-决策agent-daemon-化)。早期设想的"多 channel(飞书/CLI)路由"未实施,实际只跑 HTTP(+ Tauri IPC 逃生);多入口抽象降级为 [§5](#5-决策channel-adapter-抽象早期设想未实施) 的历史设想。

#### ④ Session Manager

```
  ├─ session 存在?状态正常?(active / paused / archived)
  ├─ 工作目录存在?git worktree 还活着?
  ├─ 写入 user message 到 SQLite
  └─ 构造 AgentContext { session, history, tools, system_prompt, role, mode }
```

- **关卡点**:session 状态机校验、磁盘健康检查、消息持久化、context 骨架
- **失败后果**:session 损坏 → 提示用户修复或归档

#### ⑤ Context 构造

```
构造骨架:
  messages = []
  tools    = filter(registry, session.allowed_tools)  // 包含 use_skill / use_memory / use_ui

子步骤:
  5a 加载 4 层 Memory(从 user / project / session / runtime,按 token 预算)
  5b 注入 Role prompt(role.system_prompt.base + suffix)
  5c 列出可用 Skill 描述(给 LLM 看的 use_skill tool schema;Skill 内容不预加载)

最终:
  messages = [system_prompt(5b) + memory(5a 摘要), ...msgs_from_db, new_user_msg]
  tools    = 基础 tools + use_skill(5c) + use_memory + use_ui + role.tools

检查:
  ├─ token 计数(超限?)
  │    └─ 是 → 触发压缩(早期裁剪老消息,后期 LLM 摘要)
  └─ tool 白名单 / 黑名单(role 黑名单 > 白名单)
```

- **关卡点**:context window 限制、token 预算、tool 白名单、prompt 注入、5a/5b/5c 加载顺序
- **这是 harness 设计的最核心战场** —— 怎么在有限的 context window 里塞下有效信息
- **5a-5c 详解见 [memory spec](../.trellis/spec/backend/memory.md) 和 [BACKLOG.md §2 Skill](./BACKLOG.md#2-agent-skill-系统) 和 [ROADMAP §1.2 L3d](./ROADMAP.md#12-路线图外完成)**

#### ⑥ LLM API 请求

```
POST https://api.anthropic.com/v1/messages
Headers: x-api-key, anthropic-version, content-type
Body: { model, messages, tools, stream: true }
  ├─ 超时?(默认 60s,长任务 10min)
  ├─ 429 / 5xx → 重试(指数退避,最多 3 次)
  ├─ 网络断开 → 重连(resume from last event id)
  └─ 鉴权失败 → 立即终止,提示用户
```

- **关卡点**:超时、重试、重连、错误分类
- **失败后果**:可重试错误静默重试,不可重试错误终止 session

#### ⑦ SSE 流式解析(边收边处理)

```
for event in stream {
  match event.type {
    message_start       => 记下 message_id, model, usage.input_tokens
    content_block_start => 准备接收 text / tool_use
    content_block_delta => emit("chat:token", delta.text)  // ← 实时显示
    content_block_stop  => 完成一个 block
    message_delta       => 更新 stop_reason, output_tokens
    message_stop        => 本轮 LLM 结束
  }
}
```

- **关卡点**:event 顺序保证、断点续传、token 累计
- 没有真正的"决策关卡",但事件流可靠解析是地基
- **交错思考(2026-07-23/24 落地)**:contentBlocks 按**真实流序**交错落库与渲染(thinking / text / tool_use 时间轴,run 分组),而非 Anthropic 的"text 全先于 tool_use"分组顺序 —— 后端保留 BlockState 时间戳序,前端 run 分组 + contentBlocks 时间轴渲染,修复 Anthropic thinking 块在中途消失 + 真工具穿插。设计见 [docs/INTERLEAVED-THINKING-DESIGN.md](./_history/2026-08-28-interleaved-thinking-design.md)。

#### ⑧ 决策分叉(LLM 给的指令 + Mode 维度)

**子步骤 8a — Mode 检查**(A2 + B7 PR1 落地,2026-06-13,**已实施**):

```
对当前 session.mode:
  ├─ Edit       → 正常 (full tool list + ⑨ 5-tier 检查; 3 档化 2026-06-13 原 Chat 改名)
  ├─ Plan       → ⑧a 三重防御:① system prompt 前缀禁止 write,
  │               ② tool list 过滤掉 write_file/edit_file/shell,
  │               ③ Tier 4 runtime intercept 兜底(LLM 漏发 tool_use)
  ├─ Background → 同 Edit,但 emit 走 "background:" 前缀(MVP 移除 UI)
  └─ Yolo       → full tool list + 跳过 Tier 4 user-ask (整段 bypass),Tier 2 hard kill list 仍生效
```

**实现位置**:`app/src-tauri/src/agent/permissions.rs`:
- `mode_system_prefix(mode)` → ① per-turn system prompt 前缀
- `filter_tools_for_mode(tools, mode)` → ② per-turn tool list 过滤
- `check()` Tier 4 → ③ runtime intercept 兜底

**详见** [permission-layer.md §"Scenario: Per-Session Mode + ⑨ 关 Permission Layer"](./../.trellis/spec/backend/permission-layer.md)。

**子步骤 8b — 内容类型分发**:
| LLM 返回          | 走向                                  |
|-------------------|---------------------------------------|
| 纯 text           | 直接到 ⑭ 走 ChatToken                |
| tool_use          | 进入 ⑨ 权限检查(5-tier) → ⑩ 执行             |
| 混合(text + tool) | text 到 ⑭,tool 进 ⑨                  |
| **ui_render**(新) | 到 ⑭ 走 UiCard(详见 [ROADMAP §1.2 B9](./ROADMAP.md#12-路线图外完成)) |

- **关卡点**:Mode 提前拦截(Plan 模式不能进 ⑨)、ui_render 跟 tool_use 区分开
- **风险**:Mode 误判 → LLM 收到 "Plan 模式下不能执行",但它应该用 Plan 模式思考再用 Chat 模式执行
- **详见 [permission-layer spec](../.trellis/spec/backend/permission-layer.md)**

#### ⑨ Tool 权限检查

> 关键关卡:A2 + B7 落地,re-grill 2026-06-13,**已实施**。

**5-tier 决策顺序**(re-grill SOT,path-based 决策层):

```
对每个 tool_use(name, input):
  │
  ├─ Tier 0. Boundary (assert_within_root) — 项目根目录硬墙,前置于 ⑨
  │   └─ 失败 → bail out,不调 execute_tool
  │
  ├─ Tier 1. Hooks           (pre-call 接口, MVP no-op)
  │   └─ 命中 hook override? → 用 hook 决定(本期不实现)
  │
  ├─ Tier 2. Deny rules      (硬 kill list, 9 个 shell regex)
  │   ├─ 命中 → Decision::Deny { critical: true, reason: ... }
  │   ├─ Yolo 也走 — 静默拒绝, audit 记 tool_denied_yolo
  │   └─ → Tier 6 写 audit event
  │
  ├─ Tier 3. Mode check      (Plan 拦截, ⑧a 第三层兜底; 3 档化 2026-06-13 Review 移除)
  │   ├─ Plan + tool ∈ {write_file, edit_file, shell}
  │   │   → Deny { reason: "I cannot execute X in Plan mode (read-only session)" }
  │   │   **不**emit permission:ask — Mode 提前到 Tier 3 消除
  │   │   旧设计的 "Plan + 始终允许" 坏交互
  │   └─ read 类工具不受影响
  │
  ├─ Tier 4. Path / Prefix / External policy
  │   │
  │   ├─ Path 工具(read_file / write_file / edit_file /
  │   │   list_dir / grep / glob):
  │   │   - 解析 `path` arg → is_within_root(session.cwd, path)?
  │   │     - YES → 查 session_tool_permissions(match_kind='path')
  │   │             → hit → Allow
  │   │                       miss → Allow (silent, 仓库内 default)
  │   │     - NO  → 查 session_tool_permissions(match_kind='path')
  │   │             → hit → Allow
  │   │                       miss → emit("permission:ask", { ..., path })
  │   │
  │   ├─ Shell:
  │   │   - first whitespace token → classify_prefix(token)
  │   │     - Allow (whitelist)  → Allow (silent)
  │   │     - Ask   (asklist/未知) → emit("permission:ask", { ..., path=cmd })
  │   │
  │   └─ Web Fetch:
  │       - 总是外部 → 查 session_tool_permissions(match_kind='tool',
  │         tool_name='web_fetch')
  │         → hit → Allow
  │                   miss → emit("permission:ask", { ..., path=url })
  │
  │   Yolo 模式:整段 Tier 4 silent,直接 Allow(不查
  │   session_tool_permissions,不发 modal)。仍受 Tier 2 拦截
  │
  ├─ Tier 5. Allow rules     (默认 allow-all, MVP 阶段)
  │   └─ 未来可在此处加全局 allow/deny 规则
  │
  └─ Tier 6. Audit hook      (每个决策路径写 session_audit_events)
      └─ kind: tool_allowed / tool_denied / tool_permission_ask /
               permission_granted / permission_timeout / tool_denied_yolo /
               mode_changed / yolo_entered / yolo_exited / request_cancelled
      ↓
  → 放行 execute_tool(若 Allow) / 构造 is_error tool_result(若 Deny)
```

**"始终允许" 持久化**(re-grill Q6:wire 3 种 match_kind):

| match_kind | match_value | 触发 |
|---|---|---|
| `tool` | NULL | web_fetch "始终允许" |
| `prefix` | 第一个 token | shell "始终允许" (`cargo`, `git`, ...) |
| `path` | parent + `/*` glob | path 工具 "始终允许" (`/Users/me/Documents/*`) |

DB schema 已在 06-12 落地(CHECK 约束支持 3 种),re-grill
只 wire 实现。`sqlite GLOB *` 不跨 `/` 是已知限制(PR3+ 考虑
自写 matcher 支持 `**`)。

**关键行为**:
- **Deny 优先于一切**:`rm -rf /` 在 Yolo 下也是静默拒绝
  (Tier 2 硬墙, 不弹窗, audit 区分 `tool_denied_yolo`)
- **Mode 提前到 Tier 3**:消除旧 "Plan + 始终允许" 坏交互
- **Yolo 整段 bypass Tier 4**:Yolo = "no questions asked"
  (Tier 2 仍 hard wall)
- **拒绝 ≠ Cancel 整轮**:拒绝只跳该 tool_use,LLM 收到
  `is_error: true` 可自决;CancellationToken(C1)才是整轮终止
- **超时 vs 主动 deny** 在 audit log 区分:`reason` 字段不同
  ("user denied" vs "permission timed out after 120s, treat as denied")

**实现位置**:
- ⑨ 关 dispatch: `app/src-tauri/src/agent/permissions/mod.rs(拆分自 mod.rs,2026-06-23 拆为 8 模块)::check()`
- Tier 2 硬 kill list: `app/src-tauri/src/agent/permissions/dangerous.rs::is_kill_listed()`
- Tier 4 shell 分类: `app/src-tauri/src/agent/permissions/shell_trust.rs::classify_prefix()`
- Tier 4 path boundary: `app/src-tauri/src/projects/boundary.rs::is_within_root()`
- IPC bridge: `app/src-tauri/src/commands/permissions.rs::{set_session_mode, permission_response, grant_tool_permission}`
- 前端消费: `app/src/stores/permissions.ts` + `app/src/components/chat/PermissionModal.vue`

**详见** [permission-layer.md §4.1 "Re-grill update 2026-06-13: 5-tier 重排 + path-based 决策"](./../.trellis/spec/backend/permission-layer.md) +
[project-cwd-boundary.md §6 "is_within_root"](./../.trellis/spec/backend/project-cwd-boundary.md) +
[docs/_history/reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md](./_history/reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md) +
[IMPLEMENTATION/decisions-2026-06.md "2026-06-13 Re-grill ADR"](./IMPLEMENTATION/decisions-2026-06.md)。

#### ⑩ Tool 执行

```rust
match tool_call.name {
    "read_file"   => read_file (with cat -n line numbers + ReadGuard.record_read),
    "write_file"  => tokio::fs::write (autoparse parent dir, boundary check),
    "edit_file"   => ReadGuard 3 道 check (read → fresh → match + uniqueness)
                     + 0 匹配报 hint + N>1 报行号 + 写后自动 invalidate,
    "shell"       => spawn_command (5min timeout, > 30KB spill to
                     <cwd>/.everlasting/outputs/<uuid>.txt + 1KB preview),
    "grep"        => tokio::process::Command::new("rg") spawn, 3 output_modes
                     (files_with_matches | content | count), 500-char line cap,
    "glob"        => globset walk, cap 100, mtime desc,
    "list_dir"    => tokio::fs::read_dir, alphabetical + `/` suffix on dirs,
                     non-recursive,
    "use_skill"   => SkillCache 取 SKILL.md 正文 → tool_result 回填(L1,2026-06-18 落地,详见 [IMPLEMENTATION §4](./IMPLEMENTATION/decisions-2026-06.md))
    "use_memory"  => 读 / 写 runtime memory(详见 [memory spec](../.trellis/spec/backend/memory.md))
    "use_ui"      => 构造 UiCard 走 ⑭ 分支(详见 [ROADMAP §1.2 B9](./ROADMAP.md#12-路线图外完成))
    ...
}
```

- **ReadGuard 防护层**(2026-06-07 工具集扩展批次加):
  - Tauri State `Mutex<HashMap<SessionId, HashMap<PathBuf, Fingerprint>>>`
  - `Fingerprint = { mtime, size, content_hash_head(xxh64 of 8KB) }`
  - `edit_file` 写前 3 道强制 check;`read_file` 成功自动 `record_read`;`edit_file` 写成功自动 `invalidate`
  - Session 隔离,切回不重读;`delete_session` 调 `clear_session` 清表
- **Bash 落盘**(2026-06-07 工具集扩展批次加):
  - > 30KB 输出 spill 到 `<session_cwd>/.everlasting/outputs/<uuid>.txt`
  - Tool result 返回 path + 1KB head+tail preview(让 LLM 拿 path 跟 `read_file` 配合)
  - `delete_session` best-effort 清理 outputs 目录(失败不 cascade)
- **关卡点**:
  - 真实文件系统操作(IO 错误、权限、磁盘满)
  - shell 命令:走 PTY(支持交互式),不是普通 exec
  - 大输出截断(spill + 1KB preview,避免 context 爆炸)
  - 超时(单个 tool 不能跑超过 N 分钟)

#### ⑪ Git 集成(隐式关卡)

写文件之后,可选:
```
  ├─ 写到 worktree 内 → git status 变更检测
  ├─ 是否自动 commit?
  │    ├─ 是 → git add . && git commit -m "agent: <summary>"
  │    └─ 否 → 留到 session 结束统一处理
  └─ 变更推给前端 → diff 视图实时更新
```

- 这一关在 frontend 看不见,但在背后持续运行

#### ⑫ 结果回填给 LLM

```json
构造 tool_result message:
{
  "type": "tool_result",
  "tool_use_id": "...",
  "content": "<执行结果 或 错误信息>",
  "is_error": false
}
追加到 messages
返回第 ⑥ 步,LLM 继续决策
```

- **关键设计**:**错误也回传给 LLM**,让它自己决定怎么修。这是 agent 自我纠错的基础

#### ⑬ 循环检测(防死循环)

```
如果连续 N 次 tool call 模式相同(同样输入产出同样 tool_use):
  └─ emit("warning:loop_detected")
  └─ 打断循环,返回错误给 LLM
  └─ 或暂停,问用户要不要继续
```

- **为什么需要**:LLM 偶尔陷入"反复试同一个错误"的死循环,白烧 token

#### ⑭ 流式 token 输出(混合事件模式)

**事件协议设计**:
- **高频事件**(`chat-event`，payload 判别):`delta`(token)、`start`、`done`、`error`,以及后续加入的 `turn_continuation`(F1 08-25,续轮渲染边界)/ `turn_complete` / `turn_usage` / `budget_trim` / `recall` / `context_compacted` / `loop_hint` / `workflow_breadcrumb` / `retrying` / `file_injections` / `speaker`(群聊)等 —— 完整 kind 枚举见 `app/src-tauri/src/llm/types/event.rs`(~19 个变体)
  - 流式 token 频率高,走单 listener + payload.type 分发,减少 listener 注册开销
  - **`session_id` 回填(2026-08-27)**:`chat-event` payload 自 `68f7cadc` 起带 `session_id`,非发起端(remote PWA)可跨客户端按 session 认领;向后兼容,老客户端忽略新字段
- **低频事件**(独立事件名):`tool:call`、`tool:result`、`permission:ask`、`ui:render`、`tool:question`(Phase C3)、`mode:change:request`(07-07)、`task:state:transition:request`(07-09)、**`stream-resync`**(08-24,SSE 崩溃恢复哨兵——重连后前端据此重发 resync 请求,服务端重放缺失段)
  - 需要精确 filter 的场景用独立事件名,前端好做 `listen("tool:call")` 过滤

```
收到 SSE chunk,按内容类型分发:
  ├─ TextDelta(t)        → emit("chat-event", { type: "delta", text })  → ⑮
  ├─ ToolUse(...)        → emit("tool:call", ...)                        → ⑨
  ├─ ToolResult(...)     → emit("tool:result", ...)                      → ⑫
  ├─ PermissionAsk(...)  → emit("permission:ask", ...)                   → ⑨
  └─ UiRender(...)       → emit("ui:render", ...)                        → ⑮
```

- **关键设计**:`ui_render` 不在 chat 流里走,单独的 UiCard 事件,前端用 component registry 渲染
- **为什么混合模式**:高频 token 需要单 listener 低开销;低频 tool/permission 需要精确 filter。两种模式各取所长
- **Phase 1 范围**:4 种 primitive(button / selector / diff / code_block),详见 [ROADMAP §1.2 B9](./ROADMAP.md#12-路线图外完成);B9+ 后补通用 button + diff 应用(UiDiffApplied 审计)
- **交错思考渲染(2026-07-23/24)**:前端按 run 分组 + contentBlocks 时间轴交错渲染(thinking/text/tool_use 按到达序),与 ⑦ 后端落库的真实流序对齐,见 [docs/INTERLEAVED-THINKING-DESIGN.md](./_history/2026-08-28-interleaved-thinking-design.md)。

#### ⑮ daemon 输出(HttpSseSink / Tauri event → client)

```
对每个 OutgoingMessage:
  ├─ 默认(daemon 模式):HttpSseSink(daemon/sse.rs)广播到 /api/v1/stream SSE
  │    └─ 前端 transport.listen 按 request_id 路由到对应 session 的 streamController
  ├─ 逃生(Full 模式):Tauri app.emit 事件,前端 listen
  ├─ 限速:防止 QPS 过高(GUI 本地不限;远程侧已实现限速 —— `ratelimit.rs` per-IP 10 次/分,见 [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md))
  └─ 消息合并:相邻 token 合并(50ms 内多条合并成一条)
```

- **关卡点**:输出载体适配(SSE / Tauri event)、限速、消息合并
- **新增**(对比原 14 关):老版本 token 是直接 `app.emit`,daemon 化后默认经 `HttpSseSink` → SSE
- **设计动机**:见 [§4 决策:Agent Daemon 化](#4-决策agent-daemon-化)。早期设想的"多 channel 输出适配(飞书/CLI)"未实施,见 [§5](#5-决策channel-adapter-抽象早期设想未实施)。

#### ⑯ 结束 / 解禁 / 统计

```
agent loop 结束(text-only response or max_turns reached):
  ├─ sink.send(ChatDone { usage, duration })    // HttpSseSink(daemon)/ Tauri emit(Full)
  ├─ 更新 session.last_active
  ├─ 解禁前端输入框(经 SSE / Tauri event 通知;纯浏览器模式同样走 SSE)
  ├─ 更新 token 用量统计(进 SQLite,给用量分析用)
  └─ 触发云端同步(若开启,详见 [BACKLOG §4](./BACKLOG.md#4-跨设备);注:远程通道已落地 —— 2026-08 起经 remote daemon 走**实时隧道**(WSS 长连接 + 反向代理),而非状态同步)
```

- **关卡点**:解禁通知走 SSE/event、云端同步是可选副作用
- **新增**(对比原 14 关):云端同步钩子,不动 LLM 流程

### 2.3 关键洞察(为什么 harness 难)

1. **关卡之间没有清晰边界** —— ⑨ 权限检查可能在 ⑩ 内部做,也可能在外层。架构选择决定了可测试性
2. **错误传播方向** —— 大部分错误要**回传给 LLM 让它自纠**,不是直接终止。这就是为什么"agent"和"普通脚本"是两种东西
3. **状态分散** —— session 状态在 DB、context 在内存、worktree 在磁盘、文件锁在 OS、daemon 在独立进程。要随时能重建
4. **token 预算是命门** —— ⑤ 步的 context 构造决定了你的 agent 能不能干长活,所有其他关卡都是"配套"
5. **用户信任链** —— ⑨ 是唯一用户能"中途喊停"的地方。这一步做错,用户就跑光了
6. **(daemon 化后)daemon 进程是状态边界** —— ⑬ 循环检测或 ⑯ 统计集中在 daemon 进程做,多 client(GUI + 浏览器)连同一 daemon 时天然共享同一 session 状态。早期设想用"Channel 抽象"表达这个边界,实际落地收敛为 HTTP/SSE 单端点
7. **(资源加载后新增)5a/5b/5c 的顺序** —— 错一个就 bug:Memory 在 Role 之前 vs 之后?Skill 描述在 Memory 之前还是之后?每改一次顺序,行为微妙变化

### 2.4 实施映射

> **18+ 关卡**(原 16 关卡 + C7 tools token / C7D stub / memory digest / C3+ compaction / unified-context-budget 硬卡 / MAX_TURNS 软卡 6 个新横切关注点,详见 §2.5.10~15)在 MVP 阶段和打磨阶段分别在哪落地,详见 [ROADMAP.md §1](./ROADMAP.md#1-已实施mvp-主体--路线图外完成) + 各阶段的技术细节分散在 [IMPLEMENTATION/decisions-2026-06.md / -07.md / -08.md](./IMPLEMENTATION/) 对应日期条目。本节不再维护细粒度"步骤 N → 关卡"映射表(随 V2 路线图重排已过时)。

### 2.5 横切关注点:16 关之外但必做的事

关卡图是纵向链路,但很多**横切关注点**贯穿多个关卡,容易被遗漏。下面列出 8 个,每个都标出"在哪个关卡被处理 / 关键设计点"。

#### 2.5.1 用户中途取消(CancellationToken)

- **触发场景**:用户在 LLM 流式输出中点 stop,或 long-running tool 内中断
- **位置**:② Tauri IPC 之后立刻建 `CancellationToken`;⑩ tool 执行内 `tokio::select!` 监听
- **关键设计**:取消不立即终止 LLM 请求,而是把"取消"事件本身作为 tool_result 回传(给 LLM 一次自我收敛的机会);只有用户二次取消才真终止
- **`shell` 进程组杀整组**:`shell` tool 子进程以 `process_group(0)` 启动,PGID == sh PID;cancel / timeout 时 `kill(-pgid, SIGKILL)` 杀整组,清理 `&` / 管道 / `nohup` 产生的孙子进程。Windows 留 P2
- **缺失后果**:用户按 stop 没反应 → 跑光了 token 还在跑 → 信任崩塌
- **当前实现**:MVP 简化决策——单次 cancel 即 emit `Done("cancelled")` 终止,**未实现"二次取消才真终止"语义**;完整 spec + 二次取消实现路径见 `docs/_history/reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` §2.1 + [IMPLEMENTATION §4 2026-06-17 ADR](./IMPLEMENTATION/decisions-2026-06.md)(RULE-A-010 已 closed 2026-06-17 via spec 偏离声明)

#### 2.5.2 ⑩ Tool 超时回填

- **阈值建议**:`shell` 5min,`read_file`/`grep` 30s,`write_file` 10s(可配)
- **kill 后的回填**:不返回成功也不返回错误,返回
  ```
  tool_result {
    is_error: true,
    content: "timeout after 300s, partial output: <截断的前 50KB>",
  }
  ```
- **LLM 据此**:可能重试、可能换 tool、可能放弃;这都是合法策略
- **实现位置**:⑩ 内部 `tokio::time::timeout` 包执行

#### 2.5.3 ⑩ 大输出截断

- **阈值**:`shell` / `read_file` 输出 > 50KB 触发
- **策略**:**head + tail** 各 25KB,中间塞 `<truncated: omitted N bytes, middle>`(LLM 必须能识别被截)
- **不能只用 head**:tail 通常包含 stack trace / 错误尾部,丢掉就丢诊断
- **实现位置**:⑩ 末尾、⑫ 之前

#### 2.5.4 ⑬ 循环检测阈值(C2 已实施 2026-06-24)

- **分级触发**(取代早期单一 `Jaccard > 0.9`,单一阈值无法适配短/长 input):
  - **Level 1 精确签名硬触发**(`HARD_WINDOW=3`):连续 3 次归一化签名完全相同 → 零误报抓真死循环
  - **Level 2 Jaccard 软提示**(`SOFT_WINDOW=5` / `SOFT_THRESHOLD=0.85`):≥2 对 token-set Jaccard > 0.85 → 容忍近重复
- **per-tool 签名**:`read_file`/`write_file`/`list_dir`=path,`grep`/`glob`=pattern+path,`edit_file`=path+old_string(含 old_string 才不误判正当的同文件多块编辑),`shell`/`run_background_shell`=command,其余 fallback `name+canonical(input)`
- **命中动作(软)**:两层都 `tracing::warn!` + 把 hint 文本插入 result message,**不跳过执行、不终止 loop**,撞线兜底见 §2.5.15(2026-08-19 起软卡询问,非硬停)。无 AuditKind 落表
- **完整设计 + 调研**:详见 [IMPLEMENTATION §4 2026-06-24 ADR](./IMPLEMENTATION/decisions-2026-06.md)

#### 2.5.5 ⑤ Context 压缩(C3+ LLM 摘要式压缩,2026-08-18 落地,**替代** C3 MVP 机械丢组 2026-06-12)

- **触发**:总 token > `context_window * 0.85`(取代 C3 MVP 的 0.80 阈值)。**触发口径 2026-08-19 统一切换**为"按发送部件加法"——`count_tokens(system_prompt) + count_tokens(tools_json) + estimate_messages_tokens(messages)` 三部件之和(`agent/budget.rs::estimate_request_tokens`),修复旧口径只数 messages、漏计 tools/system 的洞(小窗口模型 32k/64k 下可能在 messages 未达触发线时整体超窗)
- **策略**:LLM 9 段模板结构化摘要(`task / progress / facts / decisions / open / files / next` 等)+ `prior-summary` 增量合并(已存在 summary 作上下文,避免每轮全量重写)
- **保留区存活**:`clamp(15k, 10% 窗, 25k)` token 边界,**最近 turn 逐字不丢**(掉 LLM 看不到刚刚说过的话会发懵)
- **元数据**:摘要行落 `messages` 表 `metadata.kind = "compaction_summary"`(区别于 user / assistant),前端折叠渲染
- **水位**:`cutoff_seq` 精确折叠记忆,展开按需(不破坏 pair 不变量)
- **兜底**:连续 3 次 LLM 摘要失败 → 熔断回退 C3 机械丢组(0.80→0.50 旧逻辑,见代码 `agent/context.rs`)
- **硬卡**:2026-08-19 起叠加关卡⑤统一预算硬卡(`BUDGET_LINE_RATIO = 0.95`×window,裁尽仍超才 fail-fast),见 §2.5.14
- **实现位置**:`app/src-tauri/src/agent/context.rs`(`compact_messages` + 新 LLM call)+ `agent/budget.rs`(统一口径 + 硬卡引擎)+ `messages` 表 schema 兼容(messages 表 metadata 列从 JSON 字段读)
- **完整设计**:见 [ROADMAP.md §1.2 C3+](./ROADMAP.md) 行(2026-08-18 落地)+ C3 MVP 历史见 [IMPLEMENTATION/decisions-2026-06.md 2026-06-12/14/15 ADR](./IMPLEMENTATION/decisions-2026-06.md)(RULE-A-001/002/006 已闭环)

#### 2.5.6 Session 切换的并发态

- **问题**:① 防双发在 GUI 层,但 §1.3 session 切换时前 session 的 SSE 还在收 token
- **解决**:切 session 时,前 session 收到 CancellationToken,新消息被前端拦截,直到前 session ⑯ 发 `ChatDone` 才解禁
- **实现位置**:§1.3 [6] "清空当前 agent core 状态" 之前,先发 CancellationToken;前端 ① 拦截直到 `chat:done`

#### 2.5.7 LLM Provider 限流

- **必须做**:TPM (tokens per minute) + RPM (requests per minute) 限流
- **参考值**:Anthropic tier 1 默认 50 RPM、TPM 视模型 30k-100k
- **位置**:⑥ 之前加令牌桶 / leaky bucket,跨 session 共享(多 session 并发必撞)
- **超限**:`channel.send("rate_limited, retrying in Xs")`,前端提示,自动重试
- **不能省**:省钱 + 避免封号;Anthropic 429 是软警告,3 次之后硬封

#### 2.5.8 ⑯ 审计日志(A2 + B7 PR1 + C4 PR1/PR2 落地,2026-06-13/14,**已实施**)

- **记录场景**:⑨ 权限决策(7 种) + ⑩ tool 执行(`ToolExecuted` C4 PR1) + ⑯ mode 切换(`set_session_mode` inline 写)
- **存储**:`session_audit_events` 表(SQLite,`session_id` + `ts DESC` 索引)
- **payload 统一 JSON 结构**:按 kind 分发 — ⑨ 关类 `{tool_name, tool_input, reason?, mode, critical?}`;⑩ `ToolExecuted` `{tool_name, tool_input, duration_ms, exit_code: Option<i32>}`(`null` = 无 exit code,`-1` = 被 kill);⑯ mode 类 `{prev_mode, new_mode}`。`critical: bool` 决定前端 `PermissionModal` 的 3px 红左 border + shield-x icon
- **Audit write 策略**:best-effort,失败 `tracing::warn!` 不报错(必须保证不破坏 agent loop)
- **UI 查询**(C4 任务,2026-06-14 PR2 已实施):Tauri command `list_session_audit_events(session_id)` → `Vec<AuditEventRow>`;前端 `useAuditStore` + `<AuditLogModal>` 绑当前 session;kind 下拉筛选 + "仅 critical" 复选 + 计数 + 刷新;按 `ts DESC, id DESC` 稳定排序
- **28 类 AuditKind(2026-08-28 实测,`ScheduledTaskFired` 为第 28 个,见 `app/src-tauri/src/agent/permissions/audit.rs`)** + 完整 schema + payload wire shape + UI 渲染细节,按域分组:
  - **Tool 域(5)**:ToolDenied / ToolAllowed / ToolPermissionAsk / ToolExecuted / ToolDeniedYolo
  - **Permission 域(3)**:PermissionGranted / PermissionTimeout / RequestCancelled
  - **Mode 域(6)**:ModeChanged / YoloEntered / YoloExited / ModeChangeRequested(07-07 request_mode_change 工具)/ ModeChangeAllowed / ModeChangeDenied
  - **Message 域(2)**:EditMessage(D3 PR1)/ ResendMessage(D3 PR3)
  - **Loop 域(2)**:LoopIntervention(C2+ 07-05 主动干预)/ TurnLimitSoftcap(08-19 MAX_TURNS 软卡询问)
  - **Worker 域(4)**:WorkerAskAllowed / WorkerAskDenied / WorkerAskTimedOut / WorkerAskCancelled(L3b 06-22 RULE-FrontSubagent-003 fix)
  - **TaskStateTransition 域(3)**:TaskStateTransitionRequested / Allowed / Denied(07-08 workflow Phase 3 Step 3.1)
  - **Budget 域(1)**:ContextBudgetTrim(08-19 关卡⑤硬卡裁剪,unified-context-budget)
  - **UI 域(1)**:UiDiffApplied(B9+ D4 07-13 apply_ui_diff IPC 成功)
  - **Scheduler 域(1)**:ScheduledTaskFired(F2 08-28,动作 fired/catchup/skipped_dedup/skipped_queue_disabled/lost/error)
  - 实现位置:`app/src-tauri/src/agent/permissions/audit.rs`;落表点见各 variant 注释 + [IMPLEMENTATION/decisions-2026-07.md](./IMPLEMENTATION/decisions-2026-07.md) 各月 ADR

#### 2.5.9 ⑩ 并行 tool 执行(L2 MVP,2026-06-19 落地,**已实施**)

- **触发**:单 turn 内 LLM 返回的**所有** tool_use ∈ `{read_file, grep, glob, list_dir, use_skill}`(纯本地只读 + 全静默 Allow)**且**任一 path 工具的 `path` 解析后 ∈ project root → 并发执行;否则(含 write_file/edit_file/shell/update_checklist/web_fetch 或 path-outside-root)→ 整批串行
- **判定**:`is_parallel_eligible(&tool_calls, &permission_ctx.cwd)`(纯谓词)
- **实现**:`FuturesUnordered` + `permissions::check` → `execute_tool(token.clone())` → cancel 检查 → audit → `emit_tool_result`;`result_slots[i]` 按 tool_use **原始 index** 回填
- **不变量**:
  - 多 tool_result **单消息打包**(parallel-tool-use 红线:拆消息会让 LLM "学会"避免并行)
  - `web_fetch` 虽只读但 Tier 4 默认 `emit ask`,MVP 排除(走串行,保留逐个 ask UX)
  - 共享状态安全:并发集合无 shell(改 cwd)/edit_file(写 read_guard)→ 无写冲突;`PermissionStore`/`SkillCache`/`ReadGuard` 都是 `Arc<Mutex/RwLock>`,多 task 并发 read 安全
  - cancel:并发不 `break`,等所有 task 完成或被 cancel;`execute_tool` 内 `tokio::select!` 各 task 独立响应 cancel
- **完整设计 + RULE-A-013 path-in-root 收口 + 调研引用**:见 [IMPLEMENTATION §4 2026-06-19 ADR](./IMPLEMENTATION/decisions-2026-06.md) + [`spikes/2026-06-19-async-parallel-tool-research.md`](./_history/spikes/2026-06-19-async-parallel-tool-research.md)

#### 2.5.10 ⑨ C7 tools token 治理(2026-08-14 落地)

- **问题**:关卡 ⑤ context 构造时,LLM tool 列表占 prompt 大量 token(实测 25 builtin × 平均 ~1.2KB schema ≈ 30k token),`context_window * 0.85` 触发前已吃紧
- **方案**:静态裁剪 `STUB_CANDIDATES` 列表(`filter_tools_for_session_type` 在 drive.rs 第 3 环,按 session_type 砍掉不适用的 builtin,例如 group_chat 砍掉 `dispatch_subagent`、worker subagent 砍掉 `merge_worker` / `discard_worker` / workflow-only 工具)
- **度量**:`turn_trace.tools_token INTEGER` 列(C7 08-14,add_turn_trace_column_if_missing backfill,见 `db/migrations/schema.rs:994-999`)
- **完整设计**:见 [ROADMAP.md §1.2 C7](./ROADMAP.md) 行(2026-08-14 落地)

#### 2.5.11 ⑨ C7D tools stub 注册 + 元工具按需取回(2026-08-14 落地)

- **问题**:C7 静态裁剪后,某些罕见工具仍被 LLM 主动调(例如 `merge_worker` / `discard_worker` 在 worker 流程),一刀切砍掉误伤
- **方案**:`tools/stub.rs` + `StubRegistry`(session 粘性 loaded-set,记录当前 session 已经取回 schema 的工具名)+ **`load_tool_schemas` 元工具**(LLM 想调罕见工具时显式 `load_tool_schemas({"merge_worker"})` 拿回完整 schema)
- **gate**:`tools_stub_enabled` drive.rs 第 4 环(开关 && 非 worker && 非群聊时生效,worker / 群聊直接给全 schema 不走 stub)
- **度量**:`turn_trace.tools_token` 配合 stub 触发次数统计(预计 tools_token 进一步 -12%)
- **完整设计**:见 [ROADMAP.md §1.2 C7D](./ROADMAP.md) 行(2026-08-14 落地)

#### 2.5.12 ⑤ memory-gov 指令块窗口治理(2026-08-15 落地)

- **问题**:关卡 ⑤ context 构造时,AGENTS.md / CLAUDE.md 加载段占 prompt token(实测 4 文件合计 60-100k token,长项目超过 0.30×window)
- **方案 WP1 度量**:`turn_trace.memory_token INTEGER` 列(08-15,backfill 同 C7)
- **方案 WP2 切节注入**:`memory/digest.rs` fence-aware 切节目录(纯机械,标题 + 首句,无 LLM 调用);`AGENTS.md` primary 永不 digest(`mtime` 锁死),`CLAUDE.md` 且 tokens>600 才 digest
- **方案 WP3 元工具**:`load_memory_sections` 元工具(append,精确寻址 banner label 切片,LLM 看到目录找不到的内容时显式拉全文)
- **gate**:`MemoryDigestRegistry` OnceLock 单例 + `memory_digest_enabled` 缺省 on(fail-open,worker / 群聊豁免)
- **完整设计**:见 [ROADMAP.md §1.2 memory-gov](./ROADMAP.md) 行(2026-08-15 落地)

#### 2.5.13 ⑤ C3+ LLM 摘要式压缩(2026-08-18 落地)

- **见 §2.5.5**(新策略替代 C3 MVP 0.80→0.50 机械丢组);4 个新横切关卡中 C3+ 是最重的,核心 spec 详见 §2.5.5,本节仅作为横切索引存在

#### 2.5.14 ⑤ 统一上下文预算硬卡(unified-context-budget,2026-08-19 落地)

- **问题**:C3+ 压缩触发线(0.85×window)只盯 messages 旧口径,且是"事后压缩"不是"事前硬卡";多来源切片(tools / memory / 图片 / @文件 / system)各是各的账,没有一把统一的尺
- **统一口径(WP1 度量)**:按发送部件加法 — `estimate_request_tokens = count_tokens(system_prompt) + count_tokens(tools_json) + estimate_messages_tokens(messages)`(`agent/budget.rs`)。**核心不变量:归因切片(tools_token / memory_token / at_files_token / images_token / system_token)是从 messages 内部归因的展示口径,与总量口径永不互相加计**(评审 F1 重复计数教训,AC1 单测锁定)
- **新切片列**:`turn_trace.at_files_token`(@文件注入体 cl100k 估算)/ `system_token`(system prompt 体 + skill-listing 合成消息)/ `context_window`(请求时模型窗口快照,前端预算行分母)——均幂等 backfill(`add_turn_trace_column_if_missing`),NULL 为加列前行 / worker 轮
- **关卡⑤硬卡(WP2 引擎)**:`BUDGET_LINE_RATIO = 0.95`×window 触发**静默裁剪**(对齐 `SUMMARY_POSTCHECK_RATIO` 0.95,贴窗留 5% 吸收 cl100k 与 provider 计量的系统性偏差);裁尽仍超才 fail-fast。触发落 `AuditKind::ContextBudgetTrim`。软卡「压缩后续跑」force 压缩走同引擎但绕过 token 触发线(见 §2.5.15)
- **前端**:TurnCard 预算构成条(各切片占比,分母 = context_window)+ BudgetTrim 瞬时 chip + 审计条目
- **完整设计**:见 [ROADMAP.md §1.2 unified-context-budget](./ROADMAP.md) 行;spec 沉淀 `.trellis/spec/backend/agent-loop-architecture/pattern-budget-gate.md`

#### 2.5.15 ⑬ MAX_TURNS 软卡 + 手动 /compact + handoff(2026-08-19 落地)

- **MAX_TURNS 软卡**(替代硬终断):单聊主 loop 撞线(缺省 200)不再无条件 `stop_reason="max_turns"` 硬停,改为 QuestionStore 软卡询问——继续(+`TURN_LIMIT_GRANT`=200)/ 压缩后续跑(置 `force_compaction=true` 绕过 C3 token 触发线,`trigger_label="softcap"` 观测区分)/ 停止;10 分钟超时兜底(`EVERLASTING_SOFTCAP_TIMEOUT_MS` 测试钩子)。**break 门 = `effective_is_worker || group_chat_state.is_some()`**:worker(有 C1 resume)与群聊 speaker 段保持硬卡直接 break。新 `AuditKind::TurnLimitSoftcap`(action `asked/continued/compacted_continued/stopped/timeout_stopped/cancelled`,worker 与群聊不落表)。实现 `chat_loop.rs::ask_turn_limit_softcap` + `emit_max_turns_terminal`
- **手动 /compact**:空闲期 LLM 摘要压缩入口(通用内置命令直输分发),不走软卡 force 路径(软卡走 drive_turn 按值穿参,见 spec)
- **handoff 跨 session 接力**:接力摘要进下一 session + HUD 按 session 隔离修复
- **worker per-turn 度量(2026-08-20)**:`turn_trace` 表重建并入 run 维度 — 唯一键 `UNIQUE(session_id, run_id, seq)`(`''` 哨兵 = 主 loop 行,worker 行 = `subagent_runs.id`;不用 NULL 因 SQLite UNIQUE 视 NULL 互异)+ partial index `idx_turn_trace_run`(`WHERE run_id != ''`)+ `list_worker_turn_traces` IPC 全链 + SubagentDrawer「Token 明细」per-run 折叠区(`runTracesByRunId` 粘性缓存)。老库走 `schema_helpers::rebuild_turn_trace_with_run_id` 重建迁移
- **spec**:`.trellis/spec/backend/agent-loop-architecture/pattern-turn-limit-softcap.md`

---

## 3. 决策:每个 Session 一个 Git Worktree

**为什么用 worktree**:
- 不同 session 可能同时活跃(用户切来切去,或者未来多 agent 并行)
- worktree 共享 `.git`,但工作目录独立
- 不同分支,互不污染
- 切换 session 几乎瞬时,不用 `git stash` / `git checkout` 来回跳

**实现要点**:
- session 创建时:`git worktree add ~/.local/share/everlasting/worktrees/<project_uuid>/<session_id> -b session/<session_id>`(XDG 标准路径,跨机器一致,为后期 v2 跨设备接续做铺垫)
- session 结束时:可选 merge 回主分支,或保留作历史
- libgit2(`git2-rs`)的 worktree API 不完整,可能要 spawn `git worktree` 命令

**Step 4 follow-up(2026-06-08)**:worktree 不再随 session 自动创建,改为 opt-in 三态操作:

- `none`(默认):session 创建不建 worktree,非 git 项目也能用 session
- `active`:用户主动 `attach_worktree(sessionId)`,建 worktree + branch,工具 cwd 落到 worktree
- `detached`:用户 `detach_worktree(sessionId)`,worktree + branch 留盘但 session 不再绑定,工具 cwd 回退到 project.path
- 物理销毁走 `delete_worktree(sessionId)`,跟 detach 分离(后悔药可分两步走)

具体契约 + LLM 透明度(7 工具 cwd 字段 + system event 注入)见 `.trellis/tasks/archive/2026-06/06-07-step-4-follow-up-session-worktree-attach-detach-delete-git/prd.md`。

---

## 4. 决策:Agent Daemon 化

> **状态**:已实施(2026-07)。

**核心变更**:agent core 从 Tauri 进程内拆出,变成独立 `everlasting-daemon` 进程。Tauri GUI 降级为瘦客户端(Thin 模式),与浏览器 client 并列,都经同源 HTTP/SSE 连同一 daemon。

> 这条决策的完整动机与编排见 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md);决策档案(为什么 axum / 为什么 sidecar / 为什么默认 httpTransport)见 [IMPLEMENTATION/decisions-2026-07.md](./IMPLEMENTATION/decisions-2026-07.md)(daemon 化于 2026-07-23 收官)。本节只讲架构本身。

**为什么必须**:
- 远程/浏览器访问 —— agent core 要能脱离 Tauri webview 被浏览器触达(daemon 用 ServeDir 同源服务 SPA)
- 多 client 共用同一 agent core —— 桌面 GUI + 纯浏览器连同一 daemon,共享 session 状态(早期设想的飞书/CLI 多 channel 是后续项,见 [§5](#5-决策channel-adapter-抽象早期设想未实施))
- agent core 与 GUI 解耦 —— GUI 重启不影响 daemon 里的长跑 session(Thin 模式 GUI 不持有任何状态)

**架构影响(实际落地)**:
- 新增 `src-tauri/src/daemon/` 目录(`server.rs` axum router + `sse.rs` HttpSseSink + `error.rs` + `routes/` 19 个路由域文件)+ `src-tauri/src/bin/everlasting-daemon.rs`(daemon bin 入口)+ `src-tauri/src/sidecar.rs`(GUI 侧 spawn + 生命周期管理)
- 前端新增 `app/src/transport/` 抽象层(httpTransport 默认 / tauriTransport `?transport=tauri` 逃生)
- 通信:**同源 HTTP + SSE**(axum POST `/api/v1/*` + `/api/v1/stream` SSE),daemon 用 `tower-http::ServeDir` 同源服务 `dist/` SPA。**不是** Unix socket / Named pipe / WebSocket —— 早期设想的本地 IPC 已被同源 HTTP 取代
- 进程管理:GUI 经 `tauri-plugin-shell` spawn daemon 为 sidecar(`sidecar.rs::spawn_and_manage`),`RunEvent::Exit` 钩子 kill sidecar(无孤儿进程);裸跑/浏览器模式用 `scripts/daemon.sh`(start/bg/stop/restart/status/logs,PID 文件 + graceful shutdown)。**不用** systemd/pm2 —— sidecar 模式由 GUI 托管,裸跑模式由脚本托管
- 118 个原 `#[tauri::command]` handler 镜像为 REST 路由(Q0 决策:同 handler 双暴露 IPC + HTTP,代码复用;**2026-08-28 实测 118** 个)
- 新增 `crates/everlasting-remote/`(axum 云服务端:shared_secret auth + device_token、配对码 60s 一次性 + per-IP 限速(`ratelimit.rs`)、WSS 隧道服务端、反向代理、SSE 桥;DB `nodes` / `devices` / `pairing_codes` 三表)+ `crates/everlasting-remote-protocol/`(2026-08-11 workspace 翻转:根 `Cargo.toml` members 3 个,default-members 只含 remote 两 crate,Cargo.lock / target 在根)
- PC daemon 新增 `src-tauri/src/daemon/tunnel/`(client / config / dispatcher / manager / node_id / sse_bridge;WSS 长连接 + loopback 转发,取消只停转发)
- 前端新增 `app/src/transport/auth.ts`(device_token / `isRemoteContext()`)+ `app/src/router/index.ts` vue-router `isRemoteContext()` 守卫 + `PairingView` / `NodeListView` / `ChatView` / `RemoteTab.vue` + PWA 壳(vite-plugin-pwa + `public/icons/`);配对流程:PC Remote tab 生成 6 位配对码 → 手机 PWA redeem 换 64-hex device_token → nodes 列表
- 远程访问专用文档与脚本:`docs/REMOTE-DEPLOY.md` / `docs/REMOTE-ACCESS-E2E.md` / `scripts/remote.sh` / `deploy-remote.sh` / `remote-e2e-smoke.mjs`

**自研 daemon**:进程就一个,行为可预测;sidecar 由 GUI 进程托管生命周期,裸跑由 `scripts/daemon.sh` 托管。

---

## 5. 决策:Channel Adapter 抽象(早期设想,未实施)

> ⚠️ **本节是 2026-06 早期设计设想,实际未实施。** 2026-07 daemon 化落地时走了更简单的 axum HTTP 单端点路线(见 [§4](#4-决策agent-daemon-化)),没有引入 `Channel` trait。下方内容保留作为历史设计脉络参考 —— 当初设想用 trait 抽象承载多入口(飞书/CLI),后来判断"抽象过早"(本节自己的风险项之一应验),收敛为 HTTP/SSE。未来若真要做飞书/CLI 多入口,可重新评估是否抽 trait。

**当初设想的抽象**:
```rust
#[async_trait]
trait Channel: Send + Sync {
    async fn send(&self, msg: OutgoingMessage) -> Result<MessageId>;
    fn subscribe(&self) -> BoxStream<'static, IncomingMessage>;
    fn capabilities(&self) -> ChannelCapabilities;
}
```

**当初设想的实现**:
- `TauriGuiChannel` — 走 Tauri event(✅ 当时已实现,步骤 1)
- `FeishuChannel` — 走飞书 WebSocket(B10 飞书 IM,待 [ROADMAP §2 第四档](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排) 实施)
- `CliChannel` — 走 stdin/stdout(待后期实施)

**实际落地的替代**:axum HTTP `/api/v1/*` 路由 + `HttpSseSink` SSE 广播(`daemon/server.rs` + `daemon/sse.rs`)。前端经 `httpTransport`(fetch + EventSource)统一接入;Full 模式逃生经 `tauriTransport`(Tauri event)。"多入口"的诉求目前由"多 client 连同一 HTTP daemon"(GUI + 浏览器 + 经 remote daemon 的远程 PWA)满足,不需要 trait。

**当初设想的好处(供未来重新评估参考)**:
- 新增 channel 不用改 agent core,只实现 trait
- 跨 channel 行为可统一(限速、消息合并、状态同步)
- 测试友好(mock 一个 channel 就能跑 agent)

**当初的协议约束**(仍适用于未来任何多入口方案):
- 所有 message 必须可序列化到 JSON(明文),不依赖 Rust 特定类型
- 传输层无关:HTTP / WSS 都能承载同一份 JSON

**应验的风险**:
- 抽象过早:落地时只有 1 个真实入口(GUI/浏览器都走 HTTP),trait 被判 overdesign,直接用 axum 路由 + SSE。这条保留为"下次想做飞书/CLI 时再决定要不要抽 trait"的备忘。
