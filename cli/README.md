# evl — Everlasting daemon CLI

daemon(`:7456`)HTTP API 的**薄壳**命令行工具:零新后端能力,纯门面。主要使用者是
**LLM(宿主 agent 经 bash 委派任务)**,人类 TTY 使用次之。零 npm 依赖,Node ≥ 20。

设计与 wire 契约:`.trellis/tasks/09-19-everlasting-cli/design.md`(全部带源码出处)。

## 安装

```bash
node cli/bin.mjs --help          # 直跑(仓库内)
pnpm link --dir cli              # 或挂全局:任意目录裸 `evl`
```

第三种(**无需仓库 / pnpm**):GUI Settings →「CLI (evl)」一键安装——
daemon 编译期内嵌 cli/ 三件套,写出 `{app_data_dir}/cli/` 并 symlink
`~/.local/bin/evl`(不覆盖已存在的外部文件;重复安装即更新)。等价的
HTTP 入口:`POST /api/v1/evl_cli/{detect_evl,install_evl}`(契约见
docs/DAEMON-API.md §7 evl_cli 域)。

daemon 拉起:`./scripts/daemon.sh bg`(默认 `http://127.0.0.1:7456`,
env `EVERLASTING_BASE` / `--base-url` 覆盖)。

## 命令

| 命令 | 作用 | 说明 |
|---|---|---|
| `status` | daemon health + 版本 | 不可达:非零 + OS 错误翻译 + daemon.sh 提示 |
| `chat "<msg>"` | 委派一轮 agent loop | LLM 主入口;详见下 |
| `discuss "<topic>"` | 群聊审议(建群/观察/收结果) | MCP 薄壳;一场 5-15min 烧真 token;详见下 |
| `sessions` | 列 session(id/busy/stop_reason/…) | 默认跨全部 project 合并;`--project <path>` 收窄 |
| `projects` | 列 project(含隐藏) | |
| `models` | 列 model + 标默认 | 模型引用只认 UUID |
| `usage` | token 用量窗口 | `--provider <id>` 过滤 |

全局 flags:`--base-url` / `--output text|json` / `--quiet` / `--verbose` /
`--timeout <s>` / `--no-color` / `--non-interactive` / `-h` / `--version`;
chat 专属:`--mode plan|edit|yolo` / `--session <id>` / `--ephemeral` / `--model <id>` /
`--project <path>`;discuss 专属:`--preset` / `--cwd` / `--token-budget` / `--roster` /
`--wait` / `--detail`。

无参 `evl` = 顶层帮助 + daemon 健康行(stdout,恒退 0;在跑 = 版本/uptime,
停 = `./scripts/daemon.sh bg` 提示)。子命令 help 与 `--version` 纯离线。

## LLM 调用方契约(§7.5)

- **委派一律 `--output json`**:`evl chat "<任务>" --output json`。stdout 只出数据
  (单行 JSON);session id / 工具行 / 权限交互 / verbose 全 stderr。
- **json 终态形状恒定**(成功/失败同一对象,一条解析分支):

  ```json
  {"text":"…","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"context_input_tokens":0,"context_window":1000000},"session_id":"…","request_id":"…","stop_reason":"end_turn","permission_denials":0,"text_chars":13}
  ```

  error 分支:`text=""`、`text_chars=0`、`usage` 可 null、`error:{kind,message}`;
  其余键不变。`text_chars` 供消费方自判长度(不截断)。
- **退出码**:`0` done / `1` 脚本错(网络/不可达/SSE 断)/ `2` chat kind=error /
  `3` SIGINT cancelled(cancel_chat 已发,session 保留)/ `7` timeout(已发 cancel)/
  `64` 用法错误。discuss 命令面的差异见下节(2 = 异常收场族、6 = budget、
  7 = 未 cancel 讨论仍在跑)。
- **权限**:`--mode` 是前置声明,不靠运行中交互。非 TTY **默认 `plan`**(fail-closed,
  只读);写任务显式 `--mode edit`(ask 全拒,denied 工具结果继续消化,turn 正常
  done,`permission_denials>0`)或 `--mode yolo`(自动批)。`--mode` 值域 CLI 侧校验,
  非法退出 64(daemon lenient 会静默回退 edit);对 session 是**持久覆盖**(落
  `sessions.mode`),续聊命中时 stderr 提示。
- **session**:默认新建并保留(GUI 可见;id 打在 stderr),`--session <id>` 续聊
  (daemon 端自 rehydrate 水位,只发新消息即可),`--ephemeral` 发完即删。
- **超时**:默认 540s,到点先 `cancel_chat` 再退 7。**不变量:宿主 bash 超时 ≥
  `--timeout` + 60s**(规避宿主 SIGKILL 与 CLI cancel 赛跑)。宿主先杀掉 evl 时
  daemon 侧 loop 仍在跑 → `evl sessions` 查 busy / GUI Stop 兜底。

## discuss — 群聊审议(二期)

daemon `POST /mcp` 的 MCP client 薄壳(编排单源在 daemon,CLI 只做运输与
flag 翻译;零 npm 依赖手写 JSON-RPC)。**成本警告:一场 5-15min、多模型烧真
token**;`--token-budget` 是防失控保险丝而非省钱手段,不限请省略。

**全链(LLM 委派主入口)**:`evl discuss "<topic>" [flags]` — 建群 → 有界长轮询
(单次 `wait_seconds` ≤ 25,变化即返)→ 终态后收 result。stderr 先出
`session <id>`(恢复锚点)再打进度行(`--quiet` 抑制);stdout json =
`{session_id, stop_reason, summary, roster, stats, …}` 单行;text = summary +
roster/stats/tokens + transcript 落点 + 末行 stop_reason 标记(max_rounds → 0
时靠它分辨轮帽截断)。转录自动落 `{app_data_dir}/discussions/`。

**动词**(第一位置参数;恢复/异步/运维面;取数成功恒退 0):

| 动词 | 作用 |
|---|---|
| `start "<topic>"` | 只建群不等待,stdout 出 `{session_id, request_id}`(text 两行);>540s 长讨论的两段式出路 |
| `status <sid> [--wait <s>] [--detail]` | 快照;`--wait 1..540` 外层窗口(终态秒返;wait 隐含 detail,text 按字段存在性输出) |
| `result <sid>` | 终态结论(运行中 → 语义错退 1,先 status) |
| `cancel <sid>` | 停编排(session 保留;幂等,已收官报 `already_finished`) |
| `interrupt <sid>` | 收束打断(preempt:主持人现在收尾,summary 落库) |
| `inject <sid> "<text>"` | 注入用户消息(只对进行中的讨论有效) |
| `presets` | 合并预设目录(内置四档 + 用户档 + 覆盖标记) |

flags:`--preset <key|uuid|name>`(缺省 review)/ `--cwd <path>`(默认 CWD)/
`--token-budget <n>`(正整数,非法 64)/ `--roster '<json>'`(participants 整名单,
JSON 语法错 = 64,name/model 语义错 = daemon 判退 1)。议题以动词名开头时用
`--` 终止符:`evl discuss -- "status 这个词当议题"`。

**超时(--timeout,默认 540s)**:到点**不 cancel**——超时是调用方窗口的正常
交接而非故障,讨论在 daemon 侧继续跑。退出 7,json 载荷
`{session_id, stop_reason:null, error:'timeout', recovery:"evl discuss status <sid> --wait 540"}`。
**讨论仍在跑,勿重跑**(重跑会双花 token)。超长场次:显式给 `--timeout`
(宿主 bash 超时须 ≥ 值 + 60),或走 `start` + `status --wait` 续窗两段式。

**SIGINT**:首次 `cancel_discussion` 并退 3(session 保留,可 status 续观察);
二次硬退。

**退出码(全链;stop_reason 按开放集映射,daemon 可加新值)**:

| 码 | 场景 |
|---|---|
| 0 | `group_chat_end` / `max_rounds` / `cancelled`(他人硬停,result 可读)/ `preempted`(interrupt 收束完成,summary 已落库);动词取数成功 |
| 1 | 不可达/协议错/工具语义错;`interrupted`(崩溃恢复可续跑态);表外未知 stop_reason(stderr 回显原值) |
| 2 | `error` / `nominee_unknown` / `participant_unresolved`(异常收场族) |
| 3 | SIGINT(cancel_discussion 已发,session 保留) |
| 6 | `budget`(预算帽到顶;无收束轮,summary 可缺) |
| 7 | `--timeout` 到点(**不 cancel**,讨论仍在跑,勿重跑) |
| 64 | 用法错(`--token-budget`/`--roster`/`--wait` 校验、缺参) |

## 示例

```bash
evl status
evl chat "列出当前目录文件并总结" --mode plan --output json < /dev/null | jq .
evl chat "继续上一问" --session <id> --output json < /dev/null
evl sessions --output json | jq '.[] | select(.busy)'
evl usage --output json | jq '.providers[].totals'
evl discuss presets --output json | jq '.presets[].key'
evl discuss "评审这个设计的取舍" --output json          # 全链(5-15min,烧真 token)
evl discuss start "长议题" --output json                 # 两段式:先建群
evl discuss status <sid> --wait 540                      # 续窗观察(变化即返)
evl discuss result <sid> --output json | jq .summary
```

## 开发

```bash
cd cli && node --test    # 纯函数单测(参数/帧切分/格式化/project 匹配/终态分类/json 形状/MCP 三态解析/轮询状态机/退出码映射)
```

测试只覆盖纯函数,不打真 daemon、不发 LLM;端到端 live 验证手动做
(参照任务 implement.md 的 live 清单)。

## 已知限制(design §8)

- 不做事件重连/补偿:SSE 断 = 报错(退出 1),调用方重试。
- 宿主先杀 evl 时 daemon loop 仍在跑(见上超时不变量;discuss 侧
  `evl discuss status <sid>` 兜底)。
- SSE 订阅全局外部性:evl 订阅窗口内其他 session 的权限 ask 从 8s 快拒变 120s
  长等(daemon registry 订阅计数不分 session;零 daemon 改动前提无解,知悉)。
- detach 两段式(chat)/ tasks / REPL:二期剩余(R2;discuss 已落地)。
