# DAEMON-API — daemon HTTP API wire 约定速查

> **来源**:2026-09-05 群聊 headless 实跑实踩(BUGLIST-group-chat GC6)——API 消费者按 snake_case
> 猜字段名查询 `list_turn_traces`,返回行字段全 `None`(实际 wire 是 camelCase)。本文是该约定的
> 集中文档;各 Row 的权威定义在源码(`app/src-tauri/src/db/`),本文与其冲突时以源码为准。
>
> **架构背景**(daemon-server.md):daemon HTTP route 是 Tauri command 的镜像(Q0 单源)——
> route handler 反序列化 JSON body 后转发 `commands::*_inner`,返回值直接 `Json(...)` 序列化。
> 因此 wire 形状 = Rust struct 的 serde 输出,无独立转换层。

## 1. 命名约定总则

| 方向 | 约定 | 说明 |
|---|---|---|
| **请求 body**(所有端点) | `snake_case` | route handler 的 `Deserialize` struct 字段名,如 `{"session_id": "..."}` |
| **响应行 · sessions 域** | `snake_case` | `SessionRow` / `SessionSummary` / `MessageRow` / `LoadedSession` 无 serde rename,字段原样输出 |
| **响应行 · trace 域** | **`camelCase`** | `TurnTraceRow`(`db/trace.rs`)带 `#[serde(rename_all = "camelCase")]`——**GC6 的坑就在这** |
| **响应行 · providers/models 域** | `camelCase` | `ProviderRow` / `ModelRow` / `ModelWithProvider` 带 camelCase rename |

历史成因:sessions 域是最早的 IPC 面(直透 DB 行),trace/providers 域后建时按
database-guidelines.md 的「IPC payload camelCase」新约定(对齐 `AuditEventRow`)。**不要从端点
名猜 casing,按本文的域对照表来。**

## 2. trace 域 — `POST /api/v1/permissions/list_turn_traces`(camelCase 实踩点)

请求:`{"session_id": "<sid>"}`(snake_case)。返回 `TurnTraceRow[]`,**wire 字段全部 camelCase**:

| wire 字段 | Rust 字段 | 语义 |
|---|---|---|
| `id` | `id` | 行主键 |
| `sessionId` | `session_id` | 会话 |
| `runId` | `run_id` | `""` = 主 loop 行;非空 = `subagent_runs.id`(worker 行,走 `list_worker_turn_traces`) |
| `seq` | `seq` | 轮序号(与 messages 共享区间) |
| `tokenUsageJson` | `token_usage_json` | `TokenUsage` 5 字段 JSON 字符串 |
| `compactionJson` | `compaction_json` | C3 压缩事件 JSON(无则 `null`) |
| `loopHintJson` | `loop_hint_json` | C2 软提示 JSON |
| `breadcrumbJson` | `breadcrumb_json` | workflow 面包屑 JSON |
| `toolsToken` | `tools_token` | `tools[]` 序列化 cl100k 估算(列存在前的行为 `null`) |
| `memoryToken` | `memory_token` | 记忆块注入估算 |
| `imagesToken` | `images_token` | 图像 token 估算 |
| `atFilesToken` | `at_files_token` | @文件注入估算 |
| `systemToken` | `system_token` | system prompt + skill listing 估算 |
| `contextWindow` | `context_window` | 请求时窗口快照(前端预算行分母;`null` → 前端回退 200_000) |
| `createdAt` | `created_at` | 行创建时间 |

所有「维度」字段(`*Json` / `*Token`)都是 `null | string | number`——`null` 表示该维度从未
为该轮写过,不是 0。`list_worker_turn_traces` 请求 `{"run_id": "..."}`,返回行同上形状。

## 3. sessions 域 — snake_case

- `POST /api/v1/sessions/list_sessions` 请求 `{"project_id": "..."}` → `SessionSummary[]`
- `POST /api/v1/sessions/load_session` 请求 `{"session_id": "..."}` → `LoadedSession | null`
  (`{session: SessionRow, messages: MessageRow[]}`)
- `GET  /api/v1/sessions/{id}/snapshot` → `{session, pending_interaction}`(断线重连一次性拉全)

`SessionSummary` 关键字段(全 snake_case):`id` / `title` / `updated_at` / `preview` /
`project_id` / `current_cwd` / `session_type`(`"chat"` | `"group_chat"`)/ `metadata`(群聊配置
`{participants: [...]}`)/ **`busy`**(运行时信号,见 §4)/ **`stop_reason`**(见 §4)。

`SessionRow` 在 summary 之上多:`created_at` / `model` / `mode` / token 快照
(`last_context_input_tokens` 等)/ **`stop_reason`** / **`discussion_summary`**(见 §4)。

`MessageRow` 关键字段:`seq` / `role`(`"user"` | `"assistant"`)/ `content`(序列化
`Vec<ContentBlock>` JSON)/ `text`(可见文本列)/ `speaker`(群聊发言者;`null` = 经典聊)/
`status`(`null` = 终态;`"in_progress"` = 流式检查点行;`"interrupted"` = 崩溃恢复行)/
`ttfb_ms` / `gen_ms` / `total_ms` / `thinking_ms`。

## 4. 群聊生命周期消费指南(GC1/GC2/GC7,2026-09-05 起)

驱动一场 headless 群聊时的轮询语义:

```text
busy = true                       → 讨论进行中(编排级粒度:轮间空隙不回落)
busy = false + stop_reason != null → 讨论已结束,原因见 stop_reason
busy = false + stop_reason = null  → 该会话从未跑过编排(或经典聊会话)
```

- **`busy`**(`SessionSummary` only):运行时状态,DB 恒 false,由 `list_sessions_inner` 从
  `session_active_request` 补写。群聊的 busy 覆盖**整场编排**(moderator + 参与者 + 轮间空隙),
  落 false 即整场终结,不会再复活。
- **`stop_reason`**(`SessionSummary` + `SessionRow`):持久化的终止原因,取值
  `group_chat_end`(正常收官)/ `max_rounds`(30 轮帽截断)/ `cancelled`(用户 Stop 硬停,
  在途发言被斩、无总结)/ `error`(连续 3 轮生成错误熔断)/ `preempted`(2026-09-06 起:
  `preempt_group_chat` 体面打断——在途发言跑完 + moderator 收束轮,`discussion_summary`
  通常有值;收束轮两次失败兜底立断时 summary 如实缺)。每次新编排启动时清空再回填。
- **`discussion_summary`**(`SessionRow` only,即 `load_session` 可得):moderator
  `end_discussion({summary})` 的收官总结一等字段——共识清单直接可读,无需解析
  end_discussion 的 tool_result content blocks。正常收官与 preempt 收束轮两路写入。
- 终止的实时信号:`GET /api/v1/stream`(SSE)上编排器的终端 `Done` 事件
  `stop_reason` 与上表同值;SSE 不回放完整历史,断连后用 snapshot 端点补齐。

**进行中干预(2026-09-06 起,P0 打断最小语义)**:

- **注入(busy 时发消息,非破坏)**:`POST /api/v1/agent/chat` 打到 busy 群聊 → 消息进
  编排器 controls 缓冲,返回 `{"status":"injected"}`(**无流、无 request_id 事件**),
  落库带 `[用户插入] ` 文本前缀 + `metadata.kind="user_inject"`,下一 moderator 轮可见。
  讨论继续不重启(旧 3a 路径会 cancel 整场重开 = 毁场,已对群聊关闭)。纯图片消息注入
  P0 不支持,报错。不 busy 时行为不变(正常开新场,`{"status":"started"}`)。
- **体面打断**:`POST /api/v1/cancel/preempt_group_chat` `{"session_id": "..."}` → 置
  轮边界 preempt 信号:在途发言跑完 → moderator 收束轮(end_discussion 落 summary)→
  终态 `stop_reason="preempted"`。无进行中讨论时报错(非幂等静默)。返回
  `{"preempted": true}`。**与 `cancel_chat` 的分工**:cancel = rid 域硬停(Stop 按钮,
  无总结);preempt = session 域收束式打断。

## 5. 无人值守权限审批(GC3,2026-09-05 起)

headless(无 SSE 订阅者)时 `permission:ask` 的等待窗口从 120s 缩到 8s 快速拒;
有活跃 SSE 订阅者(GUI / remote PWA)则保持 120s。驱动方若想人工审批,保持
`GET /api/v1/stream` 连接即可;拒绝原因文本中带 `no live observer` 标记的是快速拒,
`permission_response` 端点照旧可用(8s 窗口内应答仍会生效)。

## 6. 群聊审议驱动入口(GCE-M1,2026-09-06 起)

headless 消费群聊的**推荐姿势是驱动脚本,不手搓 curl**——生命周期语义(轮询 busy/stop_reason、
中断 cancel、转录落盘、模型引用 UUID 解析)锁在一份实现里:

```bash
node scripts/group-chat-run.mjs projects | models | presets   # 建群三要素内省
node scripts/group-chat-run.mjs run --project <path> --preset review --topic "..." [--dry-run]
```

实现要点(手搓 curl 前必读):

- **模型引用是 UUID 不是名字**:`metadata.participants[].model` 与 `create_session.model` 进
  `catalog.get()`(`state.rs` ProviderCatalog,key = `models.id`),参与者解析**无名字 fallback**
  (名字进 metadata → `participant_unresolved` 跳轮,`group_chat_loop.rs` resolve_provider)。
  脚本收 UUID/名字并统一解析。
- `agent/chat` daemon 版是 fire-and-forget(handler 秒回,编排后台跑),终态只看 §4 的
  busy/stop_reason 轮询;**不挂 SSE** = 保住 §5 的无人值守 8s 快拒。
- LLM 调用方的指引路径:`.agents/skills/group-chat/`(何时召集/议题写法/结果解读)。

### 6.1 MCP 接口层(GCE-M2,2026-09-06 起)——宿主 agent 的首选入口

MCP 宿主(ZCode / Claude Code / Cursor 等)里的 agent **优先用 MCP 工具,不跑脚本**:
`start_discussion` / `discussion_status` / `discussion_result` / `cancel_discussion`
四工具(立即返回 + 轮询语义与 §6 一致;工具描述自带成本闸)。

**挂载(2026-09-06 起用户级)**:`~/.zcode/cli/config.json`(ZCode)/ Claude Code 各自的
用户级配置,stdio spawn 本仓库 `scripts/group-chat-mcp.mjs` 绝对路径:

```json
"mcp": { "servers": { "everlasting-group-chat": {
  "command": "node",
  "args": ["/usr/local/code/github/everlasting/scripts/group-chat-mcp.mjs"]
} } }
```

- ⚠️ **挂载配置必须写绝对路径**——配置文件作用域的 MCP server **不展开
  `${...}` 模板变量**(那是插件作用域专属特性;字面量路径会让 server 启动即
  失败、工具注册为 0,Settings → MCP 显示 failed)。换机器时须按实际检出路径改
  `args`(曾用仓库根 `.agents/mcp.json` 挂载,已移除:workspace 作用域只在
  本仓库会话可见,跨项目不可用——2026-09-06 实测)。
- 若改用仓库级挂载:放 `.agents/mcp.json` 顶层 `mcpServers` 键;它是 same-scope
  fallback,同 scope `.zcode` 定义了任何 MCP server 时被整体忽略(非合并)。
- 分工:**宿主 agent = MCP 工具**;**everlasting 内部 agent(daemon 单聊)= 脚本 + M1 纪律**
  (沙箱 errno 翻译 / prefix 授权 basename / 裸命令,见 SKILL.md 边界)——内部 agent 不是
  MCP client。
- 归因:MCP 建群盖 `metadata.created_via:"mcp"`,M1 脚本盖 `"script"`,GUI/历史 session 无此键。
- server 记账(session→request_id/project_id)落 `~/.local/state/dev.everlasting.app/mcp-discussions.json`
  (XDG state,原子写)——server 进程随宿主会话生灭,讨论跨进程存活靠它兜底。
- 冒烟:`node scripts/group-chat-mcp-smoke.mjs`(`--live` 烧真 token 走全链,可选)。

## 7. 其他常用端点(路径约定)

全部为 `POST /api/v1/<domain>/<command>`,body snake_case,与 Tauri command 同名同参:
`sessions/*`(见 §3)、`permissions/*`(模式切换 / 审批回填 / trace 三条)、
`agent/chat`(发起轮次)、`cancel/*`(Stop)、`message_queue/*`、`config/*`、
`providers/*`、`usage/*`、`files/*`、`worktree/*`、`scheduled_tasks/*`。唯一 GET:
`/api/v1/stream`(SSE)与 `/api/v1/sessions/{id}/snapshot`。

## 8. 安全边界:绑定面与零鉴权前提(2026-09-06 评估)

> 来源:GCE-M2 收官勘察发现 daemon 实际 bind `0.0.0.0` 与部分 roadmap 表述不符,
> 拟「收紧为 localhost」;经评估**接受现状、只立此边界文档**(用户裁定:收紧需先评估,
> Windows 宿主调用链与「本机 daemon」零鉴权均为有意设计)。

**现状**(`daemon/server.rs` 模块头 + `serve_daemon`):绑定 `0.0.0.0:PORT`,全 API **零鉴权**。
两者是配套的有意设计,不是疏忽:

- **0.0.0.0 是 WSL 承重墙**:WSL2 的 localhost forwarding **只转发绑定 0.0.0.0 的
  listener**——改绑 `127.0.0.1` 后 Windows 宿主浏览器/GUI 经 `http://localhost:PORT`
  的访问直接断(daemon 在 WSL 内,见 `HACKING-wsl.md` 与 REMOTE-ACCESS-ROADMAP
  「daemon 跑 WSL 2 监听 0.0.0.0 / 宿主经 localhost forwarding 访问」)。
- **零鉴权前提 = 可达面仅本机**:API 面是完整 agent 控制(chat 连带已存 provider key、
  shell 执行、文件读写),任何能打到端口的调用方等同坐在键盘前。该前提在不同部署形态下
  成立与否见下表。

**可达性矩阵**(按部署形态):

| 部署形态 | 实际可达面 | 零鉴权前提 |
|---|---|---|
| Win10 + WSL2(NAT,默认) | WSL 内部 + Windows 宿主;**物理 LAN 不可达**——WSL2 虚拟机在宿主 NAT 后,绑定不映射到物理网卡,除非手动 `netsh portproxy` | ✅ 成立(≈本机) |
| Win11 + WSL2 mirrored(`networkingMode=mirrored`) | WSL 与宿主共享网络栈,0.0.0.0 即宿主所有接口,**LAN 可达** | ⚠️ 需宿主防火墙自行收口 |
| 原生 Linux / macOS | 所有网络接口,**LAN 可达** | ❌ 不可信网络上不成立(任意同网设备可驱动 agent) |
| 远程访问路径 | 无入站暴露需求:PC daemon **出站** WSS 连 remote 中继,PWA 连中继不直连 daemon(见 REMOTE-DEPLOY.md) | ✅ 不经过此面 |

**结论与余留**:当前实际部署(Win10 + WSL2 NAT + 出站隧道)暴露面 ≈ 本机,现状接受。
**原生部署到不可信网络之前**必须先收紧——远期候选(未立项):opt-in bind 配置(默认值
不动保 WSL 路径零回归)或 host 防火墙指引;群聊侧同款前提见 GROUP-CHAT-API-ROADMAP
§3「v1 零鉴权(本机)」与 §5 远程暴露认证(立项前先过安全评审)。
