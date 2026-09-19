# daemon API wire 速查(CLI 消费面)

> 来源:docs/DAEMON-API.md + 源码实读(2026-09-19)。全文 35KB 超注入上限,
> 本文件是 CLI 消费面的提炼;原文按节号可查。

## 端点(全 POST 除注明的,body snake_case 除注明)

| 用途 | 端点 | body / 响应 |
|---|---|---|
| health | `GET /api/v1/health` | — |
| chat | `POST /api/v1/agent/chat` | `{request_id, session_id, messages:[{role,content}]}` → acceptance `{status:"started"\|"injected"}`;fire-and-forget,事件走 SSE |
| cancel | `POST /api/v1/cancel/cancel_chat` | `{request_id}` |
| projects | `POST /api/v1/projects/list_projects` `{}`;`create_project` `{path}` | 行含 `id`/`path` |
| sessions | `POST /api/v1/sessions/list_sessions`;`create_session` `{project_id, initial_cwd}` → `{id}`;`delete_session` `{session_id}` | SessionSummary 含 busy/stop_reason(§4) |
| models | `POST /api/v1/providers/list_models`;`get_default_model` | 模型引用只认 UUID |
| usage | `POST /api/v1/usage/usage_window` `{provider_id: null\|id}` | UsageWindowReport |
| permission | `POST /api/v1/permissions/permission_response` `{rid, decision, reason?}` | decision ∈ `allow_once`/`allow_always`/`deny`(stores/permissions.ts:38) |
| session mode | `POST /api/v1/permissions/set_session_mode` `{session_id, mode}` | mode ∈ `plan`/`edit`/`yolo`(agent/permissions/mode.rs:Plan 只读零 ask / Edit 默认写触发 ask / Yolo 自动批+硬拒保留;root guard)。**daemon 解析 lenient**:未知/空 mode 静默回退 edit(commands/permissions.rs:88)→ CLI 必须自己校验值域;写 `sessions.mode` 持久化 + audit(不可逆 per-call 语义,per-request override 未排期) |

## SSE(`GET /api/v1/stream`)

- event name:`chat-event` / `tool:call` / `tool:result` / `permission:ask` /
  `tool:question` / `mode:change:request` / `task:state:transition:request`。
- `chat-event` payload snake_case:`{request_id, session_id, kind, ...}`;
  kind 子集 `delta` / `thinking_delta` / `done`(含 stop_reason) / `error` /
  `turn_usage`(usage.{input,output,cache_creation,cache_read,context_input}_tokens
  + context_window)。
- **`permission:ask` payload 是 camelCase**(agent/permissions/payload.rs):
  `{rid, sessionId, toolUseId, toolName, toolInput, risk, path?, workerRunId?}`。
- registry 全局广播:按 request_id + session_id 过滤;先挂订阅再发 chat
  (turn-smoke RULE-SMOKE-001);挂上即 live observer(ask 等满 120s 等 CLI 应答,
  daemon/sse.rs has_live_observer)。

## 语义要点

- 单条 user wire 足够续聊(daemon 自 rehydrate session 水位;turn-smoke --turns 2
  cache_read 实证)。
- chat 打到 busy 群聊 → `{status:"injected"}` 无流;CLI 视非 "started" 为错误。
- **GC3 无人值守**(DAEMON-API §5):无 SSE 观察者时 ask 等 120s→8s 快拒;
  挂 SSE 即 live observer(120s 窗口)。CLI 设计选择挂 SSE 拿精确终态 +
  非 TTY 主动 deny(见 design §5 取舍)。
- 零鉴权本机前提(§8):不引入任何 auth;远程暴露不在本期。

## 关键参考实现(仓库内)

- `scripts/turn-smoke.sh` — project 解析/建 session/SSE 订阅/终态等待/清理全链。
- `scripts/group-chat-run.mjs` — 轮询驱动、退出码契约、daemon 不可达错误文案
  (EPERM → "Operation not permitted")。
