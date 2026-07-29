# review epic — C2↔C3 契约 + C2 前端可行性（定向评审）

> 评审对象：C3 schema（R7 + design §4）+ C2 TS 类型（design §7）+ C2 IPC/事件（design §2）+ C2 implement Phase 0 待确认点。
> 评审范围：仅 3 个维度，不重复全面评审。
> 评审日期：2026-07-26。
> 代码锚点核对结果附后。

---

## 维度 1：C3 schema ↔ C2 TS 类型是否真的一一对应？

**结论：主体对齐，但有 3 处真不一致 + 1 处 C3 design 错误假设。**

### 1.1 字段逐项对照

| C3 prd.md R7 | C2 design.md §7 | 一致？ |
|---|---|---|
| `schema_version: "1.0"` | `schema_version: string` | ✅ |
| `task_id` | `task_id: string` | ✅ |
| `current_round` | `current_round: number` | ✅ |
| `rounds[]` | `rounds: ReviewRound[]` | ✅ |
| `round.round` / `dimensions` | `round: number` / `dimensions: string[]` | ✅ |
| `models_present` | `models_present: string[]` | ✅ |
| `models: {<model_id>: {...}}` | `models: Record<string, ModelVerdict>`（key=model_id） | ✅ |
| `findings[].finding_id/dimension/severity/issue/suggestion/location/source_run_id` | 同 | ✅ |
| `findings[].triage.{decision,reason}` | `triage?: { decision; reason }` | ✅ |
| `rounds[].change_log` / `convergence_note` | 同 | ✅ |

**models map key**：C3 prd.md:R93「用 model_id（稳定 id），不是 display_name」+ C2 §7 `Record<string, ModelVerdict>` 注释「key = model_id」—— **已对齐**。但建议 C2 §7 TS 注释再补一句「key 必须是 model_id（来自 `dispatch_subagent` 的 model enum 的稳定 id）」，并考虑用 branded type 防止键值漂移。

### 1.2 枚举值一致性

| 枚举 | C3 值 | C2 §7 TS 值 | 一致？ |
|---|---|---|---|
| `verdict` | pass / pass_with_minor / revise / reject | 同 | ✅ |
| `severity` | critical / high / medium / low / info | 同 | ✅ |
| `status`（per-run） | completed / failed / timed_out | **completed / failed / timed_out / cancelled / truncated** | ❌ |
| `triage.decision` | adopt / reject / defer | 同 | ✅ |

**`RunStatus` 不一致（C3 写 3 值，C2 写 5 值）—— 真断裂点。**

- C3 prd.md R7 注释只列 `completed/failed/timed_out`。
- C2 §7 `RunStatus` 多出 `cancelled` 和 `truncated`。
- 来源真值侧 `db/subagent_runs.rs:34/70-126` 已经定义完整 5 值（`('running','completed','cancelled','error','incomplete')` + `transcript_truncated` 字段）—— DB 列 CHECK 约束早已扩到 5 值。
- 一旦 reviewer 失败被 cancel，C2 视图拿到 `cancelled` 时 TS 类型接受，C3 serde 写入侧却没这枚举，破坏"两边按同一 schema 解释 JSON"的不变量。

**修复建议**：C3 prd.md R7 + design.md §4 把 status 枚举扩到 5 值（与 `db/subagent_runs.rs` 对齐）。倾向**扩 C3 到 5 值**，因为来源真值侧已经有这 5 值。

### 1.3 C3 design.md §4 一个错误假设（埋雷）

C3 design.md §4 "写入约束"：
> 「原子化: 主 LLM 用 write_file 写(tmp + rename 由 write_file 工具内部保证,或主 LLM 先写临时文件再 rename —— design 阶段确认 write_file 是否原子,若否 skill 指引主 LLM 用临时文件)」

**核查结果：`write_file` 不原子。** 实际实现是 `tokio::fs::write(&validated, content).await`（`app/src-tauri/src/tools/write_file.rs:163`），不是 tmp+rename。原子写只有 `agent/workflow/task.rs:373 write_task`（`fs::rename`）这一处。

**结论**：
- C3 design.md §4 的「write_file 工具内部保证」假设错误，必须改成「主 LLM 先写 `<task>/review-state.json.tmp` 再用 shell `mv` 改名」，或 C3 直接提供专用「review_state_write」工具。
- 否则前端 `get_review_state` 在主 LLM 写入中途读到半截 JSON → 解析失败 → 进 `invalid` 错误态（C2 design §5 已规划），用户看到错误会是常态不是边缘。

### 1.4 嵌套结构

`rounds[].models` 是 map（C3 `{}` + C2 `Record`），findings 在 models 内（不是顶层）—— **两侧一致**，C3 prd.md:R92-118 + C2 §7 `ModelVerdict.findings: ReviewFinding[]` 都把 findings 嵌在 model 内。

顶层 status / source_run_id（C3 prd.md:R96 文字提及）：C2 §7 没列入顶层，**status 实际落在 ModelVerdict.status**（per-run），这是合理的（schema R7 写法本来就嵌套在 models{} 内，"顶层 status"是 C3 prd.md:R96 注释位置偏差）。**C3 design.md §4 应显式声明「status 仅 per-run，不存在顶层 status / source_run_id」**，免得后续 C2 误读。

### 1.5 维度 1 总结

- **RunStatus 枚举值范围**（5 vs 3）—— **必修**。
- **write_file 原子性假设**（C3 design.md §4）—— **必改**。
- **顶层 status / source_run_id 不存在** —— C3 design.md §4 文字澄清即可，不算断裂。
- **map key 是 model_id** —— 已对齐，C2 加 branded type 注释。
- 其余字段一一对应，**维度 1 主体已对齐**。

---

## 维度 2：C2 的前端机制在 everlasting 架构里能否跑通？

**结论：IPC + 事件设计符合现有惯例（仿 `list_workflow_plugins` + `subagent:finished` 双通道），但有 2 处必须补的链路时序/重入风险 + 1 处可优化的实现选择。**

### 2.1 IPC 双路径模式对照

| 模式 | `list_workflow_plugins`（现有） | C2 设计的 `get_review_state` |
|---|---|---|
| Tauri command | `commands/sessions.rs:500`（`#[tauri::command]` + `pub async fn`） | `commands/review.rs::get_review_state` 同模式 |
| daemon route | `daemon/routes/sessions.rs:245`（`POST /api/v1/sessions/list_workflow_plugins`，handler 直接转发 `xxx_inner`） | `daemon/routes/review.rs` 同模式 |
| 注册 | `lib.rs` + `daemon/routes/mod.rs` | 同 |
| 入参 | `project_path: String` | `task_slug: String` |

**符合惯例**，与 `list_workflow_plugins`（`commands/sessions.rs:500` + `daemon/routes/sessions.rs:245`）同结构。**无需画蛇添足**。

### 2.2 事件推送模式对照

| 模式 | `subagent:finished`（现有） | C2 设计的 `review-state-updated` |
|---|---|---|
| 发送侧 | `agent/subagent/sink.rs:349 emit_subagent_finished` → `AppHandle::emit("subagent:finished", payload)` 或 `HttpSseSink` | 待定（Phase 0.2） |
| 订阅侧 | `stores/subagentRuns.ts:472` `transport.listen<SubagentFinishedPayload>("subagent:finished", handler)` | `useReviewStateStore.start()` `transport.listen("review-state-updated", ...)` |
| payload 形态 | `{ runId, sessionId, status, finishedAt }` | `{ task_slug, current_round, total_findings, schema_version }` |
| store 生命周期 | `start()` idempotent + 双 unlisten 拆法；`stop()` 双 unlisten | 待设计 `start(taskSlug)` / `stop()`，单 unlisten |

**符合惯例**，与 `subagent:finished` 单事件名 + 独立 unlisten 的模式一致。

### 2.3 时序问题（必须补的链路分析）

**链路**：主 LLM 写 `<task>/review-state.json` → 后端发 `review-state-updated` → 前端 `transport.on` → 重新 `invoke('get_review_state')` 刷新。

**3 个时序问题**：

#### A. 事件早于文件落盘到达（**断裂风险**）

- write_file 是 `tokio::fs::write` 同步 await + **不原子**；如果后端在 `await` 返回**之前**就 emit 事件（因为 emit 在 await 之后发），前端 refresh 读到可能是空文件或半截 JSON。
- 现成先例：`emit_subagent_finished` 是 `run_subagent` 在 `update_run_finished` 提交**之后**才发（`agent/subagent/dispatch.rs:1192`）—— commit → emit 的顺序是约定。
- **必须约束 review-state-updated 也在文件 fsync/close 后再发**。
- 如果走维度 3.2 推荐的"review-only 工具 emit"，写盘用专用工具（原子 tmp+rename），这个问题自动消解。

#### B. 重复触发 + 失败重试

- wf-synthesize 在 revising 每轮重写整个 JSON；如果主 LLM 中途写 2 次（试错）或 cancel 后 resume 重写，事件会发多次，前端会重复 invoke。
- 缓解：
  - 前端 `useReviewStateStore` 加幂等保护（参考 `stores/subagentRuns.ts:197 eagerFetchedRunIds` 的 dedup Set）
  - 或后端 emit 前做 schema_version 对比（只在新版本号才 emit）
  - 事件 payload 带 `schema_version` 或 `current_round` + `revision_hash`，前端只在新版本才 refresh

#### C. 主 LLM 写非 review-state.json 路径

- write_file 通用钩子方案要校验：路径后缀是 `review-state.json` + 父目录是 `<task_slug>/` + workflow 是 review（防 dev session 误触发）。
- **强烈倾向 transition 钩子方案或 review-only 工具方案**（见维度 3.2）。

### 2.4 `useReviewStateStore` 生命周期

设计 §6 store 已有 `start(taskSlug)` + `stop()` + `unlisten` 清理。**风险点**：

- **`task_slug 变化时重新订阅**（设计 §6「生命周期」第二段）**：当前 store 设计只暴露 `start(taskSlug)`，没暴露 `restart(oldSlug, newSlug)` 或自动检测 slug 变化的逻辑。
- 风险：ChatPanel 切换 session（同一个 review plugin 但 slug 变）→ start 第二次 → 旧 unlisten 漏清 → **两个 unlisten 同时活着，事件触发两次 refresh**。
- 修复（参考 `subagentRuns.ts:436-481` 的 start idempotent）：
  ```typescript
  async function start(taskSlug: string): Promise<void> {
    if (currentSlug === taskSlug && unlisten) return; // idempotent
    stop();  // 先清旧的
    currentSlug = taskSlug;
    unlisten = await transport.listen<ReviewStateUpdatedPayload>("review-state-updated", ...);
    await refresh(taskSlug);
  }
  ```
- **必加** `currentSlug` 守门 + start 前先 stop 的幂等模式。

### 2.5 维度 2 总结

- IPC 双路径（command + route）：**符合 `list_workflow_plugins` 模式**，无问题。
- 事件推送：**符合 `subagent:finished` 模式**，但需补 3 条：
  1. 文件 close 后再 emit（或用 review-only 工具自带原子写）
  2. dedup（payload 带 schema_version 或 revision_hash）
  3. store 幂等 + `currentSlug` 守门
- **链路时序 + 重复触发 + store 重入**这 3 个风险必须在 implement Phase 2/3 显式处理。

---

## 维度 3：C2 implement Phase 0 两个待确认点的方案

### 3.1 task_slug 获取

**现状**：
- 后端有 `resolve_current_task(project_path)`（`agent/workflow/inject.rs:229`）扫 `.everlasting/tasks/*/task.json` 拿第一个 `status != Done` 的 task。
- **前端没有现成 IPC 暴露这个 slug**。现有 task IPC 只有 `create_task` + `archive_task`（`commands/task.rs:146` + `daemon/routes/task.rs:27`），没 `list_tasks` / `get_current_task`。
- `WorkflowCtx.current_task: Option<TaskJson>`（`agent/workflow/inject.rs:112`）只活在 chat_loop 的 per-turn 调用栈里，**前端不可见**。

**方案对比**：

| 方案 | 优劣 | 建议 |
|---|---|---|
| **a) 新增 `get_current_task_slug(project_id) → Option<{slug, id, title, status}>` IPC** | ✓ 单一定义，复用 `resolve_current_task`；✓ 双路径模式直接套；✓ review 视图独立 IPC，不污染其它 | **倾向方案 a** |
| b) 从 `list_workflow_plugins` 模式扩展 | ✗ 错位（plugin ≠ task） | 不建议 |
| c) 前端自己扫 `.everlasting/tasks/` 目录 | ✗ 前端不能直接做 fs 读（要走 IPC） | 不建议 |
| d) 扩 `build_workflow_ctx` 暴露 slug | ✗ `build_workflow_ctx` 是 agent loop per-turn 内部调用（`agent/chat.rs:196`） | 不建议 |

**推荐方案 a**：新增 IPC `get_current_task_slug`（`commands/review.rs` + `daemon/routes/review.rs`），inner 函数直接调 `resolve_current_task(&project_path)` 拿 `task.slug`。

**轻量优化**：同时返回 `{slug, id, title, status}`，ReviewMatrix 标题栏可直接消费（`current_round`/`total_findings` 已经在 IPC 响应里），不需二次 IPC。

### 3.2 review-state-updated 事件发送点

**先核查现状：everlasting 有 transition 钩子机制吗？**

| 机制 | 位置 | 是否适用于 workflow state？ |
|---|---|---|
| `task.json.status` 钩子（`dispatch_hook` 按 `(from, to)` match） | `agent/workflow/state.rs:183 set_task_state` + `state.rs:259 dispatch_hook` | ❌ 这是 task.json.status 的钩子，**不是 workflow state** 的钩子 |
| `WorkflowDef` | `def.rs:188` 仅 states/transitions + `validate()` | ❌ **没有 from→to 的钩子分发机制**——workflow 状态机完全靠主 LLM 在 breadcrumb 指引下"自觉"推进，Rust 端无状态机 |
| `ChatEventSink` trait | `state.rs:614` | ✅ 可用，加新方法零成本 |
| `AppHandleSink` 实现 | `state.rs:704` | ✅ 可用 |
| `HttpSseSink` 实现 | `daemon/sse.rs` | ✅ 可用 |
| `request_task_state_transition` 工具 | `tools/request_task_state_transition.rs` | ❌ 用于 task.json.status，不是 workflow state |

**所以：everlasting 没有 workflow state transition 的钩子机制。**

**方案对比**：

| 方案 | 优劣 | 建议 |
|---|---|---|
| i) workflow transition 钩子（review 专用） | ✗ **当前不存在**——要扩 `WorkflowDef` + 加 dispatcher + 扩 dev 走 happy path；改 engine 表面太大；C2 单任务不应改 engine 表面 | **不建议** |
| ii) write_file 工具后识别（通用） | ✓ 零 engine 改动；✗ 需校验路径 + workflow context + 防 dev 误触发；✗ write_file 失败被吞时不会 emit | 中性 |
| **iii) C2 专用轻量钩子——review-state.json 写后由 review plugin 自管** | ✓ review 专用，零 dev 污染；✓ 利用现有 `ChatEventSink`（`state.rs:614`），加新方法 `emit_review_state_updated(payload)` + `AppHandleSink`/`HttpSseSink` 实现；✓ 写入点 wf-synthesize skill 指引主 LLM 调专用工具；✓ 可一并解决维度 1.3 write_file 不原子问题（工具内部 tmp+rename） | **推荐** |

**推荐方案 iii**：C2 在 tools 层新建 review 专用工具 `emit_review_state_updated(task_slug: String)`。

落地路径：
1. `tools/emit_review_state_updated.rs` 新建（仿 `tools/ask_user_question.rs` 模式：拿 `Arc<dyn ChatEventSink>` + 简单 `app.emit`）。
2. `state.rs:614 ChatEventSink` trait 加默认 no-op 方法 `emit_review_state_updated(&self, _payload: &ReviewStateUpdatedPayload)`（与 `emit_tool_question` 同款默认静默 no-op）。
3. `AppHandleSink`（`state.rs:704`）实现转发 `app.emit("review-state-updated", payload)`。
4. `HttpSseSink`（`daemon/sse.rs`）镜像实现，转 SSE 广播。
5. `tools/mod.rs::builtin_tools()` 注册；**只在 review plugin 的 workflow 中可见**（用 `tools/mod.rs:223 filter_tools_for_workflow` 同款 gate）。
6. wf-synthesize skill 文案：「写完 review-state.json 后必须调 `emit_review_state_updated` 工具（task_slug, current_round, total_findings, schema_version）」。
7. 工具内部走原子写：先写 `.tmp` 再 `rename`（仿 `agent/workflow/task.rs:373 write_task`）。

**比方案 ii 干净的原因**：write_file 方案要校验"路径是 `<task>/review-state.json`"+"workflow context 是 review"——这两个 check 在 write_file 里都是新加逻辑，会让通用工具承担"插件感知"的逻辑。工具 iii 走 plugin-specific tools 列表（`filter_tools_for_workflow`），是 dev 之外的 plugin 也能复用的模式。

**关键校正**：C2 design.md §2 + §10.2 + implement.md Phase 0.2 都说"倾向 transition 钩子方案"——**这个倾向是错的**，transition 钩子不存在。必须改为方案 iii。

### 3.3 维度 3 总结

- **task_slug**：方案 a（新 IPC `get_current_task_slug`，返回 `{slug, id, title, status}`）—— 必走。
- **事件发送点**：方案 iii（新建 review-only `emit_review_state_updated` 工具，零 engine 改动 + 自带原子写）—— 必走，**且因为 transition 钩子不存在，按 C2 design.md §2 倾向的"transition 钩子"是错的**。C2 design.md §2 / §10.2 + implement.md Phase 0.2 必须改方案。

---

## 三维度要点回顾

| 维度 | 结论 | 必修项 |
|---|---|---|
| **1. Schema ↔ TS 类型** | 主体对齐 | ① RunStatus 扩到 5 值（C3 R7）② C3 §4 修正 write_file 原子性假设（不原子）③ C3 §4 文字澄清无顶层 status/source_run_id |
| **2. IPC/事件复用** | 模式正确（仿 `list_workflow_plugins` + `subagent:finished`） | ① 文件 close 后再 emit（或用 review-only 工具自带原子写）② 事件 payload 加 schema_version/revision_hash 供 dedup ③ `useReviewStateStore.start` 加 `currentSlug` 守门 + idempotent |
| **3. Phase 0 方案** | task_slug 用方案 a；事件发送点用方案 iii（**否决 design.md §2 的 transition 钩子倾向**） | 新增 `get_current_task_slug` IPC + 新建 review-only `emit_review_state_updated` 工具；同步修 design.md §2 / §10.2 + implement.md Phase 0.2 文字 |

---

## 代码锚点核对（用于修正错误行号）

| 声称 | 实际 | 状态 |
|---|---|---|
| `db/migrations.rs:644` final_text | 准（644 `add_subagent_runs_column_if_missing(... "final_text", ...)`） | ✅ |
| `commands/sessions.rs:500` `list_workflow_plugins` | 待 Phase 0.1 read 时精确核对 | 🔎 |
| `daemon/routes/sessions.rs:245` `list_workflow_plugins` route | 待精确核对 | 🔎 |
| `stores/subagentRuns.ts:472` `subagent:finished` listener | 待精确核对 | 🔎 |
| `tools/write_file.rs:163` `tokio::fs::write` | 准（`match tokio::fs::write(&validated, content).await`） | ✅ |
| `agent/workflow/task.rs:373` `fs::rename` 原子写 | 准 | ✅ |
| `db/subagent_runs.rs:34/70-126` status 5 值 CHECK 约束 | 准（CHECK `('running','completed','cancelled','error','incomplete')`） | ✅ |
| `agent/workflow/state.rs:183 set_task_state` 钩子 | 准（task.json.status 钩子，非 workflow state） | ✅ |
| `agent/workflow/inject.rs:229` `resolve_current_task` | 待 Phase 0.1 read 时精确核对 | 🔎 |
| `agent/chat.rs:196` `build_workflow_ctx` | 待精确核对 | 🔎 |
| `tools/mod.rs:223` `filter_tools_for_workflow` | 待精确核对 | 🔎 |
| `daemon/sse.rs` `HttpSseSink` | 待精确核对 | 🔎 |

---

## 相关文件路径

- C3 PRD R7 + design §4：`.trellis/tasks/07-26-review-plugin-pack/{prd.md,design.md}`
- C2 design §2 / §7：`.trellis/tasks/07-26-review-viz/design.md`
- C2 implement Phase 0：`.trellis/tasks/07-26-review-viz/implement.md`
- 待 read（Phase 0 验证）：
  - `app/src-tauri/src/agent/workflow/inject.rs`（resolve_current_task）
  - `app/src-tauri/src/commands/{sessions.rs,task.rs,review.rs}`（IPC 注册模式）
  - `app/src-tauri/src/daemon/routes/{sessions.rs,task.rs,review.rs,mod.rs}`
  - `app/src-tauri/src/agent/workflow/state.rs`（transition 钩子机制核查）
  - `app/src/stores/subagentRuns.ts`（start idempotent 模式参考）
  - `app/src-tauri/src/tools/{ask_user_question.rs,request_task_state_transition.rs}`（review-only 工具模板）
  - `app/src-tauri/src/daemon/sse.rs`（HttpSseSink 实现位置）