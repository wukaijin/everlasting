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
  `group_chat_end`(正常收官)/ `max_rounds`(30 轮帽截断)/ `cancelled`(用户 Stop)/
  `error`(连续 3 轮生成错误熔断)。每次新编排启动时清空再回填。
- **`discussion_summary`**(`SessionRow` only,即 `load_session` 可得):moderator
  `end_discussion({summary})` 的收官总结一等字段——共识清单直接可读,无需解析
  end_discussion 的 tool_result content blocks。仅正常收官路径写入。
- 终止的实时信号:`GET /api/v1/stream`(SSE)上编排器的终端 `Done` 事件
  `stop_reason` 与上表同值;SSE 不回放完整历史,断连后用 snapshot 端点补齐。

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

## 7. 其他常用端点(路径约定)

全部为 `POST /api/v1/<domain>/<command>`,body snake_case,与 Tauri command 同名同参:
`sessions/*`(见 §3)、`permissions/*`(模式切换 / 审批回填 / trace 三条)、
`agent/chat`(发起轮次)、`cancel/*`(Stop)、`message_queue/*`、`config/*`、
`providers/*`、`usage/*`、`files/*`、`worktree/*`、`scheduled_tasks/*`。唯一 GET:
`/api/v1/stream`(SSE)与 `/api/v1/sessions/{id}/snapshot`。
