# implement — evl discuss + 无参 help/健康检查

前置:prd.md(AC)、design.md(§7 文件清单/§8 权衡)。全程零 daemon 改动、
零 npm 依赖。验证基线:`cd cli && node --test` 全绿(现 54 用例,不许回退)。

## 顺序清单

### 1. lib/mcp.mjs + mcp.test.mjs(运输层先行,纯函数打底)

- [ ] `parseMcpResponse(body)`(纯):三态(协议错 isError 语义错 正常载荷,
      含 content[0].text 二次 JSON.parse + parse 失败 → 协议错)。
- [ ] `callMcpTool({base, tool, args, timeoutMs, verbose, verboseLog})`:
      双 Accept 头 POST;网络错文案复用 `fetchFailDetail`(OS 错误翻译 +
      daemon.sh 提示,同 api.mjs)。
- [ ] 单测:三态 × 若干 malformed body(content 缺失/text 非 JSON/id 不匹配)。

### 2. lib/args.mjs 扩展 + args.test.mjs

- [ ] `COMMAND_FLAG_SPEC.discuss`(preset/cwd/token-budget/roster/value,
      detail/bool)。
- [ ] 横切校验:token-budget 正整数、roster JSON 数组语法、wait 1..540,
      非法 64(错误文案按 design §3)。
- [ ] 单测:合法/非法值各档;discuss flag 在别的命令下 → 64(沿用严出惯例)。

### 3. lib/discuss.mjs + discuss.test.mjs

- [ ] 纯函数:VERBS、`stopReasonExitCode`(**开放集**:具名档全表 + 表外
      未知非空→1 + null→1)、`diffProgressLine`、
      `waitSlice(deadlineMs, nowMs)`(wait_seconds 1..25 裁剪)。
- [ ] `runDiscuss` 动词分发(start/status/result/cancel/interrupt/inject/
      presets + 无动词全链);缺参 → UsageError(64);`--` 后 topic 不被
      动词分发吞(解析器既有能力,零改动,加用例)。
- [ ] 全链:start → stderr session 行 → 轮询(deadline = flags.timeout;
      **变化判据 wait_timed_out !== true;终态短路先判**;单次 poll 传输错
      → 退 1 + 续窗提示,不重试)→ result → stdout text(末行 stop_reason
      标记)/json;超时不 cancel,json 载荷含 error:'timeout' + recovery,
      stderr 首行"讨论仍在跑,勿重跑";SIGINT cancel exit 3(二次硬退)。
- [ ] 单测:纯函数全覆盖(含表外值/键缺失=变化);runDiscuss 用注入 mock
      mcpCall(不真网络)跑全链快路径(立即终态)/超时路径/已终态
      status --wait 秒返路径。

### 4. bin.mjs:HELP + 分发 + 无参 help/健康行

- [ ] HELP_TOP 增 discuss 行(带成本警告)+ HELP.discuss;命令分发 case。
- [ ] `command == null` 分支:stdout HELP_TOP + 健康行,exit 0(替原
      stderr+64;未知命令路径不动)。
- [ ] 健康行:api(health, 1.5s)catch-all → unreachable 文案;恒 exit 0;
      子命令 help / --version 不探测。
- [ ] 手验:`evl`、`evl --help`、`evl chat --help`(离线)、daemon 停/起
      两态健康行。

### 5. 文档:cli/README.md + 根 AGENTS.md

- [ ] README 命令表增 discuss(动词表 + 全链 + 退出码表含 6/preempted→0/
      interrupted→1 + 超时不 cancel 与**防重跑**文案 + 成本警告);AGENTS.md
      evl 速查段同步。
- [ ] `evl discuss --help` 文案与 README 一致(退出码/恢复命令);preempted
      (interrupt 收束)与 cancelled(硬停)恢复文案区分;--timeout help
      文案提示"超长场次显式给值且宿主 bash 超时 ≥ 值+60,或走 start +
      status --wait 续窗"。

### 6. 验证门

- [ ] `cd cli && node --test` 全绿(旧 54 + 新用例)。
- [ ] `node cli/bin.mjs`(无参)两态(daemon 起/停)退出码恒 0。
- [ ] daemon 在跑:`node cli/bin.mjs discuss presets --output json | jq`
      通;`--token-budget 0` → 64;`status` 缺 sid → 64。
- [ ] live 冒烟(手动,烧真 token,一场即可):全链 `--output json` 走通,
      stderr 先出 session_id,终态退出码与 stop_reason 映射一致;随后
      `discuss status/result <sid>` 复核。
- [ ] SIGINT 手动:全链等待中 Ctrl-C → 3,`discuss status <sid>` 见已停。

## 风险点/回滚

- 唯一改既有行为点 = 无参分支(stderr+64 → stdout+0):脚本里若有依赖
  `evl` 裸跑报错的消费者(检索 repo 无先例,AGENTS.md 速查只演示带命令)
  需在 PR 说明。回滚 = revert 单 commit 批。
- `content[0].text` pretty JSON 二次 parse:若 daemon 未来加非 JSON text,
  协议错路径兜住(不静默吞)。
