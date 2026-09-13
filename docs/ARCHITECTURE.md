# ARCHITECTURE — 架构设计

> Everlasting 的"整体怎么搭、关键流程怎么走"。包括系统架构图、请求生命周期的 16 道关卡、以及核心架构决策。
> 需求见 [DESIGN.md](./DESIGN.md),技术选型见 [TECH.md](./TECH.md),技术路线图见 [ROADMAP.md](./ROADMAP.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 系统架构

> ✅ **当前状态(2026-08-31)**:**daemon 化(2026-07-23)+ remote-control epic S1~S6b(2026-08-11~13 收官,merge `94828cb`)+ 6 个跨层特性(2026-08-14~18:C7 tools token / C7D stub 注册 / memory-gov 指令块治理 / B1 image multimodal / D2 跨 session 全文搜索 / C3+ LLM 摘要式压缩)+ 6 个续接特性(2026-08-19~27:unified-context-budget 统一 token 预算 + 关卡⑤硬卡 / MAX_TURNS 软卡 / 手动 /compact / 跨 session 接力 handoff / worker per-turn 度量 + turn_trace 表重建 / F1 消息队列 / F4 web_search / F5 文档提取 / F6 异步可观测性 + F3 并发闸)+ F2·F2b 定时任务(08-28)+ 08-29~31 批(schedule_task 工具家族 / C6 输出截断统一 / ShellCard / 审计 keyset 分页 / Playwright 浏览器回归流水线 / Sandbox P3b / 定时任务 per_run 三档)**。agent core 跑在独立 `everlasting-daemon` 进程(axum HTTP server,见 `app/src-tauri/src/daemon/` + `bin/everlasting-daemon.rs`)。Tauri GUI 进程作为瘦客户端,经 `sidecar.rs::spawn_and_manage` spawn daemon 为子进程,前端默认走 `httpTransport`(同源 HTTP + SSE)与 daemon 通信;daemon 用 `tower-http::ServeDir` 同源服务前端 SPA,故也支持纯浏览器访问(浏览器模式)。`?transport=tauri` + Full 模式(`EVERLASTING_GUI_FULL_STATE=1`)是 daemon 故障时的逃生舱,回退到一体化 Tauri IPC(legacy in-process)。编排放 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md),决策见 [§4](#4-决策agent-daemon-化)。
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
║  │   · 116 个 #[tauri::command] 镜像为 REST 路由(2026-09-07 │          ║
║  │     实测)                                              │          ║
║  │   · /api/v1/stream (SSE) — HttpSseSink 广播事件          │          ║
║  │   · /api/v1/attachments/<id> GET 二进制(B1 08-16,首个    │          ║
║  │     非 JSON REST 路由,手机 PWA 看图路径)                │          ║
║  │   · ServeDir fallback(同源服务 dist/ SPA)              │          ║
║  │  ──────────────────────────────────────────────────────  │          ║
║  │  AppState (Arc,axum 每个 handler clone 一份)             │          ║
║  │   · SQLite pool(持有 WAL writer;Thin 模式 GUI 不开)   │          ║
║  │   · agent core(Agent Loop / Tool Registry 28 builtin     │          ║
║  │     + 1 stub 元工具 load_tool_schemas + 1 动态 dispatch  │          ║
║  │     dispatch_subagent = 30 注册名;                         │          ║
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
- **everlasting-daemon Process**:跑所有 agent 逻辑 + 持有 SQLite pool(WAL writer)。axum router 把 116 个原 `#[tauri::command]` handler 镜像为 REST 路由(2026-09-07 实测),前端同一份 handler 代码服务 IPC 与 HTTP。
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
      // Skill 按 use_skill 触发时按需加载(详见 [LIFECYCLE §2.5.12](./LIFECYCLE.md#2512-⑤-memory-gov-指令块窗口治理2026-08-15-落地))
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

**决策偏差记录**:Phase 3(dogfooding)在 dogfooding 前置条件未满足时由 epic 直接启动完成。

## 2. Harness 设计:从用户输入到文件变更的 16 道关卡

> **已拆出**(2026-09-13 doc-split):完整 walkthrough(§2.1 全景图 / §2.2 16 关详解 / §2.3 关键洞察 / §2.4 实施映射 / §2.5 横切关注点 18+ 关卡)见 [LIFECYCLE.md](./LIFECYCLE.md)——章节编号原样保留(§2.x),既有引用可直接对照。

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

> 这条决策的完整动机与编排见 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md)(daemon 化于 2026-07-23 收官)。本节只讲架构本身。

**为什么必须**:
- 远程/浏览器访问 —— agent core 要能脱离 Tauri webview 被浏览器触达(daemon 用 ServeDir 同源服务 SPA)
- 多 client 共用同一 agent core —— 桌面 GUI + 纯浏览器连同一 daemon,共享 session 状态(早期设想的飞书/CLI 多 channel 是后续项,见 [§5](#5-决策channel-adapter-抽象早期设想未实施))
- agent core 与 GUI 解耦 —— GUI 重启不影响 daemon 里的长跑 session(Thin 模式 GUI 不持有任何状态)

**架构影响(实际落地)**:
- 新增 `src-tauri/src/daemon/` 目录(`server.rs` axum router + `sse.rs` HttpSseSink + `error.rs` + `routes/` 28 个路由域文件,2026-09-07 现状)+ `src-tauri/src/bin/everlasting-daemon.rs`(daemon bin 入口)+ `src-tauri/src/sidecar.rs`(GUI 侧 spawn + 生命周期管理)
- 前端新增 `app/src/transport/` 抽象层(httpTransport 默认 / tauriTransport `?transport=tauri` 逃生)
- 通信:**同源 HTTP + SSE**(axum POST `/api/v1/*` + `/api/v1/stream` SSE),daemon 用 `tower-http::ServeDir` 同源服务 `dist/` SPA。**不是** Unix socket / Named pipe / WebSocket —— 早期设想的本地 IPC 已被同源 HTTP 取代
- 进程管理:GUI 经 `tauri-plugin-shell` spawn daemon 为 sidecar(`sidecar.rs::spawn_and_manage`),`RunEvent::Exit` 钩子 kill sidecar(无孤儿进程);裸跑/浏览器模式用 `scripts/daemon.sh`(start/bg/stop/restart/status/logs,PID 文件 + graceful shutdown)。**不用** systemd/pm2 —— sidecar 模式由 GUI 托管,裸跑模式由脚本托管
- 116 个原 `#[tauri::command]` handler 镜像为 REST 路由(Q0 决策:同 handler 双暴露 IPC + HTTP,代码复用;**2026-09-07 实测 116**(08-31 为 107,09-01 增 update_project_sandbox_policy,09-02~09-07 再增 8:list/kill_background_shell、get_disk_usage/run_disk_cleanup、resume_group_chat、preempt_group_chat、set_provider_disabled/set_model_disabled);旧 118 为含注释的 grep 口径)
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
