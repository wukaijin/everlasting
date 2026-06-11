# F5 LLM 耗时统计

> **来源**: 体验优化批次 F1-F4 (`06-11-session-loading`) 当时延后,本任务独立实施。
> **目标日期**: 2026-06-11

## Goal

让用户看到每个 LLM turn 的耗时结构(TTFB / 生成 / 端到端)和每个 tool call 的单独耗时,session 累计也显示。所有耗时持久化到 DB,切换/重启可查。第一版目标"看到自己 LLM 调用慢在哪",不做 P50/P95 统计、不做历史趋势图、不做导出。

## What I already know

### 复用的近邻实现

- **A4 token usage 模式(A4 完成于 2026-06-10)**:前端 `accumulateTokenUsage` + 后端 `db::sessions::add_token_usage` 单条 UPDATE。F5 走同样模式,新建 `accumulateLatency` / `record_tool_duration`。
- **`add_messages_column_if_missing` helper**(migrations.rs:421-435):F5 ALTER 3 列零 friction。Tool duration 走"嵌进 content JSON"路线,无 schema 改动。
- **`add_sessions_column_if_missing`** (migrations.rs:387-396):如果要在 sessions 表加累计字段(session 总耗时),用这个。

### Hook 点(已读代码确认)

- `streamController.ts:474-489` — `delta` / `done` 事件,TTFB / gen / total 计时落点
- `streamController.ts:621-625` — `tool:call` / `tool:result` 监听,per-tool 计时落点
- `streamController.ts:686-694` — A4 的 rehydrate seed 模式,F5 同样接入
- `ToolCallCard.vue:433` — statusText 旁加耗时
- `MessageList.vue` / `ChatPanel.vue` — message 角标 + session 底部 footer

### 已有 reference spec

- `archive/2026-06/06-11-session-loading/prd.md` §F5(51-70, 97-103)—— 原始方案
- `archive/2026-06/06-10-a4-token-per-session-chatinput-hint/prd.md`—— 跨层改动格式参考
- `.trellis/spec/backend/llm-contract.md`—— A4 已沉淀"Scenario: Token Usage Tracking"段,F5 类似

## Requirements

### R1 — 前端计时(TTFB / gen / total)

- 发送消息时记录 `sendAt = Date.now()`,存在 activeRequest 上
- 收到首条 `delta` 事件时记录 `firstDeltaAt`,计算 `ttfb = firstDeltaAt - sendAt`
- 收到 `done` 事件时记录 `doneAt`,计算 `gen = doneAt - firstDeltaAt`、`total = doneAt - sendAt`
- 三个耗时挂在 in-memory assistant message 上:`{ ttfbMs, genMs, totalMs }`
- 用户取消 / error 路径:`total` 算到 done 时刻但 `gen` / `ttfb` 仍记录(用户能看到"在 X 秒时断了")

### R2 — Per-tool-call 计时(嵌进 tool_result content JSON)

- `tool:call` 事件触发时:在 toolUse 对象上记 `startedAt`
- `tool:result` 事件触发时:计算 `duration = Date.now() - startedAt`,写到 in-memory toolResult 对象的 `durationMs` 字段
- 新增 IPC `record_tool_duration(session_id, tool_use_id, duration_ms)`,后端用 `serde_json::Value::pointer_mut` patch `messages.content` JSON 里对应 tool_result block 的 `durationMs` 字段
- 关键:tool:result 一定在该 turn 下一轮 delta 之前(agent loop 串行),所以 patch 在 `persist_turn` 之前完成,rehydrate 路径自然恢复

### R3 — DB schema

- `add_messages_column_if_missing(pool, "ttfb_ms", "INTEGER")`
- `add_messages_column_if_missing(pool, "gen_ms", "INTEGER")`
- `add_messages_column_if_missing(pool, "total_ms", "INTEGER")`
- 3 列 nullable(旧 assistant 消息为 NULL)
- tool duration 字段走 R2 嵌进 content JSON,**0 schema 改动**

### R4 — UI: Assistant 消息耗时显示

- 每条 assistant 消息右下角显示该轮总耗时(粗体小字,如 "3.2s")
- Hover 弹 tooltip(reka-ui `Tooltip`):三行明细
  - `TTFB: 0.4s`
  - `生成: 2.1s`
  - `端到端: 3.2s`
- NULL 状态(cancel / 旧数据):显示 "—"

### R5 — UI: ToolCallCard 耗时

- statusText(`running…` / `done` / `error`)右侧加耗时
- running 时显示 "…",done 时显示实际 duration(如 "0.3s")
- 嵌进 content 的 durationMs 通过 rehydrate 恢复

### R6 — UI: Session 底部累计

- ChatPanel 底部 footer 区(A4 token usage 旁或下方)加"本次 session LLM 累计耗时"
- 口径:`SUM(total_ms) WHERE session_id = ? AND role = 'assistant' AND total_ms IS NOT NULL`
- 切换 session 时从 DB 读累计值,新 turn done 时刷新

### R7 — Pinia store

- 新建 `useLatencyStore`(或合并到 `useChatStore`):管理 `currentSessionLatency` map(`sessionId → { ttfbMs, genMs, totalMs }`)和 `currentSessionTotalMs` 累计
- SSE `chat-event done` 监听:更新 map 和累计
- `ensureLoaded` 路径:从 DB 读 session 的累计值 seed 到 store(对齐 A4 rehydrate 模式)

### R8 — 跨层契约 spec

- `.trellis/spec/backend/llm-contract.md` 新增 "Scenario: Latency Tracking" 段
- 包含:计时口径、3 列 nullable 语义、tool duration 嵌 content JSON 模式、rehydrate 路径、cancel/error 边界

## Acceptance Criteria

- [ ] Anthropic stream:首条 delta 到达,前端能读到 `ttfbMs` 数字
- [ ] Anthropic stream:done 事件触发后,前端能读到 `genMs` 和 `totalMs`
- [ ] OpenAI stream 同上
- [ ] Cancel 场景:assistant message 仍显示 `totalMs`,ttfb/gen 可能为 NULL(中断时刻没收到 first delta)
- [ ] Error 场景:同上,UI 显示 "—" 但 total 仍记录
- [ ] DB 写入验证:`messages.ttfb_ms` / `gen_ms` / `total_ms` 在 stream done 后 UPDATE 成功
- [ ] ToolCallCard 显示 `durationMs`(`tool:result` 事件后)
- [ ] tool duration 嵌进 `messages.content` JSON 的 tool_result block,rehydrate 路径恢复
- [ ] Session 底部累计 = `SUM(total_ms) WHERE session_id = ? AND total_ms IS NOT NULL`
- [ ] 切换 session 再切回:累计值仍正确(从 DB 读)
- [ ] 重启 app:历史 session 的耗时数据可查看
- [ ] Hover 耗时 tooltip 显示三行明细(reka-ui Tooltip)
- [ ] 旧 session(无 3 列):UI 显 "—",不报错

## Definition of Done

- Tests added/updated:前端计时 hook 单测、后端 record_tool_duration 单测(json_patch 正确性)、rehydrate 路径恢复 tool duration 单测
- Lint / typecheck / CI green:vue-tsc --noEmit + pnpm build + cargo test 全过
- Docs/notes updated:docs/IMPLEMENTATION.md §4 追加"F5 耗时统计"决策条目
- Spec 沉淀:.trellis/spec/backend/llm-contract.md 新增 Scenario 段
- Rollout/rollback considered:3 列 nullable,无破坏性;tool duration 嵌 JSON,旧 session 读 NULL,前端 "—"

## Decision (ADR-lite)

### 决策 1:Tool duration 嵌进 tool_result content JSON(不新建表)

**Context**: 原 F5 spec 假设 `tool_results` 表存在加 `duration_ms` 列,实际表结构是 tool_result 嵌在 `messages.content` JSON 里。

**Decision**: tool_use_id 维度的 `durationMs` 字段写在 `messages.content` JSON 数组中对应 tool_result block 上,后端用 `serde_json::Value::pointer_mut` 走 IPC `record_tool_duration` 写盘。

**Consequences**:
- 零 schema 改动,messages 表 ALTER 只为 R3 的 3 列 ttfb/gen/total
- rehydrate 路径零修改(自然从 content JSON 读)
- trade-off:content JSON 多一个字段;需新增 1 个 IPC + 1 个后端 patch 函数

### 决策 2:前端 `Date.now()` 计时(后端不重复计时)

**Context**: A4 token usage 也是前端计算,后端只持久化;`test_provider` 有 `latencyMs` 但那是单次 HTTP 测试。

**Decision**: F5 全部计时在前端,后端不做 wall-clock 测量。R1 三项 + R2 tool duration 全部 `Date.now()` 差值。

**Consequences**:
- 一致性:跟 A4 模式同源,前端是 single source of truth
- 简洁:不引入 `Instant::now()` / `SystemTime` 后端测量
- 已知限制:前端时钟被改时(用户改系统时间)数字会失真,跟 A4 同 trade-off,可接受

## Out of Scope

- ❌ P50 / P95 百分位统计
- ❌ 历史趋势图 / session 间对比
- ❌ 导出(CSV / JSON)
- ❌ 后端精确计时(后端 `Instant::now()` 模式)
- ❌ Token 速率(tokens/秒)统计
- ❌ 按 model / provider 分组耗时
- ❌ 跨 session 全局累计
- ❌ SessionList 每行加耗时

## Technical Notes

### 关键文件

- `app/src/stores/streamController.ts:474-489` — R1 计时 hook
- `app/src/stores/streamController.ts:621-625` — R2 tool 计时 hook
- `app/src/stores/chat.ts` 或新建 `app/src/stores/latency.ts` — R7 Pinia store
- `app/src/components/chat/ToolCallCard.vue` — R5 耗时显示
- `app/src/components/chat/MessageList.vue` / `MessageItem.vue` — R4 assistant 角标
- `app/src/components/chat/ChatPanel.vue` — R6 session 底部 footer
- `app/src-tauri/src/db/migrations.rs:421-435` — R3 ALTER
- `app/src-tauri/src/db/sessions.rs` — R2 `record_tool_duration` 函数(或新文件 `db/messages.rs`)
- `app/src-tauri/src/commands/sessions.rs`(或 `commands/latency.rs`) — R2 IPC
- `.trellis/spec/backend/llm-contract.md` — R8 spec 沉淀

### 复用 reference

- `add_messages_column_if_missing` (migrations.rs:421-435) — R3 helper
- `add_token_usage` (db/sessions.rs) — R7 store 模式
- `accumulateTokenUsage` (chat.ts / streamController.ts:686) — R1 rehydrate 模式
- reka-ui `Tooltip` (已在 app/package.json) — R4 / R5 hover

### 实施顺序:1 PR 全合

R1-R8 互相耦合(ALTER 3 列 → record_tool_duration IPC → persist_turn hook → 前端 timing hook → store → UI → rehydrate),任一环节缺失,中间态都不能跑端到端测试。A4 token usage 已经验证"1 PR 全合"模式可行(8-12 文件 diff,commit message 一次说清)。**本任务 1 PR 全合。**

## Research References

(本任务未走 research-first,F5 范围明确,直接进入设计收敛。)
