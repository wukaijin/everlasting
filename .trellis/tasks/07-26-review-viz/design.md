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

### 决策：新建 IPC `get_review_state` + 事件推送 `review-state-updated`

**读取 IPC**（仿 `list_workflow_plugins`）：
- Tauri command + daemon route 双路径（Q0 单源真理模式）。
- 签名：`get_review_state(task_slug) → Result<ReviewState, AppCommandError>`。
- 实现读 `<project>/.everlasting/tasks/<task_slug>/review-state.json`，解析失败返回 `invalid` 错误（前端进错误态），文件不存在返回 `missing`（前端不渲染面板）。
- task_slug 从 `current_task`（workflow ctx）拿 —— 但前端可能没 current_task slug,design 阶段确认前端如何拿当前 task slug（可能需新 IPC `get_current_task` 或从 workflow state 暴露）。

**更新通知**（仿 `subagent:finished`）：
- 主 LLM 写完 review-state.json 后,后端发事件 `review-state-updated { task_slug, current_round, total_findings }`。
- 前端 `transport.on("review-state-updated")` 收到 → 重新调 `get_review_state` 刷新。
- **写入触发点（评审回流，方案 iii）**：新建 review-only `emit_review_state_updated` 工具（C3 design §4 定义），主 LLM 调它写 review-state.json + 发事件。工具内部 tmp+rename 原子写（解决 write_file 不原子）+ 写后发 `review-state-updated` 事件（解决事件发送点）+ 只在 review workflow 可见（`filter_tools_for_workflow` gate，零 dev 污染）。**否决** transition 钩子方案（自动钩子不存在）和 write_file 通用钩子方案（要改 ToolContext 跨 21 个工具）。

**不选轮询**：
- 轮询有延迟 + 无谓 IPC 开销。事件推送实时且只在写入时触发。

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
后端:review-only `emit_review_state_updated` 工具(C3)原子写 review-state.json → 发 review-state-updated 事件
       ↓
前端 store(useReviewStateStore):
  - transport.on("review-state-updated") → invoke("get_review_state", task_slug)
  - 解析 ReviewState,失败进 error 态
  - 暴露 state / error / loading
       ↓
ChatPanel.vue:
  - computed shouldShowReviewMatrix = workflow_enabled && plugin_name=="review" && reviewState
  - <ReviewMatrix v-if="shouldShowReviewMatrix" :state="reviewState" />
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

参考 `useSubagentRunsStore` 模式（stores/subagentRuns.ts）：
```typescript
export const useReviewStateStore = defineStore("reviewState", () => {
  const state = ref<ReviewState | null>(null);
  const error = ref<{ kind: "missing" | "invalid" | "network"; detail?: string } | null>(null);
  const loading = ref(false);
  let unlisten: UnlistenFn | null = null;

  async function refresh(taskSlug: string) { /* invoke get_review_state */ }
  function start(taskSlug: string) {
    // transport.on("review-state-updated") → refresh
    refresh(taskSlug);
  }
  function stop() { unlisten?.(); }

  return { state, error, loading, start, stop, refresh };
});
```

- 生命周期：ChatPanel 挂载（review session）时 start,卸载时 stop。
- task_slug 变化（切 task）时重新 start。
- **幂等 + currentSlug 守门**（评审 MiniMax §2.4 回流）：`start()` 前先 `stop()` 清旧监听 + `if (currentSlug === taskSlug && unlisten) return` 幂等，防 slug 变化时孤儿监听（比现有 `subagentRuns.ts` 做得更好）：
  ```typescript
  let currentSlug: string | null = null;
  async function start(taskSlug: string): Promise<void> {
    if (currentSlug === taskSlug && unlisten) return; // idempotent
    stop();  // 先清旧的
    currentSlug = taskSlug;
    unlisten = await transport.listen<ReviewStateUpdatedPayload>("review-state-updated", ...);
    await refresh(taskSlug);
  }
  ```

## 7. ReviewState TS 类型（对齐 C3 schema）

```typescript
// app/src/types/review-state.ts
export type Verdict = "pass" | "pass_with_minor" | "revise" | "reject";
export type Severity = "critical" | "high" | "medium" | "low" | "info";
export type RunStatus = "completed" | "failed" | "timed_out" | "cancelled" | "truncated";
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
- 新增后端:`commands/review.rs`（get_review_state IPC + inner）+ `daemon/routes/review.rs`（route）。事件发送由 C3 的 review-only `emit_review_state_updated` 工具负责（不在 C2 后端单独加事件发送点）。
- 改:`lib.rs`（注册 IPC）+ `ChatPanel.vue`（挂载 ReviewMatrix）+ `daemon/routes/mod.rs`（注册 route）

### 回归风险
- **非 review session 零影响**:shouldShowReviewMatrix 门控,dev/无 workflow session 不渲染面板、不订阅事件。
- **C3 schema 变动**:ReviewState TS 类型必须与 C3 schema 同步 —— 跨任务契约,任一方改 schema 要同步另一方。
- **事件发送时机**:由 C3 的 review-only 工具发（写完即发，原子写保证文件已落盘），天然只在 review workflow 触发，不污染 dev。

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
3. ✅ **读取机制**（PRD OQ3）:get_review_state IPC + review-state-updated 事件推送（非轮询）。
4. ✅ **location 跳转**（PRD OQ4）:MVP 只展示不跳转（prd 无结构化锚点）；source_run_id 跳转做（复用 get_subagent_run）。

## 10. 待 design 阶段确认的细节（评审回流，已落定）

1. ✅ **前端如何拿当前 task_slug**（评审两份都推荐）：新增 `get_current_task_slug` IPC，返回 `{slug, id, title, status}`（复用 `resolve_current_task`）。**否决**「从现有 workflow state IPC 暴露」（plugin ≠ task，不存在这样的 IPC）。
2. ✅ **review-state-updated 事件发送点**：采用 review-only `emit_review_state_updated` 工具方案（C3 design §4）。**否决** transition 钩子（自动钩子不存在）和 write_file 通用钩子（要改 ToolContext 跨 21 工具）。详见 C3 design §4 + 父任务 review-triage-c2c3.md。
3. **get_subagent_run 跳转的 UI**：弹层 vs 复用 SubagentDrawer。倾向轻量弹层（只看 final_text），不强行整套 SubagentDrawer。
