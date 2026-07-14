# E2 后端 trace 管道 — 调研结论(写点 / 签名 / schema)

> 三 Explore agent 调研提炼(2026-07-14)。本文件是 child-1 实施时的关键事实参考,所有行号基于调研时基线,实施前以代码为准复核。

## 1. 数据缺口全景(本 child 要补的)

| 维度 | 当前状态 | 补法 |
|---|---|---|
| C3 压缩 | `CompactResult{tokens_before,tokens_after,dropped_count,degradation}` 只在 `tracing`,零持久化 | emit `ContextCompacted` + 落 turn_trace.compaction_json |
| per-turn token | `update_last_turn_usage` OVERWRITE session snapshot 列,历史永久丢失 | `Done{usage}` 时 upsert turn_trace.token_usage_json |
| C2 soft hint | 1-2 连击只塞 tool_result block,无事件;仅 ≥3 落 loop_intervention 审计 | emit `LoopHint`(soft hint 时) + 落 turn_trace.loop_hint_json |
| workflow breadcrumb | 注入 messages[0] 后丢弃,无 snapshot | emit `WorkflowBreadcrumb` + 落 turn_trace.breadcrumb_json |
| 审计 turn 对齐 | session_audit_events 无 turn_seq 列 | v7 加 turn_seq 列 + record_audit_event 传 seq |

## 2. 精确写点(emit + 落盘接入位置)

| 信号 | 文件:行 | 现状 | 接入 |
|---|---|---|---|
| C3 压缩 | `agent/chat_loop.rs:1261` | `tracing::info!("agent loop: context compressed (C3)")` 带 dropped_count/tokens_before/after | 旁加 `record_compaction(sink, db, sid, seq, &compact_result)` |
| C3 StillOver | `agent/chat_loop.rs:1286` | `tracing::error!` + `ChatEvent::Error{InvalidRequest}` + return | record_compaction 同样落(degradation=still_over) |
| C2 soft hint | `agent/chat_loop.rs:2181` 附近 | soft hint 字符串塞进下一轮 tool_result block | 注入前 `record_loop_hint(sink, db, sid, seq, hit_count, verdict_kind)` |
| C2 ≥3 干预 | `agent/chat_loop.rs:2268/2350/2386/2417` | 已落 `record_loop_intervention_audit`(action=asked/continued/terminated) | 不动(已有审计);LoopHint 只覆盖 1-2 连击 |
| per-turn token | `agent/chat_loop.rs:1800` | `update_last_turn_usage`(`!skip_persist` gate 内,worker gate RULE-A-015) | 旁加 `upsert_turn_trace_token(sid, seq, usage)`,复用同一 gate |
| workflow breadcrumb | `agent/workflow/inject.rs:343` `append_workflow_breadcrumb` | 注入 Text block 到 messages[0],无 emit/落盘 | 内加 `record_breadcrumb(sink, db, sid, seq, slug, status, text)`;**确认能拿当前 seq**(inject 在 turn 循环内,可能需传 seq 参数) |
| `Done{usage}` | `llm/types.rs:341` ChatEvent::Done | 已有 event,带 `usage: Option<TokenUsage>` | 复用,补落盘 |

## 3. 关键签名 / 结构(实施前复核)

### `persist_turn`(turn 边界 + seq 来源)
`db/sessions.rs:692`:`persist_turn(pool, session_id, role, content, seq, latency: Option<&MessageLatency>)`。seq 是 agent loop per-session `next_seq` 计数器(同 `TurnComplete.seq`)。trace helper 的 seq 取同一来源。

### `CompactResult`(C3 压缩返回)
`agent/context.rs:93`:`CompactResult { messages, dropped_count, tokens_before, tokens_after, degradation: DegradationKind }`。`DegradationKind` 三态:None / NoCandidates / StillOver(后者 chat_loop 发 Error 终止 turn)。`as_str()` 序列化用。

### `loop_hit_count` / `loop_window`(C2 状态)
`agent/chat_loop.rs:1205`(loop_hit_count: u32) + `:1195`(loop_window: VecDeque<ToolCall>)。run_chat_loop 局部变量,跨 turn 累积。LoopVerdict::None 重置 0;Hard/Soft saturating_add(1)。LoopHint emit 时取当前 hit_count + verdict_kind("hard"/"soft")。

### `WorkflowCtx` / breadcrumb
`agent/workflow/inject.rs:143` `build_workflow_ctx(db, session_id)`(IPC entry 缓存一次,async/DB-heavy)+ `:343` `append_workflow_breadcrumb(turn_messages, &WorkflowCtx)`(per-turn 同步)。`build_breadcrumb_block`(`:587-637`)构造 Text block body。task_slug/status 从 `WorkflowCtx.current_task`(Option)取。

### `record_audit_event`(要扩 turn_seq)
`db/permissions.rs` + `agent/permissions/audit.rs`。当前签名无 turn_seq。grep 全调用点:`record_audit_event` / `record_tool_executed_audit` / `record_message_resend_audit` / `record_loop_intervention_audit` / `record_ui_diff_applied_audit` + `commands/question.rs` / `tools/request_mode_change.rs` / `tools/request_task_state_transition.rs` / `db/sessions.rs:1205`(edit_message)等。21 类落表。IPC 处理器(question.rs resolve_*)无 turn 循环上下文 → 传 None。

### `ChatEventSink` trait(emit 通道)
`state.rs:561-643`。`emit_chat_event(ChatEvent)` → `chat-event` Tauri channel。`AppHandleSink` impl 转发 app.emit。worker 用 `SubagentBufferSink`(不冒泡 chat-event 到主 chat,同 Recall/Retrying 隔离)。

## 4. schema 现状(migration 基线)

- 最新 migration **v6**(`db/migrations.rs:486`,2026-06-13 Mode 3 档化 backfill)。`run_migrations` 在 `:50`。本 child 用 **v7**。
- `session_audit_events`(`migrations.rs:465`,v5):`id / session_id / ts(TEXT 秒) / kind / payload_json`,索引 `(session_id, ts DESC)`,ON DELETE CASCADE。
- `messages` 表:含 `seq` + latency 4 列(ttfb/gen/total/thinking_ms)+ `content` JSON + `has_tool_calls/has_tool_results`。token 不在 messages(session 表 last_* snapshot 列是 OVERWRITE)。
- `sessions` 表:5 个 `last_*` snapshot 列(OVERWRITE)+ 4 个冻结累计列(production 不写)。

## 5. 已有可复用 IPC / store 模式

- `list_session_audit_events(session_id) -> Vec<AuditEventRow>`(`commands/permissions.rs:322`):全量下推,无分页。本 child 新增 `list_turn_traces(session_id) -> Vec<TurnTraceRow>` 同模式。
- `AuditEventRow`(`db/permissions.rs:335`):`#[serde(rename_all="camelCase")]` {id, sessionId, ts, kind, payloadJson}。TurnTraceRow 照此。
- IPC 注册:`lib.rs` + `commands/mod.rs` 白名单(同 list_session_audit_events)。
- best-effort 落盘:所有 `record_*_audit` 失败 `warn!` 吞,不传播。trace upsert 同模式。

## 6. worker 注意事项

- worker run_subagent 嵌套 run_chat_loop,复用 parent session_id。`update_last_turn_usage` 已 `!skip_persist` gate(RULE-A-015)避免 worker 冲掉 parent snapshot。turn_trace token 写入复用同 gate。
- worker 的 chat-event 不冒泡主 chat(SubagentBufferSink),故 worker turn 的 trace emit 不上行主面板 live;但落盘仍写(复用 parent session_id+seq)。MVP 接受混入,必要时加 is_worker 列(Phase 2)。
- worker 无独立审计 surface(audit.rs 注释),worker 触发的 loop_intervention 不落本表。

## 7. 实施前必复核(行号可能漂移)

- `chat_loop.rs:1261/1800/2181/2268` 等写点行号
- `inject.rs:343` breadcrumb 写点能否拿到 seq(可能要传参)
- `record_audit_event` 全调用点(grep 确认无遗漏,漏传 → 审计 turn_seq NULL 不归组)
- v7 migration 块格式(参照 v5/v6 块风格)
