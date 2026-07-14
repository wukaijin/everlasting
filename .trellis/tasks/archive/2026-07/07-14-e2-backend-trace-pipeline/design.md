# E2 backend trace pipeline — 设计(child-1)

> 权威设计见 `../07-14-e2-harness-trace-viewer/design.md`。本文件仅标注 child-1 边界 + 实施关键点,不重复 parent design 内容。

## 边界(后端-only)

产入:`agent/trace.rs`(新模块,trace_pipeline helper)+ `db/trace.rs`(或入 sessions.rs,turn_trace CRUD)+ `llm/types.rs`(3 ChatEvent 变体)+ `db/migrations.rs`(v7)+ `db/permissions.rs` + `agent/permissions/audit.rs`(record_audit_event 扩 turn_seq)+ `agent/chat_loop.rs` + `agent/workflow/inject.rs` + `agent/context.rs`(写点接入)+ `commands/`(2 IPC)+ `lib.rs` + `commands/mod.rs`(注册)。

不产入:前端任何文件。

## 实施关键点(复述 parent design 要点,详细见 parent)

### v7 migration(parent design §2)
- 新表 `turn_trace`:`id / session_id / seq / token_usage_json / compaction_json / loop_hint_json / breadcrumb_json / created_at`,UNIQUE(session_id,seq) + 索引(session_id,seq) + ON DELETE CASCADE。
- `ALTER TABLE session_audit_events ADD COLUMN turn_seq INTEGER`(历史 NULL)。
- 非破坏性,`run_migrations` 末尾追加 v7 块(参照 v5/v6 风格)。

### trace_pipeline helper(parent design §3.1)
- 新 `agent/trace.rs`:`record_compaction` / `record_loop_hint` / `record_breadcrumb`,各双写(emit ChatEvent + upsert turn_trace,best-effort 失败 warn!)。
- per-turn token:`Done{usage}` 写点旁 `upsert_turn_trace_token`,复用 `!skip_persist` gate(RULE-A-015,worker 不冲 parent)。

### 写点(parent design §3.2 + research §2)
- C3:`chat_loop.rs:1261`(`compact_messages` 返回后)+ StillOver 分支(`:1286`)。
- C2 soft hint:`chat_loop.rs:2181` 附近(注入 tool_result 前)。≥3 干预已有 loop_intervention 审计,不动。
- token:`chat_loop.rs:1800`(`update_last_turn_usage` 旁)。
- breadcrumb:`inject.rs:343` `append_workflow_breadcrumb` 内(确认 seq 可得,否则传参)。
- **行号漂移**:实施前 grep 复核(research §7)。

### record_audit_event 扩 turn_seq(parent design §4.4 + research §3)
- 签名加 `turn_seq: Option<i64>` + 所有 record_* helper 透传。
- grep 全调用点(21 类)传当前 seq;IPC 处理器(question.rs resolve_* 等无 turn 上下文)传 None。
- 漏传 → 审计行 turn_seq NULL 不归组(回看时该事件游离)。

### ChatEvent 3 变体(parent design §4.1)
`ContextCompacted{seq, tokens_before, tokens_after, dropped_count, degradation}` / `LoopHint{seq, hit_count, verdict_kind}` / `WorkflowBreadcrumb{seq, task_slug: Option, status: Option, breadcrumb_text}`。加在 `llm/types.rs:341` `Recall` 后,wire snake_case。

### IPC(parent design §4.2-4.3)
`list_turn_traces(session_id) -> Vec<TurnTraceRow>`(ORDER BY seq ASC)+ `clear_session_trace(session_id)`。TurnTraceRow 照 AuditEventRow camelCase 模式。注册 lib.rs + commands/mod.rs。

## Tradeoff / 风险(见 parent design §7/§9)

- record_audit_event 签名扩散(机械但广,grep 防漏)。
- breadcrumb seq 可得性(inject 写点验证)。
- worker turn_trace 混入 parent(MVP 接受)。
- always-on 落盘 DB 增长(清理入口兜底)。
