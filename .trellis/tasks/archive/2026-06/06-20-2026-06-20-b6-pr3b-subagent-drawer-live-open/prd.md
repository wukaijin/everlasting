# B6 PR3b — subagent drawer 实时打开 + 可视化 polish

## Goal

修复 `dispatch_subagent` 卡片在 worker SSE 持续期间无法打开 detail drawer 的 race condition，并把 drawer 的头部 / 滚动 / waiting 反馈补到 production-ready。同时把"raw JSON payload" 这块**保留**（独立 PR 处理 typed-cards 重构）。

## What I already know

### 现状（已读 5 个文件，root cause 锁定）

| 文件 | 关键事实 |
|---|---|
| `app/src/components/chat/ToolCallCard.vue:350-354` | `openSubagentDrawer()` 在 `workerSummary` 为 undefined 时**静默返回**，UI 零反馈 |
| `app/src/components/chat/ToolCallCard.vue:300-304` | `workerSummary` 从 `subagentRuns.getSummaryByToolUseId(sid, call.id)` 取 |
| `app/src/stores/subagentRuns.ts:301-309` | `getSummaryByToolUseId` 匹配 `parentRequestId.endsWith("-sub-" + toolUseId)` |
| `app/src/components/chat/ToolCallCard.vue:363-372` | `watch(isDispatchSubagent, immediate: true)` 只在 mount 时 fire-and-forget 触发 `fetchForSession` |
| `app/src-tauri/src/agent/chat_loop.rs:2081-2108` | **DB row 已经在 worker 启动前**通过 `insert_run` 写入（`parent_request_id = "{parent_rid}-sub-{tool_use_id}"`） |
| `app/src-tauri/src/agent/subagent.rs:600-643` | `SubagentBufferSink::record()` 每个 emit 都同步 emit `subagent:event` IPC，**live stream 没问题** |
| `app/src-tauri/src/agent/chat_loop.rs:2074` | worker rid 格式 `{parent_rid}-sub-{tool_use_id}`，前端 lookup 可对齐 |

### Root cause（race window）

```
t0  父 LLM 发 dispatch_subagent tool_use
t1  父 loop emit `tool:call` → 前端 render ToolCallCard → watch fires fetchForSession (fire-and-forget)
t2  父 loop 进 run_subagent
t3  run_subagent insert_run(...) → 写 subagent_runs row
t4  父 loop 启动 worker → SubagentBufferSink emit subagent:event IPC
```

`fetchForSession` IPC 和 `insert_run` 之间存在 race：
- 如果 t1 触发的 `list_subagent_runs_by_session` 在 t3 之前到达后端，返回 list 为空
- 前端缓存空 list，整段 SSE 期间不再 re-fetch（watch 只 fire 一次）
- 用户任何时候点击 → `workerSummary` undefined → silent no-op
- 直到 SSE 结束（user 切走再回来 / 后端 update_run_finished）才能命中

### 已有测试覆盖

`app/src/stores/subagentRuns.test.ts` 已覆盖：
- `fetchForSession` / `fetchRun` / `openDrawer` / `closeDrawer` 行为
- `getSummaryByToolUseId` 三种 case（命中 / 不命中 / session 未缓存）
- IPC listener 生命周期 + 200ms debounce

`app/src/components/chat/ToolCallCard.test.ts:185-` 已有 dispatch_subagent 卡片测试。
`app/src/components/chat/SubagentDrawer.test.ts` 覆盖 drawer 渲染。

新增 fix 需要补的测试：
- `subagentRuns.test.ts`：store 收到 `subagent:event`（含未知 runId）时自动调 `fetchRun` + `fetchForSession`；去重（已缓存不重复 fetch）
- `ToolCallCard.test.ts`：click 时 cache 未命中 → waiting 状态显示 → cache 命中后正常 open drawer
- `SubagentDrawer.test.ts`：header live duration timer；新 entry 出现时 auto-scroll 行为

## Requirements

### Functional

1. **R1**：dispatch_subagent 卡片在 SSE 持续期间即可点击 → drawer 打开看到 live transcript
2. **R2**：drawer 头部显示 `running 8.2s` live timer（从 `startedAt` 起算，每 100ms 更新）
3. **R3**：drawer 内新 entry 进入时 auto-scroll to bottom；用户往上滚后检测 → 暂停跟随 + 显示 `↓ N new events` 按钮
4. **R4**：用户点击 drawer 顶部 `↗ jump to latest` 按钮 → 滚到底部并恢复自动跟随
5. **R5**：ToolCallCard 在 cache 还没命中但 click 已触发时显示 `等待 worker 注册…` 状态（不是 silent no-op）
6. **R6**：ToolCallCard 已存在但 card mount 时 IPC race 输了 → 后续首个 `subagent:event` 到达时自动 warm cache（无需用户重新 click）

### Explicit non-goals

- **ToolCallCard 自身不加 live duration** — 静态 `running…` 保留（用户已确认 2026-06-20，避免主 chat 流噪音）

### Non-functional

- **N1**：纯前端改动，不动后端 wire shape / IPC schema
- **N2**：不破坏现有 `subagentRuns.test.ts` / `SubagentDrawer.test.ts` / `ToolCallCard.test.ts` 测试
- **N3**：store 收到重复 `subagent:event`（同一 runId）只 fetch 一次（已有 `getRunCache.has(runId)` 守卫可复用）
- **N4**：drawer live timer `setInterval` 在 drawer close 时清理，避免内存泄漏
- **N5**：auto-scroll 不与用户手动滚动冲突（鼠标滚轮 / 触摸滚动能被检测）

## Acceptance Criteria

- [ ] AC1：在 SSE 进行中点击 dispatch_subagent 卡片，drawer 在 200ms 内打开并显示至少 1 条已收到的 event
- [ ] AC2：drawer 头部 live timer 在 running 状态时每秒递增；terminal 状态切换为 `done in X.Xs` 或 `failed at X.Xs`
- [ ] AC3：drawer 底部出现新 entry 时自动滚到底；用户向上滚 50px 后 auto-follow 暂停，顶部出现 `↓ N new` 提示
- [ ] AC4：点击 `jump to latest` 按钮 → 滚到底 + auto-follow 恢复 + 提示消失
- [ ] AC5：ToolCallCard 在 cache miss 时 click 触发 `等待 worker 注册…` 视觉态，500ms 内若 cache 命中则静默打开 drawer
- [ ] AC6：`subagentRuns.test.ts` 新增至少 2 个测试覆盖 subagent:event 触发 fetchRun/fetchForSession 的路径
- [ ] AC7：`ToolCallCard.test.ts` 新增 1 个测试覆盖 cache miss → waiting → 命中 后 openDrawer 的时序
- [ ] AC8：`pnpm vitest run` 全部通过
- [ ] AC9：`pnpm vue-tsc --noEmit` 0 error
- [ ] AC10：dev server (`pnpm tauri dev`) 实跑一个 general-purpose subagent 任务，肉眼验证上述 AC1-5

## Definition of Done

- 代码改动限定在 `app/src/stores/subagentRuns.ts`、`app/src/components/chat/ToolCallCard.vue`、`app/src/components/chat/SubagentDrawer.vue` 三个文件（或其衍生子组件）
- 单元测试新增覆盖（如 AC6/AC7）
- vitest + vue-tsc 双绿
- dev server 实跑至少一次 manual verify（dispatch_subagent + 观察 drawer 实时刷新）
- 任务按 trellis finish-work 流程：fix → DEBT.md 回填 → archive → journal

## Out of Scope (explicit)

- **drawer payload 可视化重做**（raw JSON → typed ToolCallCard/ToolResultCard/PermissionCard/MessageItem 组件复用）— **独立 PR**，需要先讨论 chat 主面板卡片 props 是否能下沉成 shared interface（**用户已确认 2026-06-20：本次 PR 不做**）
- **新 IPC 事件**（如 `subagent:run:started`）— 当前方案 B 完全靠现有 `subagent:event` 解决 race，避免 wire shape 变更
- **后端改动** — 全部 fix 走前端
- **worker transcript 跨 session 持久化** — 仍是内存 + DB 缓存，刷新即丢，不动
- **drawer 内的 `Show chat events` toggle 行为调整** — 当前实现保留
- **transcriptTruncated banner 改动** — 当前文案保留

## Technical Approach

### Fix path（方案 B：store 订阅 + eager fetch）

`subagentRuns.start()` 里把 `subagent:event` listener 从单一 `routeEvent` 升级为：

```ts
unlisten = await listen<SubagentEventPayload>("subagent:event", (event) => {
  const e = event.payload;
  routeEvent(e);  // 现有的 buffering
  // 新增：见到新 runId 且未缓存 → eager fetch
  if (!getRunCache.has(e.runId)) {
    void fetchRun(e.runId);
    void fetchForSession(e.sessionId);  // 同时让 summary 缓存命中
  }
});
```

- `fetchRun` 自身有 `getRunCache.has(runId)` 守卫，幂等
- `fetchForSession` 替换式缓存，多次调用安全
- 两者都 fire-and-forget，不阻塞 IPC handler
- 现有 200ms debounce + liveTranscript buffer 完全不动

### Card click feedback（方案 A 局部）

`ToolCallCard.openSubagentDrawer` 改造：

```ts
const workerWaiting = ref(false);
async function openSubagentDrawer(): Promise<void> {
  const sid = chatStore.currentSessionId;
  // 1. 先尝试 fetch 一次（也覆盖 cache miss 后首次 click 的场景）
  if (sid && !subagentRuns.runSummaryBySession.has(sid)) {
    workerWaiting.value = true;
    await subagentRuns.fetchForSession(sid);
  }
  const summary = workerSummary.value;
  if (summary) {
    workerWaiting.value = false;
    await subagentRuns.openDrawer(summary.id);
    return;
  }
  // 2. 仍未命中 → waiting 态显示 + 1.5s 内 polling 重试（用户已确认 2026-06-20：1.5s / 300ms / 最多 5 次）
  workerWaiting.value = true;
  const start = Date.now();
  while (Date.now() - start < 1500) {
    await new Promise(r => setTimeout(r, 300));
    if (sid) await subagentRuns.fetchForSession(sid);
    const s = workerSummary.value;
    if (s) {
      workerWaiting.value = false;
      return subagentRuns.openDrawer(s.id);
    }
  }
  workerWaiting.value = false;  // 1.5s 后仍无 → 放弃，UI 提示
}
```

配合 template 加 `workerWaiting ? "等待 worker 注册…" : "点击查看 worker 详情"` 切换 + `cursor: wait` 样式（用户已确认 2026-06-20：复用现有 `tool-card--subagent` 样式，不引入新颜色 / 动画）。

### Drawer live polish

`SubagentDrawer.vue` 新增：

1. **Live duration timer**
   ```ts
   const now = ref(Date.now());
   let ticker: ReturnType<typeof setInterval> | null = null;
   watch(() => store.openRunId, (rid) => {
     if (ticker) clearInterval(ticker);
     if (!rid) return;
     ticker = setInterval(() => { now.value = Date.now(); }, 100);
   }, { immediate: true });
   onUnmounted(() => { if (ticker) clearInterval(ticker); });
   
   const elapsedSeconds = computed(() => {
     if (!run.value?.startedAt) return 0;
     return Math.max(0, Math.floor((now.value - new Date(run.value.startedAt).getTime()) / 1000));
   });
   ```
   头部 status pill 改成：running → `running ${elapsedSeconds}s`，completed → `done in X.Xs`，error → `failed at X.Xs`

2. **Auto-scroll with pause detection**
   ```ts
   const bodyEl = ref<HTMLElement | null>(null);
   const autoFollow = ref(true);
   const newCount = ref(0);
   
   watch(() => visibleTranscript.value.length, () => {
     if (autoFollow.value) {
       nextTick(() => bodyEl.value?.scrollTo({ top: bodyEl.value.scrollHeight }));
     } else {
       newCount.value++;
     }
   });
   
   function onScroll(e: Event) {
     const el = e.target as HTMLElement;
     const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 50;
     autoFollow.value = atBottom;
     if (atBottom) newCount.value = 0;
   }
   
   function jumpToLatest() {
     bodyEl.value?.scrollTo({ top: bodyEl.value.scrollHeight, behavior: "smooth" });
     autoFollow.value = true;
     newCount.value = 0;
   }
   ```

3. **`↗ jump to latest` 按钮在 drawer header 顶部右侧（X 按钮旁边）；`↓ N new events` 浮动在 body 底部右侧，仅 N > 0 时显示**

## Decision (ADR-lite)

**Context**: 用户报告 dispatch_subagent 卡片在 worker SSE 持续期间点击无反应；只有 SSE 结束（done / error）才能看到详情。已诊断 race condition 存在于 `fetchForSession` IPC 与后端 `insert_run` 之间，缓存可能整段 SSE 期间为空。

**Decision**: 采用方案 B — `subagentRuns.start()` 升级 `subagent:event` listener，遇到新 runId 立即 eager-fetch `getRunagent_run` 和 `list_subagent_runs_by_session`。同时在 ToolCallCard 加 waiting 态视觉反馈，drawer 加 live timer + auto-scroll 暂停检测。

**Consequences**:
- ✅ 不动后端 wire shape，零双端测试成本
- ✅ 复用现有 `subagent:event` 通道，0 新增 IPC
- ✅ race 一旦首个 event 到达即消除，user click 命中率 100%
- ✅ 三个小 drawer polish 顺手做完，不留技术债
- ⚠️ 每个 worker run 多 1-2 次 fetch IPC（`fetchRun` + `fetchForSession`），但有 `has(runId)` 守卫去重，可接受
- ⚠️ ToolCallCard 增加一个 `workerWaiting` ref 和一个 polling 循环（最多 1.5s），增加少量复杂度

## Technical Notes

### Files to modify

- `app/src/stores/subagentRuns.ts` — `start()` listener 升级；可能新增一个 `runSummariesByRunId` 旁路 Map（避免 card lookup 每次都 scan session list）
- `app/src/components/chat/ToolCallCard.vue` — `openSubagentDrawer` 重写 + `workerWaiting` 状态 + template 加 waiting 视觉
- `app/src/components/chat/SubagentDrawer.vue` — live timer + auto-scroll + jump-to-latest
- `app/src/stores/subagentRuns.test.ts` — 新增 store-side 测试
- `app/src/components/chat/ToolCallCard.test.ts` — 新增 waiting 时序测试
- `app/src/components/chat/SubagentDrawer.test.ts` — 新增 timer + scroll 测试

### Constraints

- 项目 Vue 3.5 + Pinia + reka-ui，watch / ref / computed 都用 Composition API
- 不用 lodash（subagentRuns.ts:118-120 已有明文约束）
- 不用 date-fns / dayjs — `startedAt` 是 RFC3339 string，直接 `new Date(str).getTime()`
- em-dash 完全禁止（CLAUDE.md + design-taste skill）
- 中文用户可见文案（其他 subagent drawer 文案已是中文）

### References

- `app/src-tauri/src/agent/subagent.rs:421-440` — `build_subagent_event_payload` wire shape（camelCase `runId/sessionId/kind/payload/timestamp`）
- `app/src-tauri/src/agent/chat_loop.rs:2074` — worker rid 格式 `{parent_rid}-sub-{tool_use_id}`
- `app/src-tauri/src/db/subagent_runs.rs:198-220` — `insert_run` SQL，验证 row 立即可查
- 已有 task `.trellis/tasks/archive/2026-06/06-20-b6-pr3-frontend-expand/` — PR3 上线记录（PR3a / PR3b hotfix 上下文）
- `docs/IMPLEMENTATION.md §4` — ADR 决策日志（需在 finish 时回填本任务 ADR）

## Decisions (2026-06-20 brainstorm)

| # | 决策点 | 选择 |
|---|---|---|
| D1 | Scope 边界 | 仅 race fix + 3 drawer polish；typed-cards 重做独立 PR |
| D2 | Click retry 超时 | 1.5s 总超时 / 300ms 间隔 / 最多 5 次重试 |
| D3 | Card live duration | 不加，card 保持静态 `running…` |
| D4 | Waiting 视觉 | 复用 `tool-card--subagent` 样式 + `cursor: wait` + 文案切换 |
| D5 | 按钮位置 | `↗ jump to latest` 在 header；`↓ N new` 浮动 body 底部 |