# review plugin intake 可用性修复 (C4)

> 父任务：`07-26-workflow-review-plugin`（review epic，第 5 个 child，C4）
> 依赖：**C3（review plugin pack）** 已 archive。本任务消费 C3 落地的 `workflow.json` + builtin 化机制，修它在真实 E2E 里暴露的可用性缺口。
> 来源：session `99866757-d810-4c3e-8655-c2d700a8f773`（2026-07-27 09:15–09:36）的 dogfooding，主 LLM = MiniMax-M3，在 review plugin session（`workflow_enabled=1`, `plugin_name=review`）里完全迷失。

## Goal

让 review plugin 在 **intake 阶段对主 LLM 可用**：模型能明确知道自己 (a) 在 review 流程、(b) 当前在 intake state、(c) 有哪些专属工具（`create_task` / `dispatch_subagent` / `request_task_state_transition`）、(d) 该按什么顺序走。

E2E 验收标准：在 `pnpm tauri dev` 下新建一个 review plugin session，主 LLM 在第一轮就能从 breadcrumb 读出"intake + 可用模型来自 dispatch_subagent 的 model enum"，**不需要**用 shell curl daemon 或直读 SQLite 去摸 catalog。

## Background（E2E 暴露的真实问题）

### 事实链（来自 session 99866757）

1. **session 配置正确**：`workflow_enabled=1`、`plugin_name=review`、`current_task=None`（project 下无 `.everlasting/tasks/`）。
2. **主 LLM 行为**：seq=1→19 在漫无目的地读 `ROADMAP.md` / `BACKLOG.md` / git log，完全没意识到自己在 review 流程；seq=21→29 反复 `ask_user_question`（被用户 cancelled 两次）；seq=33→47 花了 7 轮 shell 才摸清 catalog 只有 4 个模型；seq=49 thinking 里写下"我看不到 create_task tool 在我当前工具列表"后输出 `[已停止]`，**用户手动停止**。
3. **根因不是模型蠢**：是注入给模型的上下文**没有告诉它这些信息**。

### P0 bug：breadcrumb state 硬编码 fallback

`app/src-tauri/src/agent/workflow/inject.rs:599-605`：

```rust
let state_str = ctx
    .current_task
    .as_ref()
    .map(|t| t.status.as_str())
    .unwrap_or("planning");   // ← dev plugin 的初始态，不是 review 的
let breadcrumb = breadcrumb_for(&ctx.workflow_def, state_str);
```

`breadcrumb_for`（`def.rs:276`）是 `def.breadcrumb.get(state).unwrap_or("")`。review plugin 的 states 是 `intake/reviewing/revising/reported`，**没有 `planning` 键**，所以 `current_task=None` 时 breadcrumb 渲染成**空字符串**。

LLM 每轮在 `messages[0]` 实际看到的注入块（复刻自 `inject.rs:635-641`）：

```
<workflow-task-meta>
no active task — call the create_task tool to start one...
</workflow-task-meta>
                      ← breadcrumb 这里是空的！review 流程指引一字未提
```

对 dev plugin 恰好成立（`planning` 是它的初始态），对 review plugin 直接失效。

### P1：workflow-task-meta 不暴露专属工具

`inject.rs:635-641` 的 `None` 分支只说"call the create_task tool"，没告诉模型：
- 当前 plugin 是 review（4-state 流程）
- 当前在 intake state
- 它还有 `dispatch_subagent`（model enum = catalog）、`request_task_state_transition`、`update_checklist` 这些专属工具

模型因此把 review plugin session 当成普通 chat session 用，靠通用 shell/read_file 摸索。

### P1：catalog 可见性路径没点明

review plugin 的 intake breadcrumb（C3 写的）原话是"从 dispatch_subagent 的 model enum 看可用模型"。但 MiniMax-M3 在 E2E 里**忽略了 enum**，转而 `curl http://localhost:7456/api/v1/models`（404，路由没注册）→ 直读 SQLite。需要验证 enum 是否真的反映了 catalog，并在 breadcrumb 里强提示"**不要**用 shell 查 models"。

## Requirements

### R1. 修 breadcrumb state fallback（P0 bug）

`inject.rs:604` 的 `unwrap_or("planning")` 改成查 `ctx.workflow_def.initial`（每个 plugin 自己声明的初始态）：

```rust
let state_str = ctx
    .current_task
    .as_ref()
    .map(|t| t.status.as_str())
    .unwrap_or(ctx.workflow_def.initial.as_str());
```

**验收**：单测覆盖 review plugin 的 `None` 分支，断言渲染出的 breadcrumb 包含 intake 的指引文本（而不是空串）。dev plugin 的 `None` 分支行为不变（`initial="planning"`，回归通过）。

### R2. workflow-task-meta 暴露 plugin + state + 专属工具（P1）

`inject.rs:627-643` 的 `None` 分支扩展，让模型在 bootstrap 阶段就看到：

```
<workflow-task-meta>
plugin: review
state: intake (initial)
tools you have (workflow-only): create_task, dispatch_subagent, request_task_state_transition, update_checklist
no active task — call create_task to start one.
</workflow-task-meta>

<breadcrumb for intake state>
```

**验收**：注入块里出现 `plugin: review` + `state: intake` + 至少 3 个 workflow-only 工具名。

### R3. 验证 + 强化 dispatch_subagent 的 model enum 可见性（P1）

- **验证**：`dispatch.rs` 里 `definition_with_cache` 合成的 model enum 是否真把 catalog 所有模型都列进去了（E2E 里 catalog 有 4 个模型跨 2 provider）。
- **强化**：review plugin 的 intake breadcrumb（`.everlasting/workflow/review/workflow.json`）把"从 dispatch_subagent 的 model enum 看可用模型"改成更强提示，例如："**可用模型见 dispatch_subagent 的 model 参数 enum（不要用 shell/SQLite 查 models）**"。

**验收**：enum 覆盖 catalog 全部模型；breadcrumb 含明确的"不要用 shell"指引。

### R4. ask_user_question cancelled 的处理（P2，探查性）

E2E 里 seq=23 / seq=29 两次 `ask_user_question` 返回 `{"cancelled":true}` 后，模型输出 `[已停止]`（5 字符收尾），用户体验像"AI 罢工"。排查：
- cancelled 后模型是否应该等待用户下一条自然语言输入，而不是直接收尾？
- tool result 是否需要附更明确的语义（如 `reason: user_interrupted`）？

**探查结论**（2026-07-27 实施时确认）：

代码里 cancelled 有两种语义（`ask_user_question.rs:372-433`）：

| 返回值 | isErr | 语义 | 触发 |
|---|---|---|---|
| `{"cancelled": true}` | **true** | 用户点了"跳过"（PRD §R5 的 skip） | `InteractionResponse::Cancelled` (line 416-421) |
| `{"cancelled_by_session": true}` | true | session 级 cancel（Stop / 关 app） | `cancel.cancelled()` (line 374-388) 或 `RecvError` (line 422-431) |

E2E 里 session 99866757 的两次都是 `{"cancelled":true}`（用户跳过），不是 session cancel。**问题根因**：用户"跳过"的语义是"我不想回答这个，你继续/换方式"，但 `isErr=true` + 模型训练里的强关联导致 MiniMax-M3 把它当致命错误，直接输出 `[已停止]` 罢工。

**处理方案**（本任务范围：决策 + 沉淀，不在本 PR 改）：
- **方案 A（推荐，另开 task）**：把 `InteractionResponse::Cancelled` 的 `is_error` 从 `true` 改成 `false`，content 加 `{"cancelled": true, "reason": "user_skipped", "hint": "用户主动跳过此问题，请用其他方式继续或直接做决定"}`。session-cancel 保持 `isErr=true`。需同步改 3 处测试断言（line 507/547/564）。
- **方案 B**：保留 isErr，在 system prompt / 工具 description 里强化"`ask_user_question` 返回 cancelled 不是错误"。
- 倾向 A，但属于跨 plugin 的 agent 行为改动，不宜塞进 review plugin 的 C4。**记为 follow-up**：`ask-user-question-skip-semantics`。

**验收**：方案记入 PRD + 标记 follow-up（已完成）。本任务不动 ask_user_question.rs。


## Non-goals

- **不改 review plugin 的 4-state 设计**（intake/reviewing/revising/reported 不动，那是 C3 的决策）。
- **不改 reviewer 派发的真实路径**（chat_loop 的 `DispatchBatch::Concurrent`，那是 reviewing state 的事，不是 intake）。
- **不做矩阵视图的 GUI 改动**（C2 的范围）。
- **不换主 LLM 模型**（MiniMax-M3 是配置选择，本任务让 breadcrumb 对所有模型都清晰，而不是针对某一模型优化）。

## Open questions（PRD review 时拍板）

1. **R2 的工具列表来源**：硬编码 4 个工具名，还是从 `filter_tools_for_workflow` 反查？硬编码简单但会漏自定义 plugin 的工具；反查更通用但 `None` 分支时 plugin 已知、可静态推。倾向硬编码 + 注释指向 `filter_tools_for_workflow`。
2. **R4 的范围**：是否本任务只记决策、实施放到下一个 task？倾向本任务做最小修（tool result 附 `reason`），product 层面的输入框状态机另开。
