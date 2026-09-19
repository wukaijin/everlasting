# MCP wire 证据档(实读 mcp.rs / 冒烟脚本,2026-09-19)

权威源:`app/src-tauri/src/daemon/routes/mcp.rs`、docs/DAEMON-API.md §6.5、
`scripts/group-chat-mcp-http-smoke.mjs`(SDK client 先例)。

## 传输探针(冒烟脚本已验)

- POST 缺双 Accept → **406**;Accept 必须同时含 `application/json` 与
  `text/event-stream`。
- POST Content-Type 非 application/json → **415**。
- GET /mcp → 405;DELETE /mcp → 200 no-op。
- 响应 200 纯 JSON(非 SSE)。

## JSON-RPC 分发(mcp.rs:229 handle_request)

- 无状态:**不要求先 initialize**;`ping`/`tools/list`/`tools/call`/
  `initialize` 逐请求独立处理。CLI 直发 tools/call。
- 未知方法 → `-32601`;未知工具名/缺 name → `-32602`(JSON-RPC error)。
- 工具执行错误**不走** JSON-RPC error:200 + `result.isError:true`,text 为
  `{error}`(semantic)或 `{error, hint}`(infra, hint = daemon 拉起提示)。

## text_result 形状(mcp.rs:430)

```json
{ "content": [{ "type": "text", "text": "<pretty JSON>" }], "isError": false }
```

→ CLI 需 `JSON.parse(body.result.content[0].text)` 二次解析。

## 八工具关键语义(实现 `tool_*` 系列)

| 工具 | 入参 | 返回要点 |
|---|---|---|
| start_discussion | topic*+cwd*(必填),preset?(缺省 `review`,mcp.rs:1283),participants?,token_budget? | `{session_id, request_id, hint}` |
| discussion_status | session_id*, wait_seconds? 1-25, detail? | `{busy, stop_reason, elapsed_s}` + detail 时 `messages/last_speaker/tokens{total,per_speaker}` + wait 尽 `wait_timed_out:true`;**wait 隐含 detail**(mcp.rs `want_progress`) |
| discussion_result | session_id* | `{stop_reason, summary, roster{moderator,participants}, stats{messages,elapsed_s}, detail?/detail_warning?, tokens?, transcript_path?, summary_warning?}`;**无 session_id 键**;非终态 → 语义错 "still running"(mcp.rs:1138) |
| cancel_discussion | session_id* | busy → `{cancelled, note}`;非 busy 幂等 `{already_finished, stop_reason}` |
| interrupt_discussion | session_id* | `{interrupted, hint}`(preempt 收束) |
| inject_message | session_id*, text* | 非 busy → 语义错(busy guard,防重启编排抹 summary,mcp.rs:1297) |
| list_models / list_presets | — | 目录 |

- 终态判定:`busy==false && stop_reason != null`;**stop_reason 是开放集**
  (评审 09-19 实读更正):group_chat_loop.rs:141-171 具名常量
  `group_chat_end|max_rounds|nominee_unknown|participant_unresolved|error|
  cancelled|preempted|budget` + agent.rs:169/216/233 崩溃恢复写的
  `interrupted`——共 9 值且 daemon 可加新值,消费方按开放集处理。
- **`wait_timed_out` 键仅在 true 时写入**(mcp.rs:1014,`if wait_timed_out`
  才 set)——变化即返时键**缺失**;判据必须 `!== true`,写 `== false` 会把
  变化检测整体反转。终态时 status 先短路立即返回(mcp.rs:1062,不进 wait)。
- 转录惰性导出:status/result 首次观测终态时落
  `{app_data_dir}/discussions/{date}-{slug}-{sid8}.md`。

## 退出码先例(scripts/group-chat-run.mjs)

- stop_reason=budget → 退出码 **6**(EXIT.budget,m1 :58)。
- SIGINT → cancel 停编排保 session(:754)。
- `--token-budget` 正整数校验(:660)。

## daemon 侧已守门(不需要 CLI 复制)

- preset 三趟解析(内置 key/行 UUID/行名称)+ participants 与 preset 的
  moderator 合并语义 → CLI `--roster` 只拦 JSON 语法,name/model 校验透传。
- token_budget 正整数(daemon 也校验)→ CLI 侧仍拦(64 早失败优于 1 晚失败)。
