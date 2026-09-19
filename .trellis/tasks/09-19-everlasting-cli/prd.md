# Everlasting 能力开放 CLI(`evl`,mmx 式薄壳,LLM 优先)

## Goal

做一个 mmx 风格的命令行工具 `evl`,把 Everlasting daemon(`:7456`)已有的能力开放为
**bash 可调用接口**。**主要使用者是 LLM(宿主 agent 经 bash 工具委派任务)**,
人类 TTY 使用次之——与 daemon `/mcp` 端点(MCP 结构化工具面,群聊编排八件)互补
不重复:`/mcp` 服务挂了 MCP server 的宿主,`evl` 服务任意能跑 shell 的宿主与
shell 脚本。CLI 是 daemon HTTP API 的薄壳:零新后端能力,纯门面。

对标参照:`mmx-cli` 1.0.7(`resource → command → flags` 分发 + 成熟横切 flag)。

## 定位含义(LLM-first 的设计推论,评审焦点)

- **输出契约 > 交互体验**:stdout 只出数据(text 或单行 JSON),人向提示全走
  stderr;`--output json` 是 LLM 主消费格式;退出码机器可判。
- **权限必须无交互**:LLM 经 bash 调用无中途应答能力,权限策略靠
  `--mode plan|edit|yolo`(daemon `set_session_mode` 现成端点)前置声明,
  不靠运行中弹窗。**非 TTY 默认 `plan`(fail-closed)**,TTY 默认 `edit`
  (评审 09-19:默认值即安全边界,关死「忘带 flag」与「打错字」两条静默
  落入 edit 的路径;`--mode` 值域 CLI 侧校验——daemon 解析 lenient,
  未知值静默回退 edit)。
- **超时对齐**:agent loop 一轮多工具常态 3-10min,默认 `--timeout 540s`
  (宿主 bash 上限 10min − 60s 余量,规避宿主 SIGKILL 与 CLI cancel 同刻
  开跑的赛跑);不变量:宿主 bash 超时 ≥ `--timeout + 60s`。
- **发现面静态化**:命令集小而稳,靠 AGENTS.md 速查/skill 文档发现,
  不做 `config export-schema`(mmx 有,但 LLM 这里靠静态文档足够)。

## Background(调研结论,2026-09-19)

- **mmx 借鉴面**:横切 flag(`--output text|json`/`--quiet`/`--verbose`/`--timeout`
  /`--no-color`/`--non-interactive`)、每级 `--help` 带示例、TTY 感知。
- **不照搬**:auth 生命周期(daemon 零鉴权)、export-schema(见上)、update 自管理。
- **仓库可复用资产**:`scripts/turn-smoke.sh`(单轮全链参考:project 解析→建
  session→SSE 订阅→`agent/chat`→等 `kind=done`→清理)、`scripts/group-chat-run.mjs`
  (CLI 先例:退出码契约、daemon 不可达错误文案含 EPERM 翻译——沙箱分类器按字面串触发)。
- **repo 包惯例**:无 pnpm workspace,每目录独立 `package.json`(scripts/、app/ 同款)。
- **daemon 权限体系(实读)**:session mode 三档 `plan`(只读,写工具过滤,shell 走
  只读沙盒面,零权限 ask)/`edit`(默认,写操作触发 ask)/`yolo`(自动批,硬拒规则
  仍生效,root guard);headless 无 SSE 观察者时 ask 8s 快拒(GC3)。

## Requirements(已收敛)

### R1 命令面(MVP)

- `evl status` — daemon health + 版本;不可达时报错(含 OS 错误翻译)+
  `./scripts/daemon.sh bg` 提示。
- `evl chat "<message>"` — 委派一轮 agent loop:
  - project:默认按 CWD 匹配 `list_projects`,无则 `create_project`;`--project <path>` 覆盖。
  - session:默认新建并**保留**(GUI 可见,`--session <id>` 续聊);`--ephemeral`
    发完即删。
  - **`--mode plan|edit|yolo`**:建/续 session 后 `set_session_mode`。默认值
    双模:TTY=`edit`,**非 TTY=`plan`(fail-closed)**;非法值退出 64(CLI 侧
    校验)。注意 mode 对既有 session 是**持久覆盖**(落 `sessions.mode` +
    audit;per-request override 是 daemon 改动,本期不做),`--session` 命中时
    stderr 提示 persistent。
  - **非 TTY(LLM 主场景)**:SSE 静默订阅拿精确终态(`kind=done|error`,与 GUI
    同款消费逻辑);`permission:ask` 到达即**主动 deny**(毫秒级,不等 GC3 8s
    快拒;denied 工具结果由 agent 继续消化,turn 正常 done);stdout 只出终态
    (text=assistant 全文 / json=结构化终态)。
  - **TTY(人类)**:流式 delta 渲染;`permission:ask` y/a/n 交互应答
    (`allow_once`/`allow_always`/`deny`)。
  - 终态与退出码:`done`=0 / `error`=2;SIGINT → `cancel_chat`(request_id 域
    硬停,session 保留)退出 3;`--timeout`(默认 540s)到点 cancel 后退出 7。
  - `--model` 可选(默认走 session 默认)。
- 内省四件套(只读,`--output json` 机器可读):
  - `evl sessions` — `list_sessions`(id/标题/时间/busy/stop_reason)
  - `evl projects` — `list_projects`
  - `evl models` — `list_models` + `get_default_model`(标注默认)
  - `evl usage` — `usage_window`(`--provider` 过滤)

### R2 二期(留位,不做)

- `evl chat` detach 两段式(`start` 秒回 + `wait/status <request_id>` 轮询)——
  任务超宿主 bash 超时上限时的出路(群聊 MCP `wait_seconds` 同款先例)。
  **评审补记(09-19):detach 的真实成本在数据面**——daemon 无 session 消息
  历史读路由、turn_trace 无 assistant 文本、SSE replay buffer 首连(last 未
  带时)空回放且 512 ring 淘汰 + >256KiB 不入 buffer(早期 delta 补不回);
  二期 wait 须复用 Last-Event-ID。指导原则:**daemon 加只读消息路由 > CLI
  本地 spool**(破零 daemon 改动的代价小于破薄壳);立项时回 planning 评审。
- `evl discuss` — 群聊审议子命令。**评审改道(09-19):以 MCP client 身份调
  `POST /mcp`**(stateless JSON-in/JSON-out,CLI 内 ~30 行 call;flag 面由 CLI
  翻译,用户不见 MCP 内部)——编排单源留 daemon,结构性排除双实现分叉
  (group-chat-run.mjs 是自带编排的完整入口而非可复用引擎,直接 import 复用
  会复活 P4 刚退役的 JS 编排)。
- `evl tasks` / REPL(REPL 与 LLM-first 定位冲突,低优先)。

### R3 横切面

- 全局 flag:`--base-url`(env `EVERLASTING_BASE`,默认 `http://127.0.0.1:7456`)/
  `--output text|json` / `--quiet` / `--verbose`(直打 HTTP 与 SSE 事件名,LLM
  调试用)/ `--timeout` / `--no-color` / `--non-interactive`。
- TTY 判定 `process.stdout.isTTY`;非 TTY ⇒ 静默模式(流式关、ask 自动 deny、
  无交互);`--non-interactive` 强制同款。

### R4 工程形态

- 新顶层 `cli/`,独立 `package.json`,`bin` 单名 `evl`;`node cli/bin.mjs` 直跑,
  `pnpm link --dir cli` 挂全局。
- Node ≥ 20,**零运行时依赖**(SSE 手解 fetch+ReadableStream,无 eventsource/
  commander)。
- 测试 `node --test` 同 scripts/ 惯例(纯函数单测,不打真 daemon)。

## Acceptance Criteria

- [ ] AC1 `evl --help` 与各子命令 `--help` 输出用法/示例;未知子命令退出 64。
- [ ] AC2 `evl status`:daemon 在跑报 health+版本;停时非零 + OS 错误翻译 +
      `./scripts/daemon.sh bg` 提示。
- [ ] AC3 内省四件套:text 人类可读,json 单行合法 JSON 可 `| jq`。
- [ ] AC4 `evl chat` 全链(非 TTY,模拟 LLM 场景):`--mode plan --output json`
      发一轮真实只读任务(如"列出当前目录文件并总结"),stdout 单 JSON
      (`{text, usage, session_id, request_id, stop_reason, permission_denials,
      text_chars}`),退出 0;无流式噪音混入 stdout。**非 TTY 不带 --mode 时
      默认落 plan**(不落 edit)。
- [ ] AC5 `--session` 续聊上下文连续;`--ephemeral` 发完 session 不再出现;
      正常路径 stderr 打 session id(供续聊);`--session` + `--mode` 命中时
      stderr 提示 mode 持久化。
- [ ] AC6 非 TTY 权限语义:`--mode edit` 下发会触发写权限的消息,ask 被立即
      主动 deny(总耗时无 8s 挂起),turn 以 denied 工具结果继续并正常 done,
      **json 终态 `permission_denials > 0`**(语义:本 turn 内最终决策为 deny
      的 ask 计数,不论拒绝者——非 TTY 自动拒 / TTY 人按 n)。
- [ ] AC6b `--mode` 值域校验:`--mode plna`(非法)退出 64 + 提示合法值
      (daemon 侧 lenient 会静默回退 edit,CLI 必须拦)。
- [ ] AC7 SIGINT:发出 `cancel_chat` 后及时退出(码 3),session 保留可续。
- [ ] AC8 TTY 路径(手动):流式渲染;permission y/a/n 三键应答生效。
- [ ] AC9 纯函数单测(参数/匹配/SSE 帧切分/格式化)`node --test` 全绿。
- [ ] AC10 `pnpm link --dir cli` 后任意目录 `evl --help` 可用。

## 边界(不做)

- 零 daemon 改动为前提(确需后端小改动的,回 planning 评审)。
- 不做:auth/远程、daemon 生命周期命令(daemon.sh 已覆盖)、export-schema
  (静态文档为发现面)、npm 发布、bun compile、detach 两段式/discuss/tasks/REPL
  (二期)。
- 不动 `scripts/` 现有资产。

## 未决(评审 09-19 遗留,均不阻塞本期)

1. detach 数据面路线(daemon 只读消息路由 vs CLI spool)——二期立项回 planning。
2. per-request mode override(消除「每次调用声明 vs 会话属性持久变更」错位,
   daemon 接口语义修正)——未排期。
3. SSE observer 全局外部性的 daemon 侧修复(per-session observer 判定)——
   本期仅 design §5 文档化警告。
4. `tool:question`/`mode:change` 被 CLI 忽略后 daemon 侧超时行为——单发场景
   低频,未实测。

## 评审记录

- 2026-09-19 群聊评审(session `f7ebec19`,review 预设,产品/后端/架构三方,
  59 条/16min):五焦点全裁决,14 项采纳 0 驳回,转录见
  `~/.local/share/dev.everlasting.app/discussions/2026-09-19-*f7ebec19.md`。
  核心修订已并入本文:非 TTY 默认 plan、timeout 540s+宿主余量不变量、json
  形状补 permission_denials/text_chars/error?、R2 discuss 改 MCP-client 路线
  + detach 数据面缺口。
