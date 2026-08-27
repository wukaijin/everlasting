# Research — RULE-SMOKE-001 / RULE-PERM-002 深挖(2026-08-27,主 session 全代码核验)

## A. RULE-SMOKE-001:turn-smoke.sh 轮询提前命中腰斩进行中 chat

### 竞态链(已逐环核验)

1. `POST /api/v1/agent/chat` **立即返回**(`daemon/routes/agent.rs:13` 文档:handler 返回空 body,
   `chat_inner` 内部 `tokio::spawn` agent loop,事件走 SSE)。
2. agent loop 每次内层 LLM 调用的 `ChatEvent::Done` **当场落库** turn_trace 行
   (`agent/chat_loop/drive.rs:1483` `upsert_turn_trace_token`,Done 臂内)。
   多轮工具 turn(第一段 Done 带 tool_use → dispatch tools → 第二段 LLM 调用…)的
   **第一段 Done 落库时整个请求还在跑**。
3. `scripts/turn-smoke.sh:156-159` `send_and_wait` 轮询条件是
   `[ "$(max_seq)" -gt "$BEFORE_SEQ" ] && return 0` —— 第一段行落库即返回。
4. 脚本走完报告段 → EXIT trap `cleanup()` → 非 `--keep` 时 `delete_session` →
   daemon 取消 in-flight chat(实测 "cancelled in-flight chat",多轮工具 turn 被腰斩)。

### 正确的"整个请求结束"信号(零后端改动)

**循环层终端 `Done`**:provider 的 per-turn `Done` 在 drive.rs Done 臂被消费**不转发**到 sink;
只有 `should_continue == false`(模型不再发 tool_use)时循环层自己 emit 一次终端 Done
(drive.rs:1953,`emit_chat_event_via_sink(&sink, &rid, &ChatEvent::Done{stop_reason, usage})`),
cancel(`cancelled`,drive.rs:1848)/ max_turns(`loop_terminated`,drive.rs:2076)同族。
即:**wire 上的 `kind=="done"` 事件一个请求只发一次,且必然在请求真正结束时**。

wire 形状(`agent/helpers.rs:449` `ChatEventPayload` + `ChatEvent` serde):
`{"request_id":"<rid>","kind":"done","stop_reason":...,"usage":{...}}`
(`#[serde(tag="kind", rename_all="snake_case")]`,`request_id` 是外层字段,compact JSON 无空格)。

daemon 侧经 `daemon/sse.rs` `SseRegistry.broadcast("chat-event", payload)` 上
`GET /api/v1/stream`(`daemon/routes/stream.rs`)。**注意**:新连接不回放历史(replay 仅限
带 Last-Event-ID 的重连),所以订阅必须**先挂再发**——脚本里 `--assert-turn-usage`
已有同款先例注释("事件随 turn 推出,迟挂会漏")。

worker subagent 的 chat-event 不上 `chat-event` 通道(SubagentBufferSink 不转发,走
`subagent:event`),与主 rid 无串扰;每轮 REQ_ID 唯一(`turn-smoke-$(date +%s)-$BEFORE_SEQ`),
按 request_id 过滤即可。

### 修法(定稿)

- 脚本启动即挂**常驻 SSE 订阅**(mktemp 日志 + 后台 curl,EXIT trap 里 kill;现有
  `--assert-turn-usage` 的独立订阅合并进来,同一份日志复用解析)。
- `send_and_wait`:POST 后轮询 SSE 日志中等本 REQ_ID 的终态:
  - `kind=="done"`(任意 stop_reason,打印之)→ 成功返回;
  - `kind=="error"` → 报错退出(请求以错误终止);
  - 超时沿用 `$TIMEOUT`(语义从"等 turn_trace 行"变为"等请求终态",更贴实际用途)。
- 原 `max_seq` 轮询保留为报告段兜底(已有 `max_seq < 0` guard),不再做 send_and_wait 的
  退出条件。
- `--assert-turn-usage` 段改读共享日志(等价于原独立订阅;turn_usage 事件在 done 前 emit,
  done 已见则必然已写入,保留小 sleep 兜 flush)。

grep 实现注意:compact JSON,`"kind":"done"` 与 `"request_id":"$REQ_ID"` 同行,两个
grep 串联即可,不依赖字段顺序。

## B. RULE-PERM-002:grant API 写入永不生效的授权行

### 消费矩阵(读侧,已核验)

`agent/permissions/check/permission.rs` Tier 4 按 `classify_tool` 分派:

| ToolKind | 工具 | 消费的 match_kind | 读侧查询 |
|---|---|---|---|
| Path | read_file/write_file/edit_file/list_dir/grep/glob | `path`(glob) | `check_path_grant`(只查 kind='path' 行) |
| Shell | shell / **run_background_shell** | `prefix` | `check_prefix_grant`(**硬编码 `tool_name='shell'`**) |
| WebFetch | web_fetch | `tool` | has_tool_permission 族 |
| GitMutation | merge_worker / discard_worker | `tool` | 同上 |
| Other | 未知/未来 | (default Allow) | — |

写侧(`match_value_for_allow_always`,permission.rs:821)自动挑 kind 与上表一致:
Path→"path"、Shell→"prefix"、WebFetch/GitMutation/Other→"tool"。

### 债项本体

`commands/permissions.rs:217` `grant_tool_permission_inner`(IPC + daemon route
`routes/permissions.rs:49` 共用入口)对 `match_kind="tool"` 分支**不校验工具类别**:
`grant(shell, tool, None)` 成功入库但 Shell 分支只消费 prefix → 死数据,无警告。
前端从不直接调它(仅 http.ts transport 映射存在;AllowAlways 走 permission_response →
agent loop 自动挑 kind),故仅裸 API/自动化脚本踩。

### 顺带发现的同族坑(本次一并修)

AllowAlways 在 `run_background_shell` 上的点击写的是
`(run_background_shell, prefix, <token>)` 行(ask.rs:532 用原始 tool_name 直写 db),
而 `check_prefix_grant`(permission.rs:789-798)查询**硬编码 `tool_name='shell'`** →
该行同样永不命中,用户"始终允许"不粘轮。修法:读侧放宽为
`tool_name IN ('shell','run_background_shell')`(一处改动同时救活存量死行 + 两条写路径;
不做写侧归一化——归一化救不回已入库死行,且 leave 双名并存的语义问题)。

### 校验矩阵(写侧新增,拒绝死数据)

`grant_tool_permission_inner` 在既有 kind 合法性检查后,按 `classify_tool(tool_name)`
校验 kind↔类别匹配(镜像 `match_value_for_allow_always` 的挑法),不匹配 →
`ErrorCategory::InvalidRequest`,报错文案给出该工具类别唯一合法 kind:

- Path → 只许 `path`
- Shell → 只许 `prefix`
- WebFetch / GitMutation / Other → 只许 `tool`

(auto-转译方案否决:tool→prefix 无从推导前缀,空前缀 = 全放行,是提权不是兼容。)

既有测试基建:`agent/permissions/tests_check.rs` 有 `seed_shell_prefix_grant` helper +
prefix 短路用例(in-memory pool);grant 校验本体抽**纯函数**(避免构造 AppState),
纯函数单测放 commands/permissions.rs 的 tests 模块;`check_prefix_grant` 放宽用
tests_check.rs 既有 harness 补一条 `run_background_shell` 行命中用例。

## 相关 file:line 锚点

- `scripts/turn-smoke.sh:128-162`(send_and_wait)、`88-98`(cleanup trap)、`164-184`(SSE 订阅段)
- `app/src-tauri/src/agent/chat_loop/drive.rs:1382-1508`(Done 臂)、`1953`(终端 Done)
- `app/src-tauri/src/agent/helpers.rs:449`(emit_chat_event_via_sink)
- `app/src-tauri/src/daemon/sse.rs:331`(chat-event broadcast)
- `app/src-tauri/src/commands/permissions.rs:217-266`(grant_tool_permission_inner)
- `app/src-tauri/src/agent/permissions/check/permission.rs:781-804`(check_prefix_grant)、`821-856`(match_value_for_allow_always)
- `app/src-tauri/src/agent/permissions/ask.rs:524-545`(AllowAlways 写入)
