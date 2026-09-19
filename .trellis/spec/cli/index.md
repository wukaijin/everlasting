# cli 层 Spec — `evl` CLI(daemon HTTP API 薄壳)

顶层 `cli/` 目录(2026-09-19,任务 `09-19-everlasting-cli`):把 daemon 能力开放为
bash 可调用接口,**主要使用者是 LLM**(宿主 agent 经 bash 工具委派任务给
Everlasting agent loop),人类 TTY 次之。完整用户面契约在
[`cli/README.md`](../../../cli/README.md)(安装/命令表/退出码/LLM 调用方契约),
wire 契约速查在任务 `research/daemon-api-wire-notes.md`,权威源
[docs/DAEMON-API.md](../../../docs/DAEMON-API.md)。

## 纪律(与 scripts/ 同款)

- **零运行时依赖、零构建**:SSE 手解(fetch + ReadableStream),参数手解析;
  不引 commander/eventsource;Node ≥ 20(注意 `import ... with { type: 'json' }`
  20.10 才稳定——曾踩,改 `readFileSync(new URL(...))`)。
- **单测 `node --test`**(vitest 不收此目录);只测纯函数(帧切分/参数/格式化),
  不打真 daemon。
- **stdout 只出数据,stderr 承载人向提示**(session id/工具行/权限交互/verbose)
  ——宿主 agent 把 stdout 直接喂上下文。

## 关键契约(gotcha 实录)

- **SSE 大小写陷阱**:`chat-event` payload snake_case,`permission:ask` payload
  **camelCase**(`rid/sessionId/toolName/toolInput`,agent/permissions/payload.rs
  `rename_all="camelCase"`)——消费方各按各的形状解。
- **SSE 先挂再发 chat**(RULE-SMOKE-001);挂上即 live observer,ask 走 120s 窗口
  等 CLI 应答——CLI 必须处理每个 ask(非 TTY 立即主动 deny;deny≠turn 失败,
  agent 消化 denied 结果继续跑)。
- **daemon mode 解析 lenient**(未知值静默回退 edit)——`--mode` 值域必须消费方
  自己校验;mode 是 session 持久属性(落库+audit),非 per-call。
- **编排/运输分层**(评审 09-19 定案):凡带编排语义的能力(群聊、未来 tasks)
  实现唯一归属 daemon;入口(evl/MCP/GUI/定时)只做运输。二期 `evl discuss`
  走 MCP client 调 `POST /mcp`,不 import group-chat-run(那会复活已退役的 JS
  编排双实现)。分层判据显式化(评审 09-19-evl-discuss):**随入口变化的 =
  运输**(CLI 循环/bash 自轮询/GUI 轮询三入口三窗口);**塑造讨论本身的 =
  编排**。
- **入口窗口不变量**(评审 09-19-evl-discuss 架构补充,预置 R2 剩余项
  tasks/detach 免重议):每条 evl 阻塞调用 ≤ 宿主窗口;长工作经 session
  锚点跨调用续窗。`--timeout`/`--wait` 默认与上限同源 540(宿主 bash 上限
  10min − 60s 余量)。

## MCP 运输面 gotcha(`evl discuss`,任务 09-19-evl-discuss,2026-09-19)

cli 第二条运输面(REST `/api/v1/*` 之外):JSON-RPC `tools/call` 直发
`POST /mcp`(实现 `cli/lib/mcp.mjs`)。契约细节:cli/README.md + 任务
research/mcp-wire-evidence.md;此处只留坑:

- **双 Accept 头是硬约束**:`Accept: application/json, text/event-stream`
  两值都带(缺 text/event-stream → 406);Content-Type 必须
  `application/json`(否则 415)。
- **免 initialize**:端点无状态,逐请求独立分发;握手纯开销。
- **工具错误不走 JSON-RPC error**:200 + `result.isError:true`,text 为
  `{error[, hint]}`;JSON-RPC error(-32601/-32602)只留给未知工具/方法。
  成功载荷 = `content[0].text` 的 pretty JSON **二次 parse**。
- **`wait_timed_out` 键仅在 true 时写**(mcp.rs build_status_snapshot):
  变化即返时键**缺失**——变化判据必须 `!== true`;写 `== false` 会把变化
  检测整体反转(评审 09-19 抓出的实锄,单测有用例显式命名防回归)。
- **`stop_reason` 是开放集**(group_chat_loop.rs 常量 8 值 + agent.rs
  `interrupted`):消费方按开放集处理,表外值回显不吞;`preempted` 是
  interrupt 自产真终态(≠ cancelled 硬停),`interrupted` 是可续跑态。
- **`discussion_result` 载荷无 session_id 键**:CLI json 输出自补。
- **discuss 超时不 cancel**(与 chat 反义,同退 7):timeout 是调用方窗口
  属性,cancel 是工作属性;退 7 时讨论仍在 daemon 跑——防重跑文案三处
  (stderr 报告行/json recovery 字段/HELP)写死"勿重跑",否则 LLM 消费方
  按退 7=已止损的 chat 语义重试即双花。
