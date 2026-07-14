# E2 harness trace viewer — 执行计划

> 配套 `prd.md` + `design.md`。任务结构建议 parent + 2 child(见 §1),review 确认后再 create child。

## 1. 任务结构(建议,review 确认)

- **parent**: `07-14-e2-harness-trace-viewer`(本 task,持有全局 prd + design + 跨 child 验收 + 集成)。不直接实现。
- **child-1** `e2-backend-trace-pipeline`:后端 trace 管道(emit + 落盘 + migration + IPC)。先做。
- **child-2** `e2-frontend-trace-panel`:前端独立面板(live + 回看 + 渲染)。依赖 child-1 的 event + IPC + 数据结构。

> review 通过后:`task.py create "E2 backend trace pipeline" --slug e2-backend-trace-pipeline --parent .trellis/tasks/07-14-e2-harness-trace-viewer` + 同理 child-2。child 各自 prd 细化 + 复用 parent design。

## 2. child-1 后端 trace 管道 — checklist(有序)

- [ ] **B1 v7 migration**(`db/migrations.rs` 末尾):新表 `turn_trace`(UNIQUE(session_id,seq)+ 索引)+ `ALTER TABLE session_audit_events ADD COLUMN turn_seq INTEGER`。非破坏性。
- [ ] **B2 db 层**(`db/` 新 `trace.rs` 或入 `sessions.rs`):`TurnTraceRow` 实体(camelCase serde)+ `upsert_turn_trace_token/compaction/loop_hint/breadcrumb`(4 个 upsert,各补一列)+ `list_turn_traces(session_id)` + `clear_session_trace(session_id)`。
- [ ] **B3 ChatEvent 加 3 变体**(`llm/types.rs:341`,`Recall` 后):`ContextCompacted` / `LoopHint` / `WorkflowBreadcrumb`(字段见 design §4.1)+ wire snake_case。
- [ ] **B4 trace_pipeline helper**(`agent/trace.rs` 新):`record_compaction` / `record_loop_hint` / `record_breadcrumb`,各双写(emit ChatEvent + upsert turn_trace,best-effort 落盘失败 warn!)。
- [ ] **B5 写点接入**:
  - `chat_loop.rs:1261` `compact_messages` 返回后 → `record_compaction`(现有 tracing 旁)。
  - `chat_loop.rs:2181` 附近 soft hint 注入前 → `record_loop_hint`。
  - `chat_loop.rs:1800` `update_last_turn_usage` 旁(`!skip_persist` gate 内)→ `upsert_turn_trace_token`。
  - `inject.rs:343` `append_workflow_breadcrumb` 内 → `record_breadcrumb`(确认能拿当前 seq,必要时传 seq 参数)。
- [ ] **B6 record_audit_event 加 turn_seq**(`db/permissions.rs`):签名加 `turn_seq: Option<i64>` + 所有 `record_*_audit` helper 透传。grep 全调用点(21 类)传当前 seq;IPC 处理器等无 turn 上下文处传 None。
- [ ] **B7 IPC**(`commands/`):`list_turn_traces` + `clear_session_trace` 命令 + `lib.rs` + `commands/mod.rs` 白名单注册。
- [ ] **B8 测试**:`turn_trace` upsert 累积(多写点同 seq 合并)/ turn_seq 填充正确 / emit+落盘双写一致 / worker `!skip_persist` gate 不冲 parent / 历史 DB 升级 v7 零错。
- [ ] **B9 验证**:`cargo check` + `cargo test --lib` + `cargo fmt`(WSL 加 PKG_CONFIG_PATH,见 §4)。

## 3. child-2 前端 trace 面板 — checklist(有序,依赖 child-1 完成)

- [ ] **F1 类型 + store**:`TurnTrace` 前端类型(token/compaction/loop/breadcrumb 子对象)+ `useTraceStore`(Pinia:`currentSessionTraces: Map<seq, TurnTrace>` + `loadHistory(sessionId)` + `clearSessionTrace` + `panelOpen`)。
- [ ] **F2 live 增量**(`streamController.ts` `handleChatEvent`):加 3 case(`context_compacted`/`loop_hint`/`workflow_breadcrumb`)upsert `currentSessionTraces`;`startRequest` 清空(同 `recallHitsBySession` 模式)。
- [ ] **F3 回看**:`loadHistory` 调 `list_turn_traces` + `list_session_audit_events`,审计按 `turn_seq` 归组到 `TurnTrace.auditEvents`,两源映射到统一 `TurnTrace`。
- [ ] **F4 组件**:`<TracePanel>` drawer shell + `<TurnTimeline>`(seq 主轴)+ `<TurnCard>`(latency + token 分布 + compaction + loop + breadcrumb + tool calls 点开)。
- [ ] **F5 挂载**:`AppShell.vue` body 扩 `Sidebar + main + <TracePanel>(drawer)` + AppHeader toggle 入口。drawer 右滑入可折叠。
- [ ] **F6 渲染复用**:`parseAuditPayload` + 13 类 icon family 分桶;失败高亮(`exit_code!=0`/`degradation=="still_over"` → 红边复用 `audit-item--critical`);token 5 字段迷你条形图(纯 CSS + tokenUsage 色阶)。
- [ ] **F7 清理 UI**:TracePanel header 清理按钮 → `clear_session_trace` IPC + 刷新。
- [ ] **F8 测试**:vitest(store live 增量 / 回看归组 / 清理)+ vue-tsc 0 err。
- [ ] **F9 验证**:`cargo check`(IPC 类型对齐)+ vitest + vue-tsc。

## 4. 验证命令

```bash
# Rust(WSL 必须加 PKG_CONFIG_PATH,见 CLAUDE.md §Common Commands)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && cargo fmt

# 前端
cd app && pnpm build          # vue-tsc --noEmit + vite build
cd app && pnpm vitest run     # 或 pnpm test
```

## 5. 风险文件 / 回滚点

| 文件 | 风险 | 回滚 |
|---|---|---|
| `db/migrations.rs` | v7 加表/列,老 DB 升级 | DROP TABLE + DROP COLUMN(SQLite 3.35+) |
| `db/permissions.rs` + 所有 `record_*_audit` 调用点 | turn_seq 签名扩散,漏传 → 审计不归组 | git revert(机械改动) |
| `agent/chat_loop.rs` | 4 个写点接入,动主 loop | 各写点独立,逐个 revert |
| `llm/types.rs` | ChatEvent 加变体 | 删变体(前端无依赖即安全) |
| `agent/inject.rs` | breadcrumb 写点取 seq | 传 seq 参数,可回退 |
| `app/src/components/layout/AppShell.vue` | 布局改 drawer | 复原 `Sidebar + main` |
| `app/src/stores/streamController.ts` | handleChatEvent 加 case | 删 case |

回滚粒度:child-1 / child-2 各自独立可 revert(后端/前端分 task)。后端可进一步按 B1-B7 逐写点 revert。

## 6. task.py start 前 follow-up

- [ ] review 确认任务结构(parent + 2 child vs 单 task 串行)。
- [ ] 若拆 child:create child-1/child-2(`task.py create --parent`)+ 各自 prd 细化 + 复用 parent design;parent 仅持全局 prd/design + 跨 child AC。
- [ ] 若 sub-agent dispatch:child 的 `implement.jsonl` / `check.jsonl` 各 curate 至少 1 条真实 spec/research 入口(spec:`.trellis/spec/backend/tool-contract.md` / `.trellis/spec/frontend/chat.md`;research:本 task 调研结论可落 `research/`)。seed `_example` 不算。
- [ ] 自定项 D4(粒度)/D5(MVP 边界)/D6(任务结构)review 确认或调整。
- [ ] start child-1(后端)先;child-2 待 child-1 event+IPC+数据结构落地后 start。

## 7. 零回归验收(AC8)

- 现有审计写入(21 类)行为不变,仅多 turn_seq 列。
- C3 压缩决策 / C2 循环干预 / breadcrumb 注入 逻辑零改动(trace 旁路观测)。
- `cargo test --lib` 现有用例全绿 + `vitest` 现有用例全绿 + `vue-tsc` 0 err。
