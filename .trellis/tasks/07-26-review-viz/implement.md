# Implement: review visualization view (C2)

> 父任务：`07-26-workflow-review-plugin`
> 配套：`prd.md` + `design.md`
> 依赖：**C1**（review 流能跑通）+ **C3 schema**（review-state.json 是数据源，C3 R7 定稿）。
> 本文件是 ordered execution checklist + 验证命令 + 风险点。

## 执行顺序

后端 IPC + 事件 → 前端类型 + store → 组件 → 集成挂载 → 测试。先后端数据通路，再前端消费。

---

## Phase 0：两个待定细节（评审已落定，见 design §10）

### 步骤 0.1 — task_slug 获取（已定：新 IPC）

**方案**：新增 `get_current_task_slug` IPC，返回 `{slug, id, title, status}`（复用 `resolve_current_task`）。评审两份都推荐此方案，**否决**「从现有 workflow state IPC 暴露」（plugin ≠ task，不存在）。

**实现**：Phase 1 一起做（commands/review.rs 加此 IPC + get_review_state）。

### 步骤 0.2 — review-state-updated 事件发送点（已定：review-only 工具，C3 负责）

**方案**：采用 review-only `emit_review_state_updated` 工具（C3 design §4 定义），主 LLM 调它原子写 + 发事件。**否决** transition 钩子（自动钩子不存在）和 write_file 通用钩子（要改 ToolContext 跨 21 工具）。

**C2 职责**：C2 只需前端订阅 `review-state-updated` 事件（transport.listen），事件发送由 C3 的工具负责。C2 后端不单独加事件发送点。

---

## Phase 1：后端 IPC（get_review_state）

### 步骤 1.1 — commands/review.rs 新建

**新增文件**：`app/src-tauri/src/commands/review.rs`

**改动**（design §2）：
```rust
pub async fn get_review_state(
    state: State<AppState>,
    task_slug: String,
) -> Result<ReviewStatePayload, AppCommandError> {
    get_review_state_inner(task_slug, &state).await
}

pub async fn get_review_state_inner(
    task_slug: String,
    state: &AppState,
) -> Result<ReviewStatePayload, AppCommandError> {
    // 读 <project>/.everlasting/tasks/<task_slug>/review-state.json
    // 三态：Ok(json) / missing（文件不存在）/ invalid（坏 JSON）
}

pub enum ReviewStatePayload {
    State(ReviewState),   // 反序列化成功
    Missing,
    Invalid { detail: String },
}
```

参考 `list_workflow_plugins`（commands/sessions.rs）的双路径模式（Tauri command + inner）。

### 步骤 1.2 — daemon route + lib.rs 注册

**改动**：
- `daemon/routes/review.rs`：新建，`POST /api/v1/review/get_review_state` handler，转发 inner。
- `lib.rs`：注册 `commands::review::get_review_state` IPC。
- `daemon/routes/mod.rs`：注册 route。

**验证**：
```bash
cd app/src-tauri && cargo build --lib 2>&1 | tail -5
# IPC 注册成功，编译通过
```

---

## Phase 2：事件订阅（review-state-updated，C2 只订阅）

### 步骤 2.1 — 前端订阅事件（发送由 C3 工具负责）

**改动**：事件发送由 C3 的 review-only `emit_review_state_updated` 工具负责（C3 design §4），C2 只在前端订阅：
- `useReviewStateStore.start()` 调 `transport.listen<ReviewStateUpdatedPayload>("review-state-updated", handler)`，handler 触发 `refresh(taskSlug)`。
- payload 形态（与 C3 工具约定）：`{ task_slug, current_round, total_findings, schema_version }`。
- payload 的 current_round/total_findings 只作提示，前端永远以 `get_review_state` 返回的实际文件内容为准（防并发写入时序问题）。

**验证**：mock 后端发事件（C3 工具未落地前），确认前端收到 → refresh。C3 集成后端到端验证。

---

## Phase 3：前端类型 + store

### 步骤 3.1 — ReviewState TS 类型

**新增文件**：`app/src/types/review-state.ts`

**内容**：design.md §7 完整 TS 类型（Verdict/Severity/RunStatus/TriageDecision/ReviewFinding/ModelVerdict/ReviewRound/ReviewState）。**严格对齐 C3 R7 schema**（跨任务契约）。

### 步骤 3.2 — useReviewStateStore

**新增文件**：`app/src/stores/reviewState.ts`

**内容**：design.md §6 store 设计（state/error/loading + start/stop/refresh + transport.on 订阅）。参考 `useSubagentRunsStore`（stores/subagentRuns.ts）模式。

**验证**：
```bash
cd app && pnpm tsc --noEmit 2>&1 | tail -10
# 类型检查通过
```

---

## Phase 4：前端组件

### 步骤 4.1 — ReviewMatrix.vue 主面板

**新增文件**：`app/src/components/chat/ReviewMatrix.vue`

**内容**（design §5）：
- props: state: ReviewState
- 标题栏（折叠 toggle + current_round + total_findings 摘要）
- tabs（矩阵视图 / 维度对比）
- 错误态/空态渲染
- emit: 无（纯展示）

### 步骤 4.2 — ReviewMatrixGrid.vue（轮次×模型矩阵）

**新增文件**：`app/src/components/chat/ReviewMatrixGrid.vue`

**内容**：
- 行 = rounds（轮次），列 = 所有出现过的 model_display 并集（缺席 cell 显「未参与」，失败模型灰显）
- 单元格 = verdict chip + findings 数，点击展开 findings 列表
- finding 点击 → 触发 source_run_id 跳转（emit 给父，或调 get_subagent_run）

### 步骤 4.3 — ReviewDimensionCompare.vue（维度对比）

**新增文件**：`app/src/components/chat/ReviewDimensionCompare.vue`

**内容**：
- 维度选择器（dropdown，来自所有 rounds 的 dimensions 并集）
- 选定维度后，横向呈现各模型在该维度的 findings + severity + triage
- 直观对比分歧（同维度不同模型看法）

### 步骤 4.4 — ReviewFindingDetail.vue（finding 展开 + source 跳转）

**新增文件**：`app/src/components/chat/ReviewFindingDetail.vue`

**内容**：
- 单 finding 详情（issue/severity/suggestion/location/triage decision+reason）
- source_run_id 跳转按钮 → 调 get_subagent_run，弹层展示原始 final_text（层次2 安全网）

**验证**（4.1-4.4）：
```bash
cd app && pnpm tsc --noEmit 2>&1 | tail -10
cd app && pnpm test -- --run review 2>&1 | tail -15  # 若有组件测试
```

---

## Phase 5：集成挂载

### 步骤 5.1 — ChatPanel.vue 挂载 ReviewMatrix

**文件**：`app/src/components/chat/ChatPanel.vue`

**改动**（design §5）：
```vue
<ReviewMatrix
  v-if="shouldShowReviewMatrix"
  :state="reviewStateStore.state"
/>
```
- computed `shouldShowReviewMatrix`：`workflow_enabled && plugin_name=="review" && reviewStateStore.state`
- 生命周期：review session 时 `reviewStateStore.start(task_slug)`，卸载/切 session 时 `stop()`

**验证**：
```bash
cd app && pnpm tsc --noEmit 2>&1 | tail -10
cd app && pnpm dev  # 手动：切 review session，确认面板出现；dev session 不出现
```

---

## Phase 6：测试

### 步骤 6.1 — 前端组件测试

**新增**：`app/src/components/chat/ReviewMatrix.test.ts`（或同类）

**覆盖**（design §8）：
- ReviewMatrixGrid 渲染（fixture：3 轮×3 模型×含 1 失败模型）
- 维度对比逻辑（选维度→横向 findings）
- source_run_id 跳转（mock get_subagent_run）
- 错误态（missing/invalid）
- state 门控（非 review session 不渲染）

fixture 用 C3 schema 形态的 review-state.json 样本。

### 步骤 6.2 — 后端 IPC 测试

**覆盖**：
- get_review_state 文件存在 → State
- 文件缺失 → Missing
- 坏 JSON → Invalid

### 步骤 6.3 — 全量

```bash
cd app && pnpm test -- --run 2>&1 | tail -20
cd app/src-tauri && cargo test --lib 2>&1 | tail -20
cd app/src-tauri && cargo clippy --lib --tests -- -D warnings 2>&1 | tail -10
```

---

## 风险点

### R1（中）：跨任务 schema 契约同步
**问题**：C3 改 schema，C2 的 TS 类型 + 渲染逻辑要同步。C2/C3 并行实施时易漂移。
**缓解**：C3 schema（R7）定稿后冻结字段名/枚举值；C2 严格按定稿写 TS。schema_version 字段为未来演化留位（C2 解析时校验 schema_version，不匹配进 invalid 态）。

### R2（中）：事件发送时机
**问题**：若用 write_file 识别方案（Phase 0.2 降级），要避免每次 write_file 都校验路径开销 + 误触发（用户手动写 review-state.json 也发事件）。
**缓解**：倾向 transition 钩子方案（review 专用，零污染）。若只能用 write_file 方案，校验路径精确匹配 `<task>/review-state.json` + workflow context 是 review。

### R3（低）：get_subagent_run 跳转 UI
**问题**：source_run_id 跳转展示原始 final_text，UI 是弹层还是复用 SubagentDrawer。
**缓解**：倾向轻量弹层（只展示 final_text），不强行整套 SubagentDrawer（避免组件耦合）。implement 时确认 SubagentDrawer 是否可抽出 run 详情子组件复用。

---

## Follow-up（C2 范围外）

- location 字段跳转（prd 无结构化锚点，Phase 2 做）
- 可视化增强（finding 趋势图、convergence 可视化等，按需）

---

## 验证命令汇总

```bash
# 后端
cd app/src-tauri
cargo build --lib
cargo test --lib review                        # Phase 1/2/6.2
cargo test --lib                               # Phase 6.3
cargo clippy --lib --tests -- -D warnings

# 前端
cd app
pnpm tsc --noEmit                              # Phase 3/4/5 类型
pnpm test -- --run review                      # Phase 6.1 组件
pnpm test -- --run                             # Phase 6.3 全量
```
