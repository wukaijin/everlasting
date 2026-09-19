# design — `evl` CLI(daemon HTTP API 薄壳)

> 状态:brainstorm 收敛后的技术设计(2026-09-19)。wire 事实全部来自源码/Docker
> API 文档实读,标注出处。

## 1. 架构与边界

```
┌─────────── cli/ (零依赖,Node ≥20) ───────────┐
│ bin.mjs            入口:全局 flag 解析 + 分发  │
│ lib/args.mjs       参数解析(手写,无 commander) │
│ lib/api.mjs        HTTP client(base_url/超时/  │
│                    verbose/错误翻译)           │
│ lib/sse.mjs        SSE 手解(fetch+ReadableStream)│
│ lib/chat.mjs       chat 命令编排(时序见 §4)    │
│ lib/format.mjs     text/json 输出格式化        │
│ lib/commands/*.mjs status/sessions/projects/   │
│                    models/usage 子命令         │
└──────────────────────┬────────────────────────┘
                       │ HTTP :7456(零改动)
              Everlasting daemon
```

- **零 daemon 改动**:全部能力走既有 HTTP API(`docs/DAEMON-API.md`)。
- **零运行时依赖**:SSE 手解(§3),参数手解析;对标 `scripts/group-chat-run.mjs` 纪律。
- 纯函数(解析/匹配/格式化/SSE 帧切分)与 IO 分离,前者进 `node --test` 单测。

### 编排/运输分层规则(评审 09-19 采纳)

凡带编排语义的能力(群聊、未来 tasks)实现**唯一归属 daemon**;入口
(evl / MCP / GUI / 定时)只做**运输**。运输可任意增(薄),编排不可增
(编码策略:preset 合并/模型解析/busy 真相源)——stdio 壳退役(09-15 P4)
即「运输层长出策略」的历史成本实证。MVP 的 `evl chat` 走通用 REST 面
(`chat_inner` 单源)天然无分叉;二期 `evl discuss` 走 MCP client 调
`POST /mcp`(prd R2),同规则。

## 2. wire 契约清单(实读源码)

### 端点

| 用途 | 端点 | body | 出处 |
|---|---|---|---|
| health | `GET /api/v1/health` | — | DAEMON-API §7 |
| chat | `POST /api/v1/agent/chat` | `{request_id, session_id, messages:[{role,content}], resend_seq?, forced_dispatch?}` | routes/agent.rs:33 |
| SSE | `GET /api/v1/stream` | — | daemon/stream.rs |
| cancel | `POST /api/v1/cancel/cancel_chat` | `{request_id}` → `CancelOutcome` | routes/cancel.rs:14 |
| projects | `POST /api/v1/projects/list_projects` `{}` / `create_project` `{path}` | | routes/projects.rs:149 |
| sessions | `POST /api/v1/sessions/list_sessions` / `create_session` `{project_id, initial_cwd}` / `delete_session` `{session_id}` | | routes/sessions.rs:449, turn-smoke.sh |
| models | `POST /api/v1/providers/list_models` / `get_default_model` | | routes/providers.rs:268,273 |
| usage | `POST /api/v1/usage/usage_window` `{provider_id: null\|id}` → `UsageWindowReport` | | routes/usage.rs:33 |
| permission | `POST /api/v1/permissions/permission_response` `{rid, decision, reason?}`,decision ∈ `allow_once`/`allow_always`/`deny` | routes/permissions.rs:205, stores/permissions.ts:38 |
| session mode | `POST /api/v1/permissions/set_session_mode` `{session_id, mode}`,mode ∈ `plan`/`edit`/`yolo`(`plan`=只读零 ask+shell 只读沙盒面,`edit`=默认写触发 ask,`yolo`=自动批+硬拒保留+root guard;agent/permissions/mode.rs:13) | routes/permissions.rs:215 |

### SSE 事件面

event name 七种(`daemon/sse.rs:335-353`):`chat-event` / `tool:call` / `tool:result`
/ `permission:ask` / `tool:question` / `mode:change:request` / `task:state:transition:request`。
`chat-event` payload = `{request_id, session_id, ...event}`(`#[serde(flatten)]`,
state.rs),`event` 带 `kind` tag(snake_case),CLI 关心子集:

- `delta`(流式文本增量)/ `thinking_delta`(思考流,MVP 不渲染,--verbose 打行)
- `done`(终态,含 stop_reason)/ `error`(错误终态)
- `turn_usage`(token 核算:`usage.{input_tokens,output_tokens,cache_creation_input_tokens,cache_read_input_tokens,context_input_tokens}` + `context_window`,turn-smoke.sh:375-415 已锁形状)

**注意大小写差异**:`chat-event` payload snake_case,`permission:ask` payload
**camelCase**(`sessionId`/`toolName`/`toolInput`/`workerRunId`,
agent/permissions/payload.rs:21 `rename_all="camelCase"`)——CLI 各自按对应形状解。

### 关键语义(消费方必须遵守)

- **agent/chat 是 fire-and-forget**:立即返回 acceptance,事件全走 SSE;**SSE 订阅
  必须先挂再发**(turn-smoke RULE-SMOKE-001 教训;正常 turn 的终态判定 =
  `request_id` 匹配的 `kind=done`,不能拿 turn_trace 行或内层 Done 提前判)。
- **registry 全局广播**:所有 session 的事件都到达,CLI 必须按 `request_id` +
  `session_id` 过滤。
- **`has_live_observer()` 语义**(daemon/sse.rs:359):CLI 挂上 SSE 即成为 live
  observer,permission ask 会等满 120s 窗口等 CLI 应答——**CLI 必须对每个
  `permission:ask` 给出应答**,non-interactive 下立即主动 `deny`(比等 daemon
  无人值守快拒干净:不挂 120s、不依赖超时路径)。
- **单条 wire 已足够续聊**:turn-smoke `--turns 2` 同 session 连发单条 user wire,
  第二轮 cache_read 成立 = daemon 端自 rehydrate session 水位。`--session` 续聊
  同款(仅发新消息,不发历史)。
- **busy 群聊注入语义不适用于 CLI chat**:chat 打到 busy 群聊返回
  `{status:"injected"}` 无流(DAEMON-API §4)——CLI 收到 `status != "started"`
  即报错退出,不做群聊注入(discuss 二期的事)。

## 3. SSE 手解(fetch + ReadableStream)

Node 20 无全局 EventSource(22 起)。手解:

```
res = await fetch(`${base}/api/v1/stream`, { headers: { accept: 'text/event-stream' }, signal })
reader = res.body.getReader() → TextDecoder 流式累计
按 "\n\n" 切帧;帧内 "event: NAME" / "data: JSON"(data 可多行,拼接)
跨 chunk 边界:保留不完整尾部到下一轮
```

AbortController 三处触发:终态(kind=done/error)、SIGINT、超时。
产出 async iterator `events()` 吐 `{event, data}`;帧切分器做纯函数导出供单测。

## 4. `evl chat` 时序(双模:非 TTY = LLM 主场景)

```
1. resolve project   list_projects 按 CWD 匹配(path 规整后比对,
                     turn-smoke.sh:120 同款);无 → create_project
                     (--project <path> 跳过匹配直用)
2. resolve session   --session <id> 给定 → 直接用(不校验存在性,
                     400 报错自然冒出);否则 create_session
                     --mode 显式给定 → 校验值域(plan|edit|yolo,非法
                     退出 64)+ set_session_mode;未给定 → 默认双模:
                     TTY=edit(不动),非 TTY=plan(fail-closed,set 之)
                     --session 命中且 --mode 显式 → stderr 提示
                     "mode saved to session (persistent)"(§5)
3. 挂 SSE            AbortController 就位;订阅必须先于发 chat
                     (turn-smoke RULE-SMOKE-001)
4. POST agent/chat   request_id = `evl-${Date.now()}-${rand}`
                     messages = [{role:'user', content}](单条 wire)
                     acceptance.status != "started" → 报错退出
5. 消费循环          按 request_id 过滤;行为分模:
                     非 TTY(静默):delta 不渲染;permission:ask →
                       立即主动 deny(§5);kind=done|error → 终态
                     TTY(人类):delta → stdout.write;permission:ask →
                       y/a/n 交互;done|error → 终态
                     两模共同:tool:call|tool:result → --verbose 时
                       stderr 紧凑行;turn_usage → 收集(最后一条)
6. 收尾              --ephemeral → delete_session;正常 → stderr 打
                     session id(续聊提示);stdout 只出数据
```

- `--timeout`(默认 **540s** = 宿主 bash 上限 10min − 60s 余量;评审 09-19:
  规避宿主 SIGKILL 与 CLI cancel_chat 同刻开跑的赛跑)到点:先 `cancel_chat`
  再报错退出(码 7,§6)。**不变量:宿主 bash 超时 ≥ CLI `--timeout` + 60s**
  (§7.5)。超长任务等二期 detach 两段式。
- SIGINT:首次 → `cancel_chat` + 等 done(取消也是终态);二次 → 立即退出。
- `--output json`:stdout 单 JSON,形状恒定(成功/失败同一对象,LLM 解析器
  一条分支):`{text, usage, session_id, request_id, stop_reason,
  permission_denials, text_chars, error?}`;error 分支 `text=""` +
  `error:{kind,message}` + stop_reason 透传。text 模式 stdout = assistant 全文
  (两模均不截断;截断若二期做,必须 json 机器可读 `truncated:{kept,total}`,
  不做 stdout 带内标记)。

## 5. permission 语义(LLM-first 的核心张力)

- **TTY**:`permission:ask` stderr 打印 `toolName` + `toolInput` 摘要 +
  `[y]allow_once [a]allow_always [n]deny`,stdin 读单字符 → `permission_response`。
  读不到/EOF → `deny`。(CLI 挂 SSE 即 live observer,ask 等 120s 窗口,人在场
  合理。)
- **非 TTY(LLM)**:ask 到达即**主动 `deny`**(毫秒级)。设计取舍:不用
  「不挂 SSE 吃 GC3 8s 快拒」路线——CLI 需要挂 SSE 拿精确终态(kind=done/error
  单数据源,与 GUI 同款消费逻辑),那就必须处理每个 ask;主动 deny 比 8s 超时
  快且语义明确(拒绝原因可见)。**deny ≠ turn 失败**:agent 收到 denied 工具
  结果继续跑,turn 正常 done(权限策略靠 `--mode` 前置声明,§4 step 2)。
- **`--mode` 默认与校验**(评审 09-19):非 TTY 默认 `plan`(fail-closed——
  默认值即安全边界);TTY 默认 `edit`。值域 CLI 侧校验(plan|edit|yolo,非法
  退出 64)——daemon 侧 mode 解析 lenient(commands/permissions.rs:88 未知值
  静默回退 edit),CLI 不拦则 `--mode plna` 静默变 edit(fail-open)。
- **mode 持久化语义**:`set_session_mode` 写 `sessions.mode` 落库 + audit
  (commands/permissions.rs:112)——`--mode` 对既有 session 是**持久覆盖**
  而非本次调用临时语义;`--session` 命中时 stderr 提示 persistent。
  长期解法(per-request mode override)是 daemon 接口语义修正,本期不设计
  (prd 未决 #2)。
- **SSE 订阅外部性警告**(评审 09-19,仅文档化):registry 订阅者计数全局
  不分 session(daemon/sse.rs:365)——evl 订阅窗口内,**其他 session** 的
  权限 ask 从 8s 快拒变 120s 长等。群聊 turn 因 ask-free 不受影响。零
  daemon 改动前提下无解,知悉即可。
- `tool:question` / `mode:change:request`:两模一律 stderr 提示 + 忽略
  (daemon 侧超时自理;CLI 单发场景低频,daemon 实际超时行为未实测,
  prd 未决 #4)。

## 6. 退出码契约(对标 group-chat-run EXIT 表)

```
0  正常(done)          1  脚本自身错误(参数/网络/不可达)
2  chat kind=error      3  SIGINT cancelled
7  timeout(已 cancel)   64 用法错误(未知命令/缺参,usage 打 stderr)
```

daemon 不可达:非零 + OS 错误翻译(EPERM → "Operation not permitted",
group-chat-run.mjs:24 同款约定)+ `./scripts/daemon.sh bg` 提示。

## 7. 全局 flag 与 TTY 感知

`--base-url`(env `EVERLASTING_BASE`,默认 `http://127.0.0.1:7456`)/
`--output text|json` / `--quiet` / `--verbose`(直打 HTTP 请求+响应摘要与 SSE
事件名,LLM 调试用)/ `--timeout <s>`(默认 540)/ `--no-color` /
`--non-interactive` / chat 专属 `--mode plan|edit|yolo`(默认双模见 §5)/
`--session <id>` / `--ephemeral` / `--model <id>` / `--project <path>`。
TTY 判定 `process.stdout.isTTY`;非 TTY ⇒ 静默模式(流式关、ask 自动 deny、
默认 mode=plan)。

## 7.5 LLM 调用方契约(主要使用者接口面)

- **stdout 只出数据**(text=assistant 全文 / json=单行终态对象),人向信息
  (session id 续聊提示、工具行、权限交互、verbose)全 stderr——宿主 agent
  把 stdout 直接喂上下文,污染即噪声。
- **退出码**:§6 表;LLM 据 `error`/`cancelled`/`timeout` 分支重试或改策略。
- **json 终态形状(恒定,成功/失败同一对象)**:`{text, usage, session_id,
  request_id, stop_reason, permission_denials, text_chars, error?}`;
  `usage` 含四计费字段 + context_window(turn_usage 同形);error 分支
  `text=""` + `error:{kind,message}` + stop_reason 透传。`text_chars`
  供消费方自判长度(两模均不截断,评审 09-19 撤回截断方案)。
- **`permission_denials` 语义钉死**(评审 09-19):本 turn 内最终决策为 deny
  的 ask 计数,**不论拒绝者**(非 TTY 自动拒 / TTY 人按 n);CLI 自计数,
  不引 daemon 新依赖,退出码映射不为此加分支。
- **消费方指引**:text 模式按全文消费方(人类/管道)设计;**LLM 委派方一律
  `--output json`**(宿主截尾时 text 无声失败、json 响亮失败,本身即引导)。
- **权限**:非 TTY 默认 `plan`(fail-closed);委派写任务需显式 `--mode
  edit`(ask 全拒,靠 denied 语义继续)或 `--mode yolo`(自动批,硬拒规则
  仍生效)。
- **超时对齐**:默认 540s;不变量「宿主 bash 超时 ≥ CLI `--timeout` + 60s」。
- **发现面**:AGENTS.md 速查 + 后续 skill 文档(静态);不做 export-schema。

## 8. 权衡记录与已知限制

- **不做事件重连/补偿**:单发命令生命周期短,SSE 断 = 报错重试由用户重跑。
  MVP 不用 replay buffer(首连 last 未带时是**空回放**,daemon/sse.rs:204,
  依赖它是错的);二期 detach 的 wait 才复用 Last-Event-ID 续读。
- **宿主杀掉 evl 后 daemon 侧 loop 仍在跑**(已知限制,评审 09-19):宿主
  bash 超时 SIGKILL 先于 CLI `--timeout` 触发时,evl 来不及 cancel,
  `session_active_request` 残留 → session busy=true,agent loop 继续烧到自然
  结束。兜底:`evl sessions` 查 busy / GUI Stop;`--timeout 540` + 宿主余量
  60s 不变量(§7.5)就是为了让 CLI 的优雅 cancel 先赢。验收含负向用例
  (implement §6)。
- **detach 二期的数据面缺口**(评审 09-19,提前记此处防二期踩坑):daemon 无
  session 消息历史读路由、turn_trace 无 assistant 文本、SSE replay buffer
  512 ring 淘汰 + >256KiB 帧不入 buffer(早期 delta 补不回)。裁决指导原则:
  **daemon 加只读消息路由 > CLI 本地 spool**(破零 daemon 改动的代价小于破
  薄壳;策略侧留白比状态侧留白便宜)。
- **不校验 --session 存在性**:省一次往返,400 自然报错;错误文案透传 daemon。
- **stderr 承载一切人向提示**(session id/工具行/权限交互),stdout 只出数据
  (文本或 json)——管道组合第一原则。
- **测试面**:单测只覆盖纯函数(帧切分/参数/格式化/project 匹配);端到端
  live 验证走手动 + 后续可加 scripts/ 同款冒烟脚本(不进本任务 AC)。
