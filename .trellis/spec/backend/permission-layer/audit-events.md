<!-- Moved from permission-layer.md 2026-09-19 (doc-split) -->

### 6. Audit (`session_audit_events`) — 20 类 AuditKind(2026-07-07 +3 增)

PR1 在 `agent::permissions::AuditKind` 实现首批 9 行(10 variant,`yolo_entered` / `yolo_exited` 共行),后续 PR(2026-06-14 C4 PR1 `ToolExecuted` + 2026-06-17 D3 `EditMessage` / `ResendMessage` + 2026-06-22 RULE-WorkerAsk-001 4 个 `WorkerAsk*` + 2026-07-07 `request_mode_change` 3 个新 kind)扩到 **20 类**(见
`audit §3.4` 完整列表)。`payload_json` 字段统一结构:

```json
{
  "tool_name": "shell",
  "tool_input": { "command": "ls -la" },
  "reason": "matches denylist: rm -rf /",
  "mode": "edit",
  "critical": true
}
```

`critical: bool` 字段对前端 `PermissionModal` 的 3px 红左 border
+ shield-x icon 渲染至关重要(PR1 follow-up 加,PR3 使用)。

| AuditKind | 触发条件 |
|---|---|
| `tool_denied` | Tier 2 命中 + Tier 3 user deny + Tier 3 sender dropped |
| `tool_allowed` | Tier 3 AllowOnce / Tier 3 "始终允许" 命中 / Tier 5 默认 |
| `tool_permission_ask` | Tier 3 emit `permission:ask` |
| `permission_granted` | Tier 3 "始终允许" → 写 `session_tool_permissions` |
| `permission_timeout` | Tier 3 120s 超时 |
| `tool_denied_yolo` | Tier 2 命中 + mode = Yolo(audit 跟普通 tool_denied 区分) |
| `mode_changed` | `set_session_mode` 调用 |
| `yolo_entered` / `yolo_exited` | Mode 在 Yolo 之间切换 |
| `request_cancelled` | C1 cancel 触发(tier 3 await 被 cancel 打断) |
| `tool_executed` | ⑩ tool 执行完成(`record_tool_executed_audit`,C4 PR1 2026-06-14);payload `{tool_name, tool_input, duration_ms, exit_code: Option<i32>}`(`null` = 无 exit code,`-1` = 被 kill)。含 2026-06-22 L3b PR3+ `ToolKind::GitMutation` 写入对齐 |
| `edit_message` | D3 PR1 (2026-06-17):session 内 user 编辑消息(in-place update + 级联删后续 message);payload `{message_seq, new_text_preview, edited_at}`。落表点在 `db::sessions::edit_user_message` 事务尾部 |
| `resend_message` | D3 PR3 (2026-06-17):session 内 user 点 Resend 重发(不修改 content,只 cancel 旧 stream + 重 send);payload `{message_seq, content_text_preview}`。落表点 best-effort 异步(`record_message_resend_audit`) |
| `worker_ask_allowed` | worker Tier 4 交互式 ask → user "Allow" / "仅一次"(oneshot 收到 `PermissionResponse::AllowOnce` / `AllowAlways`);payload `{worker_run_id, tool_name, tool_input}`(对齐 `ToolAllowed` 形状,2026-06-22 RULE-WorkerAsk-001) |
| `worker_ask_denied` | worker Tier 4 ask → user "Deny";payload `{worker_run_id, tool_name, tool_input, reason?}`(user 可选 "拒绝并说明" feedback) |
| `worker_ask_timed_out` | worker Tier 4 ask → 120s 超时自动 Deny;payload `{worker_run_id, tool_name}`(`tokio::select!` timeout 臂命中) |
| `worker_ask_cancelled` | worker Tier 4 ask → parent session cancel 触发 resolve 为 Deny;payload `{worker_run_id, tool_name}`(`tokio::select!` cancel 臂命中,parent_token → worker_token child 取消) |
| `mode_change_requested` | `request_mode_change` tool 入口(对齐 2026-07-07 落地);LLM 调 tool 触发,记录申请;payload `{target_mode: "edit"\|"plan"\|"yolo", reason: Option<String>, noop: bool}`(`noop=true` 是 LLM 申请切到当前 mode 的留痕) |
| `mode_change_allowed` | `request_mode_change` resolve 走允许路径(2026-07-07);user 在 card 上点"允许"(Yolo 路径经二次 modal "确认"后);payload `{prev_mode: "edit"\|"plan"\|"yolo"\|"background", new_mode: "...", target_mode: "..."}`,**不**含 `mode_changed`(由 `db::update_session_mode` 自动产生,职责分离) |
| `mode_change_denied` | `request_mode_change` resolve 走拒绝路径(2026-07-07);user 在 card 上点"拒绝" / Yolo 二次 modal "取消" / `is_running_as_root` 守卫触发;payload `{target_mode, reason: "user denied"\|"yolo_root_guard"\|"yolo_cancelled_confirm"}` |

