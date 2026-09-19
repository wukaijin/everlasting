# evl — Everlasting daemon CLI

daemon(`:7456`)HTTP API 的**薄壳**命令行工具:零新后端能力,纯门面。主要使用者是
**LLM(宿主 agent 经 bash 委派任务)**,人类 TTY 使用次之。零 npm 依赖,Node ≥ 20。

设计与 wire 契约:`.trellis/tasks/09-19-everlasting-cli/design.md`(全部带源码出处)。

## 安装

```bash
node cli/bin.mjs --help          # 直跑(仓库内)
pnpm link --dir cli              # 或挂全局:任意目录裸 `evl`
```

daemon 拉起:`./scripts/daemon.sh bg`(默认 `http://127.0.0.1:7456`,
env `EVERLASTING_BASE` / `--base-url` 覆盖)。

## 命令

| 命令 | 作用 | 说明 |
|---|---|---|
| `status` | daemon health + 版本 | 不可达:非零 + OS 错误翻译 + daemon.sh 提示 |
| `chat "<msg>"` | 委派一轮 agent loop | LLM 主入口;详见下 |
| `sessions` | 列 session(id/busy/stop_reason/…) | 默认跨全部 project 合并;`--project <path>` 收窄 |
| `projects` | 列 project(含隐藏) | |
| `models` | 列 model + 标默认 | 模型引用只认 UUID |
| `usage` | token 用量窗口 | `--provider <id>` 过滤 |

全局 flags:`--base-url` / `--output text|json` / `--quiet` / `--verbose` /
`--timeout <s>` / `--no-color` / `--non-interactive` / `-h` / `--version`;
chat 专属:`--mode plan|edit|yolo` / `--session <id>` / `--ephemeral` / `--model <id>` /
`--project <path>`。

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
  `64` 用法错误。
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

## 示例

```bash
evl status
evl chat "列出当前目录文件并总结" --mode plan --output json < /dev/null | jq .
evl chat "继续上一问" --session <id> --output json < /dev/null
evl sessions --output json | jq '.[] | select(.busy)'
evl usage --output json | jq '.providers[].totals'
```

## 开发

```bash
cd cli && node --test    # 纯函数单测(参数/帧切分/格式化/project 匹配/终态分类/json 形状)
```

测试只覆盖纯函数,不打真 daemon、不发 LLM;端到端 live 验证手动做
(参照任务 implement.md 的 live 清单)。

## 已知限制(design §8)

- 不做事件重连/补偿:SSE 断 = 报错(退出 1),调用方重试。
- 宿主先杀 evl 时 daemon loop 仍在跑(见上超时不变量)。
- SSE 订阅全局外部性:evl 订阅窗口内其他 session 的权限 ask 从 8s 快拒变 120s
  长等(daemon registry 订阅计数不分 session;零 daemon 改动前提无解,知悉)。
- detach 两段式 / discuss / tasks / REPL:二期(R2)。
