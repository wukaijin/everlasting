# N20 调研:ACP (Agent Client Protocol) 接入 —— 现状缺口 / 改动面 / 取舍

> 调研日期:2026-09-29。对照材料:ACP v1 schema(`agentclientprotocol/agent-client-protocol` main 分支 `schema/v1/schema.json`,2026-09-29 拉取,247KB / 170 defs)+ crates.io API;本项目代码实读(行号为 2026-09-29 main 现状,核心核对由代码扫描完成,关键结论均有 文件:行号)。
> 调研问题(BACKLOG 附录 C.2 N20 行):ACP 协议面与 everlasting daemon 客户端面的映射完整度、改动面量级、技术选型与取舍,为 N20 是否立项及范围裁定提供依据。

## 结论速览

| 维度 | 判定 | 一句话依据 |
|------|------|-----------|
| 架构路线 | **shim bin 连回 daemon,agent core 零改动(MVP)** | daemon 已服务 4 种客户端形态(Tauri Thin / 浏览器 SPA / 远程 PWA / evl CLI),`evl chat` 已完整验证「外部瘦客户端驱动一整轮 LLM turn」通路 |
| 协议匹配度 | **高:核心面近乎 1:1** | 权限选项 `allow_once/allow_always/deny` 与 ACP PermissionOption 同名同义;文本/thinking 均 token 级流直映射;cancel/mode/session 生命周期全有端点 |
| 最大缺口 | 在途 permission ask 无恢复面 | `pending_interaction` 枚举无 Permission 变体(question_store.rs:324-342),迟到/断线订阅者只能吃 daemon 快拒(sse.rs:356-367)——MVP 用「shim 启动即挂 SSE」规避(evl 同款),残余缝记 follow-up |
| 体验降级(可接受) | tool 执行无流式中间输出 | `tool:call`/`tool:result` 两个终态事件(event.rs:100-107 / state.rs:786-794),ACP 状态机降级为 pending→completed/failed 两态跳变——协议合法,仅长命令在编辑器里看不到增量输出 |
| Rust crate | **官方 crate 存在且活跃,选型问题消失** | `agent-client-protocol`(repo `agentclientprotocol/rust-sdk`)最新 2.2.0(2026-09-18)、累计 475 万下载;1.x 线(1.3.0,2026-07-20)对应协议 v1 stable |
| 工作量 | **中:4 PR,与 N2 checkpoint 同级,远小于 daemon 化 epic** | 全部改动集中在新 bin(shim 翻译层)+ 测试 + 文档;daemon 侧 MVP 零改动,follow-up 仅一条小增量 |
| 版本锚定 | 协议 v1 stable + crate 1.x | v2 in development(crate 2.x `unstable_*` features:session_fork / plan_operations / mcp_over_acp 等),不跟 |

**总评**:N20 是「能力面扩展」而非「缺口修补」,但工程量被现有客户端面大幅压薄——`evl chat` 把最难验证的「外部进程完整驱动一轮 turn(SSE 消费 + 权限应答 + 取消 + 终态判定)」已经趟通,ACP shim 本质是「把同一消费循环的输出端从 stdout JSON 换成 ACP JSON-RPC」。建议立项:4 PR MVP「Zed 里可用 everlasting」,优先级 P2。

---

## 0. 背景与动机

ACP = Zed Industries 2025 年开源的编辑器↔agent JSON-RPC 协议(agentclientprotocol.com),定位类比 MCP:MCP 管 agent↔工具,ACP 管 client↔agent。agent 作为**子进程被编辑器 spawn**,stdio 通信。2026-09 现状:协议 v1 stable / v2 开发中;生态迁到 `agentclientprotocol` GitHub org(官方 + Claude/Codex/Gemini 等适配器);Zed 是参考客户端。

两大 harness 调研均把 ACP 列为入口形态:dsh 五形态之一(Web/Electron/headless/SDK/ACP,`docs/_history/research/deepseek-harness-survey.md` §1)、mcode 三入口之一(TUI/exec/ACP,`docs/_history/research/minimax-code-survey.md` §1.1)。everlasting 接入 ACP 的收益:Zed(及任何 ACP 客户端)用户直接用 everlasting 当 agent 后端——GUI/浏览器/CLI 之外的第四类触达面,且是唯一的「嵌进既有编辑器工作流」形态。

## 1. ACP 协议面盘点(v1 stable)

### 1.1 传输与生命周期

- agent 二进制被客户端 spawn,stdin/stdout 按行 JSON-RPC(stderr 留给 agent 日志);
- 握手 `initialize`(客户端→agent)→ 之后每 session 一个 `sessionId`,`session/prompt` 一问一答(request 响应携带 `stopReason`);
- agent 可随时向客户端发**反向请求**(permission / fs / terminal / elicitation)与**通知**(`session/update` 流);
- Zed 注册:`settings.json` 的 `agent_servers` 条目 `{type:"custom", command, args, env}`,Agent Panel 新建线程菜单即出现;调试走 `dev: open acp logs`。某些 CLI 支持 self-register(`pool acp setup --editor zed` 自动写配置)。

### 1.2 方法全集(agent 侧实现 / client 侧反向调用)

agent 侧:`initialize`、`authenticate`/`logout`(可空)、`session/new`、`session/load`(能力门)、`session/list`/`delete`/`close`(能力门)、`session/prompt`、`session/cancel`(通知)、`session/set_mode`、`session/set_config_option`。

client 侧:`session/update`(通知流)、`session/request_permission`、`fs/read_text_file`/`fs/write_text_file`(能力门)、`terminal/*` 五方法(能力门)、`elicitation/*`(能力门)。

**能力协商是核心减负机制**:initialize 响应的 `agentCapabilities` 中所有可选能力(`loadSession` / `promptCapabilities.image/audio/embeddedContext` / `fs` / `terminal` / `mcp` / `session.list/delete/close` …)默认 false,不声明即不需要实现——fs/terminal/elicitation 全部让 agent 自持工具承担,是协议设计内的合法降级。

### 1.3 session/update 11 变体(v1 schema 实测提取)

`user_message_chunk` / `agent_message_chunk` / `agent_thought_chunk`(均混入 ContentChunk)/ `tool_call`(混入 ToolCall)/ `tool_call_update`(混入 ToolCallUpdate)/ `plan` / `available_commands_update` / `current_mode_update` / `config_option_update` / `session_info_update` / `usage_update`。

关键结构:
- `ToolCallStatus` 四态:`pending`(输入流中或待审批)/ `in_progress` / `completed` / `failed`——**允许跳变**(pending 直达 completed 合法,这正是 everlasting 两态映射的依据);
- `ToolCallUpdate.content` 是 ToolCallContent 集合(替换语义非追加),含 title / locations(代码位置引用,可省)/ rawInput / rawOutput;
- `PromptRequest.prompt` 为 ContentBlock 数组,基线必须支持 `Text` 与 `ResourceLink`,Image/Audio/EmbeddedContext 由能力门控。

### 1.4 关键枚举

- `StopReason`(prompt 响应):`end_turn` / `max_tokens` / `max_turn_requests` / `refusal` / `cancelled`;
- `PermissionOptionKind`:`allow_once` / `allow_always` / `reject`(选项结构 `{optionId, name, kind}`,agent 自造 optionId,响应用 optionId 回选);
- `NewSessionRequest`:`{cwd, additionalDirectories?, mcpServers?}`——`cwd` 就是客户端工作区目录(Zed 打开的项目根)。

### 1.5 Rust crate

`agent-client-protocol`(crates.io,repo `agentclientprotocol/rust-sdk`):2025-07 首发,2026-09-18 发布 2.2.0,累计下载 475 万、近 90 天 183 万——活跃官方实现。版本线:1.x(最新 1.3.0,2026-07-20)对应协议 v1 stable;2.x 带 `unstable_*` feature 门(session_fork / plan_operations / mcp_over_acp / session_compaction / end_turn_token_usage / llm_providers / session_notices)。crate 定位「Core protocol types and traits」(schema 类型 + 连接 trait),stdio 循环薄。**选型结论:锚 crate 1.x + 协议 v1,不碰 2.x unstable。**

## 2. everlasting 现状核对(六点,证据行号)

### 2.1 SSE 事件面

全局单流 `GET /api/v1/stream`(非 per-session),事件带全局递增 id(Last-Event-ID 重放)+ 512 帧 replay buffer + 30s ping。事件名 10 个(`daemon/sse.rs:333-368`):`chat-event` / `tool:call` / `tool:result` / `permission:ask` / `tool:question` / `mode:change:request` / `task:state:transition:request` + worker 两态(`subagent:event`/`subagent:finished`)+ 断线 sentinel `stream-resync`。

`chat-event` 的 kind = `ChatEvent` 枚举 20 种(`llm/types/event.rs:64-312`):ACP 相关的核心是 `delta`(token 级,provider `text_delta` 直映射,`llm/provider/anthropic/events.rs:118-130`)、`thinking_delta` / `signature_delta` / `redacted_thinking_delta`、`tool_call`、`done{stop_reason, usage}`、`error`、`retrying`、`turn_usage`。

tool 执行只有终态:`tool:call` 在 tool_use block 组装完后一次性发(`event.rs:100-107`),`tool:result` 只有 `{request_id, session_id, tool_use_id, content, is_error, images?}`(`state.rs:786-794`);后台 shell 输出仅能轮询,不入 SSE。

**零订阅者 = unattended 快拒**(`sse.rs:356-367`):无 SSE 订阅者时权限 ask 被快速拒绝——shim 必须在 POST chat 前先挂 SSE(evl 已把它固化为 RULE-SMOKE-001)。

### 2.2 chat 驱动

`POST /api/v1/agent/chat`,body `{request_id, session_id, messages: Vec<ChatMessage>, resend_seq?, forced_dispatch?}`(`daemon/routes/agent.rs:33-42`)——messages 由客户端构造(evl 只发单条 user 消息),无 project_id(session 绑定)。响应 = fire-and-forget 受理结果 `ChatAcceptance`(`agent/chat.rs:295-311`):`started` / `queued{id,position}` / `injected`(群聊)。turn 终态从 SSE `done{stop_reason}` 读;cancel 路径发 `stop_reason:"cancelled"`(`commands/cancel.rs:5-8`)。

### 2.3 审批/ask 桥

- 权限 ask:只活在 `permission:ask` SSE 事件(camelCase payload `{rid, sessionId, toolUseId, toolName, toolInput, risk, reason?, path?, workerRunId?, grantPattern?}`,`agent/permissions/payload.rs:20-62`)+ 服务端 oneshot 等待。应答 `POST /api/v1/permissions/permission_response` `{rid, decision: "allow_once"|"allow_always"|"deny", reason?}` → `{resolved: bool}`(`daemon/routes/permissions.rs:230-253`,false = rid 未知/超时)。
- **`pending_interaction` 枚举无 Permission 变体**(`agent/question_store.rs:324-342`,只有 Question/ModeChange/TaskStateTransition/LoopIntervention/TurnLimitSoftcap)——在途权限 ask 不进快照/轮询面,这是 shim 视角最大的结构缺口。
- 三类非权限卡的 resolve 端点齐全(`daemon/routes/question.rs:108-118`),GUI 消费中;evl 明确忽略(tool:question 等 daemon 超时自理)。

### 2.4 cancel

`POST /api/v1/cancel/cancel_chat` `{request_id}` → `{cancelled, cleared_queued}`(`daemon/routes/cancel.rs:18-29`)。协作式:agent loop 的 select 在事件边界退出,部分轮持久化 + 发 `done(stop_reason:"cancelled")`;rid 不存在静默 no-op(`commands/cancel.rs:36-86`)。

### 2.5 session / project / mode API

- `create_session {project_id, initial_cwd, model?, session_type?, metadata?}`(`daemon/routes/sessions.rs:50-77`)——**无 mode 字段**,创建后调 `set_session_mode`;
- `list_sessions` / `load_session`(messages 全量可读)/ `GET snapshot`(断线恢复)齐备;
- project:`create_project {path}` + `list_projects`,**无按 path 查找端点**——客户端本地比对(evl 词规整 resolve 比对,`cli/lib/chat.mjs:26-31`);
- mode:`set_session_mode {session_id, mode}` 值域 `edit|plan|yolo`(`commands/permissions.rs:103-106`),运行中可切、未知值静默回退 edit;
- worktree:opt-in 三态,**默认 none = 工具 cwd 直落 project.path**(`docs/ARCHITECTURE.md:240-245`)——ACP session 走默认态即直接操作 Zed 工作区目录,零 worktree 改动。

### 2.6 evl CLI 先例(通路已验证)

`cli/lib/chat.mjs` 全链:project 解析(list 比对/建)→ create_session → set_session_mode → **SSE 先挂** → POST agent/chat(要求 `started`,queued/injected 报错)→ 按 `request_id` 过滤全局流消费(delta/thinking/retrying/turn_usage/done/error;tool 事件仅 verbose 日志;permission:ask 按 sessionId 过滤后应答)→ 超时/SIGINT → cancel_chat + 终态宽限。已知实现坑:payload 命名不对称(permission:ask 是 camelCase,chat-event 是 snake_case)。

## 3. 全量映射表(ACP ↔ daemon)

| ACP 面 | everlasting 面 | 难度 / 说明 |
|---|---|---|
| `initialize` | shim 握手;声明 loadSession=true、promptCapabilities 全 false(基线)、fs/terminal/mcp 不声明 | 低,纯声明 |
| `session/new {cwd}` | list_projects 本地比对 → 无则 create_project → create_session{initial_cwd} | 低,evl 同款;worktree 默认 none |
| `session/prompt` | SSE 先挂 → POST agent/chat{messages:[{role:user,content}]} → 按 request_id 过滤消费 | 低,evl 已验证 |
| `user_message_chunk` | (可选 echo prompt 文本) | 零 |
| `agent_message_chunk` | `ChatEvent::delta`(token 级) | 低,直映射 |
| `agent_thought_chunk` | `thinking_delta`(signature/redacted 忽略) | 低 |
| `tool_call`(pending) | `tool:call`(ToolCallPayload) | 中:toolCallId = tool_use_id;locations 无源不带 |
| `tool_call_update` | `tool:result{content,is_error}` → completed/failed;`in_progress` 无事件源,跳过 | 中:两态合法跳变;content 块结构转换 |
| `plan` | 无直接源(workflow checklist/breadcrumb 最近) | MVP 不发,记增强 |
| `available_commands_update` / `config_option_update` | 无源(/command 是 GUI 面) | 不发 |
| `current_mode_update` + `session/set_mode` | `set_session_mode{edit/plan/yolo}`;SessionMode 自定义 id/name/kind | 低 |
| `usage_update` | `turn_usage` | 低,可选 |
| `session_info_update` | session rename API | 可选,后置 |
| `session/request_permission` | `permission:ask` → 反向请求;options = allow_once/allow_always/reject;响应回 `permission_response{rid, decision}` | **中,核心桥**;语义同名同义 |
| `session/cancel` | `cancel_chat{request_id}` | 低 |
| prompt 响应 `stopReason` | SSE `done{stop_reason}` 值域映射(error→refusal 等) | 低,实施时对照值域表 |
| `session/load`(重放) | `load_session` → messages 生成 update 流(文本→chunk;tool_use/result→tool_call completed + update) | 中 |
| `session/list` | `list_sessions`(按 sessionCapabilities.list) | 低,可选 |
| `fs/*` / `terminal/*` / `elicitation/*` | 不声明能力(agent 自持 FS/shell 工具) | 零 |
| `authenticate` / `logout` | 本机零鉴权,authMethods=[] | 零 |

## 4. 三段式结论

### 4.1 现状缺口(5 项)

1. **在途 permission ask 无恢复面**(结构缺口):`pending_interaction` 无 Permission 变体,迟到/断线重连订阅者拿不到在途 ask,只能吃快拒。MVP 规避 = shim 启动即挂 SSE 且全程保持(evl 同款,ask 必有订阅者);残余缝 = shim 自身重启瞬间的在途 ask 丢失 → follow-up 小增量:Permission 入 pending_interaction / snapshot(~20-40 行,daemon 侧)。
2. **tool 无流式中间输出**(体验缺口):两态跳变协议合法;长命令(前台 shell)在 Zed 无增量输出。ACP 客户端无进度也不阻塞;记注,不实施。
3. **queued 受理无流可等**(语义缺口):busy 时 `queued` 无 per-request 流。MVP 同 evl:拒绝并报错(Zed 单人场景撞 busy 概率低);ACP prompt 本身串行。
4. **daemon 拉起策略**(部署缺口):shim 被 Zed spawn 时 daemon 未必在跑。MVP = 健康检查失败即 initialize 报错(信息里给 `daemon.sh` 指引);自动 spawn 记增强(须处理路径发现/生命周期归属)。
5. **payload 命名不对称**(实现坑):permission:ask camelCase vs chat-event snake_case——shim 内部统一归一层,写测试锚钉死。

### 4.2 改动面

**全部主体 = 新 bin `everlasting-acp`**(workspace 新成员或 app/src-tauri 附加 bin,依赖官方 crate 1.x + reqwest/事件循环;复用 evl 验证过的消费循环逻辑,但用 Rust 重写):

- **PR1 生命周期**:stdio JSON-RPC 循环 + initialize + session/new/load/list/close + project path 解析(~400-600 行);
- **PR2 翻译层**:prompt 驱动 + SSE 消费 → update 流(delta/thinking/tool_call/tool_call_update/usage/stopReason 映射表全落地)(~300-500 行);
- **PR3 交互桥**:request_permission 反向调用 ↔ permission_response、cancel、set_mode/current_mode_update(~150-300 行);
- **PR4 收口**:测试(ACP 客户端 mock:起 shim 子进程对驱动,断言 update 序列与权限环)+ Zed 实测清单 + 文档(agent_servers 注册指引、daemon 前置条件)+ CI 接线(~300 行测试)。

daemon 侧 MVP **零改动**;唯一 follow-up = 缺口 1 的 Permission 恢复面(独立小增量,不阻塞)。对照量级:N2 checkpoint(4 PR)同级,远小于 daemon 化/remote epic;外部依赖仅官方 crate。

### 4.3 取舍(8 项决策)

1. **Rust bin vs Node shim(抄 evl)**:Rust——官方 schema crate 类型安全、单二进制部署、unstable feature 门控;Node 免编译但要手写协议层且引入 Node 运行时依赖。裁:Rust。
2. **版本锚定**:协议 v1 stable + crate 1.x;v2/2.x unstable 不碰(协议 v2 尚未定稿)。
3. **queued**:拒绝并返回错误,不做排队重放。
4. **worktree**:ACP session 恒走 none 态(直连 cwd,编辑器语义);不暴露 attach。
5. **图片**:MVP text-only(promptCapabilities 默认);B1 wire 面已支持图片,增强期开 image=true + ContentBlock 转换(小增量)。
6. **MCP over ACP**(NewSessionRequest.mcpServers):不声明 mcpCapabilities——everlasting agent 自有工具/MCP 面,不中转。
7. **session 模型**:ACP session:new 1:1 建 daemon session;session/list 直映 list_sessions(同 project 过滤)。
8. **鉴权**:本机零鉴权前提,与 `/mcp` 端点同前提(远程暴露须安全评审,不在本任务范围)。

## 5. 立项建议

**立项**:N20 = 「`everlasting-acp` shim MVP:Zed 可用」——PR1-4 如上,daemon 零改动,验收 = Zed 注册后可建 session、跑 turn(文本/thinking/tool 流可见)、权限审批经编辑器 UI、可取消、可重载历史;Permission 恢复面记 follow-up 缺口。优先级 P2(能力扩展而非缺口修补,但工程量已被 evl 通路与官方 crate 压薄)。前置依赖:无(与 N15 LSP 等候选正交)。
