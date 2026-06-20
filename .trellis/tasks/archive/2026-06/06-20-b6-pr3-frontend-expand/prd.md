# B6 PR3: 前端 drawer 实时 stream + subagentRuns store (含 PR2 改动)

## Goal

主对话保持紧凑,worker subagent 详情通过**右边 drawer**(reka-ui Sheet)实时 stream 展示:用户点 `dispatch_subagent` 工具卡 → drawer 滑入 → 实时显示 worker 的 transcript(ChatEvent / ToolCall / ToolResult / PermissionAsk),worker 跑期间事件 debounce 200ms batch 更新,worker 完成后展示最终 summary + transcript。

附带:**PR2 hotfix**(`SubagentBufferSink` 不再沉默,新增 `subagent:event` IPC channel)+ **顺手修 RULE-A-016** 关闭 DEBT。

## What I already know(已读源码 + spec)

### 现有前端 store 模式(`.trellis/spec/frontend/state-management.md`)
- **`chat.ts` + `streamController.ts`**:facade + singleton 模式(streamController 单源)
- **`audit.ts`**(C4 PR1):reactive audit 状态 + IPC 监听,参考
- **`permissions.ts`**:IPC 事件(`permission:ask`)+ IPC invoke + 120s timer + `<PermissionModal>`
- **`checklist.ts`(B12)**:per-session handle + 派生 store + 跨层 coerce mirror
- **reka-ui 依赖**:项目已用,PermissionModal 经验(reka-ui DialogContent 用 `:deep()` gotcha)

### 现有 ToolCallCard / MessageItem
- `app/src/components/chat/ToolCallCard.vue`(PR1 注册 dispatch_subagent,前端无特殊渲染)
- `MessageItem.vue` 有 `VIRTUAL_TOOLS = new Set(["update_checklist"])` 折叠模式(B12)
- vitest 用 `vi.mock("@tauri-apps/api/core", ...)` 模拟 invoke

### Tauri command / event 模式
- **command**:`commands/permissions.rs:307-330` `list_session_audit_events`(参考实现)
- **event**:`streamController` 监听 `chat-event` / `tool:call` / `tool:result` / `permission:ask` 4 channel(`app/src-tauri/src/commands/mod.rs:99` 注册 invoke_handler + 各 emit)

### PR2 spec §tool-contract.md Scenario: subagent_runs persistence
- `db::subagent_runs::insert_run` / `update_run_finished` / `get_run` / `list_runs_by_session` 5 API
- `SubagentRunRow` 11 字段 + `SubagentStatusDb` 4 状态
- `SubagentBufferSink::transcript_snapshot()` 返回 `Vec<TranscriptEntry>`
- `TranscriptKind`:ChatEvent / ToolCall / ToolResult / PermissionAsk
- `transcript_truncated` flag(4 MiB cap)
- PR2 当前实现:`SubagentBufferSink` **沉默**(不 emit 父 frontend chat-event channel),worker 完成时一次性 `update_run_finished` 落 transcript

### 前端 IPC wire shape
- Rust `#[serde(rename_all = "camelCase")]` → TS `subagentName` / `parentSessionId` / `tokenUsageJson` 等

## Decisions(全部收敛,5 旧 + 3 新)

| # | 决策点 | 结论 |
|---|---|---|
| 1 | IPC 形态(commands) | **双 command**:`list_subagent_runs_by_session(session_id) → Vec<SubagentRunSummary>`(不含 transcript_json)+ `get_subagent_run(run_id) → Option<SubagentRunRow>`(含 transcript_json)。需要 `SubagentRunSummary` 新类型投影 9 字段 |
| 2 | transcript 展示 | **分类 default hide ChatEvent delta**(drawer 内):ToolCall + ToolResult + PermissionAsk 默认显示;ChatEvent toggle 默认 hidden;summary 已存 worker final text |
| 3 | store 集成 | **独立 `subagentRuns.ts` Pinia store**:reactive `runSummaryBySession` Map + `getRunCache` Map + `liveTranscript` Map(新增,Q7 实时流) + IPC invoke list/get_run + IPC listener `subagent:event`(Q7) |
| 4 | 前端渲染 cap | **不额外 cap**:Q2 default hide + Q8 debounce 已足够 |
| 5 | RULE-A-016 修复归属 | **PR3 顺手修,关闭 DEBT**:ask_path worker 分支改走 transcript PermissionAsk,~5 行 + 1 测试更新 |
| **6** | **drawer 组件** | **reka-ui Sheet**(项目已有 reka-ui,PermissionModal 经验),与状态/store 解耦(状态装 store,DOM 装 component) |
| **7** | **stream channel** | **新增独立 IPC channel `subagent:event`** payload `{ runId, sessionId, kind: TranscriptKind, payload, timestamp }`,**PR2 hotfix**:`SubagentBufferSink::emit_*` 加 `app_handle.emit("subagent:event", payload)` |
| **8** | **背压节流** | **前端 listener 200ms debounce batch commit** 到 `liveTranscript` reactive Map(自写,不依赖 lodash) |

## Requirements

### PR2 改动(必须先于 PR3)

- [R0] **PR2 hotfix**:`SubagentBufferSink` 不沉默 —— `emit_chat_event` / `emit_tool_call` / `emit_tool_result` / `emit_permission_ask` 4 个方法加 `app_handle.emit("subagent:event", payload)`,payload 含 `runId` / `sessionId` / `kind` / `payload` / `timestamp`。`SubagentBufferSink` 需要拿到 `app_handle: AppHandle`(构造时注入,从 `run_subagent` 传 worker_sink 时连带)
  - 影响:`run_subagent` 签名增 `app_handle: AppHandle` 参数,从 `chat_loop.rs` run_chat_loop 闭包捕获;`SubagentBufferSink::new()` 接受 app_handle
  - PR2 新增 Rust 测试:`SubagentBufferSink` 在 emit 时同时写 transcript + emit IPC event(`EventCollector` mock sink 类似 audit 测试模式)

### 后端(PR3a)

- [R1] `SubagentRunSummary` 新类型(`db::subagent_runs` 模块):投影 9 字段,排除 transcript_json + transcript_truncated
- [R2] `db::subagent_runs::list_runs_summary_by_session(db, session_id) -> Vec<SubagentRunSummary>` 函数
- [R3] 2 个 Tauri commands:
  - `list_subagent_runs_by_session(session_id) -> Vec<SubagentRunSummary>`
  - `get_subagent_run(run_id) -> Option<SubagentRunRow>`
  - 注册到 `commands/mod.rs` invoke_handler
- [R4] **RULE-A-016 顺手修**:`permissions::ask_path` 顶部 record_audit 加 `if !ctx.is_worker { session_audit_events } else { transcript PermissionAsk }`;`audit_not_polluted_by_worker` 测试断言仍 `delta == 2`(worker 改走 transcript);DEBT RULE-A-016 close + §优先级分布 P2 22→21 / Total 47→46

### 前端(PR3b)

- [R5] `app/src/stores/subagentRuns.ts` Pinia store:
  - reactive `runSummaryBySession: Map<string, SubagentRunSummary[]>`(list cache)
  - reactive `getRunCache: Map<string, SubagentRunRow>`(get_run cache)
  - reactive `liveTranscript: Map<string, TranscriptEntry[]>`(Q7 实时流,drawer 显示来源)
  - reactive `openRunId: string | null`(drawer 当前打开的 worker run_id)
  - API:
    - `fetchForSession(sessionId)` → invoke `list_subagent_runs_by_session`
    - `fetchRun(runId)` → invoke `get_subagent_run`,写 `getRunCache` + 覆盖 `liveTranscript`
    - `openDrawer(runId)` → 设 `openRunId`,fetchRun(若未 cache)
    - `closeDrawer()` → 清 `openRunId`
    - `getSummaryByToolUseId(sessionId, toolUseId)` → 在 list cache 找对应 worker
  - **IPC listener**:`listen<SubagentEventPayload>("subagent:event", ...)`(`@tauri-apps/api/event`)+ 200ms debounce batch commit 到对应 runId 的 `liveTranscript`(自写,setTimeout-based,不依赖 lodash)
  - listener 生命周期:`start()` 在 `ChatWindow.vue` `onMounted`(类似 permissions.ts);`stop()` 在 `onUnmounted`
- [R6] `app/src/components/chat/SubagentDrawer.vue`(新组件)用 reka-ui `Sheet`:
  - open state 绑 `store.openRunId`(computed `open = computed(() => store.openRunId !== null)`)
  - 顶部:status badge + subagent_name + started_at + finished_at(if any)+ summary(if any)
  - 中部:transcript 列表(`store.liveTranscript.get(openRunId) ?? store.getRunCache.get(openRunId)?.transcript ?? []`)
  - 每 entry:kind badge(色码:ChatEvent=灰/ToolCall=蓝/ToolResult=绿/PermissionAsk=橙)+ payload 格式化(`JSON.stringify(payload, null, 2)`)+ 时间戳
  - ChatEvent 默认隐藏 toggle:checkbox 顶部 "Show chat events" toggle
  - `transcript_truncated` flag 显示 "原 transcript 已截断(head + tail)" 提示
  - empty state:worker 还没完成 + 没实时 event → "Worker is starting..."
- [R7] `app/src/components/chat/ToolCallCard.vue` dispatch_subagent 特殊分支:
  - 折叠态:status badge + subagent_name + summary(短,200 字)
  - **不展开** —— 点击整个 card 触发 `subagentRuns.openDrawer(parent_run_id)`,不内嵌 transcript(改用 drawer)
  - parent_run_id 关联:dispatch_subagent tool_use 的 `id` 字段 → `subagentRuns.getSummaryByToolUseId(sessionId, toolUseId)` → summary 的 `parentRequestId` → drawer open
- [R8] 前端 vitest:
  - `subagentRuns.test.ts` 覆盖 store API + listener + debounce(用 `vi.useFakeTimers()`)
  - `SubagentDrawer.test.ts` 覆盖 drawer 渲染(liveTranscript + cache fallback + kind badge + truncated 提示)
  - `ToolCallCard.test.ts` 加 dispatch_subagent 点击触发 drawer 调用 store.openDrawer 的测试
- [R9] spec 沉淀:`state-management.md` 加 "subagentRuns store" 段(对标 checklist.ts 段结构,描述 reactive state + API + IPC 契约 + `TranscriptKind` default hide + 跨层 TranscriptEntry shape mirror + debounce 200ms 约定 + drawer UX 模式)

## Acceptance Criteria

- [AC1] 后端 `cargo test --lib` 现有 726 tests 全 pass(0 新 warning)+ PR2 hotfix 新增 SubagentBufferSink emit 路径测试 + PR3a 新增 2 commands 测试
- [AC2] **PR2 hotfix 验证**:`SubagentBufferSink::emit_chat_event` 等 4 方法测试用 EventCollector mock,断言 emit 时既写 transcript 又 emit `subagent:event` channel(payload 含 runId/sessionId/kind/payload/timestamp)
- [AC3] **RULE-A-016 端到端**:worker general-purpose + Edit + write_file → audit `delta == 2`,worker tool_denied 改走 transcript `PermissionAsk`(transcript list 可见)
- [AC4] 前端 `pnpm vitest run` 全 pass(含 `subagentRuns.test.ts` + `SubagentDrawer.test.ts` + `ToolCallCard.test.ts` dispatch_subagent)
- [AC5] 手动 verify:
  1. 主对话派 researcher/general-purpose worker → ToolCallCard 显示 status + summary
  2. 点击 ToolCallCard → drawer 滑入右侧
  3. drawer 顶部 status + subagent_name + summary
  4. drawer 中部 transcript 列表(worker 跑期间:实时 stream;完成后:完整)
  5. ToolCall/Result/PermissionAsk 默认可见,ChatEvent toggle 可见
  6. `transcript_truncated` flag 提示(若触发)
- [AC6] spec `state-management.md` 加 subagentRuns store 段 + drawer UX 模式段

## Definition of Done

- PR2 hotfix:`SubagentBufferSink` 加 `app_handle` 依赖 + 4 emit 方法加 `app_handle.emit("subagent:event", payload)`;`run_subagent` 签名增 app_handle 参数;Rust 测试覆盖 emit 路径
- 后端:SubagentRunSummary 类型 + list_runs_summary_by_session + 2 Tauri commands + commands/mod.rs 注册
- 后端:RULE-A-016 顺手修复
- 前端:`subagentRuns.ts` store + IPC listener + debounce + `SubagentDrawer.vue` + `ToolCallCard.vue` 特殊分支(改 drawer 而非展开)
- 前端 vitest + 后端 cargo test 全 pass
- spec 沉淀:`state-management.md` 新增 subagentRuns store 段 + drawer UX 模式段
- DEBT.md:`RULE-A-016` closed + §优先级分布 P2 22→21 / Total 47→46
- git commit:feat(PR2 hotfix) + feat(PR3a + PR3b) + docs(spec) 3 commits

## Out of Scope

- 异步 fan-out `dispatch_subagents` plural
- worker 嵌套(worker 派 worker)
- Markdown frontmatter subagent 定义加载
- worker 独立 model
- transcript 大小改 PR2 决策(4MB 不变)
- audit 表 schema 改
- drawer 内 markdown 渲染(暂时 plaintext + JSON.stringify)
- drawer 嵌套(同时开多个 worker drawer,只支持单 drawer,打开 B 关闭 A)
- drawer 持久化状态(刷新页面后 drawer 关闭)

## Technical Notes

- **架构改动核心**:PR2 hotfix 让 worker 事件从沉默改为主动 emit 父 frontend `subagent:event` channel。drawer 通过 store listener 实时接收。前端 main chat UI 不受影响(chat-event channel 不变,worker 事件走独立 channel)。
- **SubagentBufferSink 需要 app_handle**:从 `chat_loop.rs` run_chat_loop 闭包捕获,传 `run_subagent(deps, ..., app_handle)`,run_subagent 构造 `SubagentBufferSink::new(app_handle, run_id, session_id)`。Sink emit 时同时 append transcript + emit IPC。
- **drawer UX 与 ToolCallCard 解耦**:ToolCallCard 仅触发 `openDrawer(runId)`,drawer 内部从 store 拉数据。store 维护 drawer 状态(openRunId)。Drawer unmount on close(节省 DOM)。
- **debounce 实现**:自写 setTimeout-based,避免 lodash 依赖。`liveTranscriptBuffer: Map<runId, TranscriptEntry[]>` listener 写入,200ms timer flush 到 reactive `liveTranscript`(清 buffer + 设置 reactive)。`vi.useFakeTimers()` 测试可控。
- **`.ts` mirror `TranscriptKind`**:TS 端 enum 字符串字面量必须与 Rust `#[serde(rename_all = "snake_case")]` 一致(ChatEvent / ToolCall / ToolResult / PermissionAsk)。跨层 drift 是 bug,trellis-check 验证。
- **audit 不污染父(RULE-A-016 修复)**:worker ⑨ 决策 → transcript PermissionAsk;父 audit 表无 worker 行(原有测试断言 delta == 2 仍成立)。
- **`SubagentRunSummary` 与 `SubagentRunRow` 区分**:list 端返 Summary(无 transcript_json),drawer 展开时 fetchRun 端返 Row(含 transcript_json)。两者 FromRow 派生,Row 加 `Serialize` derive 即可,Summary 加 `#[serde(skip)]` 字段或独立 type。
- **drawer 与 popover-pattern.md 区别**:drawer 是 click-triggered + persistent + side-panel(reka-ui Sheet),popover 是 hover-triggered + ephemeral + anchored(自写 Teleport)。两者不冲突,各管各的状态。
- **PR2 hotfix 不破坏 PR2 已修复的不变量**:audit 不污染父(RULE-A-015 closed + RULE-A-016 修复中)+ token_usage streaming + transcript 4 MiB cap + status 字段独立 + CASCADE。emit 不影响 transcript 累积,仅多一个 emit 边。

## Implementation Plan (small chunks)

### PR2 hotfix(`SubagentBufferSink` 不沉默)
- `app/src-tauri/src/agent/subagent.rs`:
  - `SubagentBufferSink` struct 加 `app_handle: AppHandle` + `run_id: String` + `session_id: String` 字段
  - `new(app_handle, run_id, session_id)` 构造
  - 4 emit 方法加 `let payload = serde_json::json!({...}); let _ = self.app_handle.emit("subagent:event", payload);`(append transcript + emit IPC 双写)
- `app/src-tauri/src/agent/chat_loop.rs`:
  - `run_subagent` 签名增 `app_handle: AppHandle` 参数(从 run_chat_loop 闭包捕获,AppHandle 是 Arc 共享)
  - 构造 `SubagentBufferSink::new(app_handle, worker_rid, parent_session_id)`
- `app/src-tauri/src/agent/chat.rs`:production call site 传 `app: AppHandle`(已可用)
- 新增 Rust 测试:`SubagentBufferSink` emit 路径(EventCollector mock 验证 transcript append + IPC emit,EventCollector 维护一个 `app_handle` mock 收集 emit 调用)

### PR3a — 后端(commands + RULE-A-016 顺手修)
- `db/subagent_runs.rs` 加 `SubagentRunSummary` 类型(投影 9 字段,sqlx::FromRow)
- `db/subagent_runs.rs` 加 `list_runs_summary_by_session` 函数
- `commands/subagent_runs.rs`(新文件)加 2 commands:`list_subagent_runs_by_session` + `get_subagent_run`
- `commands/mod.rs` 注册新 commands
- `commands/mod.rs` 可能需要 emit `subagent:event` channel 在 `emit` 注册?(实际上 `app_handle.emit` 直接可用,无需注册)
- **顺手修 RULE-A-016**:`agent/permissions/mod.rs` ask_path 顶部 record_audit 加 `if !ctx.is_worker` 分支;+ 1 测试更新 `audit_not_polluted_by_worker` 断言仍 `delta == 2`
- `db/tests.rs` 加 `list_runs_summary_by_session` 测试
- DEBT.md `RULE-A-016` close + §优先级分布 表更新

### PR3b — 前端 + spec
- `app/src/stores/subagentRuns.ts` 新 Pinia store:
  - reactive `runSummaryBySession` + `getRunCache` + `liveTranscript` + `liveTranscriptBuffer`(debounce 中转)+ `openRunId`
  - `fetchForSession` / `fetchRun` / `openDrawer` / `closeDrawer` / `getSummaryByToolUseId`
  - IPC listener `subagent:event` + 200ms debounce self-implemented
- `app/src/components/chat/SubagentDrawer.vue` 新组件 reka-ui `Sheet`:
  - open 绑 `store.openRunId !== null`
  - 顶部 status badge + summary
  - 中部 transcript 列表(kind badge + payload JSON.stringify + timestamp)
  - ChatEvent toggle + truncated 提示
- `app/src/components/chat/ToolCallCard.vue` 改 dispatch_subagent 分支:点击触发 `store.openDrawer(runId)`,不展开
- vitest:`subagentRuns.test.ts` + `SubagentDrawer.test.ts` + `ToolCallCard.test.ts` 增量
- spec `.trellis/spec/frontend/state-management.md` 加 "subagentRuns store" 段 + drawer UX 模式段