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
- `POST /api/v1/sessions/list_group_chat_sessions`(2026-09-07,GCE M4b 讨论库)
  请求 `{"project_id": <opt>, "stop_reason": <opt>}`(空 body / 空对象 = 全量浏览)→
  `GroupChatSessionHit[]`,按 `updated_at DESC`。只返回 `session_type='group_chat'` 场。
- `POST /api/v1/sessions/search_group_chat_discussions`(2026-09-07,GCE M4b 讨论库)
  请求 `{"query": "...", "project_id": <opt>, "stop_reason": <opt>}` → `GroupChatSessionHit[]`
  (同上过滤 + 关键词命中 title / discussion_summary / task_name / participants 任一即中;
  `query` 空 / 纯空白 = 全量浏览)。LIKE 匹配 `%`/`_`/`\` 按字面处理(转义同 search_messages)。

`SessionSummary` 关键字段(全 snake_case):`id` / `title` / `updated_at` / `preview` /
`project_id` / `current_cwd` / `session_type`(`"chat"` | `"group_chat"`)/ `metadata`(群聊配置
`{participants: [...], token_budget?: <u64,见下>}`)/ **`busy`**(运行时信号,见 §4)/
**`stop_reason`**(见 §4)。

群聊 metadata 可选键 `token_budget`(2026-09-08,C1.2 止损包,additive):声明该场讨论的
token 预算上限(计费口径 = input + output + cache_creation + cache_read 四字段求和,每内层
LLM 轮的 `TurnUsage` 累计)。超限 → 编排器在下一轮头终态停,`stop_reason="budget"`
(无收束轮)。缺键 = 不限。**四通道声明面(2026-09-08,M4 成本治理)**:GUI 建群弹窗 /
M1 脚本 `--token-budget <n>` / MCP `start_discussion` 的 `token_budget` 可选参 /
定时任务 `group_chat_config.token_budget`(§6.3)——全部「显式声明才限,缺省无键」。
核算读侧:`POST /api/v1/sessions/group_chat_token_usage`(2026-09-08,M4c),请求
`{"session_id": "..."}` → `{"total": <u64>, "by_speaker": [{"speaker": "...",
"tokens": <u64>}]}`(tokens 降序;同口径四字段求和,`turn_trace` JOIN `messages.speaker`,
worker 行/无 usage 行排除;GUI edit 弹窗成本区消费,脚本/MCP 走 `list_turn_traces` +
`load_session` 客户端聚合同口径)。

`SessionRow` 在 summary 之上多:`created_at` / `model` / `mode` / token 快照
(`last_context_input_tokens` 等)/ **`stop_reason`** / **`discussion_summary`** /
**`discussion_detail`**(C2 结构化收官结论,见 §4)。

`MessageRow` 关键字段:`seq` / `role`(`"user"` | `"assistant"`)/ `content`(序列化
`Vec<ContentBlock>` JSON)/ `text`(可见文本列)/ `speaker`(群聊发言者;`null` = 经典聊)/
`status`(`null` = 终态;`"in_progress"` = 流式检查点行;`"interrupted"` = 崩溃恢复行)/
`ttfb_ms` / `gen_ms` / `total_ms` / `thinking_ms`。

`GroupChatSessionHit`(M4b 讨论库,2026-09-07,全 snake_case,一场一行):`session_id` /
`project_id` / `title` / `task_name`(`null` = 非定时场;`metadata.scheduled_task_name`)/
`participants`(`string[]`,`metadata.participants[].name`)/ `stop_reason`(`null` = 从未终局)/
`discussion_summary`(`null` = 未以 end_discussion 收官)/ `total_tokens`(`null` = 尚无计费
轮,2026-09-08 M4c;与 group_chat_token_usage 同口径的场级合计)/ `created_at` / `updated_at`。

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
- **`discussion_detail`**(`SessionRow` only,C2 证据链 2026-09-09 起):结构化收官结论的
  JSON 文本,形状 `{"conclusions": [{"claim", "anchors": [{"path", "line"?, "check"?}],
  "stance": "verified"|"inferred"|"disputed"}], "open_questions": [string]}`(全 snake_case)。
  `stance` 是 moderator 自报可信度(实证读码 / 推测 / 争议);`check` 是编排器落库前的
  **锚点后校验**结果(`ok` / `not_found` / `line_out_of_range` / `outside_root` /
  `unvalidated`,root 外路径零 fs 访问)——校验**只标注不修改**,不改写 claim 与
  stance,断证信号留给消费方。`null` = 该场收官无结构化产物(旧场 / 只发 summary 的
  朴素收官);与 summary 同生命周期,复用场重置清空。消费面:MCP
  `discussion_result.detail`(坏 JSON 降级 `detail_warning`)、GUI 收官卡(stance 徽章 +
  锚点核验记号)、M1/定时双转录导出器的 `## conclusions` / `## open_questions` 节。
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

> **2026-09-15 起唯一载体**:daemon 原生 `/mcp` streamable-HTTP 端点(§6.5,零子进程、
> 工具语义 1:1)。stdio 壳(2026-09-06~09-14 的主入口)与 standalone bin 部署面已随
> P3 挂载切换 + P4 退役删除(任务 09-15-gce-mcp-stdio-retire);历史契约见 git 历史
> 与 GCE-ROADMAP §5。

MCP 宿主(ZCode / Claude Code / Cursor 等)里的 agent **优先用 MCP 工具,不跑脚本**。
八工具 = 生命周期四件套 `start_discussion` / `discussion_status` / `discussion_result` /
`cancel_discussion`(立即返回 + 轮询语义与 §6 一致;工具描述自带成本闸)+ **M3 控制面两件**:
`interrupt_discussion`(session 域收束打断,preempt 端点 1:1,分工同 §4;返回轮询指引)与
`inject_message`(往**进行中**的讨论注入用户消息,下一 moderator 轮可见)。后者带**前置
busy guard**:非 busy / 已收官的群聊 session 直接报错不发起——误发会重启编排器并无条件
清空上一场 stop_reason/summary(代价不可逆),guard 把它挡在客户端层;guard 通过后若
acceptance 非 `injected`(guard 判定与编排器落库间的竞态),以自有 request_id 即时
`cancel_chat` 止损再报错。另有只读内省两件 `list_models` / `list_presets`(09-11/09-12 起,
查建群可用模型与合并预设视图,宿主填 participants/preset 用)。

**挂载(2026-09-15 起 HTTP,任务 09-15 P3)**:`~/.zcode/cli/config.json`(ZCode)用户级
`mcp.servers`,server 名保持 `everlasting-group-chat`(即保 `mcp__everlasting-group-chat__*`
工具前缀):

```json
"mcp": { "servers": { "everlasting-group-chat": {
  "type": "http",
  "url": "http://127.0.0.1:7456/mcp"
} } }
```

- 前置:daemon 在跑(`:7456`)——HTTP 挂载零子进程,宿主直连 daemon 既有端口;
  daemon 停则工具不可用(与 GUI 同生命周期)。
- 历史:2026-09-06~09-14 为 stdio spawn 本仓库 `scripts/group-chat-mcp.mjs`(绝对路径,
  换机器须改 args);09-06~09-15 曾有 bun standalone bin 部署面(`group-chat-mcp-deploy.mjs`,
  任务 09-06-gce-mcp-standalone)——两者均随 P4 退役删除。仓库级 `.agents/mcp.json`
  挂载 2026-09-06 已移除(workspace 作用域跨项目不可见)。切换前双挂状态备份
  `~/.zcode/cli/config.json.p3-dual.bak`。
- 分工:**宿主 agent = MCP 工具**;**everlasting 内部 agent(daemon 单聊)= 脚本 + M1 纪律**
  (沙箱 errno 翻译 / prefix 授权 basename / 裸命令,见 SKILL.md 边界)——内部 agent 不是
  MCP client。
- 归因:MCP 建群盖 `metadata.created_via:"mcp"`,M1 脚本盖 `"script"`,GUI/历史 session 无此键。
- `discussion_result` 输出自 2026-09-09 起携带 **`detail`**(§4 `discussion_detail` 的解析
  对象;只发 summary 收官的场与旧场无此键)——外部 agent 消费结论时可按 `stance` 分层
  可信度、按锚点 `check` 判断证据是否断链;坏 JSON 降级 `detail_warning` 不炸。
- 记账:stdio 壳时代的 XDG state 记账文件(`~/.local/state/dev.everlasting.app/mcp-discussions.json`)
  已随收敛退役(§6.5——rid 由 `session_active_request` 派生);旧文件残留无消费方,可删。
- 冒烟:`node scripts/group-chat-mcp-http-smoke.mjs`(§6.5;`--live` 烧真 token 走 start →
  wait 长轮询 → result 全链)。

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
  `schedule`(F2 六种周期 kind + once 单次档 = 7 档全兼容)。
- `group_chat_config` 必填,JSON 形状:`{"moderator_model_id": "...", "participants":
  [{"name": "...", "model_id": "...", "persona_md"?: "..."}], "token_budget"?: <u64>}`——
  结构校验(非空名单、无重名;`token_budget` 有值必须正整数,不限 = 省略键)+
  模型存在性(moderator 与全部 participants 查 models 表)。fire 建群时 `token_budget`
  仅在声明时写入 `sessions.metadata`(不落 null 键,2026-09-08 M4c)。
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
无 preset 概念(前端展开后提交,API 调用方同理)。GCE-P1(2026-09-12)起 `group_chat_config`
新增可选 `preset_key`(创建/更新任务时选了预设则记录出处,内置 key 或用户预设行 id);
**fire 路径零读取**(快照语义:config 是创建时展开的完整 UUID 阵容,预设后续编辑/删除
不回溯生效),缺省不写键。

### 6.4 用户群聊预设 CRUD(GCE-P1,2026-09-12 起;内置档覆盖行 GCE-P1b 同日)—— group_chat_presets 域

Settings「群聊预设」页背后的四条 IPC(`POST /api/v1/group_chat_presets/<cmd>`,与 Tauri
命令 1:1)。**响应行 camelCase**(`GcPresetRow` 带 `rename_all`);**请求 body 顶层
snake_case**(IPC 形状铁律),嵌套 `participants` 元素例外 —— 按 `GcPresetParticipant`
的 camelCase serde 反序列化:`{"name": "...", "modelId": "<uuid>", "persona": "arch"}`。

```jsonc
// GcPresetRow(响应, camelCase)
{
  "id": "<uuid>", "name": "我的评审团", "description": "",
  "moderatorModelId": "<uuid>",                 // models.id UUID(soft ref,允许 disabled)
  "participants": [{ "name": "架构", "modelId": "<uuid>", "persona": "arch" }],
  "createdAt": "RFC3339", "updatedAt": "RFC3339",
  "builtinKey": "arch"                          // 可选(GCE-P1b):仅覆盖行携带,None 不序列化
}
```

| cmd | 请求(顶层 snake) | 响应 |
|---|---|---|
| `list_group_chat_presets` | `{}` | `GcPresetRow[]`(ORDER BY name 稳定序) |
| `create_group_chat_preset` | `{"name","description","moderator_model_id","participants"` + 可选 `"builtin_key"}` | `GcPresetRow`(id 服务端生成) |
| `update_group_chat_preset` | 同 create + `"id"`(无 builtin_key —— 该列不可变) | `GcPresetRow`(不存在 → 400) |
| `delete_group_chat_preset` | `{"id"}` | `{"ok": true}`(幂等,不存在也 ok) |

**内置档覆盖行**(GCE-P1b,2026-09-12 起):`create_group_chat_preset` 可带可选顶层
`builtin_key`(请求侧 snake,值 ∈ 四内置 key review/fe_review/arch/retro)。带该键的行 =
覆盖行,原位顶替对应内置预设(GUI 两消费方选中该档即用覆盖后阵容)。约束:

- 每个内置 key 至多一条覆盖行 —— commands 层 create 前置查重给可读 400(「内置预设
  「k」已有覆盖,请编辑现有覆盖行」),DB 层 `idx_group_chat_presets_builtin_key`
  UNIQUE 索引兜底(SQLite UNIQUE 对 NULL 互不相撞,普通用户行不受影响)。
- `builtin_key` **创建时定死**:update 不接受也不触碰该列;覆盖行不能转普通行。
- 删除覆盖行 = 恢复内置(JSON 源码定义),走同一条 `delete_group_chat_preset`。

**引擎侧消费**(GCE-P2,2026-09-12 起):M1 CLI 与 MCP server 运行时调
`list_group_chat_presets` 拉全部行,与内置 JSON 四档在客户端合并(规则镜像 GUI
mergedPresets:用户行追加、覆盖行原位顶替)。preset 引用三趟解析:内置 key →
用户行 id(UUID)→ 用户行名称(精确→忽略大小写)。拉取失败(daemon 不可达或
旧版本路由 404/405)fail-open 降级内置四档,MCP 侧 `list_presets` 工具返回
`degraded: true` + detail;standalone bin 内置 JSON 仍烤进产物,免重部署。

**校验**(commands 层单一事实源,违规 400 InvalidRequest,message 中文可读):名称 trim
非空 ≤40 字符、与其它用户行**及内置 key**(review/fe_review/arch/retro)大小写不敏感
不重名;描述 ≤200;participants 2..=3 条且名字非空 ≤20 预设内唯一;persona ∈ 五内置
kind(arch/product/backend/frontend/outsider);moderator 与全部 participants 的模型 id
必须存在(允许 disabled)。**只管用户预设** —— 内置四档是
`scripts/group-chat-presets.json` 单一事实源(M1 CLI / MCP 消费),不在本域,本域 CRUD
对它们零影响。

### 6.5 MCP 收敛端点 `/mcp`(2026-09-14 起)——daemon 原生 streamable-HTTP server

§6.1 stdio 壳之外的第二载体:daemon **自带 MCP server**(实现 =
`app/src-tauri/src/daemon/routes/mcp.rs`,任务 09-14-gce-mcp-daemon-converge,
roadmap §5 路径③)。动机 = 内存:stdio 壳随宿主会话 spawn node(或 98 MB bun
standalone bin),HTTP transport **零子进程** —— 宿主直连 daemon 既有端口;实测
冒烟全链后 daemon RSS 增量 ~1 MB(AC 闸 <10 MB)。

- **极简无状态 profile**(wire 契约反向提取自 `@modelcontextprotocol/sdk` 1.30.0,
  宿主 SDK 客户端开箱即连):
  - `POST /mcp`:JSON-RPC 2.0 单条或批;请求 → 200 纯 JSON;纯通知 / 客户端响应 →
    202 空 body;Accept 必须同时含 `application/json` 与 `text/event-stream`(缺 → 406);
    Content-Type essence 必须 `application/json`(否则 415)。
  - `GET /mcp` → 405(不开服务端主动流;SDK 客户端把 405 视为「无 GET 流」预期分支);
    `DELETE /mcp` → 200 no-op。
  - 不分配 `mcp-session-id`(无状态免 404 面);`mcp-protocol-version` 头 lenient 只记
    日志;initialize 版本协商 = echo 策略(请求版本 ∈ 2024-10-07…2025-11-25 集合则原样
    回,否则回缺省 2025-03-26)。
  - **工具执行错误不走 JSON-RPC error**:200 + `isError:true` text result(§6.1 同款,
    宿主把它呈现给模型而非判协议故障);未知工具 / 未知方法才是 JSON-RPC error
    (-32602 / -32601)。
- **八工具语义 1:1 平移 stdio 壳**(wire schema 与预算锁 4200 chars 同源):编排原语
  全部走 daemon 内部 `*_inner`(不经 HTTP 自绕);preset 合并消费 DB 用户行 / 覆盖行
  (§6.4 规则;`list_presets.degraded` 恒 `false` —— 数据源就是本进程,降级语义随收敛
  消失,键保留兼容宿主习惯)。与 stdio 壳的差异两处:
  - 转录落点 `{cwd}/out/group-chat-{slug}-{ts}.md`(惰性导出,status/result 终态首次
    观测触发;与定时场 `{data}/discussions/` 落点分叉);
  - XDG 记账文件(`mcp-discussions.json`)**退役** —— rid 从 `session_active_request`
    内存表派生,daemon 即编排宿主,无跨进程记账需求。
- **内置四档预设**:编译期 `include_str!` 嵌入 `scripts/group-chat-presets.json`
  (单一事实源保持 —— 改 JSON 需重编译 daemon 才生效;M1 CLI 读文件路径不变)。
- **冒烟**:`node scripts/group-chat-mcp-http-smoke.mjs`(前置 daemon 在跑;非 live =
  SDK 握手 + ping + tools/list + 预算 + 错误链 + list_presets/models + GET 405 /
  DELETE 200 / 406 / 415 传输探针;`--live` 烧真 token 走 start → wait_seconds 长轮询 →
  result 全链)。
- **挂载切换(P3 ✅ 2026-09-15;P4 退役同日)**:用户级配置 `everlasting-group-chat`
  已按两步走完成切换——别名 `everlasting-group-chat-http` 双挂验证宿主兼容后,原名条目
  原位换 `{"type":"http","url":"http://127.0.0.1:7456/mcp"}` 并删别名(保住
  `mcp__everlasting-group-chat__*` 工具前缀;双挂态备份 `config.json.p3-dual.bak`)。
  stdio 壳 / bun bin / deploy 脚本已随 P4 删除(任务 09-15-gce-mcp-stdio-retire),
  本端点为唯一 MCP 实现。
- ⚠️ **安全边界**:继承 daemon 全 API 零鉴权本机前提(§8);**remote tunnel 暴露
  `/mcp` 须先过安全评审**(roadmap §5「远程暴露认证」立项前置,本端点不改变该结论)。

## 7. 其他常用端点(路径约定)

全部为 `POST /api/v1/<domain>/<command>`,body snake_case,与 Tauri command 同名同参:
`sessions/*`(见 §3)、`permissions/*`(模式切换 / 审批回填 / trace 三条)、
`agent/chat`(发起轮次)、`agent/resume_group_chat`(群聊断点续跑,§4)、
`cancel/*`(Stop)、`message_queue/*`、`config/*`、
`background_shells/*`(list_background_shells / kill_background_shell,09-02)、
`disk/*`(get_disk_usage / run_disk_cleanup,09-03)、
`providers/*`、`usage/*`、`files/*`、`worktree/*`、`scheduled_tasks/*`。GET 端点:
`/api/v1/health`、`/api/v1/stream`(SSE)、`/api/v1/sessions/{id}/snapshot`,以及
二进制下载 `/api/v1/attachments/{session_id}/{file}`(B1 08-16)与 files 域三条
本地路径直连(两条取字节 + 一条存在性探针,见下);`/api/v1` 域外另有 MCP 端点
`/mcp`(POST/GET/DELETE 三态,§6.5);其余全 POST。

### files 域本地路径 GET 路由(09-13 起两条:图片 + 文件;09-14 加存在性探针)

聊天 markdown / 工具输出里识别出的本地路径,前端弹层直连 daemon 取字节,**不进
`CMD_TO_DOMAIN`**(GET binary 与 attachments 同一先例;`path` query 传参,pwa-remote
经 remote proxy catch-all 转发 + `?access_token=` query 鉴权)。

- `GET /api/v1/files/image?path=<abs|~/前缀>` — 本地图片字节(`<img>` 直连)
- `GET /api/v1/files/raw?path=<abs|~/前缀>` — 本地文本/pdf 字节(FileViewerModal
  fetch;pdf 由前端 `window.open` 同 URL 新标签,走浏览器原生 viewer)
- `GET /api/v1/files/stat?path=<abs|~/前缀>`(09-14)— 存在性探针:200 = 存在且
  是普通文件(body 空),404 = 不存在/非普通文件,**body 哨兵 `stat: file not
  found`**(前端区分"文件不存在"与陈旧 daemon 路由 fallback 的 404,字面量与
  `utils/pathExistence.ts` 成对持有)。前端 linkify **乐观渲染**链接后
  异步确认,确认缺失才把锚点降级回纯文本(`utils/pathExistence.ts`;抖动与缓存
  权衡见其模块注释)。白名单取 image ∪ raw **并集**(可链接 ⇔ 可探,oracle 面与
  两条读路由严格持平);`metadata` 一把 O(1),不读内容、无大小上限;`Cache-Control:
  no-store`(存在性是即时事实,文件随时可能被创建/删除)。

共同契约:`path` 必须是绝对路径或 `~/` 前缀(相对路径 400 —— 会话 cwd 只有前端
知道,由前端解析后调用);白名单外统一 400 不给存在性旁信道;校验全在
`commands/files.rs` 的 `read_image_at_inner` / `read_raw_at_inner` /
`stat_local_file_inner`,route 层只做错误码映射;`Cache-Control: private, max-age=60`
(stat 除外,见上)。

| | `/files/image` | `/files/raw` | `/files/stat` |
|---|---|---|---|
| 扩展白名单 | png / jpg / jpeg / gif / webp / bmp / avif / ico(svg 有意排除——独立文档打开时脚本会跑) | 文本类(md markdown txt log json jsonl csv tsv yaml yml toml ini conf cfg xml html htm css js mjs cjs jsx ts tsx vue svelte py rs go java kt kts c h cpp hpp cc cs rb php sh bash zsh fish sql proto graphql gql diff patch)+ pdf;svg 同样排除 | image ∪ raw **并集** |
| Content-Type | 按扩展映射 `image/*` | 文本类**一律** `text/plain; charset=utf-8`(.html/.htm 也一样——MIME 即闸门,任何消费方只见源码);pdf `application/pdf` | body 空 |
| 大小上限 | 32 MiB | 文本 2 MiB(整串进 DOM,防卡死 UI)/ pdf 32 MiB | 无(不读内容,metadata O(1)) |
| 额外校验 | — | 文本类严格 UTF-8,非法 → 400(二进制误命名当拒) | 非 `is_file`(目录误命名)→ 404 |
| 大小检方式 | metadata + 读后复核双检(TOCTOU 兜底),两路由同 | 同左 | 不适用 |
| 错误码 | 400 非白名单/相对路径 · 404 不存在/非普通文件 · 413 超限 · 500 IO | 同左 | 400 非白名单/相对路径 · 404 不存在/非普通文件 |

前端识别集(`utils/markdown.ts` `FILE_EXT` = 图片 ∪ 文本 ∪ pdf 单正则)与后端
白名单有意各持一份:后端是唯一安全闸门,前端集偏大只会点开见 400。对齐约定见
`.trellis/spec/frontend/chat/message-list-and-markdown.md` §5。

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
