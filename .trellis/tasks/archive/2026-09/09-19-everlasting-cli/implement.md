# implement — `evl` CLI 执行计划

> 前置:prd.md / design.md 已评审。执行顺序即依赖顺序;每步带验证命令。

## 0. 脚手架

- [ ] `cli/package.json`:`"name": "everlasting-cli"`, `"private": true`,
      `"type": "module"`, `"bin": {"evl": "./bin.mjs"}`, `"packageManager":
      "pnpm@11.24.0"`,`"engines": {"node": ">=20"}`;无 dependencies。
- [ ] `cli/bin.mjs`:shebang `#!/usr/bin/env node`;`--help` / 未知命令骨架
      (退出码 64)。
- 验证:`node cli/bin.mjs --help` 出用法;`node cli/bin.mjs nope; echo $?` → 64。

## 1. lib/args.mjs — 参数解析(纯函数)

- [ ] 手写解析器:全局 flag(§design 7)+ 子命令位置参数 + `--key value` /
      `--flag` 重复与布尔;未知 flag 报用法错误。
- [ ] 单测:`cli/args.test.mjs`(node --test;cases:全局 flag 提取/子命令分流/
      缺参/未知 flag/EVERLASTING_BASE 默认值)。

## 2. lib/api.mjs — HTTP client

- [ ] `post(path, body)` / `get(path)` 封装:超时(AbortSignal)、verbose 请求
      响应摘要打 stderr、非 2xx 抛带 daemon 错误文案的 Error、连接失败翻译
      OS 错误(cause.code EPERM → "Operation not permitted",group-chat-run
      同款)+ 附 `./scripts/daemon.sh bg` 提示。
- [ ] 单测:错误翻译纯函数(errno 映射表)可测;fetch 本身 mock 不做(端到端
      手动验)。

## 3. status + 内省四件套

- [ ] `lib/commands/status.mjs`:GET health;text/json 两档输出。
- [ ] `lib/commands/list.mjs`(sessions/projects/models/usage 四命令):
      各自调端点 → `lib/format.mjs` 表格(text)/ 单行 JSON(json)。
      models 合并 `get_default_model` 标默认;usage 透传 `--provider`。
- [ ] 单测:`format.test.mjs`(表格列选取/JSON 单行/空列表)。
- 验证(daemon 在跑,手动):`node cli/bin.mjs status && node cli/bin.mjs sessions
  --output json | jq .` 四件套各跑 text + json。

## 4. lib/sse.mjs — SSE 手解

- [ ] `parseSseChunk(buffer)` 纯函数:输入累计字符串,输出 `{frames, rest}`
      (按 `\n\n` 切帧,`event:`/`data:`(多行拼接)解析,冒号后单空格剥离)。
- [ ] `subscribe(base, signal, verbose)` → async iterator `{event, data}`
      (fetch + getReader + TextDecoder,消费 §design 3)。
- [ ] 单测:`sse.test.mjs`(整帧/跨 chunk 拆两半/多行 data/CRLF 容忍/注释行忽略)。

## 5. lib/chat.mjs — chat 命令(核心)

- [ ] resolveProject(cwd 匹配纯函数 + API 调用;`--project` 旁路)。
- [ ] 时序编排(§design 4):session 解析 → mode 解析(显式 `--mode` 值域校验
      plan|edit|yolo,非法退出 64;未给定默认双模 TTY=edit/非TTY=plan)→
      set_session_mode → SSE 先挂 → agent/chat → request_id 过滤消费 → 终态。
      `--session` + 显式 `--mode` → stderr 提示 persistent。
- [ ] 双模消费:非 TTY 静默(delta 不渲染、ask 立即主动 deny);TTY 流式渲染 +
      y/a/n 交互;`tool:question`/`mode:change` stderr 提示 + 忽略。
- [ ] SIGINT(首次 cancel_chat 等 done / 二次硬退)、`--timeout`(默认 540s)
      到点 cancel。
- [ ] `--ephemeral` 删 session;正常路径 stderr 打 session id 续聊提示。
- [ ] `--output json` 终态结构(§design 7.5 恒定形状):done 分支
      `{text, usage, session_id, request_id, stop_reason, permission_denials,
      text_chars}`;error 分支同对象 `text=""` + `error:{kind,message}` +
      stop_reason 透传;`permission_denials` = 本 turn 最终 deny 的 ask 计数
      (CLI 自计数,双模同义)。
- [ ] 单测:resolveProject 匹配逻辑、`--mode` 值域校验(非法→64)、终态分类
      (done/error/cancel)、json 两分支输出形状、denials 计数。
- 验证(手动,live):
  - 非 TTY(LLM 主场景):`node cli/bin.mjs chat "只回一句问候" --output json
    < /dev/null | jq .usage.input_tokens` → 数值 > 0,stdout 单 JSON,
    且默认 mode 落 plan(不带 --mode)
  - `--session <id>` 续聊上下文连续;`--ephemeral` 后 sessions 不见
  - `--mode edit` 发触发写权限的消息 → ask 主动 deny 无 8s 挂起,turn 正常
    done,`permission_denials > 0`
  - `--mode plna` → 退出 64
  - TTY:流式渲染;`--mode plan` 只读任务零交互跑通;Ctrl-C → exit 3 session 保留

## 6. 收尾

- [ ] `cli/README.md`:安装(pnpm link --dir cli)/命令表/退出码表/LLM 调用方
      契约(json 优先、mode 默认、超时不变量宿主 bash ≥ --timeout+60s)。
- [ ] AGENTS.md 增补一行 CLI 速查(§速查区,docs/DAEMON-API.md 不动)。
- [ ] 负向用例(评审 09-19):模拟宿主超时杀 CLI(`--timeout 540` +
      `timeout 5 kill`)→ `evl sessions` 见 busy=true(loop 仍在跑),验证
      540s+60s 余量设计;GUI Stop 可兜底。
- [ ] 全量验证:`node --test cli/`(全部单测)+ 步骤 3/5 的手动 live 清单。

## 回滚点

- 全部新增文件在 `cli/`(bin.mjs/lib/*),AGENTS.md 一行增补——回滚 = 删目录 +
  revert 单行,零 daemon/前端/scripts 影响。

## 风险文件

- 无既有文件修改(除 AGENTS.md 速查行)。group-chat-run 等 scripts 资产不动。
