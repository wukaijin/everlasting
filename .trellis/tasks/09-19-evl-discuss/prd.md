# evl CLI 二期:`discuss` 子命令 + 无参 help/daemon 健康检查

## Goal

把 PRD R2(上位任务 `09-19-everlasting-cli`)留位的两项落地:

1. **`evl discuss`** — 群聊审议命令面:以 MCP client 身份调 daemon `POST /mcp`
   (stateless JSON-RPC `tools/call`),编排单源留 daemon,CLI 只做运输与 flag
   翻译,用户不见 MCP 内部。评审 09-19 已裁决该路线(结构性排除双实现分叉,
   不 import `group-chat-run.mjs`)。
2. **无参默认 help + help 带 daemon 健康检查** — `evl` 裸跑从"缺参报错 64"
   改为"默认打帮助";顶层帮助尾部附 daemon 健康行(在跑=版本/uptime,不在=
   拉起提示),让使用者第一眼看到 daemon 状态。

主要使用者仍是 LLM(宿主 agent 经 bash 委派),人类 TTY 次之。

## Background(实读证据,2026-09-19)

- **MCP wire**(`daemon/routes/mcp.rs`,契约 docs/DAEMON-API.md §6.5):
  - `POST /mcp`:Accept 须同时含 `application/json` 与 `text/event-stream`
    (缺 → 406);Content-Type 须 `application/json`(否则 415);响应 200 纯 JSON。
  - **无状态**:`tools/call` 不要求先 `initialize`(mcp.rs:229 逐请求独立分发);
    CLI 直发 `tools/call` 即可,~30 行 JSON-RPC call。
  - 响应三层:JSON-RPC error(未知工具/方法,-32602/-32601)→ 协议错;
    工具执行错误 → 200 + `result.isError:true` + `content[0].text` 为
    `{error: msg}`(infra 错误另带 `hint`);成功 → `content[0].text` 为工具
    返回值的 pretty JSON 字符串(需二次 `JSON.parse`)。
- **八工具输入/返回形状**(mcp.rs tool_defs + 各 `tool_*`):
  - `start_discussion` `{topic*, cwd*, preset?, participants?, token_budget?}` →
    `{session_id, request_id, hint}`;preset 缺省 `review`(mcp.rs:1283)。
  - `discussion_status` `{session_id*, wait_seconds?(1-25), detail?}` →
    `{busy, stop_reason, elapsed_s[, messages, last_speaker, tokens][, wait_timed_out]}`;
    wait 隐含 detail;终态或无 wait 时立即返回快照。
  - `discussion_result` `{session_id*}` → `{stop_reason, summary, roster, stats,
    detail?/detail_warning, tokens, transcript_path?, summary_warning?}`;非终态
    → 语义错误 "still running"。**返回里没有 session_id 键**(CLI json 输出需
    自补)。
  - `cancel_discussion`(幂等,非 busy → `already_finished`)/
    `interrupt_discussion`(收束)/ `inject_message`(busy guard,非 busy 语义错)。
  - `list_presets` → 合并预设目录(内置四档 + 用户档 + 覆盖标记)。
- **repo 先例**:`scripts/group-chat-run.mjs`(M1)SIGINT → cancel 保 session;
  stop_reason=budget → 退出码 6;`--token-budget` 正整数校验。
- **CLI 现状**:`cli/lib/args.mjs` COMMAND_FLAG_SPEC 驱动命令集(新增命令 =
  加 spec 条目);positionals 已天然支持"动词 + 参数"形态;`api.mjs` 有
  fetch 封装/OS 错误翻译/verbose 钩子,但只打 `/api/v1/*`,MCP 端点需新路径
  + 双 Accept 头。
- **成本面**:一场审议 5-15min、多模型烧真 token(AGENTS.md 速查反复强调
  "预算帽是保险丝非省钱手段,建议不填")——help/README 文案必须携带该警告。

## Requirements

### R1 `evl discuss` 命令面

**全链(主入口,LLM 委派单命令)**:

- `evl discuss "<topic>" [flags]` — start → 轮询 `discussion_status`
  (内部 `wait_seconds=25` 有界长轮询循环,变化即续呼)→ 终态后取
  `discussion_result` 出结果。
- stderr 立即打 `session_id`(恢复锚点);进度行(messages/last_speaker/
  tokens)按变化打 stderr,`--quiet` 抑制。
- stdout:`text` = summary + roster/stats/tokens 摘要 + transcript_path;
  `json` = result 载荷平铺 `{session_id, stop_reason, summary, roster, stats,
  …}` 单行 JSON。
- flags:`--preset <key|uuid|name>`(缺省 review)/ `--cwd <path>`(缺省
  process.cwd())/ `--token-budget <n>`(正整数,CLI 侧校验非法 64;文案提示
  "保险丝非省钱")/ `--roster '<json>'`(participants 整名单替换;**分层**:
  JSON 语法错 = CLI 64,name/model 语义错 = daemon isError 退 1)。topic 以
  动词名开头时用 `--` 终止符:`evl discuss -- "status 这个词当议题"`。
- **超时语义(与 chat 的关键差异,评审 09-19 维持)**:`--timeout`(默认
  540s,不升 30min——宿主 bash 超时 ≥ timeout+60 不变量)到点**不发
  cancel**——审议 5-15min 且烧真 token,超时是调用方窗口的正常交接而非故障;
  退出 7,json 载荷 `{session_id, stop_reason:null, error:'timeout',
  recovery:'<续窗命令>'}`;**防重跑三处文案**(超时 stderr 首行/json recovery
  字段/HELP/README)写死"讨论仍在跑,勿重跑"——退 7 与 chat 同码但语义相反
  (chat 已 cancel,discuss 未停),不写死会触发 LLM 重试双花。
- **SIGINT 语义**:首次 → `cancel_discussion`(与 chat/M1 一致:停编排保
  session)退出 3;二次立即硬退。
- **stop_reason → 退出码(值域按开放集处理,评审 09-19 重排)**;具名档:
  `group_chat_end|max_rounds|cancelled|preempted` → 0(preempted 是
  interrupt_discussion 自产真终态且 summary 已落库);`error|
  nominee_unknown|participant_unresolved` → 2(异常收场族);
  `interrupted` → 1(可续跑态,stderr 提示 resume 语义,非未知异常);
  `budget` → 6(M1 同款唯一借档);`null`(终态却无值,防御)与表外未知
  非空值 → 1 + stderr 回显原值,单测锁表外行为。SIGINT 路径自身退 3。
- **text 输出末行补 stop_reason 标记**(max_rounds→0 时 text 消费方须能
  分辨轮帽截断与自然收官;json 天然带字段)。

**控制动词(第一位置参数;恢复/异步/运维面)**:

- `evl discuss start "<topic>" [flags]` — 只建群不等待,stdout 出
  `{session_id, request_id}`(text 模式两行);>540s 长讨论的两段式出路。
- `evl discuss status <sid> [--wait <s>] [--detail]` — 快照;`--wait 1..540`
  外层窗口(CLI 内部循环 `wait_seconds=25`);**变化检测判据 =
  `wait_timed_out !== true`**(该键 daemon 仅在超时才写,mcp.rs:1014——按
  `== false` 实现会把"变化即返"整体反转,评审 09-19 抓出);**终态短路
  先于窗口循环**(wait 打已终态讨论须秒返);wait 隐含 detail(mcp.rs
  `want_progress`),text 输出按**字段是否存在**取,不按 `--detail` flag。
  成功取到恒退 0(busy/终态是数据不是错误)。
- `evl discuss result <sid>` — 终态结论(运行中 → 语义错误退 1,stderr 提示
  先 status);成功取到恒退 0(stop_reason 是数据;LLM 读 json 判读)。
- `evl discuss cancel <sid>` / `interrupt <sid>` — 停/收束。
- `evl discuss inject <sid> "<text>"` — 注入用户消息(busy guard 语义错透传)。
- `evl discuss presets` — 合并预设目录(text 表 / json 原样)。

**错误与退出码(动词通用)**:daemon 不可达 → 1(OS 错误翻译同款);工具
语义错误(isError:true)→ 1 + stderr 出 `error` 字段;JSON-RPC 协议错 → 1;
用法错 → 64。`--verbose` 打 JSON-RPC 请求/响应摘要(stderr)。

### R2 无参 help + 健康检查

- `evl` 裸跑 → HELP_TOP 打 **stdout**、退出 **0**(与 `--help` 一致);未知
  命令/缺参报错路径维持 64 不变。
- 顶层帮助(裸跑与 `evl --help`/`-h`)尾部追加健康行:
  - 在跑:`daemon: running <daemonVersion>(uptime <n>s)@ <resolved-base-url>`
  - 不可达:`daemon: unreachable — 先拉起:./scripts/daemon.sh bg(<resolved-base-url>)`
- 探测 `GET /api/v1/health`,超时 ~1.5s;**恒不改变退出码**(帮助永远是 0)、
  恒不抛错(探测失败就是 unreachable 文案)。
- 子命令 help(`evl chat --help` 等)与 `--version` 保持纯离线,不探测。

## Acceptance Criteria

- [ ] AC1 `evl` 裸跑:stdout 出完整帮助 + 健康行,退出 0;daemon 在跑时含
      版本与 uptime,停时含 daemon.sh 提示;两种情况退出码均 0。
- [ ] AC2 `evl --help` 同款健康行;`evl chat --help` 与 `--version` 无网络
      请求(纯离线)。
- [ ] AC3 `evl discuss presets`(daemon 在跑):text 表 + json 单行可 `| jq`,
      含内置四档;`--preset` 引用其中合法值。
- [ ] AC4 全链 live:建一场真讨论,stderr 先出 session_id,终态后 stdout
      json 单行 `{session_id, stop_reason, summary, roster, stats, …}`,
      正常收官(group_chat_end/max_rounds)退出 0;text 模式末行带
      stop_reason 标记。
- [ ] AC5 超时:全链 `--timeout` 到点不 cancel(不产生 cancel_chat/停编排),
      退出 7,json 载荷含 `{session_id, stop_reason:null, error:'timeout',
      recovery}`,stderr 首行"讨论仍在跑,勿重跑" + 恢复命令;随后
      `evl discuss status <sid>` 能看到讨论仍在跑或已自然收官。
- [ ] AC6 退出码映射(开放集):具名档 `error|nominee_unknown|
      participant_unresolved`→2、`budget`→6、`preempted`→0、`interrupted`→1;
      表外未知非空 → 1 + stderr 回显原值;动词 status/result/presets 取数
      成功恒 0。
- [ ] AC7 SIGINT(手动):全链等待中 Ctrl-C → cancel_discussion 已发、退出 3、
      session 保留;二次 Ctrl-C 硬退。
- [ ] AC8 flag 校验(纯 CLI,不打 daemon):`--token-budget 0/abc` → 64;
      `--roster` 非 JSON → 64;`--wait` 越界(0 或 >540)→ 64;`status` 缺
      sid / `inject` 缺 text → 64;`evl discuss -- "status 当议题"` 经 `--`
      终止符不被动词分发吞掉。
- [ ] AC9 `--wait` 语义:打已终态讨论秒返(不耗窗口);变化检测按
      `wait_timed_out !== true`;`--wait` 后 text 输出含 detail 字段
      (wait 隐含 detail,按字段存在性输出)。
- [ ] AC10 MCP client 纯函数单测(JSON-RPC 响应三态解析:text 载荷 JSON.parse、
      isError 语义错、-326xx 协议错)+ 轮询循环状态机(含 wait_timed_out
      键缺失 = 变化)+ 退出码映射(含表外值),`cd cli && node --test` 全绿。
- [ ] AC11 HELP_TOP / `evl discuss --help` / cli/README.md / AGENTS.md 速查
      更新:discuss 用法 + 成本警告(5-15min、预算帽=保险丝)+ 超时防重跑
      文案。
## 边界(不做)

- 零 daemon 改动(全部走既有 `/mcp` 八工具与 health 端点)。
- 不做:`evl tasks` / REPL / chat detach 两段式(其余 R2 项,另立);
  `--roster` 之外的 preset 编辑;转录导出覆盖(MCP 侧已自动落
  `{app_data_dir}/discussions/`)。
- 不 import `scripts/group-chat-run.mjs`(编排单源纪律,spec cli/index.md)。
- 不做 initialize 握手(无状态端点不需要;直发 tools/call)。

## 评审记录

- 2026-09-19 用户裁决:范围 = discuss + 无参 help/健康检查;流程 = 建任务走
  完整 planning(本任务)。
- 2026-09-19 群聊评审(session `20fd3665`,review 预设,产品/后端/架构三方,
  60 条/13.5min,转录见
  `~/.local/share/dev.everlasting.app/discussions/2026-09-19-*20fd3665.md`):
  五焦点全裁决,10 项结论全 verified,0 驳回,3 未决项均不阻塞开工。核心
  修订已并入本文:焦点1 维持全链+七动词 + `--` 终止符文档;焦点2 维持超时
  不 cancel/默认 540 + json 超时载荷与防重跑三处文案;焦点3 双口径维持但
  映射重排(preempted→0、interrupted 具名档 1、开放集+表外值单测、text 补
  stop_reason 标记);焦点4 维持无改动;焦点5 维持分层 + wait_timed_out
  判据/终态短路/字段存在性三修。未决项处置:轮询中途传输错 → 退 1 走与
  退 7 同款续窗提示(不重试);preempted/cancelled 恢复文案区分(收束 vs
  硬停)与 --timeout help 文案(显式给值 + 宿主不变量)留实现文案阶段。
