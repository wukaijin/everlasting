# 定向评审：C2↔C3 契约 + C2 前端可行性

> 评审对象：C3 design.md §4 (schema+写入流程)、C2 design.md §2/§5/§7、C2 implement.md Phase 0
> 评审维度：schema 对齐 / 前端机制可行性 / Phase 0 待确认点

---

## 维度 0：方法论前提 —— code fact check

评 C2/C3 的 feasibility 之前，先验证几个 design/implement 引用的代码事实是否正确。以下是通过 grep/read 从当前代码库核实的结论：

| design.md 引用 | 声称是什么 | 实际情况 | 影响 |
|---|---|---|---|
| C3 design §0 表行 1 | `skill/loader.rs:463` `if workflow_name != "dev"` | ✅ 确认存在，`builtin_plugin_skills` 确实是硬编码 dev 分支 | C3 扩展点正确 |
| C3 design §0 表行 5 | `dispatch_subagent` 的 `model` enum 在 `subagent/mod.rs:381` | ⚠️ 位置描述不精确，实际在 `chat_loop.rs:681-708` (`model_briefs` 快照构建后喂给 `definition_with_cache`) | 不影响设计，但 implement 时 find 不到会浪费时间 |
| C2 design §0 表行 1 | 前端能感知 `workflow_enabled` + `plugin_name` — `stores/chat.ts:1791/1819` | ✅ 确认存在，`setWorkflowEnabled` / `setPluginName` 两个 public facades | C2 视图门控条件 `workflow_enabled && plugin_name=="review"` 立即可用 |
| C2 design §0 表行 3 | `transport.on` 参考 `stores/subagentRuns.ts:177-185` | ✅ 确认存在，`transport.listen<SubagentEventPayload>("subagent:event", ...)` 和 `"subagent:finished"` | 前端事件订阅模式正确 |
| C2 design §0 表行 5 | `finding.source_run_id` 指向 `subagent_runs.id`，跳转走现有 `get_subagent_run` IPC | ✅ 确认存在，`db/subagent_runs.rs` 有 `get_run(run_id)` 返回完整 `SubagentRunRow`（含 `final_text`） | source_run_id 跳转链路可复用 |
| C2 implement Phase 0.2 | "read 点：`app/src-tauri/src/agent/workflow/state.rs`（是否有 transition 钩子机制）" | ❌ **重大误读**：`state.rs` 的 `set_task_state` + `dispatch_hook` 是 **task.json 状态机**（Planning→InProgress→Done），与 workflow **plugin session 状态机**（intake→reviewing→revising→reported）是完全不同的两个系统。前者通过 `task.json` 中 `status` 字段管理 task 生命周期，钩子做 spec distillation 等；后者在 agent loop breadcrumb 注入中按 workflow.json 的 transitions 管理 session 流转。**`state.rs` 里没有、也不会为 workflow plugin 的 session state 转移提供钩子。** | **C2 implement Phase 0.2 的根本前提是错的。** 见维度 3 详细分析。 |
| C2 implement Phase 0.2 降级方案 | "write_file 写 review-state.json 后发事件" | ❌ write_file 工具 (`tools/write_file.rs:46`) 的 `ToolContext` 不含任何 event_sink；write_file 的 execute 函数返回 `(String, bool)` 纯文本，完全无副作用管道 | write_file hook 方案同样不存在，需新建机制 |
| C2 design §2 | "`list_workflow_plugins` IPC 已存在，仿此模式新建 IPC" | ✅ `commands/sessions.rs:500` / `daemon/routes/sessions.rs:245` 双路径模式 | C2 的 `get_review_state` IPC 有明确范本，正确 |

**关键发现**：C2 implement Phase 0.2 列的两个方案（transition 钩子 vs write_file 识别）都基于对代码的错误假设。**两者在当前代码库中都不存在，且都不是 trivial 的"接一下"改动。** review-state-updated 事件发送需要从零设计一个新的 event emission path。这是 C2 最大的实施风险，远超过 implement.md 列的 R1/R2。

---

## 维度 1：C3 schema 与 C2 TS 类型对齐

### 结论：已对齐，无断裂。但有一处文档漂移 + 一处隐式假设。

### 1.1 逐字段比对

**顶层 (ReviewState)**：

| C3 schema 字段 | C3 来源 | C2 TS 类型 | 对齐？ |
|---|---|---|---|
| `schema_version: string` | design §4 "PRD R7 已给" | `schema_version: string` | ✅ |
| `task_id: string` | design §4 | `task_id: string` | ✅ |
| `current_round: number` | design §4 | `current_round: number` | ✅ |
| `rounds: ReviewRound[]` | design §4 | `rounds: ReviewRound[]` | ✅ |

**ReviewRound**：

| C3 字段 | C2 TS | 对齐？ |
|---|---|---|
| `round: number` | `round: number` | ✅ |
| `dimensions: string[]` | `dimensions: string[]` | ✅ |
| `models_present: string[]` | `models_present: string[]` | ✅ |
| `models: Record<model_id, ModelVerdict>` | `models: Record<string, ModelVerdict>`（注释 key=model_id） | ✅ |
| `change_log?: string[]` | `change_log?: string[]` | ✅ |
| `convergence_note?: string` | `convergence_note?: string` | ✅ |

**ModelVerdict**：

| C3 字段 | C2 TS | 对齐？ |
|---|---|---|
| `model_display: string` | `model_display: string` | ✅ |
| `run_id: string` | `run_id: string` | ✅ |
| `status: "completed" \| "failed" \| "timed_out" \| ...` | `status: RunStatus` (`"completed" \| "failed" \| "timed_out" \| "cancelled" \| "truncated"`) | ✅ 枚举值一致 |
| `verdict: "pass" \| "pass_with_minor" \| "revise" \| "reject"` | `verdict: Verdict`（同枚举） | ✅ |
| `summary?: string` | `summary?: string` | ✅ |
| `findings: ReviewFinding[]` | `findings: ReviewFinding[]` | ✅ |

**ReviewFinding**：

| C3 字段 | C2 TS | 对齐？ |
|---|---|---|
| `finding_id: string` | `finding_id: string` | ✅ |
| `dimension: string` | `dimension: string` | ✅ |
| `severity: "critical" \| "high" \| "medium" \| "low" \| "info"` | `severity: Severity`（同枚举） | ✅ |
| `issue: string` | `issue: string` | ✅ |
| `suggestion?: string` | `suggestion?: string` | ✅ |
| `location?: string` | `location?: string` | ✅ |
| `source_run_id: string` | `source_run_id: string` | ✅ |
| `triage?: { decision, reason }` | `triage?: { decision: TriageDecision, reason: string }` | ✅ decision 枚举 `adopt/reject/defer` 一致 |

**枚举值一致性**：Verdict (`pass`/`pass_with_minor`/`revise`/`reject`)、Severity (`critical`/`high`/`medium`/`low`/`info`)、RunStatus (`completed`/`failed`/`timed_out`/`cancelled`/`truncated`)、TriageDecision (`adopt`/`reject`/`defer`) — 全部对齐。

### 1.2 文档漂移：PRD vs design.md

C3 PRD R7 仍然展示的是旧版 schema（`{rounds: [{round, models: {<model>: {verdict, findings: [{dimension, severity, issue, location}]}}}], dimensions: [...]}`），不含 `schema_version`/`task_id`/`current_round`/`finding_id`/`source_run_id`/`triage`/`change_log`/`convergence_note`/`status`/`models_present` 等扩展字段。同样 C2 PRD R1 也展示旧版。

**C3 design.md §4 声称 "PRD R7 已给 schema（含 schema_version/... 等全部字段）"——这是个虚假声明。** PRD R7 的实际文字仍是旧版，扩展字段只存在于 design.md。

**风险**：如果有人只看 PRD 不看 design.md 实施（或者反过来），会拿到不同版本的 schema。建议 C3 PRD R7 同步更新到扩展版 schema，保持单一 source of truth。

### 1.3 隐式假设：写入方的 JSON 格式

C3 design §4 说：
> "主 LLM 须严格按 schema 写字段名/枚举值"
> "skill 强调「这是机器读取文件,不能加 markdown 包裹或注释」"

这两个约束是对的，但有一个 gap：**Rust serde 序列化 JSON 的行为和 LLM "写 JSON" 的行为有差异。** Rust 的 `serde_json::to_string_pretty` 会做 Unicode 转义（如 `\uXXXX`），LLM 直接输出中文则不会。如果 C3 后端用 serde 写（C3 design §5 是资源包，不涉及 Rust 写代码；写入方是**主 LLM 通过 write_file 工具**，即纯文本输出），而 C2 前端用 `JSON.parse()` 读——两者对 JSON 的容忍度不同：
- LLM 可能写 trailing comma → `JSON.parse()` 会爆
- LLM 可能输出裸中文（合法 JSON）→ 没问题
- LLM 可能在某些 string 里嵌套双引号未转义 → `JSON.parse()` 会爆

**建议**：C2 前端解析 review-state.json 时，除了 `try-catch JSON.parse()`，还应考虑用更宽松的 JSON 解析器（如 `json5`），或者 C3 wf-synthesize skill 里加一条「输出后用 write_file 写入前，先检查 JSON 合法性」——但主 LLM 没有 JSON validator 工具。最务实的做法是前端用 `json5` 或手写一个容错解析（去掉 trailing comma、注释）。

---

## 维度 2：C2 前端机制在 everlasting 架构里能否跑通？

### 2.1 IPC：`get_review_state` — 可行 ✅

参考 `list_workflow_plugins`（`commands/sessions.rs:500` + `daemon/routes/sessions.rs:245`）的双路径模式（Tauri command + daemon route），C2 的 `get_review_state(task_slug) → Result<ReviewStatePayload>` **完全符合现有惯例**。

具体：
- `list_workflow_plugins` 的 IPC 函数签名是 `(project_path: String) → Result<Vec<String>, AppCommandError>`，C2 的 `(task_slug: String) → Result<ReviewStatePayload, AppCommandError>` 结构一致 ✅
- daemon route 用 `POST /api/v1/review/get_review_state`（implement Phase 1.2），与其他 routes 同模式 ✅
- 三态返回（State/Missing/Invalid）在现有 IPC 中有先例——`read_task` 等很多函数都返回 `Result<Option<T>, Error>` 形态 ✅

### 2.2 事件：`review-state-updated` — 有问题 ⚠️

这是 C2 设计中最薄弱的环节。让我逐层分析。

**现有事件系统的结构**（从 daemon/sse.rs 核实）：
- 所有实时事件通过 `ChatEventSink` trait 或 `SubagentEventSink` trait 发出
- 这些 trait 的方法签名是**类型化的**：`emit_chat_event(&self, payload: &ChatEventPayload)`——它只接受 `ChatEventPayload` enum，不接受任意 JSON
- Daemon 的 `HttpSseSink` 把事件转成 `SseFrame { name, data }` 推入全局 `SseRegistry`
- `SseRegistry` 是一个 `Vec<Sender<SseFrame>>` 广播队列——任何连接 `/api/v1/stream` 的前端都收到所有事件
- 前端 `transport.listen("subagent:event", callback)` 靠 **event name 字符串匹配** 过滤——`httpTransport.listen` 内部 `EventSource.onmessage` 解析 `event:` 字段后路由

**问题**：`review-state-updated` 事件要加入这条管道，需要：
1. 有一个新的 event name（如 `"review-state-updated"`）
2. 有一个 Rust 端调用点来发出这个事件
3. 这个调用点能访问 `ChatEventSink` 或等价的 event emission 通道

**当前代码库里没有一个通用 "emit arbitrary event" 的 API。** `ChatEventSink` trait 只有 `emit_chat_event(ChatEventPayload)` 一个方法。要加新事件类型，要么：
- **(a)** 给 `ChatEventPayload` enum 加一个新 variant（如 `ReviewStateUpdated { task_slug, current_round, total_findings }`）——这意味着这个事件会走 chat 事件的 SSE 通道，前端通过 event name 过滤
- **(b)** 新增一个独立的 trait 方法或 channel——更干净，但改动面更大

**(a) 方案最务实**：`ChatEventPayload` 已有 7+ 种 variant（chat delta/start/done/error + tool_call + tool_result + permission_ask 等），加一个 `ReviewStateUpdated` variant 是最小改动。前端 `transport.listen("review-state-updated", ...)` 不会与其他 handler 冲突，因为事件分发靠 name string。

### 2.3 事件推送的时序问题

C2 design §2 说：
> "主 LLM 写完 review-state.json 后,后端发事件 `review-state-updated` → 前端 `transport.on` 收到 → 重新调 `get_review_state` 刷新"

这是标准的 **write-then-notify** 模式，但没有考虑以下时序问题：

**问题 1：并发写入 vs 事件到达**
如果主 LLM 在 revising 阶段短时间内写了两次 review-state.json（比如第一次写错了，立刻修正再写一次），前端收到两波 `review-state-updated` 事件。第二波事件触发 `get_review_state` 时，文件可能已被第二次写入覆盖，但第一次事件的 `current_round`/`total_findings` 可能与实际读取的内容不一致。**这不是 bug**（前端最终展示的是最新文件内容），但 implement 时事件 payload 里的 `current_round`/`total_findings` 应该只作**提示/摘要**，前端不应依赖它做任何状态判断——永远以 `get_review_state` 返回的实际文件内容为准。

**问题 2：事件早于文件落盘**
如果事件在 `write_file` 返回之前就发出（这在 Rust async 中可能发生，如果事件发送和文件写入不在同一个 await point），前端 `get_review_state` 可能读到旧文件或文件不存在。**缓解**：事件发送必须在文件写入**之后**。如果事件在 agent loop 的 workflow transition 处发（而非 write_file 工具内），需要确保 transition 发生在 write_file 完成之后——这在 agent loop 中天然成立，因为工具执行是同步的（LLM 调 write_file → agent 执行 → 结果回填 → 下一轮 LLM 输出，transition 在工具执行后的特定时刻）。

**问题 3：重复触发**
前端 `transport.listen` 返回的 `unlisten` 需要在 store 的 `stop()` 中调用（C2 design §6 已规划），防止组件卸载后残留监听。✅ 这个在现有 `useSubagentRunsStore` 中有先例，模式正确。

### 2.4 `useReviewStateStore` 生命周期

C2 design §6 的 store 设计：
```typescript
function start(taskSlug: string) { transport.on("review-state-updated") → refresh; refresh(taskSlug); }
function stop() { unlisten?.(); }
```

**审计**：
- `task_slug` 变化时需要 `stop()` + `start(newSlug)` ✅——implement §5.1 提到"切 session 时 stop()"
- `refresh()` 调 `invoke("get_review_state", taskSlug)`——这是一个 async 调用，如果组件在 promise resolve 前卸载，`state.value` 赋值可能触发已卸载组件的响应式更新。**在 Vue 3 + Pinia 中这不是问题**——Pinia store 的 ref 赋值不依赖组件生命周期，已卸载组件不会报错（只会产生一个无观察者的响应式变更）。✅
- **孤儿监听风险**：如果 `stop()` 没被调用（组件卸载时 `onUnmounted` 漏了），`unlisten` 永远不执行 → 后端每次发事件都会触发 `refresh` → 每次 `refresh` 都调 `invoke`。这在长期运行的 daemon 中会累积无效 IPC 调用。**缓解**：`start()` 中加一个 guard `if (unlisten) return;`（idempotent），确保重复 `start()` 不会创建多个监听。这个在 implement.md 没提，但 `useSubagentRunsStore` 的 `mountListener`（`subagentRuns.ts:423`）也没有这个 guard——所以现有代码也面临同样问题，不是 C2 独有的。

**结论**：store 生命周期设计整体 OK，孤儿监听风险是现有系统级别的技术债，不应该是 C2 的阻塞项。但 C2 implement 时应在 `start()` 里加 idempotent guard（`if (unlisten) return;`），比现有 `subagentRuns.ts` 做得更好。

---

## 维度 3：C2 implement Phase 0 的两个待确认点

### 3.1 task_slug 获取

**现状核实**：
- 后端有 `resolve_current_task(project_path) → Option<TaskJson>`（`inject.rs:229`），返回第一个非 Done/Completed 的 task。`TaskJson` 有 `slug: String` 字段。
- 前端 chat store 暴露了 `workflow_enabled` 和 `plugin_name`，但**没有暴露 `task_slug`**。现有 IPC 里也没有 `get_current_task`。

**三个方案**：

| 方案 | 描述 | 优点 | 缺点 |
|---|---|---|---|
| **(a) 新增 `get_current_task_slug` IPC** | 单用途 IPC，调用 `resolve_current_task` 返回 `Option<String>` | 最小改动、语义清晰、不影响现有 IPC | 新增 IPC + daemon route，约 30 行代码 |
| **(b) 从 session metadata 暴露** | 在 `SessionSummary` / `SessionRow` 加 `current_task_slug` 字段，随 session 加载自动填充 | 前端不需要额外 IPC 调用，数据已在首次加载时获取 | `current_task` 是**实时查询**（每次 resolve_current_task 都重新扫 tasks 目录），而 session 是在创建时快照的。如果 task 在 session 运行期间新增/完成，session metadata 里的 task_slug 就过时了。❌ **不可行**——resolve_current_task 的语义是"当前时刻第一个未完成 task"，不能缓存到 session 创建时。 |
| **(c) 从 workflow state breadcrumb 解析** | 主 LLM 的 breadcrumb 里可能包含 task slug（如 `当前 task: my-feature`），前端从 chat messages 里解析 | 零后端改动 | 解析自由文本不可靠；不是所有 message 都包含 task slug；需要在 message stream 中持续追踪。❌ **不可靠** |

**推荐方案 (a)**：新增 `get_current_task_slug` IPC。签名：
```rust
// Tauri command
pub async fn get_current_task_slug(state: State<AppState>) -> Result<Option<String>, AppCommandError> {
    let project_path = state.project_path()?;
    let task = crate::agent::workflow::inject::resolve_current_task(&project_path).await;
    Ok(task.map(|t| t.slug))
}
```

这是干净的只读查询，不修改状态，延迟 <1ms（纯目录扫描 + JSON 解析），不需要缓存。

**关于 implement.md 的「方案 b：从现有 workflow state IPC 暴露」**：workflow state IPC（如 `list_workflow_plugins`）返回的是**项目级别**的 plugin 列表，与当前 session 绑定的 task 无关。不存在"现有 workflow state IPC"能暴露 task_slug。这个方案不可行，应直接删除，避免 implement 时浪费时间调研。

### 3.2 review-state-updated 事件发送点

这是 Phase 0 最关键的决策。先澄清代码事实，再给方案。

**代码事实**：

`app/src-tauri/src/agent/workflow/state.rs` 的 `set_task_state` + `dispatch_hook` 管理的是 **task.json 的 `status` 字段**（`Planning → InProgress → Done`），这是 task 级别的生命周期。钩子做 spec distillation / preflight check 等 task 管理自动化。

**workflow plugin session 的状态转移**（如 review 的 `intake → reviewing → revising → reported`）是另一个维度的东西——它定义在 `workflow.json` 的 `transitions` 数组里，由 agent loop 在 per-turn breadcrumb 注入 + `resolve_task_state_transition` IPC（用户确认转移请求）驱动。**这个流转完全不走 `set_task_state`**。

换言之：**C2 implement Phase 0.2 设想的"transition 钩子"在 workflow plugin session state 层面不存在。**

现在来给实际可行的方案：

| 方案 | 描述 | 优点 | 缺点 |
|---|---|---|---|
| **(a) 在 agent loop 的 workflow state transition 处发事件** | 在 `chat_loop.rs` 中处理 workflow plugin state transition 的位置（即 `resolve_task_state_transition` IPC handler 的后端逻辑，在 state 转移生效后）发 `review-state-updated` 事件 | 天然在文件写入之后、state 转移之时；review 专用判断简单（`if plugin_name == "review" && new_state ∈ {reviewing, revising, reported}`） | 需要找到 agent loop 中处理 workflow state transition 的精确位置；需要该位置有 event_sink 访问权限 |
| **(b) 在 `ChatEventPayload` 加 variant + 在 write_file 工具后识别** | write_file 执行后，如果写入的文件路径匹配 `<task>/review-state.json`，发事件 | 直接在工具层面触发，不依赖 workflow 逻辑 | write_file 的 ToolContext 目前没有 event_sink；需要修改 ToolContext 结构并传递 event_sink 给所有工具（这是一个跨所有 21 个 builtin tools 的签名变更）；路径匹配逻辑脆弱 |
| **(c) 用 polling 替代事件推送** | 前端每 N 秒调 `get_review_state`，不依赖事件 | 零后端事件改动；前端完全自主 | 延迟（N 秒内看不到最新数据）；浪费 IPC 调用 |

**推荐方案 (a)**，理由：
- 改动最小——只影响 workflow state transition 的一个代码路径
- Review 专用判断清晰——只需检查 `plugin_name == "review"`，零污染 dev
- 时序正确——state 转移发生在 write_file 完成、工具结果回填、askUserQuestion 之后，文件必定已落盘
- event_sink 在 agent loop 中已经可用（`chat_loop.rs` 持有 `event_sink: Arc<dyn ChatEventSink>`）

**实施路径**：
1. 给 `ChatEventPayload` enum 加 `ReviewStateUpdated { task_slug: String, current_round: u32, total_findings: u32 }` variant
2. 在 agent loop 的 workflow state transition 生效位置（`chat_loop.rs` 中处理 `resolve_task_state_transition` 的路径，或 per-turn 检测 state 变化的逻辑），加：如果 `plugin_name == "review"` 且新 state 是 reviewing/revising/reported，则 `event_sink.emit_chat_event(ChatEventPayload::ReviewStateUpdated { ... })`
3. 前端 `transport.listen("review-state-updated", callback)` 自动路由到此事件（因为所有 ChatEventPayload variant 序列化后都带 event name）

**不推荐方案 (b)**：修改 ToolContext 加 event_sink 会波及 21 个工具文件的签名，且 write_file 被大量 session 调用（dev 也会写文件），每调用都得检查路径——这是为 review 一个功能引入全系统的复杂度。

**关于 implement.md 的「降级为 write_file 方案」**：implement.md 把 write_file 方案列为 transition 钩子的降级路径。但实际代码显示，**write_file 方案需要改动 ToolContext（交叉切割所有工具），transition 钩子方案需要绕过对 state.rs 的误解，结果应该是相反的**：workflow session transition 处发事件是"正常方案"，write_file 识别才是"复杂方案"。

---

## 总结

| 维度 | 结论 |
|---|---|
| **维度 1：C3↔C2 schema 对齐** | ✅ **已对齐**，所有字段名/枚举值/嵌套结构一一对应。PRD 文档有漂移（还是旧版 schema），建议同步更新。唯一的技术债：前端 JSON 解析需要容错（LLM 输出的 JSON 可能不严格合标）。 |
| **维度 2：前端机制可行性** | ✅ IPC 路径完全可行（仿 `list_workflow_plugins`）。⚠️ 事件路径可行但需要澄清设计：当前没有通用事件 emission API，最务实的方案是给 `ChatEventPayload` enum 加 `ReviewStateUpdated` variant，在 agent loop 的 workflow state transition 处发出。Store 生命周期设计 OK。 |
| **维度 3：Phase 0 两个待确认点** | **(a) task_slug**：推荐新增 `get_current_task_slug` IPC（约 30 行代码）。implement.md 列的「方案 b：从现有 workflow state IPC 暴露」不可行（不存在这样的 IPC），应删除。**(b) 事件发送点**：implement.md 列的 transition 钩子方案基于对 state.rs 的错误理解（task.json state ≠ workflow session state）。正确方案是在 agent loop 的 workflow session state transition 处发 `ChatEventPayload::ReviewStateUpdated`。**write_file 方案反而是最复杂的（需改 ToolContext 跨 21 个工具）。** |
