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
  编排双实现)。
