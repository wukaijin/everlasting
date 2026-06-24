# ARCHITECTURE — 架构设计

> Everlasting 的"整体怎么搭、关键流程怎么走"。包括系统架构图、请求生命周期的 16 道关卡、以及核心架构决策。
> 需求见 [DESIGN.md](./DESIGN.md),技术选型见 [TECH.md](./TECH.md),决策档案见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),技术路线图见 [ROADMAP.md](./ROADMAP.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 系统架构

> ⚠️ **当前状态 vs 目标态**:
> - **当前 MVP(2026-06-07)**:agent core 跑在 Tauri 进程**内**(`app/src-tauri/src/lib.rs` 的 `chat` 命令 spawn tokio 任务,直接 `reqwest` + 手写 SSE)。**未做**进程拆分,无独立 daemon。
> - **目标态**(本节图示,见 [§4 决策:Agent Daemon 化](#4-决策agent-daemon-化-为多-channel-接入铺路)):agent core 拆出独立 daemon 进程,Tauri 降级为 GUI client,跟飞书 client 并列。触发条件:BACKLOG §6 飞书 channel 决定实施时(详见 §2.8 占位)。
> - 后续小节(§2 16 关卡 / 通道抽象)用"目标态"语言描述;当前 MVP 实际是 in-process,channel = Tauri event emit 走单进程。

### 1.1 进程拓扑(daemon 化后,目标态)

```
┌─────────────────────────────────────────────────────────┐
│              Tauri GUI Process (Client)                   │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐    │
│  │ 项目列表  │ │ Session  │ │ Chat UI  │ │ Diff /   │    │
│  │          │ │ 列表     │ │ (流式)   │ │ 终端     │    │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘    │
│         ↕ Tauri Events (emit/listen) ↕ Tauri Commands   │
└──────────┬──────────────────────────────────────────────┘
           │ IPC: Unix socket / Named pipe / WebSocket
┌──────────▼──────────────────────────────────────────────┐
│              Agent Daemon Process (tokio)                 │
│  ┌─────────────────────────────────────────────────┐    │
│  │  Channel Router                                  │    │
│  │  ├ TauriGuiChannel (Tauri event)                 │    │
│  │  ├ FeishuChannel  (飞书 WebSocket,可选)          │    │
│  │  └ CliChannel     (stdin/stdout,可选用作调试)   │    │
│  └─────────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────────┐    │
│  │  Session Manager                                  │    │
│  │  - session 生命周期 (create / resume / archive)  │    │
│  │  - 消息历史持久化                                  │    │
│  │  - worktree 隔离 (git2-rs)                       │    │
│  └─────────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────────┐    │
│  │  Project Manager                                 │    │
│  │  - 多项目注册表 (SQLite)                          │    │
│  │  - 每个 project: path / git remote / config      │    │
│  └─────────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────────┐    │
│  │  agent core  ← 核心,自研                      │    │
│  │  ┌─────────────────────────────────────────┐    │    │
│  │  │  LLM Client (自研 Provider trait)        │    │    │
│  │  │  - Anthropic / OpenAI 自研 adapter        │    │    │
│  │  │  - 手写 SSE 状态机                         │    │    │
│  │  └─────────────────────────────────────────┘    │    │
│  │  ┌─────────────────────────────────────────┐    │    │
│  │  │  Tool Registry                           │    │    │
│  │  │  - read_file / write_file / edit_file   │    │    │
│  │  │  - shell / grep / glob / list_dir       │    │    │
│  │  │  - web_fetch (06-12 落地,SSRF 拦截)     │    │    │
│  │  │  - use_skill / use_memory / use_ui      │    │    │
│  │  └─────────────────────────────────────────┘    │    │
│  │  ┌─────────────────────────────────────────┐    │    │
│  │  │  Agent Loop                              │    │    │
│  │  │  - 上下文管理 / 压缩 / 优先级裁剪         │    │    │
│  │  │  - 权限检查 (per-tool, per-mode)         │    │    │
│  │  │  - 失败重试 / 循环检测                    │    │    │
│  │  │  - 事件流 → Channel Router                │    │    │
│  │  └─────────────────────────────────────────┘    │    │
│  └─────────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────────┐    │
│  │  Resource Loaders (共用 frontmatter loader)      │    │
│  │  - Memory loader (每次 LLM 调用前自动加载)        │    │
│  │  - Skill loader (LLM 调 use_skill 时按需加载)   │    │
│  │  - Role loader (session 启动时加载)             │    │
│  │  - Command registry (用户 / 触发)                │    │
│  └─────────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────────┐    │
│  │  Infrastructure                                   │    │
│  │  - SQLite (sqlx) — 元数据 + 消息历史            │    │
│  │  - git2-rs — worktree / diff / commit            │
│  │  - tiktoken-rs — memory token 估算                      │
│  │  - (无 WSL Bridge:Tauri 全跑 WSL 内,无 wslapi)  │
│  └─────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
         ↓ LLM API                  ↓ Local FS / Git
    (Anthropic / OpenAI)         (WSL 内 $HOME/projects)
```

**进程边界说明**:
- **Tauri GUI Process**:只负责渲染 + IPC 转接,无业务逻辑
- **Agent Daemon Process**:跑所有 agent 逻辑,长跑任务不被打断
- **本地 IPC**:Unix socket(Linux/macOS) / Named pipe(Windows)
- **远程接口**:WebSocket(为 [BACKLOG.md §7 云端同步](./BACKLOG.md#7-云端状态同步) 预留)
- **daemon 化动机**:多 client 共用(桌面 + 飞书 + CLI),GUI 重启不打断长跑任务。详见 [§4 决策:Agent Daemon 化](#4-决策agent-daemon-化为多-channel-接入铺路)

### 1.2 关键数据流:用户发一条消息(目标态;当前 MVP 走 in-process 简化版)

> 📌 **当前 MVP 实测路径**:Frontend → `invoke('chat', ...)` → Tauri Rust `chat` 命令(同进程 tokio task)→ `chat_stream_with_tools()` (reqwest + 手写 SSE)→ emit `chat-event` / `tool:call` / `tool:result` → Frontend 单 SSE listener(在 `streamController.ts`,按 `request_id` 路由)→ Pinia store 增量更新。**没** Channel Router / **没** 独立 daemon,功能等价于目标态的 TauriGuiChannel 单通道路径。

```
[1] Frontend (Vue 3)
    用户输入消息 → tauri.invoke('chat', { requestId, messages })

[2] Tauri GUI Process
    收到 invoke → Rust 端 spawn 异步任务处理
    invoke resolve 立即返回("已受理",非"已完成")

[3] Agent Daemon
    Channel Router → TauriGuiChannel 收到消息
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
            TextDelta(t)  => TauriGuiChannel.send(ChatToken(t)),
            ToolUse(...)  => 权限检查(per-mode) → 执行 → 构造 tool_result 回填,
            UiRender(...) => TauriGuiChannel.send(UiCard(...)),
          }
        }
      }
      TauriGuiChannel.send(ChatDone)

[4] Frontend
    listen("chat-event") → payload.type 分发:
      "delta"  → 追加 token 到 UI
      "done"   → 解禁输入框
      "error"  → 显示错误提示
    (后续步骤 2+ 会加 "tool:call" / "tool:result" / "permission:ask")
```

### 1.3 关键数据流:session 切换(当前 MVP)

> 📌 **当前 MVP 实测路径**:`switchSession(id)` → `chatStore` 委托 `streamController.ensureLoaded(id)` → LRU 命中则从 `messagesBySession` Map 拿;未命中则 `invoke('load_session', { sessionId })` 从 SQLite 读 → 写入 Map → `currentSessionId.value = id` → `currentCwd` 更新 → UI 重新渲染。**前 session 的 in-flight SSE 流不受影响**(流指示器在 SessionList 蓝点继续 pulse 直到 `done` 到达)。详细架构见 `.trellis/spec/frontend/state-management.md` §"Stream Controller Pattern"。

```
[1] User clicks project A → session B
[2] Frontend: tauri.invoke('load_session', { sessionId: B })
[3] Tauri backend: 从 SQLite 读 messages → 返回 SessionSnapshot
```

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
   ② Tauri IPC ─────── 拒
           ↓
   ③ Channel 入口(daemon)
       │  ├ 消息去重(client_msg_id)
       │  └ Channel Router 路由
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

#### ① 前端校验(Vue 3)

```
输入框 → onSend(prompt)
  ├─ 非空?截断超长文本?
  ├─ 是否有未完成的 tool call?(防双发)
  └─ 当前 session 状态是否 idle?
```

- **关卡点**:空消息、过长输入、并发请求、session 锁定
- **失败后果**:UI 拦截,不发请求

#### ② Tauri IPC 边界

```ts
await invoke("chat", { requestId, messages })
```

```
  ├─ 参数反序列化(JSON → Rust struct)
  ├─ 命令是否在白名单?(Tauri capability 限制)
  ├─ rate limit?(每 session 每分钟 N 条)
  └─ spawn 异步任务处理 LLM stream
       └─ invoke resolve 立即返回("已受理")
```

- **关卡点**:参数类型校验、Tauri 2 capability 权限(默认拒绝)、简单限流、IPC 转发
- **失败后果**:返回错误,前端 toast 提示
- **重要**:invoke resolve **不代表** "已处理",只代表"已转发到 daemon"。结果走 ⑮ 通道回来

#### ③ Channel 入口(daemon 接收)

```
Channel Router:
  ├─ 收到 IncomingMessage { channel, user_id, content, client_msg_id, attachments... }
  ├─ 去重:同一个 client_msg_id 短时间内重复 → 丢弃(防网络重发)
  ├─ 权限:这个 channel 的用户有权限触发 agent 吗?
  │    └─ 否 → 拒绝,回 channel.send("无权访问")
  └─ 路由:按 channel 选对应的 Session(飞书的 session 跟 GUI 的 session 可能是不同的)
```

- **关卡点**:消息去重、用户鉴权、session 路由
- **失败后果**:静默丢弃重复 / 显式拒绝无权
- **设计动机**:见 [§4 决策:Agent Daemon 化](#4-决策agent-daemon-化为多-channel-接入铺路)

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

#### ⑮ Channel 输出(daemon → client)

```
对每个 OutgoingMessage:
  ├─ 找到对应的 IncomingMessage 的 channel
  ├─ 按 channel 能力做适配:
  │    ├─ TauriGuiChannel: emit 事件,前端 listen
  │    ├─ FeishuChannel: 发消息 / patch 卡片
  │    └─ CliChannel: stdout
  ├─ 限速:防止 QPS 过高(飞书 5/秒,GUI 不限)
  └─ 消息合并:相邻 token 合并(50ms 内多条合并成一条)
```

- **关卡点**:channel 能力适配、限速、消息合并
- **新增** (对比原 14 关):老版本 token 是直接 `app.emit`,daemon 化后必须经 channel 路由
- **设计动机**:见 [§5 决策:Channel Adapter 抽象](#5-决策channel-adapter-抽象为多入口铺路)

#### ⑯ 结束 / 解禁 / 统计

```
agent loop 结束(text-only response or max_turns reached):
  ├─ channel.send(ChatDone { usage, duration })
  ├─ 更新 session.last_active
  ├─ 解禁前端输入框(对 TauriGuiChannel 走 emit;对 FeishuChannel 不需要)
  ├─ 更新 token 用量统计(进 SQLite,给用量分析用)
  └─ 触发云端同步(若开启,详见 [BACKLOG §7](./BACKLOG.md#7-云端状态同步))
```

- **关卡点**:解禁只对 GUI 有意义(飞书/CLI 没有"解禁"概念)、云端同步是可选副作用
- **新增**(对比原 14 关):云端同步钩子,不动 LLM 流程

### 2.3 关键洞察(为什么 harness 难)

1. **关卡之间没有清晰边界** —— ⑨ 权限检查可能在 ⑩ 内部做,也可能在外层。架构选择决定了可测试性
2. **错误传播方向** —— 大部分错误要**回传给 LLM 让它自纠**,不是直接终止。这就是为什么"agent"和"普通脚本"是两种东西
3. **状态分散** —— session 状态在 DB、context 在内存、worktree 在磁盘、文件锁在 OS、Channel 在另一个进程。要随时能重建
4. **token 预算是命门** —— ⑤ 步的 context 构造决定了你的 agent 能不能干长活,所有其他关卡都是"配套"
5. **用户信任链** —— ⑨ 是唯一用户能"中途喊停"的地方。这一步做错,用户就跑光了
6. **(daemon 化后新增)Channel 是状态边界** —— ⑬ 循环检测或 ⑯ 统计在哪做,影响能不能跨 client 共享。daemon 进程是天然的中心化点
7. **(资源加载后新增)5a/5b/5c 的顺序** —— 错一个就 bug:Memory 在 Role 之前 vs 之后?Skill 描述在 Memory 之前还是之后?每改一次顺序,行为微妙变化

### 2.4 实施映射

> 16 关卡在 MVP 阶段和打磨阶段分别在哪落地,详见 [ROADMAP.md §1](./ROADMAP.md#1-已实施mvp-主体--路线图外完成) + 各阶段的技术细节分散在 [IMPLEMENTATION §4 决策日志](./IMPLEMENTATION.md#4-决策日志) 对应日期条目。本节不再维护细粒度"步骤 N → 关卡"映射表(随 V2 路线图重排已过时)。

### 2.5 横切关注点:16 关之外但必做的事

关卡图是纵向链路,但很多**横切关注点**贯穿多个关卡,容易被遗漏。下面列出 8 个,每个都标出"在哪个关卡被处理 / 关键设计点"。

#### 2.5.1 用户中途取消(CancellationToken)

- **触发场景**:用户在 LLM 流式输出中点 stop,或 long-running tool 内中断
- **位置**:② Tauri IPC 之后立刻建 `CancellationToken`;⑩ tool 执行内 `tokio::select!` 监听
- **关键设计**:取消**不立即终止 LLM 请求**,而是把"取消"事件本身作为 tool_result 回传(给 LLM 一次自我收敛的机会);只有用户二次取消才真终止
- **`shell` 进程组杀整组**(RULE-E-002,2026-06-14):`shell` tool 的子进程以 `process_group(0)` 启动,PGID == sh PID;cancel / timeout 时 `kill(-pgid, SIGKILL)` 杀整组,清理 `&` / 管道 / `nohup` 产生的孙子进程,不再留孤儿。Windows 留 P2
- **缺失后果**:用户按 stop 没反应 → 跑光了 token 还在跑 → 信任崩塌
- **已知偏离**(RULE-A-010,2026-06-17):当前实现单次 cancel 即 emit `Done("cancelled")` 终止,**未实现"二次取消才真终止"语义**。MVP 简化决策:不走"取消→tool_result 回传 LLM→二次 cancel 才真终止"链路,而是"一次 cancel = 立即终止"。理由:(1) tool 取消窗口短,LLM 自我收敛窗口需要再发一轮 LLM 调用,延迟 + 成本不一定划算;(2) 单用户桌面应用场景下,误点 stop 的概率极低,二次取消 UX 增加 friction 而价值有限。完整 spec 见 `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` §2.1 + DEBT.md §RULE-A-010 (已 closed 2026-06-17 via spec 偏离声明 + ADR `docs/IMPLEMENTATION.md §4` 2026-06-17 "D3 完成")。若未来要补二次取消语义,实现路径:agent loop 的 tool 取消分支 + cancel check 之间加一个 "已 cancel 过 N 次" 状态机,N==1 时构造 synthetic `tool_result` 回填 LLM 续流,N==2 才 emit `Done("cancelled")`。

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

- **分级触发**(取代早期单一 `Jaccard > 0.9` —— 单一阈值无法适配短/长 input):
  - **Level 1 精确签名硬触发**(`HARD_WINDOW=3`):窗口内连续 3 次归一化签名完全相同 → 零误报抓真死循环(read/grep/shell 同输入)。per-tool 签名:`read_file`/`write_file`/`list_dir`=path,`grep`/`glob`=pattern+path,**`edit_file`=path+old_string**(含 old_string 才不误判正当的同文件多块编辑),`shell`/`run_background_shell`=command,其余 fallback `name+canonical(input)`
  - **Level 2 Jaccard 软提示**(`SOFT_WINDOW=5`/`SOFT_THRESHOLD=0.85`):窗口内 ≥2 对 token-set Jaccard > 0.85 → 容忍近重复(主要是 shell 长命令)
- **token 切分**:纯 Rust `split_whitespace` + trim 首尾标点(CLI flag `--` 剥离、路径保留),**不复用** `memory::tokens::count_tokens`(tiktoken:async Mutex + CJK 切碎 + subword 噪音)
- **命中动作(软,§2.5.4「不强制打断」原意)**:两层都 `tracing::warn!` + 把 hint 文本作为 `ContentBlock::Text` 插到 result message 的 `result_blocks[0]`,LLM 下一轮在 tool_results 前看到提示;**不跳过执行、不终止 loop**,MAX_TURNS=200 仍是硬兜底。无 AuditKind 落表(见 §2.5.8)
- **实现位置**:`app/src-tauri/src/agent/loop_detection.rs`(纯函数 `detect` / `LoopVerdict` / `signature_of`)+ `chat_loop.rs` ⑬ 关卡(tool_calls 收集后更新窗口 + detect;result_blocks 构造后注入 hint)。worker nested run_chat_loop 自动继承
- **完整 PRD**:[`.trellis/tasks/06-24-c2-loop-detection/prd.md`](../../.trellis/tasks/06-24-c2-loop-detection/prd.md) + 调研 [`research/similarity-algorithm-and-tokenizer.md`](../../.trellis/tasks/06-24-c2-loop-detection/research/similarity-algorithm-and-tokenizer.md)

#### 2.5.5 ⑤ Context 超限降级(C3 MVP,2026-06-12 落地,**已实施**)

- **触发**:总 token > `context_window * 0.80`(MVP 阈值,留 0.20 余量给 tiktoken cl100k_base 1-2% 漂移)
- **保护顺序**(先保护什么):
  1. **不动**:`system_prompt` + `role.system_prompt` + 4 层 Memory 合成段(B5 `memory_synthetic` + `assistant_ack` 永远不被裁剪)
  2. **优先丢**:runtime tool_result(从最老 turn 开始丢)
  3. **次优丢**:老 user / assistant turn(从最老开始丢)
  4. **裁剪目标**:降到 `context_window * 0.50`
  5. **未来手段**(未实施):LLM 摘要中间消息(贵且慢,留给 C3-v2)
- **配对保护**:`assistant(tool_use)` + `user(tool_result)` 必须成对丢,避免 API 400
- **不丢**:Thinking / RedactedThinking blocks(只随整 turn 丢,不会"丢一半";signature 对不上会 400)
- **不丢**:当前 user message、当前 tool_result
- **不能做**:丢 system prompt、丢 role prompt、丢所有 memory
- **MAX_TURNS 兜底**:20 → 50 → 200(2026-06-12 C3 PR1 改 20→50;2026-06-22 再 50→200 覆盖长 worker;正常 token 预算会先触发,200 轮兜底覆盖极端 case)
- **实现位置**:`app/src-tauri/src/agent/context.rs`(`estimate_messages_tokens` + `compact_messages` + 配对保护 + 优先级算法)
- **完整 PRD**:[.trellis/tasks/archive/2026-06/06-12-c3-context-token/prd.md](./../trellis/tasks/archive/2026-06/06-12-c3-context-token/prd.md)
- **未实施**(MVP 留口子):前端"context compressed at turn N"UI 标记(PR2)+ compressed_out DB 列(C4 覆盖)+ LLM summarization(C3-v2)
- **BUG 修复(2026-06-14,RULE-A-001 + RULE-A-002)**:① `group_droppable_turns` 的 orphan 分支改 skip(隐式保护 tool_use-bearing assistant),不再 singleton drop 留下孤立 tool_use/tool_result(撞 Anthropic 400);② `CompactResult` 加 `degradation: DegradationKind`,全 droppable 丢完仍超窗时返回 `StillOver { tokens_after, target }`,agent loop(chat.rs + chat_loop.rs 副本同步)emit `ChatEvent::Error { InvalidRequest }` + 早返回,不静默发超窗 prompt 撞 `prompt is too long`。详见 [DEBT.md](./../trellis/reviews/DEBT.md) RULE-A-001/002。
- **Agent loop body 统一(2026-06-15,RULE-A-006 闭环)**:`chat` Tauri 命令的 spawn 闭包体改为单次 `chat_loop::run_chat_loop(...)` 调用,production 与 test 共享同一函数体(无副本)。所有四个 emit 通道(chat-event / tool:call / tool:result / permission:ask)统一走 `dyn ChatEventSink` trait(生产接 `AppHandleSink`,测试接 `MockEmitter`)。9 个 `agent_loop_*` 集成测试现覆盖 production 真实路径,改 agent loop body = 改 1 处,无 drift hazard。详见 [DEBT.md](./../trellis/reviews/DEBT.md) RULE-A-006。

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

**每次记录**:
- ⑨ 权限决策(10 类 `AuditKind` 枚举,见下)
- ⑩ tool 执行(C4 PR1, 2026-06-14 落表:tool_name, tool_input, duration_ms, exit_code — `record_tool_executed_audit` 在 agent loop tool 执行完成处写)
- ⑬ 循环检测触发(只 `tracing::warn!`,**未落表** — 收益低,C4 OOS;**C2 已实施 2026-06-24**,见 §2.5.4)
- ⑮ channel 路由(从哪个 channel 进,从哪个 channel 出;**未落表** — daemon 化前是单 client,无路由可记)

**存储**:`session_audit_events` 表(SQLite),schema:
```sql
CREATE TABLE session_audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    ts TEXT NOT NULL DEFAULT (datetime('now')),
    kind TEXT NOT NULL,           -- AuditKind 字符串
    payload_json TEXT,            -- 统一 JSON 结构
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
CREATE INDEX idx_session_audit_events_session_ts
    ON session_audit_events(session_id, ts DESC);
```

**11 类 AuditKind**(`agent::permissions::AuditKind`,C4 PR1 加 `ToolExecuted`):
| Kind | 触发条件 |
|---|---|
| `tool_denied` | Tier 2 命中 + Tier 3 user deny + Tier 3 sender dropped |
| `tool_allowed` | Tier 3 AllowOnce / Tier 3 "始终允许" 命中 / Tier 5 默认 |
| `tool_permission_ask` | Tier 3 emit `permission:ask` |
| `permission_granted` | Tier 3 "始终允许" → 写 `session_tool_permissions` |
| `permission_timeout` | Tier 3 120s 超时 |
| `tool_denied_yolo` | Tier 2 命中 + mode = Yolo(跟普通 `tool_denied` 区分) |
| `mode_changed` | `set_session_mode` 调用 |
| `yolo_entered` / `yolo_exited` | Mode 在 Yolo 之间切换 |
| `request_cancelled` | C1 cancel 触发(tier 3 await 被 cancel 打断) |
| `tool_executed`(C4 PR1, 2026-06-14)| ⑩ tool 执行完成(agent loop 调 `record_tool_executed_audit`) |

**统一 payload JSON 结构** — 按 kind 分发:

⑨ 关类(7 种走 `permissions::record_audit`,统一 `{tool_name, tool_input, reason?, mode, critical?}`):
```json
{
  "tool_name": "shell",
  "tool_input": { "command": "ls -la" },
  "reason": "matches denylist: rm -rf /",
  "mode": "edit",
  "critical": true
}
```

⑩ `tool_executed` payload(C4 PR1,独立 helper `record_tool_executed_audit`):
```json
{
  "tool_name": "shell",
  "tool_input": { "command": "cargo build" },
  "duration_ms": 3210,
  "exit_code": 0
}
```
`exit_code` 是 `Option<i32>`:`null` = 该 tool 无 exit code(read_file / write_file / edit_file / grep / glob / list_dir / web_fetch);`0` = 成功;`-1` = 被 kill(timeout / cancel);非 0 = 失败。C4 前端按值着色(绿 / 警示 / 红)。

⑯ mode 类(`set_session_mode` 直接 inline 写,不走 `record_audit`):
```json
{
  "prev_mode": "edit",
  "new_mode": "yolo"
}
```

`critical: bool` 字段对前端 `PermissionModal` 的 3px 红左 border + shield-x icon 渲染至关重要;`tool_denied` / `tool_denied_yolo` = `true`,其他 ⑨ 关类 = `false`。

**Audit write 策略**:best-effort,失败 `tracing::warn!` 不报错
(必须保证不破坏 agent loop)。

**UI 查询**(C4 任务,2026-06-14 PR2 已实施):
- Tauri command `list_session_audit_events(session_id)` → `Vec<AuditEventRow>`(`camelCase` wire:`id` / `sessionId` / `ts` / `kind` / `payloadJson: Option<String>`)
- 前端 `useAuditStore` + `<AuditLogModal>`(reka-ui Dialog)挂在 `ChatPanel` header Memory 按钮旁,绑当前 session。顶部 kind 下拉筛选 + "仅 critical" 复选 + 计数 + 刷新;列表按 `ts DESC, id DESC` 排序(秒精度 tie 由 `id DESC` 稳定化),按 kind 分发渲染 reason / duration+exit_code / mode from→to。critical 事件 3px 红左条(复用 PermissionModal 约定)。MVP 全量拉取,无分页 / 无虚拟滚动 / 无实时推送。

**用途**:回看"agent 刚才为啥没做 X"、"那次权限拒绝是不是太严了"、
"哪步最慢"(⑩ `tool_executed` 落表后可用)、Yolo 模式下被静默
拒绝的 hard-kill 命令审计(`tool_denied_yolo` 字段配合
`critical: true`)。

#### 2.5.9 ⑩ 并行 tool 执行(L2 MVP,2026-06-19 落地,**已实施**)

- **触发**:单 turn 内 LLM 返回的**所有** tool_use ∈ `{read_file, grep, glob, list_dir, use_skill}`(纯本地只读 + 全静默 Allow)**且**任一 path 工具的 `path` 解析后 ∈ project root(复用 `projects::boundary::is_within_root`)→ 并发执行;否则(含任意 write_file/edit_file/shell/update_checklist/web_fetch 或 path-outside-root)→ 整批串行(行为同 L2 前)
- **判定**:`is_parallel_eligible(&tool_calls, &permission_ctx.cwd)`(纯谓词,`chat_loop.rs:1486+`,拆分自 `chat_loop.rs`,2026-06-23 抽 `run_subagent` 后行号下移 ~522 → 现 `agent/chat_loop.rs:~964+`)
- **实现**:`FuturesUnordered`,每 task 内 `permissions::check` → `execute_tool(token.clone())` → RULE-A-004 cancel 检查 → audit → `emit_tool_result`;`result_slots[i]` 按 tool_use **原始 index** 回填(不依赖完成时序)
- **不变量**:
  - 多 tool_result 仍**单消息打包**(parallel-tool-use 红线:拆消息会让 Claude "学会"避免并行)
  - RULE-A-004:cancelled tool 不落 `tool_executed` audit(并行下用 `AtomicBool` 广播回主循环 `cancelled` 标志)
  - 共享状态安全:并发集合无 shell(唯一改 `current_ctx.cwd` 者)→ 无 cwd 写冲突;无 edit_file(唯一写 `read_guard` 者)。`PermissionStore`/`SkillCache`/`ReadGuard` 都是 `Arc<Mutex/RwLock>`,多 task 并发 read 安全
  - cancel:并发不 `break`,等所有 task 完成或被 cancel;`execute_tool` 内 `tokio::select!` 各 task 独立响应 cancel
- **Q2 排除 web_fetch**:web_fetch 虽只读但 Tier4 默认 `emit ask`,纳入会引入并发多 modal 问题 → MVP 排除(走串行,保留逐个 ask UX)。
- **RULE-A-013 收口(2026-06-19)**:谓词从"tool name 白名单"升级为"name 白名单 + path-in-root"。任一 path 工具的 `path` 解析到 project root 之外(`is_within_root` 返回 false)→ 谓词返回 false → 整批拉回串行,保留"并发集合**绝对** silent Allow"不变量;`use_skill` 无 path arg 不参与 path check(Tier 5 default-allow 永远 silent)。path 解析约定与 `permissions/mod.rs(拆分自 mod.rs,2026-06-23 拆为 8 模块):560-571` 完全一致(absolute → as-is;relative → `root.join(p)`;None/empty → 视作 eligible,沿用 permissions 层"无 path 走 Allow"约定)。`is_within_root` 已有 8 个 boundary 单测覆盖(prefix-trap / nonexistent / empty / 等),不重复测试。详见 `DEBT.md RULE-A-013`(`Status: closed (2026-06-19)`)。
- **流式 UI**:并行下 `emit_tool_result` 按完成时序(乱序)到达前端,但 `streamController.ts` 按 `tool_use_id` 匹配(`Map.get`),DB reload 后按 tool_use 原始顺序 → UI 正确;streaming 期间可能短暂乱序(MVP 接受,Out of Scope 不改前端)
- **实现位置**:`app/src-tauri/src/agent/chat_loop.rs:997-1168`(并行路径,拆分自 `chat_loop.rs`,2026-06-23 抽 `run_subagent` 后行号下移 ~522 → 现 ~475-646)+ `1169+`(串行路径 → 现 ~647+,逐字保留)
- **调研**:[spikes/2026-06-19-async-parallel-tool-research.md](./spikes/2026-06-19-async-parallel-tool-research.md) + [-independent-research.md](./spikes/2026-06-19-async-parallel-tool-independent-research.md);完整 PRD 走 `.trellis/tasks/06-19-l2-parallel-readonly-tool-batch/`;RULE-A-013 follow-up PRD 走 `.trellis/tasks/06-19-l2-followup-rule-a-013-boundary-silent-batch/`

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

## 4. 决策:Agent Daemon 化(为多 channel 接入铺路)

**核心变更**:agent core 从 Tauri 进程内拆出,变成独立 daemon 进程。Tauri 降级为 GUI client,跟飞书 client 并列。

> 这条决策的"全部动机"在 [BACKLOG.md §6 IM 通道(飞书)](./BACKLOG.md#6-im-通道飞书)。本节只讲架构本身。

**为什么必须**:
- 飞书 channel 不能依赖 GUI(用户关 GUI 还想收飞书消息)
- 多个 client 同时连(桌面 GUI + 飞书,可能未来还有 CLI / Web)
- 长跑任务不被打断(GUI 重启不影响 daemon 里的 session)

**架构影响**:
- 新增 `src-tauri/src/daemon.rs`
- 通信:本地用 Unix socket / Named pipe,远程接口预留 WebSocket(为 [BACKLOG.md §7](./BACKLOG.md#7-云端状态同步) 留接口)
- 进程管理:写个简单 supervisor(或后期用 systemd / launchd)
- 与第 5 章 Channel 抽象配合

**自研 daemon,不用 pm2 / supervisord**:进程就一个,行为可预测,systemd unit 几十行就够

---

## 5. 决策:Channel Adapter 抽象(为多入口铺路)

**核心抽象**:
```rust
#[async_trait]
trait Channel: Send + Sync {
    async fn send(&self, msg: OutgoingMessage) -> Result<MessageId>;
    fn subscribe(&self) -> BoxStream<'static, IncomingMessage>;
    fn capabilities(&self) -> ChannelCapabilities;
}
```

**当前实现**:
- `TauriGuiChannel` — 走 Tauri event(✅ 已实现,步骤 1)
- `FeishuChannel` — 走飞书 WebSocket(待 [BACKLOG.md §6](./BACKLOG.md#6-im-通道飞书) 实施)
- `CliChannel` — 走 stdin/stdout(待后期实施)

**好处**:
- 新增 channel 不用改 agent core,只实现 trait
- 跨 channel 行为可统一(限速、消息合并、状态同步)
- 测试友好(mock 一个 channel 就能跑 agent)

**协议约束**(远期 v2 跨设备前置条件):
- 所有 message 必须可序列化到 JSON(明文),不依赖 Rust 特定类型
- Channel 传输层无关:Unix socket / HTTPS / WSS 都能承载同一份 JSON
- 这条不要求 MVP 实现 network channel,只要求 trait 设计不锁传输

**风险**:
- 抽象过早:现在只有 1-2 个 channel,trait 可能 overdesign
- 缓解:trait 只放最小接口,先跑起来,后期按需扩展
