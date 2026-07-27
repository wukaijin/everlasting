# Design: review visualization view (C2)

> 父任务：`07-26-workflow-review-plugin`
> 配套：`prd.md`
> 依赖：**C1**（resume，review 流能跑通）+ **C3 schema**（review-state.json 是视图数据源，C3 R7 定稿）。
> 本 design 落定 PRD Open Questions（视图位置 / 指挥交互 / 读取机制 / location 跳转）。

## 0. 调研事实

| 事实 | 位置 | 对设计的影响 |
|---|---|---|
| 前端能感知 `workflow_enabled` + `plugin_name` | `stores/chat.ts:1791/1819`（SessionSummary 字段） | 视图触发条件：`workflow_enabled && plugin_name=="review"` |
| `list_workflow_plugins` IPC 已存在 | `commands/sessions.rs` + `daemon/routes/sessions.rs:245` | C2 读 review-state.json 仿此模式新建 IPC |
| 前端事件订阅用 `transport.on`（Tauri listen） | `stores/subagentRuns.ts:177-185`（subagent:event/finished） | review-state.json 更新通知走同样事件推送 |
| SubagentDrawer 是独立抽屉组件（Dialog + Transition） | `components/chat/SubagentDrawer.vue` | C2 视图参考此组件结构 |
| review-state.json 在 task 目录（`<task>/review-state.json`） | C3 R7 | IPC 按 task slug 读文件 |
| finding.source_run_id 指向 subagent_runs.id（C3 schema） | C3 R7 | 跳转原始 final_text 走现有 get_subagent_run IPC |

## 1. 视图位置：ChatPanel 内嵌面板（PRD OQ1 落定）

### 决策：review session 时，在 ChatPanel 顶部（消息流上方）内嵌一个可折叠的 ReviewMatrix 面板

**为何不选独立抽屉**（像 SubagentDrawer）：
- SubagentDrawer 是「按需查看」（点 worker 卡片才开），review 矩阵是「持续可见」（review 过程中用户要一直看着指挥）——抽屉的开合摩擦不合适。
- 内嵌面板让矩阵常驻视野，revising 的 askUserQuestion 在 chat 里问，用户看着矩阵回答，心智连贯。

**为何不选 Tab**：
- Tab 切换会脱离 chat 上下文（askUserQuestion 在 chat），用户要在 Tab 和 chat 间来回切。

**形态**：
```
┌─ ChatPanel ─────────────────────────────────┐
│ ┌─ ReviewMatrix（可折叠，review session 时显示）┐ │
│ │ 轮次×模型矩阵 + 维度对比 + triage          │ │
│ └────────────────────────────────────────────┘ │
│ ┌─ 消息流 ─────────────────────────────────┐ │
│ │ ...chat 消息 + askUserQuestion 卡片...    │ │
│ └────────────────────────────────────────────┘ │
└────────────────────────────────────────────────┘
```

- 默认展开（review 进行中信息密度高，折叠后用户易忘看）。
- 用户可手动折叠（标题栏 toggle）。
- 非 review session / 无 review-state.json 时不渲染（PRD R5）。

## 2. review-state.json 读取机制（PRD OQ3 落定）

### 决策：新建 IPC `get_review_state` + `get_current_task_slug`，刷新靠 streamController 的 tool:call 路由

**背景变更（重要）**：原 design 假设 C3 会建 review-only `emit_review_state_updated` 工具发 `review-state-updated` 事件。**C3 已砍掉该工具**（C3 design §4 改用通用 write_file，贴 PRD R7 + Out of Scope 层次 3）。因此 C2 不能依赖后端领域事件，改为**复用 streamController 已有的 tool:call 全局监听**（零后端事件改动）。

**读取 IPC**（仿 `list_workflow_plugins`，Q0 单源真理双路径）：
- Tauri command + daemon route 双路径。
- 签名：`get_review_state(task_slug) → Result<ReviewStatePayload, AppCommandError>`。
- `ReviewStatePayload` 三态：`State(ReviewState)`（解析成功）/ `Missing`（文件不存在，前端不渲染面板）/ `Invalid { detail }`（坏 JSON，前端进错误态）。
- 读 `<project>/.everlasting/tasks/<task_slug>/review-state.json`。

**task_slug 获取 IPC**（design §10.1 已定，前端无任何 task slug state）：
- 新增 `get_current_task_slug(project_path) → Result<{slug, id, title, status} | null, AppCommandError>`，复用后端 `resolve_current_task`（`agent/workflow/inject.rs:229`）。
- ChatPanel 在 review session 挂载时调一次拿到 slug，传给 ReviewMatrix + reviewStateStore。

**刷新机制：streamController.handleToolCall 路由（照 B12 checklist 模式）**：
- 现状：`streamController.ts:1364 handleToolCall` 已有按 `payload.name` 路由到子 store 的先例 —— B12 checklist 在 `:1381` 用 `if (payload.name === CHECKLIST_TOOL_NAME) useChecklistStore().handleToolCall(...)`。
- C2 照抄：在 `handleToolCall` 加分支：
  ```typescript
  if (payload.name === "write_file") {
    const path = payload.input?.path;
    if (typeof path === "string" && matchesReviewStatePath(path, currentTaskSlug)) {
      useReviewStateStore().handleReviewStateWritten(req.sessionId, currentTaskSlug);
    }
  }
  ```
- `matchesReviewStatePath(path, slug)`：write_file 的 path 可能是相对路径（相对 `ctx.cwd`，见 `write_file.rs:58-65`），匹配策略 = 规范化后 `endsWith("/tasks/<slug>/review-state.json")` 或 basename === `review-state.json` 且 path 含 `/tasks/<slug>/`。相对路径无 `/` 时（如 `review-state.json`）也匹配（保守触发 refresh，get_review_state 读不到只是返回 Missing，无害）。
- `handleReviewStateWritten` → debounce 200ms（防一轮写多个 chunk）→ `refresh(slug)`（调 get_review_state 重读）。
- **不轮询**（无先例 + 延迟感）、**不新建后端事件**（C3 已决定不建工具）、**不让 write_file 发通用事件**（之前否决，blast radius 大）。

**为什么这是最契合现有架构的方案**：
- 零后端事件改动（除 get_review_state + get_current_task_slug 两个读取 IPC）。
- 复用 streamController 已有的 tool:call 全局监听（不重复 listen）。
- 与 B12 checklist 的 tool:call→store 路由模式完全一致（架构先例）。
- 代价（可接受）：相对路径匹配有边界（已在 matchesReviewStatePath 用保守策略兜底）；刷新耦合到 chat 流管道而非干净领域事件（但 review-state.json 本来就只在 review session 的 chat 流里被写，耦合合理）。

## 3. 指挥交互（PRD OQ4 落定）

### 决策：视图纯展示 + 指引回 chat 的 askUserQuestion

**为何不加「继续/定稿」按钮**：
- revising 的 askUserQuestion 已是「继续/定稿」的入口（主 LLM 发起）。视图加按钮会重复触发逻辑,且按钮点击要转成 askUserQuestion 响应,链路绕。
- 视图旁加一行提示「↑ 在 chat 中回答 askUserQuestion 指挥收敛」,引导用户回 chat。

**纯展示的边界**：
- 矩阵/finding/source_run_id 跳转都是只读交互（展开、跳转）,不改状态。
- 唯一的「动作」是折叠/展开面板本身。

## 4. location 跳转（PRD OQ4 落定）

### 决策：location 字段只展示不跳转（MVP）

**为何不做跳转**：
- location 格式自由（`prd.md§2` / `prd.md:42` / `design.md 目标段`）,解析 + 定位 prd 内位置需要 prd 的结构化锚点（heading/line）,prd 是自由 markdown 无锚点。
- 跳转要打开 prd 编辑器视图 + 滚动定位 —— 是另一个前端组件（prd viewer）,scope 过大。

**MVP**：location 作为文本展示（`prd.md§2`），用户手动去 prd 找。若后续 prd 有结构化锚点（Phase 2）,再加跳转。

**source_run_id 跳转**（这个做）：
- finding 的 `source_run_id` 指向 subagent_runs.id,点击调现有 `get_subagent_run(run_id)` IPC 拿原始 final_text,在弹层/抽屉展示（复用 SubagentDrawer 的 run 详情渲染）。
- 这是层次 2 的安全网（评审议题2）—— 用户对比「主 LLM 提炼」vs「reviewer 原话」。

## 5. 组件结构

### 新增组件

```
app/src/components/chat/
├── ReviewMatrix.vue           # 主面板（标题栏 + 折叠 + 矩阵 + 维度对比）
├── ReviewMatrixGrid.vue       # 轮次×模型矩阵（行=轮次,列=模型并集）
├── ReviewDimensionCompare.vue # 维度对比视图（选维度→横向各模型 findings）
└── ReviewFindingDetail.vue    # 单 finding 展开（issue/severity/triage/source_run_id 跳转）
```

### 数据流

```
主 LLM(write_file 工具)写 <task>/review-state.json
       ↓ (write_file 不发领域事件,但 tool:call 事件携带 name + input.path)
streamController.handleToolCall:
  - payload.name === "write_file" && matchesReviewStatePath(input.path, slug)
    → useReviewStateStore().handleReviewStateWritten(sessionId, slug)
       ↓ (debounce 200ms)
useReviewStateStore:
  - refresh(slug) → invoke("get_review_state", slug)
  - 解析 ReviewStatePayload: State → 渲染 / Missing → 不渲染 / Invalid → 错误态
  - 暴露 state / error / loading
       ↓
ChatPanel.vue:
  - onMounted(review session) → invoke("get_current_task_slug") 拿 slug → reviewStateStore.start(slug) 首次加载
  - computed shouldShowReviewMatrix = workflow_enabled && plugin_name=="review" && reviewStateStore.state
  - <ReviewMatrix v-if="shouldShowReviewMatrix" :state="reviewStateStore.state" />
       ↓
ReviewMatrix.vue:
  - 标题栏(折叠 toggle) + tabs(矩阵/维度对比)
  - <ReviewMatrixGrid> / <ReviewDimensionCompare>
  - finding 点击 → <ReviewFindingDetail> 或触发 source_run_id 跳转
```

### 错误态（PRD R3 解析降级）

- `get_review_state` 返回 `missing` → ChatPanel 不渲染面板（静默）。
- 返回 `invalid`（坏 JSON）→ 渲染错误态「review 数据暂不可用,请查看原始 reviewer 输出」+ 链接打开 SubagentDrawer。
- 网络失败 → 重试按钮。

## 6. store 设计（useReviewStateStore）

参考 `useChecklistStore` 的「被 streamController 路由调用」模式（checklist.ts，B12），**不自己 listen 事件**（刷新由 streamController.handleToolCall 路由触发）：
```typescript
export const useReviewStateStore = defineStore("reviewState", () => {
  const state = ref<ReviewState | null>(null);
  const error = ref<{ kind: "missing" | "invalid" | "network"; detail?: string } | null>(null);
  const loading = ref(false);

  // streamController.handleToolCall 路由调用（write_file 命中 review-state.json 时）
  // debounce 200ms 防一轮写多 chunk；slug 守门防跨 session 误触发
  let currentSlug: string | null = null;
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  function handleReviewStateWritten(sessionId: string, slug: string): void {
    if (slug !== currentSlug) return;  // 不是当前 task,忽略
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => { void refresh(slug); }, 200);
  }

  async function refresh(taskSlug: string) { /* invoke get_review_state, 三态分流 */ }
  async function start(taskSlug: string) {
    currentSlug = taskSlug;  // ChatPanel 挂载时调,首次 refresh
    await refresh(taskSlug);
  }
  function stop() {
    currentSlug = null;
    if (debounceTimer) { clearTimeout(debounceTimer); debounceTimer = null; }
  }

  return { state, error, loading, start, stop, refresh, handleReviewStateWritten };
});
```

- 生命周期：ChatPanel 挂载（review session）时 start（首次加载），卸载/切 session 时 stop。
- task_slug 变化（切 task）时重新 start（currentSlug 更新 + 重新 refresh）。
- **不 listen 任何事件**：刷新完全由 streamController.handleToolCall 路由驱动（store 只暴露 handleReviewStateWritten 给 streamController 调）。
- **slug 守门**：handleReviewStateWritten 检查 slug === currentSlug，防跨 task/session 误触发（streamController 是全局监听，会收到所有 session 的 tool:call）。

## 7. ReviewState TS 类型（对齐 C3 schema）

```typescript
// app/src/types/review-state.ts
export type Verdict = "pass" | "pass_with_minor" | "revise" | "reject";
export type Severity = "critical" | "high" | "medium" | "low" | "info";
export type RunStatus = "running" | "completed" | "cancelled" | "error" | "incomplete";
export type TriageDecision = "adopt" | "reject" | "defer";

export interface ReviewFinding {
  finding_id: string;
  dimension: string;
  severity: Severity;
  issue: string;
  suggestion?: string;
  location?: string;
  source_run_id: string;
  triage?: { decision: TriageDecision; reason: string };
}

export interface ModelVerdict {
  model_display: string;
  run_id: string;
  status: RunStatus;
  verdict: Verdict;
  summary?: string;
  findings: ReviewFinding[];
}

export interface ReviewRound {
  round: number;
  dimensions: string[];
  models_present: string[];
  models: Record<string, ModelVerdict>;  // key = model_id
  change_log?: string[];
  convergence_note?: string;
}

export interface ReviewState {
  schema_version: string;
  task_id: string;
  current_round: number;
  rounds: ReviewRound[];
}
```

与 C3 R7 schema 一一对应（跨任务契约）。

## 8. 影响面 + 回归风险

### 改动文件
- 新增前端:`components/chat/ReviewMatrix.vue` + 3 子组件 + `stores/reviewState.ts` + `types/review-state.ts`
- 新增后端:`commands/review.rs`（get_review_state + get_current_task_slug IPC + inner）+ `daemon/routes/review.rs`（route）
- 改:`lib.rs`（注册 IPC）+ `daemon/routes/mod.rs`（注册 route）
- 改:`ChatPanel.vue`（挂载 ReviewMatrix + 调 get_current_task_slug）
- 改:`stores/streamController.ts`（handleToolCall 加 write_file→reviewStateStore 路由分支 + matchesReviewStatePath helper）

### 回归风险
- **非 review session 零影响**:shouldShowReviewMatrix 门控,dev/无 workflow session 不渲染面板。streamController 的 write_file 路由分支虽然会对所有 session 的 write_file 触发匹配,但 matchesReviewStatePath 不命中就 no-op,且 reviewStateStore.handleReviewStateWritten 有 slug 守门 —— dev session 无 review-state.json,匹配不命中,零开销。
- **C3 schema 变动**:ReviewState TS 类型必须与 C3 schema 同步 —— 跨任务契约,任一方改 schema 要同步另一方。C3 已定稿（commit 83207f4 + workflow-plugin-builtin.md spec）,C2 严格按定稿写 TS。
- **write_file 相对路径匹配**:write_file 的 path 可能是相对路径（相对 ctx.cwd）,matchesReviewStatePath 用保守策略（basename + /tasks/<slug>/ 子串 + 纯 basename 兜底）,误触发只是多一次 get_review_state（读不到返 Missing,无害）,漏触发由用户手动折叠/展开面板或切 session 重 start 兜底。

### 单测/测试
- ReviewMatrix 组件渲染（fixture: 3 轮×3 模型×含失败模型）
- 维度对比逻辑（选维度→横向 findings）
- source_run_id 跳转（mock get_subagent_run）
- 错误态（missing/invalid/network）
- state 门控（非 review session 不渲染）
- get_review_state IPC（文件存在/缺失/坏 JSON 三态）

## 9. Open Questions 落定

1. ✅ **视图位置**（PRD OQ1）:ChatPanel 顶部内嵌可折叠面板（非抽屉/非 Tab）。
2. ✅ **指挥交互**（PRD OQ2）:纯展示 + 指引回 chat 的 askUserQuestion（不加按钮）。
3. ✅ **读取机制**（PRD OQ3）:get_review_state IPC（读文件三态）+ get_current_task_slug IPC（前端无 task slug state）+ streamController.handleToolCall 路由刷新（write_file 命中 review-state.json → reviewStateStore.refresh）。**非轮询、非后端领域事件**（C3 砍了 emit_review_state_updated 工具，C2 复用现有 tool:call 全局监听）。
4. ✅ **location 跳转**（PRD OQ4）:MVP 只展示不跳转（prd 无结构化锚点）；source_run_id 跳转做（复用 get_subagent_run）。

## 10. 待 design 阶段确认的细节（评审回流，已落定）

1. ✅ **前端如何拿当前 task_slug**（评审两份都推荐）：新增 `get_current_task_slug` IPC，返回 `{slug, id, title, status} | null`（复用 `resolve_current_task`，`agent/workflow/inject.rs:229`）。**否决**「从现有 workflow state IPC 暴露」（plugin ≠ task，不存在这样的 IPC；前端无任何 task slug state，见 research）。
2. ✅ **刷新触发点**（C3 砍 emit 工具后的修正）：采用 **streamController.handleToolCall 路由**（照 B12 checklist 模式，write_file 命中 review-state.json 路径 → reviewStateStore.handleReviewStateWritten → debounce → refresh）。详见 §2 + §6。**否决**：(a) 后端 notify 文件监听 + 领域事件（要加 notify watcher + 事件类型 + 生命周期，C2 从纯前端变前后端）；(b) 轮询（无先例 + 延迟感）；(c) write_file 通用钩子发事件（blast radius 大，之前否决）。研究依据：write_file 不发领域事件（`tools/write_file.rs:46-186` 纯 tokio::fs::write），但 tool:call payload 携带 name + input.path（`state.rs:569 ToolCallPayload`），streamController 已全局监听 tool:call（`streamController.ts:1831`）且已有 name 路由到子 store 的先例（`:1381` B12 checklist）。
3. **get_subagent_run 跳转的 UI**：弹层 vs 复用 SubagentDrawer。倾向轻量弹层（只看 final_text），不强行整套 SubagentDrawer。
