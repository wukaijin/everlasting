# fix: subagent drawer 空白 + status 卡 running（runId 错配 + 完成后不刷新）

## Goal

修复 B6 PR3 subagent drawer 的两个显示 bug：用户点击 `dispatch_subagent` 卡片打开 drawer 后，
(a) transcript 列表完全空白；(b) 状态 tag 一直显示「运行中」且持续时间无限增长，即使 worker
已确认执行多轮工具调用并结束。让 drawer 能正确展示 worker 的完整 transcript + 终态 status。

## What I already know（根因，已通过代码确认）

### Bug 1（致命）：`subagent:event` 的 `runId` 与 `summary.id` 不同源

后端 `run_subagent` 用了两个不同的 id：

| id | 值 | 来源 | 用途 |
|---|---|---|---|
| `worker_rid` | `"{parent_rid}-sub-{tool_use_id}"` | `chat_loop.rs:2074` `format!` | 传给 `SubagentBufferSink` 当 `run_id` → `subagent:event` 的 `runId` |
| `worker_run_id` | UUID v4 | `subagent_runs.rs:201` `Uuid::new_v4()` | DB 行主键 → `list_runs_summary_by_session` 的 `summary.id` |

证据链：
- `chat_loop.rs:2090-2093` — `insert_run(..., &worker_rid, ...)` 返回 UUID `worker_run_id`（成为 DB `id`）
- `chat_loop.rs:2143-2146` — `SubagentBufferSink::new(handle, worker_rid.clone(), ...)` 把 **worker_rid** 存为 sink `run_id`
- `subagent.rs:613-614` — `build_subagent_event_payload(&self.run_id, ...)` → 事件 `runId = worker_rid`

前端 key 错配：
- `subagentRuns.ts:338-345` `routeEvent` → `liveTranscript` / `getRunCache` 的 key = `event.runId`（worker_rid）
- `ToolCallCard.vue:376` `openDrawer(immediate.id)` → `openRunId` = `summary.id`（UUID）
- `SubagentDrawer.vue:148-155` transcript computed → `liveTranscript.get(UUID)` 查不到 → fallback `run.value?.transcriptJson`
- `run.value` = `getRunCache.get(UUID)` 也查不到（key 是 worker_rid；且 eager-fetch 的 `fetchRun(worker_rid)` 调 `get_subagent_run` 按 UUID `WHERE id=?` 查 worker_rid 返回 null，从未写入）
- → `run.value = undefined` → transcript `[]`（空白）+ `status` fallback `"running"`（一直涨时间）

卡片能正确显示 completed 是因为 `getSummaryByToolUseId`（`subagentRuns.ts:319-321`）走 `parentRequestId.endsWith("-sub-"+toolUseId)` 后缀匹配，**不依赖 id 一致性**。

### Bug 2（状态不刷新）：worker 完成后前端 cache 不刷新

`getRunCache`（drawer 数据源）只在 eager-fetch（worker 首次发事件）时写一次（running 时刻 snapshot）。
worker 完成（`update_run_finished`）后**无任何事件触发刷新**。`openDrawer` 还有 `if (!getRunCache.has(runId))`
守卫（`subagentRuns.ts:294`），已 cache 就不 refetch。所以 drawer 的 status 卡 running。
`runSummaryBySession`（卡片数据源）会在任意 `fetchForSession` 时刷新 → 卡片 completed、drawer running 的割裂。

## Requirements

- R1：`subagent:event` 的 `runId` 必须等于 `summary.id`（即 DB 行 id = `worker_run_id`），使前端 store 的
  `liveTranscript` / `getRunCache` key 与 `openRunId` 一致。
- R2：worker 进入终态（completed / cancelled / error）后，drawer 若打开着，status + finishedAt + transcript
  必须自动更新为终态（不再卡 running、持续时间不再增长）。
- R3：用户在 worker 完成后才首次打开 drawer 时，`getRunCache` 必须拿到终态 row（而非 running snapshot）。
- R4：不破坏现有 `getSummaryByToolUseId` 的 parentRequestId 后缀匹配（卡片 lookup 逻辑不变）。

## Acceptance Criteria

- [ ] AC1：后端单元测试断言 `SubagentBufferSink` 发出的 `subagent:event` payload `runId` 等于 `insert_run` 返回的 id。
- [ ] AC2：前端 store 测试覆盖「listener 用 event.runId 写入 → openDrawer(summary.id) 能读到同一份数据」的跨 id 联动（回归测试，锁 Bug 1）。
- [ ] AC3：worker 终态后 drawer 打开时 status 显示「完成/已停止/出错」+ 冻结持续时间，不再显示「运行中」。
- [ ] AC4：drawer transcript 在 worker 完成后展示完整 tool_call/tool_result 序列（非空）。
- [ ] AC5：`get_subagent_run` 的 runId 参数语义文档化（= DB id，非 worker_rid）。

## Definition of Done

- Rust `cargo test --lib` 通过（含新增 sink run_id 断言）
- 前端 `vitest` 通过（含新增跨 id 联动回归测试）
- `vue-tsc --noEmit` 通过
- 不 commit（等用户验证）

## Out of Scope

- subagent 嵌套（worker dispatch 自己的 subagent）—— MVP 已排除
- daemon 化 worker / 独立 session row —— V2 路线图项
- drawer 内 transcript 的实时滚动 / 截断 UI 调整 —— 现有逻辑保留
- `openDrawer` 防御性 refetch（完成事件到达前的极小窗口）—— 不做，事件在 `update_run_finished` 之后立即 emit，窗口可忽略

## Technical Approach

### Bug 1：sink run_id 改用 DB id

`chat_loop.rs::run_subagent` 里，sink 构造（2142）在 `insert_run`（2090）之后。把
`worker_run_id_opt` 作为 sink 的 `run_id` 传入（insert 失败时 fallback `worker_rid`，此时无 DB 行 /
drawer 打不开，无副作用）：

```rust
let event_run_id = worker_run_id_opt.clone().unwrap_or_else(|| worker_rid.clone());
SubagentBufferSink::new(handle.clone(), event_run_id, parent_session_id.to_string())
```

这样 `subagent:event` 的 `runId === summary.id`，前端 store 的 `liveTranscript` / `getRunCache` key
与 `openRunId` 统一。`SubagentBufferSink.run_id` 字段语义从「worker request id」改为「DB row id
（用于 IPC 事件路由）」—— 注释同步更新。

### Bug 2：后端 emit `subagent:finished` 完成事件

在 `run_subagent` 的 `update_run_finished` 成功后，emit 独立 Tauri 事件 `subagent:finished`：

```json
{ "runId": "<worker_run_id>", "sessionId": "<parent_session_id>", "status": "completed|cancelled|error", "finishedAt": "<rfc3339>" }
```

- **为何独立事件而非扩展 `subagent:event`**：`TranscriptKind`（chat_event/tool_call/tool_result/
  permission_ask）语义是「transcript 条目」，终态不是 transcript 条目，塞进去会污染 drawer 的
  transcript 渲染（`SubagentDrawer.vue` 的 `transcript` computed 会把它当条目列出）。独立通道语义清晰。
- 前端 `subagentRuns.ts` listener 增加对 `subagent:finished` 的监听：收到后 `fetchRun(runId)` 刷新
  `getRunCache`（拿到终态 status + finishedAt + 完整 transcript）+ `fetchForSession(sessionId)` 刷新
  `runSummaryBySession`（卡片同步转态）。drawer 的 `status` / `statusDisplay` / `transcript` computed
  自动跟随 `getRunCache` 更新 —— 无需改 drawer 组件。
- `subagent:finished` 不走 `eagerFetchedRunIds` dedup（它本身就是一次性的终态信号）。

## Decision (ADR-lite)

**Context**：drawer 空白 + 卡 running 的根因是跨层 id 错配（event.runId 用 worker_rid，summary.id 用
UUID），叠加 worker 完成后无刷新机制。

**Decision**：
1. Bug 1 —— sink run_id 改用 `insert_run` 返回的 DB id（`worker_run_id`），统一事件 runId 与 summary.id。
2. Bug 2 —— 后端 emit 独立 `subagent:finished` 事件（不扩展 TranscriptKind），前端 listener 收到后
   refetch run + summary，让 drawer 自动转终态。

**Consequences**：
- ✅ 两个症状一次性解决，drawer 数据源（getRunCache）与卡片数据源（runSummaryBySession）保持事件驱动同步。
- ✅ 不改 drawer 组件、不改 ToolCallCard、不改 `getSummaryByToolUseId` 后缀匹配。
- ⚠️ 新增一个 Tauri 事件 + listener 分支；`subagent:finished` 的 payload schema 需在前端 type 定义里固化。
- ⚠️ 完成事件到达前有一个极小窗口（emit 与 DB update 之间），但 emit 严格在 `update_run_finished`
  之后，窗口可忽略，不做防御性 refetch（见 Out of Scope）。

## Technical Notes

- `chat_loop.rs` sink 构造（2142）在 `insert_run`（2090）之后，可直接把 `worker_run_id_opt` clone 进 sink
- `SubagentBufferSink.run_id` 字段当前唯一用途是 `record()` 里 `build_subagent_event_payload` 的 runId（已确认无其他用途）
- insert 失败（`worker_run_id_opt = None`）是极端罕见情况，此时无 DB 行 / 无 summary，drawer 打不开，event runId 用 worker_rid 兜底无副作用
- 前端 listener 已在 `ChatWindow.vue:55` `subagentRuns.start()` 注册，复用同一通道即可
