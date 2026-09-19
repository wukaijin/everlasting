# design — evl discuss(MCP client 路线)+ 无参 help/健康检查

前置:prd.md(需求/AC)。本文件只讲技术方案。零 daemon 改动;不 import
`group-chat-run.mjs`(编排单源纪律,`.trellis/spec/cli/index.md`)。

## 1. 边界与分层

```
evl(bin.mjs 解析/分发)
 └─ lib/discuss.mjs   动词分发 + 全链编排(CLI 侧)     ← 新
     └─ lib/mcp.mjs   JSON-RPC tools/call(运输)        ← 新
         └─ POST {base}/mcp  → daemon routes/mcp.rs(编排唯一归属,不动)
```

CLI 侧"编排"仅指**轮询节奏与退出码翻译**,不含群聊编排语义(建群/轮次/
preset 合并全在 daemon)。`--roster`/`--token-budget` 等仅透传。

## 2. MCP client(lib/mcp.mjs,~60 行)

- 请求:`POST {base}/mcp`;headers 双声明确保过 406/415 探针:
  `{'Content-Type':'application/json', Accept:'application/json, text/event-stream'}`;
  body `{jsonrpc:'2.0', id:<进程内自增>, method:'tools/call',
  params:{name, arguments}}`。**不 initialize**(无状态端点,逐请求独立分发,
  mcp.rs:229)。
- 响应三态(纯函数 `parseMcpResponse(body)` 导出供单测):
  1. `body.error` → 协议错(`-32601/-32602`)→ EvlError(exit 1);
  2. `body.result.isError === true` → 语义/infra 错:parse
     `content[0].text`(JSON,含 `error`/`hint`)→ EvlError(message =
     `error` + 可选 `hint`;infra 带拉起提示);
  3. 正常 → `JSON.parse(content[0].text)` 返回工具载荷(pretty JSON 二次
     parse;parse 失败 = 协议错)。
- 非 200 → EvlError(406/415 只会在 header 写错时出现,属 CLI bug,文案带
  HTTP status + body 前 300 字,同 api.mjs 风格);网络层错误文案复用
  `fetchFailDetail`(OS 错误翻译,沙箱分类器依赖该字面串)。
- 超时:默认 30s;`discussion_status` 带 wait 时 `wait_seconds*1000 + 15000`。
- `--verbose`:stderr 打 `→ mcp <tool> <args 摘要>` / `← ok|isError <摘要>`。

## 3. 参数面(lib/args.mjs 扩展)

`COMMAND_FLAG_SPEC.discuss = { preset, cwd, token-budget, roster, wait, detail }`,
全部 value 型除 `detail`(bool)。解析期横切校验(沿用 output/timeout 同款
"解析期一并锁"惯例,非法 64):

- `token-budget`:正整数(文案:保险丝非省钱,不限请省略);
- `roster`:JSON.parse 成功且为 array(元素 name/model 非空校验留给 daemon
  ——它与 preset 合并语义耦合,CLI 只拦语法);
- `wait`:整数 1..540(越界 64);
- `preset`/`cwd`:纯透传不校验(daemon 三趟解析:内置 key/行 UUID/行名称)。

动词是 positionals,不改解析器:`evl discuss status <sid>` → command=discuss,
positionals=[status, sid]。动词集
`VERBS = ['start','status','result','cancel','interrupt','inject','presets']`。

## 4. discuss.mjs:动词分发与全链

```
runDiscuss({base, flags, positionals, io}):
  verb = VERBS.includes(positionals[0]) ? shift(positionals) : null
  verb == null → 全链(topic = positionals.join(' ').trim();空 → 64)
  'start'  → topic 同上;start_discussion;stdout {session_id, request_id};exit 0
  'status' → sid = positionals[0](缺 → 64)
             单次:call status {session_id, detail}
             --wait n:外层窗口 n 秒,内部循环 wait_seconds=min(25, 剩余,≥1)
               ——**终态先判秒返**(不耗窗口);变化(wait_timed_out!==true)
               即返;窗口尽返末次快照
             text 按字段存在性输出(wait 隐含 detail,mcp.rs want_progress;
               不按 --detail flag 取)
             exit 0(取到即 0;busy/终态是数据)
  'result' → call result;运行中 → EvlError(stderr 提示先 status)exit 1;
             成功 → text 摘要 / json 平铺;exit 0
  'cancel' / 'interrupt' → 对应工具;载荷原样出;exit 0
  'inject' → sid + text(positionals[1..].join(' '),空 → 64)
  'presets'→ list_presets;text 表(key/moderator/人数/覆盖标记)/json 原样
```

### 全链(主入口)

```
1. start_discussion({topic, cwd: flags.cwd ?? process.cwd(),
     preset: flags.preset /*缺省 review 由 daemon 定*/,
     participants: roster?, token_budget: tokenBudget?})
   → stderr 立即:session <id>(恢复:evl discuss status/result <id>)
   topic 以动词名开头 → 文档引导 -- 终止符(解析器既有能力,无代码改动)
2. 轮询循环(deadline = now + flags.timeout*1000):
     snap = status({session_id, wait_seconds: min(25, ceil((deadline-now)/1000)) 下限 1})
     **终态短路先于一切**:busy==false && stop_reason → break(daemon 侧
       同款短路 mcp.rs:1062,CLI 侧再判一次防窗口循环吞终态)
     **变化判据 = snap.wait_timed_out !== true**(键仅超时才写,mcp.rs:1014;
       变化时键缺失——按 == false 实现会整体反转,评审 09-19 抓出)
     变化(diffProgressLine(prev, snap) 非 null)→ stderr 进度行(--quiet 抑制)
     now ≥ deadline → 超时:**不 cancel**;stderr 首行"讨论仍在跑,勿重跑"
       + 恢复命令;json = {session_id, stop_reason:null, error:'timeout',
       recovery:'<续窗命令>'};exit 7
     单次 poll 传输错误 → 退 1 + 与退 7 同款续窗提示(不重试;session 在
       daemon 继续,恢复哲学同构——评审未决项 #1 处置)
3. result({session_id})
   stdout:text = summary + roster/stats/tokens + transcript_path + 末行
            stop_reason 标记(max_rounds→0 时 text 消费方可分辨轮帽截断);
          json  = {session_id, ...payload} 平铺单行(result 载荷无 session_id 键)
   exit = stopReasonExitCode(stop_reason)
```

- `diffProgressLine(prev, next)`(纯函数,导出):比较
  `messages/last_speaker/busy/stop_reason/tokens.total`,变化才产出行
  `[discuss] msg N · last X · tokens T(elapsed Es)`,不变返回 null。
- `stopReasonExitCode(reason)`(纯函数,导出;**值域开放集**,评审 09-19):
  `group_chat_end|max_rounds|cancelled|preempted → 0`(preempted =
  interrupt_discussion 自产真终态,summary 已落库);
  `error|nominee_unknown|participant_unresolved → 2`(异常收场族,
  group_chat_loop.rs:141-171 实读);`budget → 6`;`interrupted → 1`
  (可续跑态,stderr 提示 resume 语义);`null`(终态却无值,防御)与
  表外未知非空 → 1 + stderr 回显原值。单测锁表外值行为。
- SIGINT:首次 → `cancel_discussion`(10s 超时;发出即算)→ stderr 提示
  → exit 3;二次 → 立即 exit 3(不等待)。session 保留(daemon 语义)。

## 5. 无参 help + 健康检查(bin.mjs)

- `main()` 顺序不变:version → help → `command == null` 分支改为:
  `printHelp(null)`(stdout)+ 健康行 + `return 0`。HELP_TOP 文案"用法"段
  补一句"无参 = 本帮助"。
- `-h/--help` 顶层与裸跑同路径(都带健康行);子命令 help(`HELP[command]`)
  不探测(纯离线文档);`--version` 不变。
- 健康行实现:复用 `api.mjs` 的 `api(base,'health',{method:'GET',
  timeoutMs:1500})`,catch 一切 → unreachable 文案;**恒不影响退出码/不抛**。
  行尾带 resolved base(flag/env 优先级复用 `resolveBaseUrl`,写错地址时
  unreachable 行能自解释)。
- `--quiet` 不抑制健康行(它在 stdout,是帮助内容的一部分)。

## 6. 退出码总表(增量;stop_reason 映射按开放集,评审 09-19 重排)

| 码 | 场景 | 备注 |
|---|---|---|
| 0 | group_chat_end / max_rounds / cancelled / **preempted**;动词取数成功;help | preempted=interrupt 自产真终态(summary 落库);cancelled:cancel 是正常终态且 result 可读 |
| 1 | 不可达/协议错/工具语义错;**interrupted**(可续跑态);终态 stop_reason=null;**表外未知非空值**(stderr 回显原值);轮询中途传输错(续窗提示) | 语义错 stderr 带 daemon error 字段 |
| 2 | error / **nominee_unknown** / **participant_unresolved** | 异常收场族(group_chat_loop.rs:141-171) |
| 3 | SIGINT(cancel_discussion 已发) | session 保留 |
| 6 | budget | M1 同款唯一借档(commit 772e823d 先例) |
| 7 | --timeout 到点(**不 cancel**) | 与 chat 同码反义:讨论继续在 daemon,防重跑三处文案 |
| 64 | 用法错 | 含 token-budget/roster/wait 校验 |

**为什么不照抄 M1 退出码表**(评审 09-19):M1 max_rounds→2,但 evl 已有
0/1/2/3/7/64 六档契约,budget→6 是唯一借档;max_rounds 是"产出完整可读的
正常截断"(text 末行 stop_reason 标记已让消费方可分辨),升 2 会破坏
"退出码判读成败"的委派心智。后人勿当 bug 改回。

## 7. 文件清单

| 文件 | 动作 |
|---|---|
| `cli/lib/mcp.mjs` | 新:callMcpTool + parseMcpResponse(纯) |
| `cli/lib/discuss.mjs` | 新:runDiscuss + VERBS + stopReasonExitCode/diffProgressLine/waitSlice(纯) |
| `cli/lib/args.mjs` | 改:discuss flag spec + 横切校验 |
| `cli/bin.mjs` | 改:HELP_TOP/HELP.discuss、discuss 分发、无参 help+健康行 |
| `cli/mcp.test.mjs` / `cli/discuss.test.mjs` / `cli/args.test.mjs` | 新/改:单测 |
| `cli/README.md`、根 `AGENTS.md` | 改:命令面速查 + 成本警告 |

## 8. 权衡记录

- **超时不 cancel(vs chat,评审 09-19 维持)**:timeout 是**调用方窗口属性**,
  cancel 是**工作属性**——chat 的工作单元≈窗口单元,到点即止损自洽;discuss
  实测 9-16min 常态超出 540s 窗口,到点是正常交接而非故障。默认 540 不升
  30min:宿主 bash 超时 ≥ timeout+60 不变量,静默破坏 → SIGKILL(137)严格
  劣于干净退 7;超长场次走 start + status --wait 续窗。chat 的 cancel 语义
  保留在 SIGINT(用户主动)。
- **入口窗口不变量(评审 09-19 架构补充,预置 R2 剩余项免重议)**:每条 evl
  阻塞调用 ≤ 宿主窗口;长工作经 session 锚点跨调用续窗。--wait 上限 540 与
  全链默认 540 同源。
- **分层判据显式化**:随入口变化的 = 运输(CLI 循环/bash 自轮询/GUI 轮询三
  入口三窗口);塑造讨论本身的 = 编排(建群/轮次/preset 合并,唯一归属
  daemon)。
- **status/result 动词取数成功恒 0(双口径维持)**:动词是运输语义——exit
  0=拿到 daemon 真相 / 1=没拿到(与 chat 委派 vs sessions 取数的既有分法
  同构);只有全链把终态翻译成退出码。result 非终态退 1 是 daemon isError
  运输通道语义,不外推到 status 的 busy 上。
- **--wait 外层窗口循环**:单次 wait_seconds 上限 25(MCP 契约,防宿主 30s
  工具超时);CLI 的每次 call 是 HTTP 往返,无 MCP 宿主的"每 call 一 LLM
  turn"成本,循环无罚。三处判据(评审 09-19):变化 = wait_timed_out !==
  true(键仅超时才写);终态短路先于窗口循环;text 按字段存在性输出(wait
  隐含 detail)。
- **preempted→0 / interrupted→1(评审 09-19)**:preempted 是
  interrupt_discussion 自产、summary 已落库的真终态(M1 映射漏了它,照抄
  先例会继承同洞);interrupted 是崩溃恢复标记的可续跑态,非未知异常,退 1
  带 resume 语义提示。
- **cancelled→0 而非 3**:3 保留给"本进程主动 SIGINT";他人/它路 cancel 后
  CLI 读到的 result 是正常可消费数据。
- **不做 initialize**:端点无状态(mcp.rs 无 session 表);握手是纯开销。
  风险:若未来 daemon 改有状态会破——届时 404/400 面,CLI 报协议错,不静默。
- **健康行进 stdout 而非 stderr**:它是帮助内容(用户明确要求"help 中加入"),
  且 stdout 已定位"数据";探测恒 1.5s 上界,daemon 停时不拖慢 help。裸跑
  64 与 --help 0 的现状自相矛盾,改后反而统一("help 恒离线恒快"两条不变量
  ——退出码恒 0 + 子命令文档零网络——均保住)。
- **roster 只拦 JSON 语法**:语法错=64(CLI 早失败),语义错=1(daemon
  isError);name/model/preset 合并语义在 daemon(与 preset 耦合),CLI
  复制校验必漂移。

## 9. 回滚

纯新增 + bin.mjs 两个小改点(args spec / 无参分支)。回滚 = revert 整批;
无数据迁移、无 daemon 面。
