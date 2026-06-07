# ARCHITECTURE — 架构设计

> Everlasting 的"整体怎么搭、关键流程怎么走"。包括系统架构图、请求生命周期的 16 道关卡、以及核心架构决策。
> 需求见 [DESIGN.md](./DESIGN.md),技术选型见 [TECH.md](./TECH.md),实现路径见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

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
│  │  │  LLM Client (基于 rig-core)              │    │    │
│  │  │  - Anthropic / OpenAI / Ollama           │    │    │
│  │  │  - SSE 流式解析                          │    │    │
│  │  └─────────────────────────────────────────┘    │    │
│  │  ┌─────────────────────────────────────────┐    │    │
│  │  │  Tool Registry                           │    │    │
│  │  │  - read_file / write_file / edit_file   │    │    │
│  │  │  - shell / grep / glob                   │    │    │
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
│  │  - notify — 文件监听(后期)                      │
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

**子步骤 8a — Mode 检查**(在解析出 tool_use 之前先做):
```
对当前 session.mode:
  ├─ Chat       → 正常
  ├─ Plan       → 拒绝所有 tool_use,改返回 text "我不能执行,只能分析"
  ├─ Review     → 只允许 read 工具,拒绝 write/edit
  ├─ Background → 同 Chat,但 emit 走 "background:" 前缀,前端不强提示
  └─ Yolo       → 跳过 ⑨ 权限检查,直接执行(危险,默认关)
```

**子步骤 8b — 内容类型分发**:
| LLM 返回          | 走向                                  |
|-------------------|---------------------------------------|
| 纯 text           | 直接到 ⑭ 走 ChatToken                |
| tool_use          | 进入 ⑨ 权限检查 → ⑩ 执行             |
| 混合(text + tool) | text 到 ⑭,tool 进 ⑨                  |
| **ui_render**(新) | 到 ⑭ 走 UiCard(详见 [BACKLOG §5](./BACKLOG.md#5-生成式-ui-开关)) |

- **关卡点**:Mode 提前拦截(Plan 模式不能进 ⑨)、ui_render 跟 tool_use 区分开
- **风险**:Mode 误判 → LLM 收到 "Plan 模式下不能执行",但它应该用 Plan 模式思考再用 Chat 模式执行
- **详见 [BACKLOG.md §4.2 多模式](./BACKLOG.md#42-多模式mode)**

#### ⑨ Tool 权限检查(关键关卡)

```
对每个 tool_use:
  ├─ 工具在 session 白名单?(role.tools.whitelist)
  │    └─ 否 → 拒绝,tool_result = "tool not allowed"
  ├─ 参数 schema 校验(JSON 合法?字段对?)
  │    └─ 否 → 拒绝,告诉 LLM "参数错误,请重试"
  ├─ 路径检查(读写的文件在工作目录内?)
  │    └─ 否 → 拒绝
  ├─ 是否需要用户确认?(per-tool, per-mode 决定)
  │    ├─ 是 → 走 channel 发 "permission:ask",等用户 yes/no
  │    └─ 否 → 放行
  └─ 危险操作?(rm -rf /, git push --force, sudo ...)
       └─ 必须 confirm,默认 deny
```

- **这是 harness 设计中最容易写错也最重要的关卡**
- 常见模式:
  - **静态规则**:路径前缀匹配、命令白名单
  - **动态规则**:根据 LLM 推理结果判断("它在删文件 → 要 confirm")
  - **用户偏好**:某些操作永远 ask,某些永远 allow
- **失败后果**:返回错误给 LLM,LLM 会自我修正 —— 这是 agent 区别于普通脚本的核心
- **跟 Mode 配合**:Plan 模式到这里被 ⑧a 拦了,根本到不了 ⑨;Yolo 模式跳过 ⑨

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
    "use_skill"   => 加载 skill 内容 → 注入 system prompt(详见 [BACKLOG §2](./BACKLOG.md#2-agent-skill-系统))
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

> 📌 **编号语义注意**:本表里的"步骤 N"指 [IMPLEMENTATION.md §2 路线图](./IMPLEMENTATION.md#2-实施路线图)的步骤号。其中"步骤 6"在 IMPLEMENTATION 里是"MCP + 多 Provider",**不是** 路线图外完成的 Anthropic extended thinking(虽然 commit `05671f5` 标题误用了"步骤 6"字样,详见 [IMPLEMENTATION §4 决策日志 2026-06-05 条](./IMPLEMENTATION.md#2026-06-05--路线图状态校对步骤-3a-完成步骤-3b-暂缓extended-thinking-路线图外完成))。

| 关卡     | 最早实现(MVP)                       | 打磨阶段                     |
|----------|--------------------------------------|------------------------------|
| ① ②     | 步骤 1(基础)                         | 步骤 6 完善错误提示          |
| ③       | 步骤 6(随 daemon 化)                 | 步骤 6 完善                  |
| ④       | 步骤 3a(引入 Session)                | 步骤 5 状态机                |
| ⑤       | 步骤 3a(基础)                        | 后续阶段 压缩、摘要           |
| 5a-5c   | 后续阶段(随 BACKLOG 实施)            | 实施对应功能时                |
| ⑥       | 步骤 1(reqwest) / 步骤 3b(rig)       | 步骤 6 多 provider + 重试    |
| ⑦       | 步骤 1                               | 步骤 5 重连、断点续传        |
| ⑧       | 步骤 2                               | BACKLOG §4.2 实施后 ⑧a 启用 |
| ⑧a      | BACKLOG §4.2 实施后                  | 状态机细化                   |
| ⑨       | 步骤 5(基础 allow/deny)              | 细粒度策略(后续)             |
| ⑩       | 步骤 2                               | 步骤 5 PTY、xterm            |
| ⑪       | 步骤 4                               | 步骤 7 自动 commit 策略      |
| ⑫       | 步骤 2                               | —                            |
| ⑬       | 后续阶段                             | 远期                         |
| ⑭       | 步骤 1                               | 步骤 7 用量统计              |
| ⑮       | 步骤 6(随 daemon 化)                 | 限速、合并、调优              |
| ⑯       | 步骤 1                               | 步骤 7 用量统计 + BACKLOG §7 钩子 |

### 2.5 横切关注点:16 关之外但必做的事

关卡图是纵向链路,但很多**横切关注点**贯穿多个关卡,容易被遗漏。下面列出 8 个,每个都标出"在哪个关卡被处理 / 关键设计点"。

#### 2.5.1 用户中途取消(CancellationToken)

- **触发场景**:用户在 LLM 流式输出中点 stop,或 long-running tool 内中断
- **位置**:② Tauri IPC 之后立刻建 `CancellationToken`;⑩ tool 执行内 `tokio::select!` 监听
- **关键设计**:取消**不立即终止 LLM 请求**,而是把"取消"事件本身作为 tool_result 回传(给 LLM 一次自我收敛的机会);只有用户二次取消才真终止
- **缺失后果**:用户按 stop 没反应 → 跑光了 token 还在跑 → 信任崩塌

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

#### 2.5.4 ⑬ 循环检测阈值

- **不能严格相等**:LLM 输出有非确定性,严格 `hash(arg) == hash(prev_arg)` 几乎不命中
- **建议算法**:滑动窗口内 **N=5 次 tool call**,用 **token-set 相似度** (Jaccard) > 0.9 判定近重复
- **命中后**:emit `warning:loop_detected`,LLM 收到 `tool_result = "loop detected, please reconsider"`,**不强制打断**(让 LLM 有机会说明为什么)
- **实现位置**:⑬ 关卡内,需 LLM 端做相似度计算,不能纯 hash

#### 2.5.5 ⑤ Context 超限降级

- **触发**:总 token > 模型 window 的 90%
- **保护顺序**(先保护什么):
  1. **不动**:`system_prompt` + `role.system_prompt` + `4 层 Memory`(agent 行为不能丢)
  2. **优先丢**:runtime tool_result(从最老开始丢)
  3. **次优丢**:老 user / assistant 消息(从最老开始丢)
  4. **最后手段**:LLM 摘要中间消息(贵且慢,只在前 3 步都不够时)
- **不丢**:当前 user message、当前 tool_result
- **不能做**:丢 system prompt、丢 role prompt、丢所有 memory

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

#### 2.5.8 ⑯ 审计日志

- **每次记录**:
  - ⑨ 权限决策(allow / deny / ask-and-result)
  - ⑩ tool 执行(tool_name, args hash, duration, exit_code)
  - ⑬ 循环检测触发次数
  - ⑮ channel 路由(从哪个 channel 进,从哪个 channel 出)
- **存储**:`~/.local/share/everlasting/audit/<date>.jsonl` 每行一个事件
- **用途**:回看"agent 刚才为啥没做 X"、"那次权限拒绝是不是太严了"、"哪步最慢"
- **不能省**:个人项目也必备 — 没 audit 排查问题靠记忆

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
