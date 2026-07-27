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

**方案**：新增 `get_current_task_slug` IPC，返回 `{slug, id, title, status} | null`（复用 `resolve_current_task`，`agent/workflow/inject.rs:229`）。评审两份都推荐此方案，**否决**「从现有 workflow state IPC 暴露」（plugin ≠ task，不存在；前端无任何 task slug state）。

**实现**：Phase 1 一起做（commands/review.rs 加此 IPC + get_review_state）。

### 步骤 0.2 — 刷新机制（已定：streamController tool:call 路由，**非后端事件**）

**背景变更**：原 design 假设 C3 建 review-only `emit_review_state_updated` 工具发 `review-state-updated` 事件。**C3 已砍掉该工具**（C3 design §4 改 write_file，贴 PRD R7 + Out of Scope 层次 3）。C2 改为复用 streamController 已有的 tool:call 全局监听。

**方案**（照 B12 checklist 模式，`streamController.ts:1381`）：在 `handleToolCall` 加 write_file 路由分支 —— `payload.name === "write_file" && matchesReviewStatePath(input.path, slug)` → `useReviewStateStore().handleReviewStateWritten(sessionId, slug)`。store 内 debounce 200ms + slug 守门 → refresh。

**否决**：(a) 后端 notify 文件监听 + 领域事件（C2 从纯前端变前后端）；(b) 轮询（无先例）；(c) write_file 通用钩子发事件（blast radius 大，之前否决）。

**C2 职责**：C2 后端只做两个**读取** IPC（get_review_state + get_current_task_slug），**不发任何事件**。刷新完全前端驱动（streamController 路由）。详见 design §2/§6。

---

## Phase 1：后端 IPC（get_review_state + get_current_task_slug）

### 步骤 1.1 — commands/review.rs 新建

**新增文件**：`app/src-tauri/src/commands/review.rs`

**改动**（design §2）：两个读取 IPC（Q0 单源真理双路径：Tauri command + inner）：
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
    // 三态：State(json) / Missing（文件不存在）/ Invalid{detail}（坏 JSON）
}

pub async fn get_current_task_slug(
    state: State<AppState>,
) -> Result<Option<CurrentTaskInfo>, AppCommandError> {
    // 复用 resolve_current_task（agent/workflow/inject.rs:229）
    // 返回 {slug, id, title, status} | null
}

pub enum ReviewStatePayload {
    State(ReviewState),   // serde_json 反序列化成功
    Missing,
    Invalid { detail: String },
}

pub struct CurrentTaskInfo { pub slug: String, pub id: String, pub title: String, pub status: String }
```

参考 `list_workflow_plugins`（commands/sessions.rs）的双路径模式（Tauri command + inner）。**不发任何事件**（C3 已决定不发，C2 刷新靠前端 tool:call 路由）。

### 步骤 1.2 — daemon route + lib.rs 注册

**改动**：
- `daemon/routes/review.rs`：新建，两个 handler：`POST /api/v1/review/get_review_state` + `GET /api/v1/review/current_task`，转发 inner。
- `lib.rs`：注册 `commands::review::get_review_state` + `commands::review::get_current_task_slug` IPC。
- `daemon/routes/mod.rs`：注册两条 route。

**验证**：
```bash
cd app/src-tauri && cargo build --lib 2>&1 | tail -5
# 两个 IPC 注册成功，编译通过
```

---

## Phase 2：刷新路由（streamController.handleToolCall，照 B12 checklist 模式）

### 步骤 2.1 — streamController 加 write_file → reviewStateStore 路由

**文件**：`app/src/stores/streamController.ts`

**改动**（design §2/§6）：在 `handleToolCall`（`:1364`）的 B12 checklist 路由分支（`:1381`）旁，加 write_file 路由：
```typescript
if (payload.name === "write_file") {
  const path = payload.input?.path;
  const slug = useReviewStateStore().currentSlugForRouting;  // 或从 ChatPanel 传入
  if (typeof path === "string" && slug && matchesReviewStatePath(path, slug)) {
    useReviewStateStore().handleReviewStateWritten(req.sessionId, slug);
  }
}
```

`matchesReviewStatePath(path, slug)` helper（保守匹配，误触发只是多一次 get_review_state 读 Missing，无害）：
- 规范化 path（去 `./`、统一 `/`）
- 命中条件（任一）：`endsWith("/tasks/<slug>/review-state.json")` / basename === `review-state.json` 且含 `/tasks/<slug>/` / 纯 basename `review-state.json`（兜底，相对路径无分隔符）

**注意**：streamController 是全局监听（收所有 session 的 tool:call），slug 守门在 `handleReviewStateWritten` 里做（`slug !== currentSlug` 直接 return），路由层只做 path 形态匹配。

### 步骤 2.2 — slug 如何到 streamController

两个选项（implement 时选简单的）：
- (a) reviewStateStore 暴露 `currentSlug` ref，streamController 读它（store 是单例，跨组件共享）。
- (b) ChatPanel onMounted 时把 slug 写入一个 streamController 可读的位置。

倾向 (a)（store 单例最简单，checklist 也是 store 持有状态）。

**验证**：
```bash
cd app && pnpm tsc --noEmit 2>&1 | tail -10
cd app && pnpm test -- --run streamController 2>&1 | tail -15  # 若有 streamController 测试
# 手动：mock 一个 write_file tool:call payload（path 命中 review-state.json），确认 reviewStateStore.refresh 被调
```

---

## Phase 3：前端类型 + store

### 步骤 3.1 — ReviewState TS 类型

**新增文件**：`app/src/types/review-state.ts`

**内容**：design.md §7 完整 TS 类型（Verdict/Severity/RunStatus/TriageDecision/ReviewFinding/ModelVerdict/ReviewRound/ReviewState）。**严格对齐 C3 R7 schema**（跨任务契约）。

### 步骤 3.2 — useReviewStateStore

**新增文件**：`app/src/stores/reviewState.ts`

**内容**：design.md §6 store 设计。**不 listen 任何事件**（刷新由 streamController.handleToolCall 路由驱动，照 B12 checklist 的「被路由调用」模式）。暴露：`state / error / loading / currentSlug` + `start(slug) / stop() / refresh(slug) / handleReviewStateWritten(sessionId, slug)`。debounce 200ms + slug 守门在 handleReviewStateWritten 内。参考 `useChecklistStore`（被 streamController 路由调用的先例）。

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
- 生命周期：review session 时 onMounted 调 `get_current_task_slug` 拿 slug → `reviewStateStore.start(slug)`（首次加载）；卸载/切 session 时 `stop()`
- ReviewMatrix 插入点：`<main class="chat-panel__main">` 开标签后（消息流上方，见 research §5，约 `:656`）

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
- ReviewMatrixGrid 渲染（fixture：3 轮×3 模型×含 1 失败模型，对齐 C3 schema 的 status=error）
- 维度对比逻辑（选维度→横向 findings）
- source_run_id 跳转（mock get_subagent_run）
- 错误态（missing/invalid）
- state 门控（非 review session 不渲染）

fixture 用 C3 schema 形态的 review-state.json 样本（models key=model_id，status 枚举 running/completed/cancelled/error/incomplete，findings 含 finding_id/dimension/severity/issue/suggestion/location/source_run_id/triage）。

### 步骤 6.2 — 刷新路由测试（streamController tool:call 路由）

**覆盖**（design §2/§6，照 B12 checklist 测试模式）：
- matchesReviewStatePath：命中 `/tasks/<slug>/review-state.json` / basename `review-state.json` / 相对路径兜底
- handleReviewStateWritten：slug 守门（非当前 slug 不 refresh）+ debounce（连续多次写只 refresh 一次）
- write_file tool:call payload → reviewStateStore.refresh 被调（mock get_review_state 返回 State）
- 非 write_file 的 tool:call 不触发 refresh

### 步骤 6.3 — 后端 IPC 测试

**覆盖**：
- get_review_state 文件存在 → State
- 文件缺失 → Missing
- 坏 JSON → Invalid
- get_current_task_slug 有 active task → Some({slug,...})；无 task → None

### 步骤 6.4 — 全量

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

### R2（中）：write_file 路径匹配的可靠性
**问题**：刷新靠 streamController 匹配 write_file 的 input.path（可能是相对路径）。漏匹配 → 视图不更新；误匹配 → 多一次 get_review_state（读 Missing，无害）。
**缓解**：matchesReviewStatePath 用保守策略（basename + /tasks/<slug>/ 子串 + 纯 basename 兜底），漏匹配由用户折叠/展开面板或切 session 重 start 兜底；误匹配无害（slug 守门 + get_review_state 读不到返 Missing）。不追求 100% 精确匹配（review-state.json 只在 review session 写，场景窄）。

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
