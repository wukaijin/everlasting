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
  通常有值;收束轮两次失败兜底立断时 summary 如实缺)/ `interrupted`(2026-09-06 P1a 起:
  daemon 进程级中断——崩溃/被杀,无 finalize;由**下次启动的 boot sweep** 标记,
  `group_chat_checkpoints` 行残留即断点凭据,**可续跑**,见下方「断点续跑」)。每次新编排
  启动时清空再回填。
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
- 两个动作的 MCP 包装见 §6.1(`interrupt_discussion` / `inject_message`,M3 起);
  GUI 打断按钮同权同语义(群聊会话头部 chip,讨论进行中可见)。
- **断点续跑(GCE P1a,2026-09-06 起)**:`POST /api/v1/agent/resume_group_chat`
  `{"session_id": "..."}` → 从 checkpoint 落库的断点续(interrupted / cancelled / error
  态可续;`group_chat_end` / `preempted` / `max_rounds` 终局不可续)。语义:轮预算**继承**
  (从断点轮起算,30 轮总帽不重置,防 crash-resume 循环烧轮)、GC5 熔断计数归零、moderator
  首轮带恢复指令(不重新开场)、roster 按当前 metadata 重解析。返回 `{"status":"started"}`
  后流照常(新 request_id,SSE 消费同 §4);五类校验失败(非群聊 / busy / 无断点 / 预算耗尽 /
  终局态)返回 400 + 明确文案。checkpoint 行每轮头落库(`group_chat_checkpoints` 表),
  可续跑退出(cancelled / error)保留、终局退出删除——「行在 + !busy」即可拾起。GUI 续跑
  按钮同权同语义(可续跑态时群聊 chip 区出现)。

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

MCP 宿主(ZCode / Claude Code / Cursor 等)里的 agent **优先用 MCP 工具,不跑脚本**。
六工具 = 生命周期四件套 `start_discussion` / `discussion_status` / `discussion_result` /
`cancel_discussion`(立即返回 + 轮询语义与 §6 一致;工具描述自带成本闸)+ **M3 控制面两件**:
`interrupt_discussion`(session 域收束打断,preempt 端点 1:1,分工同 §4;返回轮询指引)与
`inject_message`(往**进行中**的讨论注入用户消息,下一 moderator 轮可见)。后者带**前置
busy guard**:非 busy / 已收官的群聊 session 直接报错不发起——误发会重启编排器并无条件
清空上一场 stop_reason/summary(代价不可逆),guard 把它挡在客户端层;guard 通过后若
acceptance 非 `injected`(guard 判定与编排器落库间的竞态),以自有 request_id 即时
`cancel_chat` 止损再报错。

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
- 冒烟:`node scripts/group-chat-mcp-smoke.mjs`(`--live` 烧真 token 走全链;`--bin <path>` 对
  standalone bin 冒烟,断言链同构)。

**部署面(standalone bin,2026-09-06 落地,任务 09-06-gce-mcp-standalone)**:MCP server 可打成
**bun compile 单文件可执行**(内嵌运行时 + SDK,免 node / 免 node_modules / 免源码检出;98 MB 量级)。
`node scripts/group-chat-mcp-deploy.mjs` 一条命令 = 构建 → 装到
`~/.local/share/dev.everlasting.app/bin/everlasting-group-chat-mcp` → 把上方 user-scope 配置**原位替换**为
`{ command: <bin 绝对路径> }`(写前留单份备份 `config.json.mcp-deploy.bak`,全程幂等)。bin 同目录 sidecar
`everlasting-group-chat-mcp.build-info`(git short rev + 构建时间戳)用于诊断 stale bin。`--revert` 把配置
切回 node 挂载(node 挂载保留为**开发态默认**:改 .mjs 无需重编译),`--uninstall` 删配置项与 bin。引擎
`group-chat-run.mjs` / `group-chat-mcp.mjs` 零改动——bun compile 下 isMain 守卫恒真的误判由部署入口的
`process.argv[1]` 哨兵化解(根因见任务 research)。v1 只做宿主 linux-x64;跨平台编译矩阵(macOS/Windows)、
Tauri app 分发与其他宿主配置写入记 follow-up(GCE-ROADMAP §5)。

### 6.2 实时跟随(SSE follow,GCE-M3,2026-09-06 起)

打断 / 注入之外的第三权:外部调用方可在 start 后挂一条 SSE 连接**实时跟随**讨论,也可
只用 §4 的轮询。端点是 §7 的唯一流 `GET /api/v1/stream`——**全局单流**,所有 session 的
事件共用一条连接,群聊消费方按载荷里的 `session_id` 过滤:

- **事件通道**:SSE `event:` 名即通道。群聊相关的是 `chat-event`;其余(`tool:call` /
  `tool:result` / `permission:ask` / `tool:question` / `mode:change:request` /
  `task:state:transition:request` / `subagent:event` / `subagent:finished`)按需消费。
- **`chat-event` 载荷**(JSON,snake_case):`{ request_id, session_id, kind, ... }`——
  `kind` 全集(SoT = `llm/types/event.rs` ChatEvent,serde tag;2026-09-06 live 实测核对):
  - `speaker`(`{ speaker }`)——每个发言者**轮次**开始,编排器发出;后续 `delta` 不带
    speaker,消费方自行盖戳到当前 speaker(前端同款契约);
  - `start`——run 内每次 LLM 调用边界;`delta`(`{ text }`) / `thinking_delta` /
    `signature_delta`——流式增量与思考摘要流;
  - `turn_usage` / `turn_complete`——轮级用量 / 完成记账;
  - `done`(`{ stop_reason, usage }`)——**轮**级终止(每个 speaker 轮一次);`stop_reason`
    为 null 的普通轮界、跳轮值(`nominee_unknown` / `participant_unresolved`)均非终态。
    **场级**终止 = 编排器 post-loop 的最后一个 `done`,`stop_reason` 与 §4 表同值
    (`group_chat_end` / `max_rounds` / `cancelled` / `error` / `preempted`),此后 `busy=false`。
    (`interrupted` 永不出现在 SSE——进程级中断意味着 daemon 已死,该值只经 DB/轮询面
    可见,P1a。)
- **消费序列范例**:`start_discussion` → 挂流 → 逐轮 `speaker` → `delta`… → 轮 `done` →
  … → 场级 `done`(终态 stop_reason)→ `discussion_status` 复核 → `discussion_result`。
- **断连恢复**:浏览器 EventSource 自动重连回带 `Last-Event-ID`;daemon 在重放窗口内补发
  (SSE `id:` 字段),超窗发 `stream-resync` sentinel——消费方收到后应
  `GET /api/v1/sessions/{id}/snapshot` 补齐再续。keepalive 30s。
- ⚠️ **跟随连接改变无人值守语义**(§5):有活跃 SSE 订阅者时 permission ask 等 120s 而非
  8s 快拒。follow 消费方要么准备应答 `permission_response`(8s/120s 窗口内先到先赢),
  要么用完即断——不要挂着纯看。

### 6.3 定时审议(GCE-M4a,2026-09-07 起)——scheduled_tasks 的 group_chat 档

`scheduled_tasks/*` 新增 `target_mode: "group_chat"` 档:定时到点由 **daemon 原生**建群发题
(复用 F2 30s tick + M0 群聊生命周期;无需外部 cron 驱动脚本)。GUI 之外可经
`POST /api/v1/scheduled_tasks/create_scheduled_task` 直接建任务,到点后按 §4 消费该 session。

**建任务**(校验矩阵,违规 400):
- `target_mode: "group_chat"` + `prompt`(议题原文,fire 时**原样**发题,无注脚)+
  `schedule`(F2 七种 schedule kind + once 全兼容)。
- `group_chat_config` 必填,JSON 形状:`{"moderator_model_id": "...", "participants":
  [{"name": "...", "model_id": "...", "persona_md"?: "..."}]}`——结构校验(非空名单、
  无重名)+ 模型存在性(moderator 与全部 participants 查 models 表)。
- **不收** `target_session_id`(400 矛盾)与 `model_id`(moderator 在 config 内);
  `max_runs` / `ends_at` 结束条件与既有档同语义。
- update 的 `group_chat_config` 为双层 Option:缺省 = 保留存档;对象 = 校验后写入;
  显式 `null` = 清空(仅切离 group_chat 档合法)。

**fire 语义**(每 due 点四态路由,依据 `last_run_session_id` 所指旧场判定):
| 旧场状态 | 动作 | `last_fire_outcome` | 计 `run_count` |
|---|---|---|---|
| 仍在跑(busy) | 跳过本期 + 审计 `skipped_busy` | `skipped_busy` | 否 |
| interrupted 且 checkpoint 在(round<30) | 自动续跑(P1a 五闸)+ 审计 `resumed_group_chat` | `resumed` | 是 |
| interrupted 但 checkpoint 已删 | 审计 error,**本期不动**(绝不双开场) | `error` | 否 |
| 僵尸(round≥30)/ 停摆(stop_reason NULL 且无 checkpoint) | 补 `finalize(error)` → 审计 `recovered` → 开新场 | `recovered` | 否 |
| 终态 / 无旧场 | catalog 预检(moderator+participants 模型齐全)→ 建群 → 发题 | `started` | 是 |

计数矩阵:**所有臂都消费 due**(`last_fired_at` 记 due 不记 now);`run_count` 只计真正
开跑的 fire(开新场受理 / 续跑受理 / 开新场 Err)——busy 跳过与拒绝臂不烧预算。

**归因与产物**:fire 建群写入 session metadata 三键 `created_via="scheduled"` +
`scheduled_task_id` + `scheduled_task_name`;审计动作 `fired_group_chat` / `resumed_group_chat`
/ `skipped_busy` / `recovered`(permissions/audit 自由串域);任务行 `last_fire_outcome`
五值快照(`started/resumed/skipped_busy/error/recovered`)。收官时 daemon **自动导出转录**到
`{app_data_dir}/discussions/{YYYY-MM-DD}-{任务名}-{sid8}.md`(仅定时场;GUI/MCP/script 场
不导出),头部含阵容 / 起止 / stop_reason / discussion_summary。GUI 收官单 toast(专用,
抑制通用轮次通知)。

**边界**:LLM `schedule_task` 工具**不能**建群聊任务(恒 fixed 语义——群聊一场数十万
token,不开放给 agent 自主创建);preset 预设在 `scripts/group-chat-presets.json`,daemon
无 preset 概念(前端展开后提交,API 调用方同理)。

## 7. 其他常用端点(路径约定)

全部为 `POST /api/v1/<domain>/<command>`,body snake_case,与 Tauri command 同名同参:
`sessions/*`(见 §3)、`permissions/*`(模式切换 / 审批回填 / trace 三条)、
`agent/chat`(发起轮次)、`agent/resume_group_chat`(群聊断点续跑,§4)、
`cancel/*`(Stop)、`message_queue/*`、`config/*`、
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
