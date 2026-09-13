# LIFECYCLE — Harness 请求生命周期(16 道关卡)

> **来源**:2026-09-13 从 `ARCHITECTURE.md` §2 拆出(doc-split)。ARCHITECTURE 保留系统架构与核心决策,本文专注"一条消息从用户输入到文件变更"的请求生命周期 walkthrough;章节编号保留原 §2.x,既有"§2.x"引用可直接对照。
> **同源文档**:[ARCHITECTURE.md](./ARCHITECTURE.md)(系统架构 / worktree / daemon 化决策)· [ROADMAP.md](./ROADMAP.md)(落地状态与排期)
> **何时读本文**:调试请求流转 / 追查某关卡行为 / 改 agent loop 横切关注点(压缩 / 预算 / 截断 / 沙盒 / 循环检测)时。

---

## 2. Harness 设计:从用户输入到文件变更的 16 道关卡

这一节把架构图展开成**具体的请求生命周期**。理解了这 16 关,就理解了 harness engineering 在做什么。

> **演进说明**:早期版本是 14 道关卡,daemon 化(见 [§4](./ARCHITECTURE.md#4-决策agent-daemon-化))和资源加载系统(见 [TECH.md §5](./TECH.md#5-决策skill--memory--role-共用-frontmatter-loader))扩展后变成 16 关。

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
- **设计动机**:见 [§4 决策:Agent Daemon 化](./ARCHITECTURE.md#4-决策agent-daemon-化)。早期设想的"多 channel(飞书/CLI)路由"未实施,实际只跑 HTTP(+ Tauri IPC 逃生);多入口抽象降级为 [§5](./ARCHITECTURE.md#5-决策channel-adapter-抽象早期设想未实施) 的历史设想。

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
[docs/_history/reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md](./_history/reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md)。

#### ⑩ Tool 执行

```rust
match tool_call.name {
    "read_file"   => read_file (with cat -n line numbers + ReadGuard.record_read),
    "write_file"  => tokio::fs::write (autoparse parent dir, boundary check),
    "edit_file"   => ReadGuard 3 道 check (read → fresh → match + uniqueness)
                     + 0 匹配报 hint + N>1 报行号 + 写后自动 invalidate,
    "shell"       => spawn_command (5min timeout, C6 统一截断契约——spill 落
                     app_data_dir/outputs/<session>/ + 恢复指引),
    "grep"        => tokio::process::Command::new("rg") spawn, 3 output_modes
                     (files_with_matches | content | count), 500-char line cap,
    "glob"        => globset walk, cap 100, mtime desc,
    "list_dir"    => tokio::fs::read_dir, alphabetical + `/` suffix on dirs,
                     non-recursive,
    "use_skill"   => SkillCache 取 SKILL.md 正文 → tool_result 回填(L1,2026-06-18 落地)
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
- **Bash 落盘**(C6 统一起,2026-08-30):
  - 大输出(spill 模式)落盘到 `app_data_dir/outputs/<session_id>/<uuid>.txt`(spill 目录是 sandbox 可写根之一)
  - Tool result 返回 path + 恢复指引(统一 `<truncated>` 标记;LLM 拿 path 跟 `read_file` 配合,offset/limit 恢复)
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
- **设计动机**:见 [§4 决策:Agent Daemon 化](./ARCHITECTURE.md#4-决策agent-daemon-化)。早期设想的"多 channel 输出适配(飞书/CLI)"未实施,见 [§5](./ARCHITECTURE.md#5-决策channel-adapter-抽象早期设想未实施)。

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

> **18+ 关卡**(原 16 关卡 + C7 tools token / C7D stub / memory digest / C3+ compaction / unified-context-budget 硬卡 / MAX_TURNS 软卡 / C6 输出截断统一 / Sandbox 执行期沙盒 8 个新横切关注点,详见 §2.5.10~16)在 MVP 阶段和打磨阶段分别在哪落地,详见 [ROADMAP.md §1](./ROADMAP.md#1-已实施mvp-主体--路线图外完成)。本节不再维护细粒度"步骤 N → 关卡"映射表(随 V2 路线图重排已过时)。

### 2.5 横切关注点:16 关之外但必做的事

关卡图是纵向链路,但很多**横切关注点**贯穿多个关卡,容易被遗漏。下面列出横切关注点(2.5.1~2.5.16),每个都标出"在哪个关卡被处理 / 关键设计点"。

#### 2.5.1 用户中途取消(CancellationToken)

- **触发场景**:用户在 LLM 流式输出中点 stop,或 long-running tool 内中断
- **位置**:② Tauri IPC 之后立刻建 `CancellationToken`;⑩ tool 执行内 `tokio::select!` 监听
- **关键设计**:取消不立即终止 LLM 请求,而是把"取消"事件本身作为 tool_result 回传(给 LLM 一次自我收敛的机会);只有用户二次取消才真终止
- **`shell` 进程组杀整组**:`shell` tool 子进程以 `process_group(0)` 启动,PGID == sh PID;cancel / timeout 时 `kill(-pgid, SIGKILL)` 杀整组,清理 `&` / 管道 / `nohup` 产生的孙子进程。Windows 留 P2
- **缺失后果**:用户按 stop 没反应 → 跑光了 token 还在跑 → 信任崩塌
- **当前实现**:MVP 简化决策——单次 cancel 即 emit `Done("cancelled")` 终止,**未实现"二次取消才真终止"语义**;完整 spec + 二次取消实现路径见 `docs/_history/reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` §2.1(RULE-A-010 已 closed 2026-06-17 via spec 偏离声明)

#### 2.5.2 ⑩ Tool 超时回填

- **阈值建议**:`shell` 5min,`read_file`/`grep` 30s,`write_file` 10s(可配)
- **kill 后的回填**:不返回成功也不返回错误,返回
  ```
  tool_result {
    is_error: true,
    content: "timeout after 300s, partial output: <截断的前 50KB>",
  }
  ```
  (截断输出走 C6 统一契约——超时 partial output 同样经 `tool_output.rs` 截断 + 恢复指引,见 §2.5.3)
- **LLM 据此**:可能重试、可能换 tool、可能放弃;这都是合法策略
- **实现位置**:⑩ 内部 `tokio::time::timeout` 包执行

#### 2.5.3 ⑩ 大输出截断(C6 统一契约,2026-08-30 落地,**替代**早期散装 50KB head+tail)

- **统一的对象是「截断契约」,不是「上限数字」**:每次截断必须自带一条恢复通路,且标记 machine-parsable、全工具同一格式
- **三恢复模式(sanctioned)**:A 落盘 spill + read_file offset/limit 恢复 / B range 参数恢复 / C 收窄 pattern 恢复;统一 `<truncated>` 标记
- **统一实现**:`tools/tool_output.rs` 契约模块,shell/background_shell/read_file/web_fetch/grep 五工具共用;char-boundary 安全以 RULE-E-009 为准绳(shell 裸切片 panic 为 C6 修复的 P1)
- **spill 落点**:`app_data_dir/outputs/<session>/`(C6 迁出 `<cwd>/.everlasting/outputs/`——agent 自我污染 / git 噪音 / 语义混杂),read_file 恢复路径捆绑 trusted carve-out
- **web_fetch 走落盘不走重取**:两次 fetch 内容可漂移,落盘一次、切片多次
- **实现位置**:⑩ 末尾、⑫ 之前;完整契约见 `.trellis/spec/backend/agent-loop-architecture/pattern-output-truncation.md`

#### 2.5.4 ⑬ 循环检测阈值(C2 已实施 2026-06-24)

- **分级触发**(取代早期单一 `Jaccard > 0.9`,单一阈值无法适配短/长 input):
  - **Level 1 精确签名硬触发**(`HARD_WINDOW=3`):连续 3 次归一化签名完全相同 → 零误报抓真死循环
  - **Level 2 Jaccard 软提示**(`SOFT_WINDOW=5` / `SOFT_THRESHOLD=0.85`):≥2 对 token-set Jaccard > 0.85 → 容忍近重复
- **per-tool 签名**:`read_file`/`write_file`/`list_dir`=path,`grep`/`glob`=pattern+path,`edit_file`=path+old_string(含 old_string 才不误判正当的同文件多块编辑),`shell`/`run_background_shell`=command,其余 fallback `name+canonical(input)`
- **命中动作(软)**:两层都 `tracing::warn!` + 把 hint 文本插入 result message,**不跳过执行、不终止 loop**,撞线兜底见 §2.5.15(2026-08-19 起软卡询问,非硬停)。无 AuditKind 落表

#### 2.5.5 ⑤ Context 压缩(C3+ LLM 摘要式压缩,2026-08-18 落地,**替代** C3 MVP 机械丢组 2026-06-12)

- **触发**:总 token > `context_window * 0.85`(取代 C3 MVP 的 0.80 阈值)。**触发口径 2026-08-19 统一切换**为"按发送部件加法"——`count_tokens(system_prompt) + count_tokens(tools_json) + estimate_messages_tokens(messages)` 三部件之和(`agent/budget.rs::estimate_request_tokens`),修复旧口径只数 messages、漏计 tools/system 的洞(小窗口模型 32k/64k 下可能在 messages 未达触发线时整体超窗)
- **策略**:LLM 9 段模板结构化摘要(`task / progress / facts / decisions / open / files / next` 等)+ `prior-summary` 增量合并(已存在 summary 作上下文,避免每轮全量重写)
- **保留区存活**:`clamp(15k, 10% 窗, 25k)` token 边界,**最近 turn 逐字不丢**(掉 LLM 看不到刚刚说过的话会发懵)
- **元数据**:摘要行落 `messages` 表 `metadata.kind = "compaction_summary"`(区别于 user / assistant),前端折叠渲染
- **水位**:`cutoff_seq` 精确折叠记忆,展开按需(不破坏 pair 不变量)
- **兜底**:连续 3 次 LLM 摘要失败 → 熔断回退 C3 机械丢组(0.80→0.50 旧逻辑,见代码 `agent/context.rs`)
- **硬卡**:2026-08-19 起叠加关卡⑤统一预算硬卡(`BUDGET_LINE_RATIO = 0.95`×window,裁尽仍超才 fail-fast),见 §2.5.14
- **实现位置**:`app/src-tauri/src/agent/context.rs`(`compact_messages` + 新 LLM call)+ `agent/budget.rs`(统一口径 + 硬卡引擎)+ `messages` 表 schema 兼容(messages 表 metadata 列从 JSON 字段读)
- **完整设计**:见 [ROADMAP.md §1.2 C3+](./ROADMAP.md) 行(2026-08-18 落地)(RULE-A-001/002/006 已闭环)

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
- **UI 查询**(C4 任务,2026-06-14 PR2 已实施):Tauri command `list_session_audit_events(session_id)` → `Vec<AuditEventRow>`;前端 `useAuditStore` + `<AuditLogModal>` 绑当前 session;kind 下拉筛选 + "仅 critical" 复选 + 计数 + 刷新;按 `ts DESC, id DESC` 稳定排序。**2026-08-30 起**(RULE-PERM-001):AuditLogModal 改走 keyset 分页命令 `list_session_audit_events_page`(首屏 100 行 + 「加载更多」,过滤/计数下推 SQL);旧全量命令保留供 traceStore 使用
- **29 类 AuditKind(2026-08-31 实测,`SandboxedShellExecution` 为第 29 个,见 `app/src-tauri/src/agent/permissions/audit.rs`;09-01 P3c/P3d 复核仍 29——升级闭环审计零新 kind,复用既有 kinds)** + 完整 schema + payload wire shape + UI 渲染细节,按域分组:
  - **Tool 域(6)**:ToolDenied / ToolAllowed / ToolPermissionAsk / ToolExecuted / SandboxedShellExecution(P3b 08-31 起,沙盒档(sandbox_policy ≠ off,含 Plan 只读面)shell 命令沙盒执行完成,payload 带 command_sha256_12 前缀 + ruleset(含 face)+ tool_name)/ ToolDeniedYolo
  - **Permission 域(3)**:PermissionGranted / PermissionTimeout / RequestCancelled
  - **Mode 域(6)**:ModeChanged / YoloEntered / YoloExited / ModeChangeRequested(07-07 request_mode_change 工具)/ ModeChangeAllowed / ModeChangeDenied
  - **Message 域(2)**:EditMessage(D3 PR1)/ ResendMessage(D3 PR3)
  - **Loop 域(2)**:LoopIntervention(C2+ 07-05 主动干预)/ TurnLimitSoftcap(08-19 MAX_TURNS 软卡询问)
  - **Worker 域(4)**:WorkerAskAllowed / WorkerAskDenied / WorkerAskTimedOut / WorkerAskCancelled(L3b 06-22 RULE-FrontSubagent-003 fix)
  - **TaskStateTransition 域(3)**:TaskStateTransitionRequested / Allowed / Denied(07-08 workflow Phase 3 Step 3.1)
  - **Budget 域(1)**:ContextBudgetTrim(08-19 关卡⑤硬卡裁剪,unified-context-budget)
  - **UI 域(1)**:UiDiffApplied(B9+ D4 07-13 apply_ui_diff IPC 成功)
  - **Scheduler 域(1)**:ScheduledTaskFired(F2 08-28,动作 11 个:fired/catchup/skipped_dedup/skipped_queue_disabled/lost/error + completed + M4a 群聊档 skipped_busy/resumed_group_chat/fired_group_chat/recovered,09-07)
  - 实现位置:`app/src-tauri/src/agent/permissions/audit.rs`;落表点见各 variant 注释

#### 2.5.9 ⑩ 并行 tool 执行(L2 MVP,2026-06-19 落地,**已实施**)

- **触发**:单 turn 内 LLM 返回的**所有** tool_use ∈ `{read_file, grep, glob, list_dir, use_skill}`(纯本地只读 + 全静默 Allow)**且**任一 path 工具的 `path` 解析后 ∈ project root → 并发执行;否则(含 write_file/edit_file/shell/update_checklist/web_fetch 或 path-outside-root)→ 整批串行
- **判定**:`is_parallel_eligible(&tool_calls, &permission_ctx.cwd)`(纯谓词)
- **实现**:`FuturesUnordered` + `permissions::check` → `execute_tool(token.clone())` → cancel 检查 → audit → `emit_tool_result`;`result_slots[i]` 按 tool_use **原始 index** 回填
- **不变量**:
  - 多 tool_result **单消息打包**(parallel-tool-use 红线:拆消息会让 LLM "学会"避免并行)
  - `web_fetch` 虽只读但 Tier 4 默认 `emit ask`,MVP 排除(走串行,保留逐个 ask UX)
  - 共享状态安全:并发集合无 shell(改 cwd)/edit_file(写 read_guard)→ 无写冲突;`PermissionStore`/`SkillCache`/`ReadGuard` 都是 `Arc<Mutex/RwLock>`,多 task 并发 read 安全
  - cancel:并发不 `break`,等所有 task 完成或被 cancel;`execute_tool` 内 `tokio::select!` 各 task 独立响应 cancel
- **完整设计 + RULE-A-013 path-in-root 收口 + 调研引用**:见 [`spikes/2026-06-19-async-parallel-tool-research.md`](./_history/spikes/2026-06-19-async-parallel-tool-research.md)

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

- **问题**:关卡 ⑤ context 构造时,AGENTS.md / EVERLASTING.md 加载段占 prompt token(实测 4 文件合计 60-100k token,长项目超过 0.30×window)
- **方案 WP1 度量**:`turn_trace.memory_token INTEGER` 列(08-15,backfill 同 C7)
- **方案 WP2 切节注入**:`memory/digest.rs` fence-aware 切节目录(纯机械,标题 + 首句,无 LLM 调用);`AGENTS.md` primary 永不 digest(`mtime` 锁死),`EVERLASTING.md` 且 tokens>600 才 digest
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

#### 2.5.16 ⑩ 执行期沙盒(Sandbox P3b~P3d,2026-08-31 + 09-01 落地)

- **定位**:判定层(A2+ P1+P2 复合命令拆分 + 写重定向检测)之下的**执行期限损层**——变量展开 / `$()` / `eval` / alias 等静态盲区把命令误判时,损害被限制在「可写面 + 其余只读、无出网、无 `/init` 与 `/mnt/c` 执行」之内;P3c 后 `shell_trust::classify_prefix` **不再参与触发**——沙盒档下全命令进沙盒,判定层只服务 `off` 档的经典路径
- **主路线(Landlock + seccomp,自研零外部二进制依赖)**:Landlock ruleset(EXECUTE + 写族 handled,读不控)+ seccomp BPF(拦 `socket(AF_INET/AF_INET6)`,AF_UNIX 放行)+ `PR_SET_NO_NEW_PRIVS`;弃 bubblewrap(userns 可用性不稳 + 二进制分发 + interop 逃逸面,spike 08-31 实测定案)
- **触发(P3c `resolve_policy`,09-01 单一决策真源)**:P3b 的「ReadOnly 档 ∧ 非 Yolo ∧ kill-switch ∧ 能力探测」四道 gate 废弃;求值序 **capability → Yolo → 项目 off → kill-switch → Plan → 项目面**(惰性读序勿重排,config 读在 gate 通过后);Yolo 恒不沙盒、kill-switch 关 = 全局 Off(不设 pre_exec)、能力探测失败 fail-open;`resolve_session_policy` 真源一处、消费两处——Tier 4 shell 分支头短路(跳过 prefix-grant/三档分类/ask,直接 Allow + ToolAllowed 审计;Tier 1–3 硬拒不被取代)+ spawn 侧 `decide`
- **三态 per-project 配置(P3c,09-01)**:`projects.sandbox_policy` 三态 `off/readwrite/readonly`(默认 **readwrite = 行为变更**,存量项目全命令进沙盒;回滚 = kill-switch 或单项目切 off);写通道 `update_project_sandbox_policy`(daemon route + Tauri command)+ 设置面 `ProjectSandboxTab.vue`(RULE-SBX-002 raw/effective 分离)
- **Plan 只读面(P3c,09-01)**:Plan 模式重新暴露 shell 族(tool list 回归);session 级只读面 `Face(ReadOnly)`——worktree 移出可写根、**显式补进 exec 面**(项目脚本仍可运行),`/tmp` 为调查型构建逃生口;项目 off 短路在前 → Plan + off 回退工具过滤,**绝不落「Plan + 弹窗放行写」**
- **前台升级闭环(P3c,09-01)**:沙盒命令 exit≠0 且 `classify_block` 命中(写串先行 / `Operation not permitted` = 断网)→ `permissions/escalation.rs` 先查 prefix-grant(命中零卡不沙盒重跑;复合命令不享 grant)→ 未命中 Ask 卡(`reason_override` + stderr 证据行)→ 批准**逐字节同 command/env/cwd 一次性不沙盒重跑**(RULE-E-001/002 不变,仅无 pre_exec;重跑结构性不再升级)/ Deny → 原失败 + 模式感知指引;Plan 排除(只读身份确定);每 tool call 至多一次;`failure_guidance` 三路拦截指引(写 × Edit/Plan、断网 × Edit/Plan,Plan 文案含 diff 提案 + /tmp 逃生口)
- **后台升级闭环(P3d,09-01)**:`run_background_shell` 面外失败同款闭环——registry 等待任务对 `trigger==Normal ∧ outcome==Failed ∧ 沙盒启动 ∧ origin_tool_use_id 有 ∧ classify_block 命中` 的通知带 `EscalationOffer{tool_use_id, block, stderr_evidence}`;**下轮注入时**(drain 后、组装 turn 前)`chat_loop/background_escalation.rs::resolve_all` 解析:Plan 门 → `escalation_source` 查重跑输入 → prefix-grant 零卡直跑 / Ask 卡挂**原调用卡**(ShellCard `isPendingApproval` 对后台卡按 `isBackground` 豁免 `!hasResult` 守卫)→ 批准 `start(sandbox=None, origin=None)` 一次性不沙盒重跑;无 offer 走 legacy 格式逐字节不动(AC5 锚)。`ToolContext.tool_use_id` 新字段由 dispatch 对所有工具统一盖章,仅 run_background_shell 消费(免先 start 后注册的丢载竞争)
- **规则集契约(两路共用)**:可写根 = worktree + `/tmp` + spill(`outputs/<session>`) + extras(`sandbox_extra_writable`;ReadOnly 面 worktree 移出);exec 允许面 = PATH 解析目录(过滤 `/mnt/` 前缀)∪ `/lib` `/lib64` `/usr/lib` 静态根(动态链接 ELF 解释器)∪ `/dev` `/tmp` ∪ 可写根 ∪ 工具链探测目录,设备节点 per-file `WRITE_FILE` 放行,**显式不含 `/init` 与 `/mnt/c`**(WSL interop 收口);全部服务端解析,永不采信 tool 参数路径(CVE-2025-59532 铁律)
- **审计**:新增 `AuditKind::SandboxedShellExecution`(第 29 变体,追加变体零迁移;payload `command_sha256_12` 前缀——不存全命令,全文由 `tool_executed` 行承载 + `ruleset` 摘要(含 `face=rw|ro`)+ tool_name);升级闭环**零新 kind**(ask 侧既有 kinds + 首个 `sandboxed_shell_execution` 行 + `tool_executed` 终态)
- **完整设计**:见 spike `.trellis/tasks/08-31-a2-p3a-sandbox-spike/`;spec 沉淀 `.trellis/spec/backend/sandbox-executor.md`
