# Pattern: 请求终态 Done 事件(wire 恰一次;turn_trace 行 ≠ 请求结束)

> 来源:任务 `.trellis/tasks/archive/2026-08/08-27-rule-smoke-perm-cleanup/`
> (RULE-SMOKE-001 闭合)。消费方:前端 `streamEvents.ts` 的 finalize 门、
> `scripts/turn-smoke.sh` 的 send_and_wait 轮询。

## 不变量(load-bearing)

**`chat-event` 通道上的 `{"kind":"done"}` 事件一个 chat 请求恰好 emit 一次,
且必然在请求真正结束的路径上。** 两层 Done 语义必须区分:

- **provider 的 per-turn `Done`**(每次 LLM 调用一个):在 `drive.rs` 的
  Done 臂被 loop **消费,不上 wire** —— 它落库 turn_trace 行
  (`upsert_turn_trace_token`)、emit `TurnUsage` 事件,但此刻多轮工具
  turn 仍在进行(后面还有 dispatch tools + 后续 LLM 调用)。
- **循环层终端 `Done`**(loop 自己构造):仅在 `should_continue == false`
  (模型不再发 tool_use)时 emit 一次;cancel(`stop_reason="cancelled"`)、
  max_turns(`"loop_terminated"`)、softcap 停止等同族出口各自 emit。
  wire JSON:`{"request_id":"<rid>","kind":"done","stop_reason":...,"usage":...}`
  (compact,`ChatEventPayload` 外层 request_id + `ChatEvent` serde tag=kind)。

**推论:turn_trace 行出现 ≠ 请求结束。** 任何"等这个请求跑完"的外部观察者
(脚本 / 测试 harness / 未来 E2E)必须等本 rid 的 `kind=done`,不能轮询
DB 行出现就收线 —— 否则紧随其后的清理(如 `delete_session`)会取消
in-flight chat,把多轮工具 turn 腰斩(RULE-SMOKE-001 实测事故形态:
"cancelled in-flight chat")。

## 订阅时序与过滤

- daemon `GET /api/v1/stream`(全局单流)新连接**不回放历史**(replay 仅限
  带 Last-Event-ID 的重连)→ 观察者必须**先挂订阅再发请求**,迟挂会漏终态。
- 按 `request_id` 过滤即可隔离串扰:rid 每请求唯一;worker subagent 的
  chat-event 不上 `chat-event` 通道(SubagentBufferSink 隔离,走
  `subagent:event`)。
- `kind=error`(同 rid)是失败终态,与 done 互斥先到;观察者应分别处理。

## turn-smoke.sh 的实现要点(2026-08-27 起)

常驻后台 `curl -sN /api/v1/stream` 到 mktemp 日志(无 `--max-time`,EXIT
trap kill),`send_and_wait` POST 后轮询日志中本 rid 的终态。bash 陷阱:
`set -o pipefail` 下 `grep -q` 命中即退会让上游 grep 收 SIGPIPE(141)、
整管道误判失败 → 终态判定用变量捕获行(`LINE="$(grep … | tail -n 1 || true)"`)
而非 `grep -q`。
