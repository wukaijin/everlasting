# ARCHITECTURE — 架构设计

> Everlasting 的"整体怎么搭、关键流程怎么走"。包括系统架构图、请求生命周期的 16 道关卡、以及核心架构决策。
> 需求见 [DESIGN.md](./DESIGN.md),技术选型见 [TECH.md](./TECH.md),决策档案见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),技术路线图见 [ROADMAP.md](./ROADMAP.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 系统架构

> ✅ **当前状态(2026-07-23,daemon 化已落地)**:agent core 跑在独立 `everlasting-daemon` 进程(axum HTTP server,见 `app/src-tauri/src/daemon/` + `bin/everlasting-daemon.rs`)。Tauri GUI 进程作为瘦客户端,经 `sidecar.rs::spawn_and_manage` spawn daemon 为子进程,前端默认走 `httpTransport`(同源 HTTP + SSE)与 daemon 通信;daemon 用 `tower-http::ServeDir` 同源服务前端 SPA,故也支持纯浏览器访问(浏览器模式)。`?transport=tauri` + Full 模式(`EVERLASTING_GUI_FULL_STATE=1`)是 daemon 故障时的逃生舱,回退到一体化 Tauri IPC(legacy in-process)。编排放 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md),决策见 [§4](#4-决策agent-daemon-化) + [IMPLEMENTATION.md §4](./IMPLEMENTATION.md)。
>
> 📜 **历史脉络**:2026-06-07 初版本文档时,daemon 化还是"目标态",且当时设想用 `Channel Router` + `TauriGuiChannel`/`FeishuChannel`/`CliChannel` 抽象(见 [§5](#5-决策channel-adapter-抽象早期设想未实施))承载多入口。**实际落地(2026-07)走的是更简单的 axum HTTP 单端点路线**,没有引入 Channel trait —— 该抽象降级为「早期设想,未实施」,保留在 §5 供历史参考。§2 16 关卡中残留的 "Channel Router" 字样是当时叙事载体,实际对应 daemon 的 axum 路由 + `HttpSseSink`。

### 1.1 进程拓扑(daemon 化后,2026-07 落地)

```
两种运行形态,共享同一份 agent core 代码(AppState + agent loop)。

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
║  │   · 91 个 #[tauri::command] 镜像为 REST 路由             │          ║
║  │     (同 handler 双暴露 IPC + HTTP,Q0 决策)              │          ║
║  │   · /api/v1/stream (SSE) — HttpSseSink 广播事件          │          ║
║  │   · ServeDir fallback(同源服务 dist/ SPA)              │          ║
║  │  ──────────────────────────────────────────────────────  │          ║
║  │  AppState (Arc,axum 每个 handler clone 一份)             │          ║
║  │   · SQLite pool(持有 WAL writer;Thin 模式 GUI 不开)   │          ║
║  │   · agent core(Agent Loop / Tool Registry 24 builtin    │          ║
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

   daemon 进程外部依赖(两种形态共用):
         ↓ LLM API                  ↓ Local FS / Git
    (Anthropic / OpenAI)         (WSL 内 $HOME/projects)
```
**进程边界说明**:
- **Tauri GUI Process(Thin 模式)**:只渲染 SPA + 经 `httpTransport` 转发请求,**不**加载 `AppState`、**不**开 DB pool、**不**跑 sweep/hygiene 后台任务。spawn daemon 子进程,`RunEvent::Exit` 钩子回收 sidecar(无孤儿进程)。
- **everlasting-daemon Process**:跑所有 agent 逻辑 + 持有 SQLite pool(WAL writer)。axum router 把 91 个原 `#[tauri::command]` handler 镜像为 REST 路由,前端同一份 handler 代码服务 IPC 与 HTTP。
- **通信**:同源 HTTP(POST `/api/v1/...`)+ SSE(`/api/v1/stream`)。sidecar 模式下 daemon 监听 `0.0.0.0:7456`(WSL-first:Windows 宿主浏览器经 WSL2 localhost 转发可达),GUI 同源访问无 CORS。**不是** Unix socket / WebSocket —— 早期设想的本地 IPC 已被同源 HTTP 取代(见 [§5](#5-决策channel-adapter-抽象早期设想未实施))。
- **逃生舱**:`?transport=tauri` + Full 模式(`EVERLASTING_GUI_FULL_STATE=1`)回退到 legacy in-process —— GUI 加载 `AppState` + 走 Tauri IPC,不 spawn sidecar。daemon 故障时用。
- **daemon 化动机**:远程/浏览器访问;agent core 与 GUI 解耦;多 client 共用同一 agent core。详见 [§4 决策:Agent Daemon 化](#4-决策agent-daemon-化)。

### 1.2 关键数据流:用户发一条消息(daemon 化后,默认 httpTransport)

> 📌 **当前默认路径(Thin 模式)**:Frontend → `transport.invoke('chat', ...)`(`httpTransport`:fetch POST 到 daemon `/api/v1/chat`)→ daemon 进程的 axum 路由调同一份 `chat` handler → `chat_stream_with_tools()`(reqwest + 手写 SSE)→ `HttpSseSink`(`daemon/sse.rs`)经 `/api/v1/stream` 同源 SSE 广播 `chat-event` / `tool:call` / `tool:result` → Frontend 单 SSE listener(`streamController.ts`,按 `request_id` 路由)→ Pinia store 增量更新。
> 逃生路径(Full 模式 `?transport=tauri`):`tauriTransport` 走 Tauri IPC,handler 在 GUI 进程内,事件经 Tauri event emit。两条路径共享同一 `#[tauri::command]`/REST 双暴露 handler。

```
[1] Frontend (Vue 3)
    用户输入消息 → transport.invoke('chat', { requestId, messages })
      └ 默认 httpTransport:fetch POST /api/v1/chat(同源 daemon)
      └ 逃生 tauriTransport:tauri.invoke('chat', ...)(Full 模式,GUI 进程内)

[2] everlasting-daemon Process(axum)  /  Full 模式下的 Tauri GUI Process
    axum 路由 / Tauri command 收到请求 → spawn 异步任务处理
    invoke/fetch resolve 立即返回("已受理",非"已完成")

[3] agent core(同一份 handler 代码,两种入口)
    SessionManager::handle_message(session_id, content)
      → 写入 SQLite (user message)
      → 触发 agent core
    agent core:
      构造 messages: [system_prompt + role + memory, ...history, new_user_msg]
      // Skill 按 use_skill 触发时按需加载(详见 [ARCHITECTURE §2.2 第 ⑤a 关](#5a-资源加载-skill--memory--role))
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

```
[1] User clicks project A → session B
[2] Frontend: transport.invoke('load_session', { sessionId: B })
[3] daemon / Tauri backend: 从 SQLite 读 messages → 返回 SessionSnapshot
```

### 1.4 群聊模式(group chat,2026-07-29 落地,08-04~08-07 迭代加固)

> 📌 **session_type 区分两种循环**:`sessions.session_type = 'chat'`(默认)走 `agent/chat_loop.rs`(经典单 agent);`'group_chat'` 走 `agent/group_chat_loop.rs`(多参与者 turn-taking 编排)。
>
> **群聊循环**(`group_chat_loop.rs`)由一个 **moderator**(主持人)agent 协调多个 **参与者** agent 轮流发言:moderator 用 `nominate_speaker` 点名下一发言者(**唯一调度机制**,`nominate_speaker` / `end_discussion` 均 moderator-only,参与者不得调用),参与者发言后回到 moderator,moderator 调 `end_discussion(summary)` 终止并给出全场总结。每条 message 落库带 `speaker` 列(参与者标识),前端按 speaker 渲染独立气泡 + 实时发言人 chip,`end_discussion` 的 summary 由 `DiscussionSummaryCard.vue` 渲染为"讨论总结"卡片。
>
> **上下文构建 — per-role history 隔离(08-07)**:每个角色从共享 DB transcript 经 `role_history(full, current_role)` 组装**独立** LLM 上下文:只保留自己的 assistant 行(verbatim,含 thinking + signature),他人发言改写为 `role:user`(归属由 wire 层插 `@name:` 前缀),他人 thinking / 工具对(工具结果不共享)与 moderator 仲裁对被剥离。取代早期 `participant_view`(多身份 assistant 共存 = 同模型串台根因)。
>
> **工具白名单(08-07)**:moderator 与参与者只拿调研类工具 `read_file`/`grep`/`glob`/`list_dir`/`web_fetch`,moderator 额外持有仲裁工具(白名单取代黑名单,新增 builtin 工具默认不进群聊);参与者 `max_turns=20`(可取材实证)、moderator `max_turns=1`。
>
> **入口与事件(08-04~08-07)**:入口持久化去重 + 参与者身份护栏(防 LLM 自名开头)+ 终止/发言人事件 + 逐轮流式 + 人类抢占插话;moderator 未调 `nominate_speaker` 时**重试 moderator turn**(08-06 废弃 round-robin 机械派人)+ wire 层孤儿 tool_use 自愈;编排器静默路径变可见 `Done{stop_reason}` 事件,非终态挂 notice 不 finalize;identity_contract 契约测试守身份不变量。Phase 1-4 见 `.trellis/tasks/archive/2026-07/07-29-group-chat/`,08-04~08-07 迭代见 `.trellis/tasks/archive/2026-08/08-0{4,6,7}-group-chat-*`。

---

## 2. Harness 设计:从用户输入到文件变更的 16 道关卡

这一节把架构图展开成**具体的请求生命周期**。理解了这 16 关,就理解了 harness engineering 在做什么。

> **演进说明**:早期版本是 14 道关卡,daemon 化(见 [§4](#4-决策agent-daemon-化为多-channel-接入铺路))和资源加载系统(见 [TECH.md §5](./TECH.md#5-决策skill--memory--role-共用-frontmatter-loader))扩展后变成 16 关。

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
  ├─ 权限/鉴权:本地单用户场景目前无多用户鉴权(远程访问加固是后续项)
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
- **5a-5c 详解见 [BACKLOG.md §3 多层 Memory](./BACKLOG.md#3-多层-memory-与约束) 和 [BACKLOG.md §2 Skill](./BACKLOG.md#2-agent-skill-系统) 和 [BACKLOG.md §4.1 Role](./BACKLOG.md#41-多角色role)**

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
- **交错思考(2026-07-23/24 落地)**:contentBlocks 按**真实流序**交错落库与渲染(thinking / text / tool_use 时间轴,run 分组),而非 Anthropic 的"text 全先于 tool_use"分组顺序 —— 后端保留 BlockState 时间戳序,前端 run 分组 + contentBlocks 时间轴渲染,修复 Anthropic thinking 块在中途消失 + 真工具穿插。设计见 [docs/INTERLEAVED-THINKING-DESIGN.md](./INTERLEAVED-THINKING-DESIGN.md)。

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

**详见** [tool-contract.md §"⑨ 关 Permission Decision Layer"](./../trellis/spec/backend/tool-contract.md) +
[llm-contract.md §"Per-Session Mode + ⑨ 关 Permission Layer"](./../trellis/spec/backend/llm-contract.md)。

**子步骤 8b — 内容类型分发**:
| LLM 返回          | 走向                                  |
|-------------------|---------------------------------------|
| 纯 text           | 直接到 ⑭ 走 ChatToken                |
| tool_use          | 进入 ⑨ 权限检查(5-tier) → ⑩ 执行             |
| 混合(text + tool) | text 到 ⑭,tool 进 ⑨                  |
| **ui_render**(新) | 到 ⑭ 走 UiCard(详见 [BACKLOG §5](./BACKLOG.md#5-生成式-ui-开关)) |

- **关卡点**:Mode 提前拦截(Plan 模式不能进 ⑨)、ui_render 跟 tool_use 区分开
- **风险**:Mode 误判 → LLM 收到 "Plan 模式下不能执行",但它应该用 Plan 模式思考再用 Chat 模式执行
- **详见 [BACKLOG.md §4.2 多模式](./BACKLOG.md#42-多模式mode)**

#### ⑨ Tool 权限检查(关键关卡,A2 + B7 落地,re-grill 2026-06-13,**已实施**)

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

**详见** [tool-contract.md §"Scenario: Path-based Permission Layer"](./../trellis/spec/backend/tool-contract.md) +
[project-cwd-boundary.md §6 "is_within_root"](./../trellis/spec/backend/project-cwd-boundary.md) +
[docs/_reviews/REVIEW-a2-b7-regrill-path-based-2026-06-13.md](./_reviews/REVIEW-a2-b7-regrill-path-based-2026-06-13.md) +
[IMPLEMENTATION.md §4 "2026-06-13 Re-grill ADR"](./IMPLEMENTATION.md)。

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
    "use_skill"   => SkillCache 取 SKILL.md 正文 → tool_result 回填(L1,2026-06-18 落地,详见 [IMPLEMENTATION §4](./IMPLEMENTATION.md#4-决策日志))
    "use_memory"  => 读 / 写 runtime memory(详见 [BACKLOG §3](./BACKLOG.md#3-多层-memory-与约束))
    "use_ui"      => 构造 UiCard 走 ⑭ 分支(详见 [BACKLOG §5](./BACKLOG.md#5-生成式-ui-开关))
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
- **高频事件**(`chat-event`，payload 判别):`delta`(token)、`start`、`done`、`error`
  - 流式 token 频率高,走单 listener + payload.type 分发,减少 listener 注册开销
- **低频事件**(独立事件名):`tool:call`、`tool:result`、`permission:ask`、`ui:render`
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
- **Phase 1 范围**:4 种 primitive(button / selector / diff / code_block),详见 [BACKLOG §5](./BACKLOG.md#5-生成式-ui-开关)
- **交错思考渲染(2026-07-23/24)**:前端按 run 分组 + contentBlocks 时间轴交错渲染(thinking/text/tool_use 按到达序),与 ⑦ 后端落库的真实流序对齐,见 [docs/INTERLEAVED-THINKING-DESIGN.md](./INTERLEAVED-THINKING-DESIGN.md)。

#### ⑮ daemon 输出(HttpSseSink / Tauri event → client)

```
对每个 OutgoingMessage:
  ├─ 默认(daemon 模式):HttpSseSink(daemon/sse.rs)广播到 /api/v1/stream SSE
  │    └─ 前端 transport.listen 按 request_id 路由到对应 session 的 streamController
  ├─ 逃生(Full 模式):Tauri app.emit 事件,前端 listen
  ├─ 限速:防止 QPS 过高(GUI 不限;远程/未来多 client 场景预留)
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
  └─ 触发云端同步(若开启,详见 [BACKLOG §7](./BACKLOG.md#7-云端状态同步))
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

> 16 关卡在 MVP 阶段和打磨阶段分别在哪落地,详见 [ROADMAP.md §1](./ROADMAP.md#1-已实施mvp-主体--路线图外完成) + 各阶段的技术细节分散在 [IMPLEMENTATION §4 决策日志](./IMPLEMENTATION.md#4-决策日志) 对应日期条目。本节不再维护细粒度"步骤 N → 关卡"映射表(随 V2 路线图重排已过时)。

### 2.5 横切关注点:16 关之外但必做的事

关卡图是纵向链路,但很多**横切关注点**贯穿多个关卡,容易被遗漏。下面列出 8 个,每个都标出"在哪个关卡被处理 / 关键设计点"。

#### 2.5.1 用户中途取消(CancellationToken)

- **触发场景**:用户在 LLM 流式输出中点 stop,或 long-running tool 内中断
- **位置**:② Tauri IPC 之后立刻建 `CancellationToken`;⑩ tool 执行内 `tokio::select!` 监听
- **关键设计**:取消不立即终止 LLM 请求,而是把"取消"事件本身作为 tool_result 回传(给 LLM 一次自我收敛的机会);只有用户二次取消才真终止
- **`shell` 进程组杀整组**:`shell` tool 子进程以 `process_group(0)` 启动,PGID == sh PID;cancel / timeout 时 `kill(-pgid, SIGKILL)` 杀整组,清理 `&` / 管道 / `nohup` 产生的孙子进程。Windows 留 P2
- **缺失后果**:用户按 stop 没反应 → 跑光了 token 还在跑 → 信任崩塌
- **当前实现**:MVP 简化决策——单次 cancel 即 emit `Done("cancelled")` 终止,**未实现"二次取消才真终止"语义**;完整 spec + 二次取消实现路径见 `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` §2.1 + [IMPLEMENTATION §4 2026-06-17 ADR](./IMPLEMENTATION.md#4-决策日志)(RULE-A-010 已 closed 2026-06-17 via spec 偏离声明)

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
- **命中动作(软)**:两层都 `tracing::warn!` + 把 hint 文本插入 result message,**不跳过执行、不终止 loop**,MAX_TURNS=200 仍是硬兜底。无 AuditKind 落表
- **完整设计 + 调研**:详见 [IMPLEMENTATION §4 2026-06-24 ADR](./IMPLEMENTATION.md#4-决策日志)

#### 2.5.5 ⑤ Context 超限降级(C3 MVP,2026-06-12 落地,**已实施**)

- **触发**:总 token > `context_window * 0.80`(MVP 阈值,留 0.20 余量给 tiktoken cl100k_base 1-2% 漂移)
- **保护顺序**:
  1. **不动**:`system_prompt` + `role.system_prompt` + 4 层 Memory 合成段
  2. **优先丢**:runtime tool_result(从最老 turn 开始丢)
  3. **次优丢**:老 user / assistant turn(从最老开始丢)
  4. **裁剪目标**:降到 `context_window * 0.50`
- **不变量**:`assistant(tool_use)` + `user(tool_result)` 必须成对丢(API 400 红线);Thinking / RedactedThinking blocks 只能随整 turn 丢;当前 user message、当前 tool_result 不丢
- **不能做**:丢 system prompt / role prompt / 所有 memory
- **MAX_TURNS 兜底**:200(从 20 → 50 → 200 演进,详见 ADR)
- **实现位置**:`app/src-tauri/src/agent/context.rs`(`estimate_messages_tokens` + `compact_messages` + 配对保护)
- **完整设计 + BUG 修复历史**:详见 [IMPLEMENTATION §4 2026-06-12/14/15 ADR](./IMPLEMENTATION.md#4-决策日志)(RULE-A-001/002/006 已闭环)

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
- **17 类 AuditKind + 完整 schema + payload wire shape + UI 渲染细节**:见 [IMPLEMENTATION §4 2026-06-13/14 ADR](./IMPLEMENTATION.md#4-决策日志) + `app/src-tauri/src/agent/permissions/audit.rs`

#### 2.5.9 ⑩ 并行 tool 执行(L2 MVP,2026-06-19 落地,**已实施**)

- **触发**:单 turn 内 LLM 返回的**所有** tool_use ∈ `{read_file, grep, glob, list_dir, use_skill}`(纯本地只读 + 全静默 Allow)**且**任一 path 工具的 `path` 解析后 ∈ project root → 并发执行;否则(含 write_file/edit_file/shell/update_checklist/web_fetch 或 path-outside-root)→ 整批串行
- **判定**:`is_parallel_eligible(&tool_calls, &permission_ctx.cwd)`(纯谓词)
- **实现**:`FuturesUnordered` + `permissions::check` → `execute_tool(token.clone())` → cancel 检查 → audit → `emit_tool_result`;`result_slots[i]` 按 tool_use **原始 index** 回填
- **不变量**:
  - 多 tool_result **单消息打包**(parallel-tool-use 红线:拆消息会让 LLM "学会"避免并行)
  - `web_fetch` 虽只读但 Tier 4 默认 `emit ask`,MVP 排除(走串行,保留逐个 ask UX)
  - 共享状态安全:并发集合无 shell(改 cwd)/edit_file(写 read_guard)→ 无写冲突;`PermissionStore`/`SkillCache`/`ReadGuard` 都是 `Arc<Mutex/RwLock>`,多 task 并发 read 安全
  - cancel:并发不 `break`,等所有 task 完成或被 cancel;`execute_tool` 内 `tokio::select!` 各 task 独立响应 cancel
- **完整设计 + RULE-A-013 path-in-root 收口 + 调研引用**:见 [IMPLEMENTATION §4 2026-06-19 ADR](./IMPLEMENTATION.md#4-决策日志) + [`spikes/2026-06-19-async-parallel-tool-research.md`](./spikes/2026-06-19-async-parallel-tool-research.md)

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

具体契约 + LLM 透明度(7 工具 cwd 字段 + system event 注入)见 `.trellis/tasks/06-07-step-4-follow-up-session-worktree-attach-detach-delete-git/prd.md`。

---

## 4. 决策:Agent Daemon 化(已实施,2026-07)

**核心变更**:agent core 从 Tauri 进程内拆出,变成独立 `everlasting-daemon` 进程。Tauri GUI 降级为瘦客户端(Thin 模式),与浏览器 client 并列,都经同源 HTTP/SSE 连同一 daemon。

> 这条决策的完整动机与编排见 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md);决策档案(为什么 axum / 为什么 sidecar / 为什么默认 httpTransport)见 [IMPLEMENTATION.md §4](./IMPLEMENTATION.md)。本节只讲架构本身。

**为什么必须**:
- 远程/浏览器访问 —— agent core 要能脱离 Tauri webview 被浏览器触达(daemon 用 ServeDir 同源服务 SPA)
- 多 client 共用同一 agent core —— 桌面 GUI + 纯浏览器连同一 daemon,共享 session 状态(早期设想的飞书/CLI 多 channel 是后续项,见 [§5](#5-决策channel-adapter-抽象早期设想未实施))
- agent core 与 GUI 解耦 —— GUI 重启不影响 daemon 里的长跑 session(Thin 模式 GUI 不持有任何状态)

**架构影响(实际落地)**:
- 新增 `src-tauri/src/daemon/` 目录(`server.rs` axum router + `sse.rs` HttpSseSink + `error.rs` + `routes/` 19 个路由域文件)+ `src-tauri/src/bin/everlasting-daemon.rs`(daemon bin 入口)+ `src-tauri/src/sidecar.rs`(GUI 侧 spawn + 生命周期管理)
- 前端新增 `app/src/transport/` 抽象层(httpTransport 默认 / tauriTransport `?transport=tauri` 逃生)
- 通信:**同源 HTTP + SSE**(axum POST `/api/v1/*` + `/api/v1/stream` SSE),daemon 用 `tower-http::ServeDir` 同源服务 `dist/` SPA。**不是** Unix socket / Named pipe / WebSocket —— 早期设想的本地 IPC 已被同源 HTTP 取代
- 进程管理:GUI 经 `tauri-plugin-shell` spawn daemon 为 sidecar(`sidecar.rs::spawn_and_manage`),`RunEvent::Exit` 钩子 kill sidecar(无孤儿进程);裸跑/浏览器模式用 `scripts/daemon.sh`(start/bg/stop/restart/status/logs,PID 文件 + graceful shutdown)。**不用** systemd/pm2 —— sidecar 模式由 GUI 托管,裸跑模式由脚本托管
- 91 个原 `#[tauri::command]` handler 镜像为 REST 路由(Q0 决策:同 handler 双暴露 IPC + HTTP,代码复用)

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
- `FeishuChannel` — 走飞书 WebSocket(待 [BACKLOG.md §6](./BACKLOG.md#6-im-通道飞书) 实施)
- `CliChannel` — 走 stdin/stdout(待后期实施)

**实际落地的替代**:axum HTTP `/api/v1/*` 路由 + `HttpSseSink` SSE 广播(`daemon/server.rs` + `daemon/sse.rs`)。前端经 `httpTransport`(fetch + EventSource)统一接入;Full 模式逃生经 `tauriTransport`(Tauri event)。"多入口"的诉求目前由"多 client 连同一 HTTP daemon"(GUI + 浏览器)满足,不需要 trait。

**当初设想的好处(供未来重新评估参考)**:
- 新增 channel 不用改 agent core,只实现 trait
- 跨 channel 行为可统一(限速、消息合并、状态同步)
- 测试友好(mock 一个 channel 就能跑 agent)

**当初的协议约束**(仍适用于未来任何多入口方案):
- 所有 message 必须可序列化到 JSON(明文),不依赖 Rust 特定类型
- 传输层无关:HTTP / WSS 都能承载同一份 JSON

**应验的风险**:
- 抽象过早:落地时只有 1 个真实入口(GUI/浏览器都走 HTTP),trait 被判 overdesign,直接用 axum 路由 + SSE。这条保留为"下次想做飞书/CLI 时再决定要不要抽 trait"的备忘。
