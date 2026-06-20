# B6 Subagent 系统评估

> 评估日期: 2026-06-20 | 评估范围: B6 PR1+PR2+PR3 全部落地代码

## 评估概览

| 角度 | 评级 | 关键问题 |
|---|---|---|
| 工具配给 | 🟢 合理 | 粒度两档（OOS 已知），structural-disabled 列表充分 |
| Mode 结合 | 🟡 有缺陷 | **system_prompt 未生效**（已知 deviation），worker 行为与 prompt 矛盾 |
| SSE/流 | 🟢 合理 | 主聊天流零可见性，但 IPC 实现正确 |
| 衔接通讯 | 🟡 有缺陷 | **parent 无法访问 worker 内部状态**，error 后 parent 盲目 |
| 整体架构 | 🟢 合理 | MVP 范围内设计决策审慎，已知 deviation 和 OOS 均有文档 |

---

## 1. 工具配给 (Tool Allocation)

### ✅ 设计亮点

**双层过滤**：`SubagentDef.tools` allowlist + 无条件 `STRUCTURALLY_DISABLED` 剥离（`subagent.rs:304-310`），即使未来支持 frontmatter 自定义工具集也不会意外开启嵌套 dispatch 或 background shell。

**空 allowlist = 自维护全集**：`general-purpose` 的 `tools: &[]`（`subagent.rs:201`）被 `filter_tools_for_subagent`（`:324-328`）解释为 "builtin_tools() 减去禁用集"，未来新增 tool 时 general-purpose 自动可用而不需改定义。

**5 个结构性禁项**均具有充分理由：

| 禁项 | 原因 |
|---|---|
| `update_checklist` | worker 不能写 parent 的 progress tracker |
| `dispatch_subagent` | 禁止嵌套（MVP 单层） |
| `run_background_shell` / `shell_status` / `shell_kill` | L1a background shell 通知队列是 session 级别的，worker 如果在同一 session 启动 background shell，其完成通知会泄漏到 parent 的 next-turn drain |

### ⚠️ 问题

- **粒度只有两档**：纯只读 (`researcher` — 4 tools: `read_file`/`grep`/`glob`/`list_dir`，`subagent.rs:176`) 和近乎全能 (`general-purpose` — 全集减禁项)。缺少中等粒度（如"可写不可 shell"）。PRD 标记为 v2 OOS。
- **general-purpose + yolo = 无限制**：当 parent mode 为 yolo 时，general-purpose worker 拥有完整 toolset 且全自动执行，无确认。这是设计意图（继承 parent mode），但用户可能意识不到 worker 拥有此能力。

---

## 2. Mode 结合 (Mode Integration)

### ✅ 设计亮点

- **Mode 继承干净**：worker 通过 `PermissionContext` 继承 parent 的 Mode（Yolo/Edit/Plan），`run_subagent` 传入 `is_worker: Some(true)` 作为 `run_chat_loop` 的第 21 个 override 参数（`chat_loop.rs:2213`）。
- **Tier 4 collapse**：`is_worker=true` 时 `ask_path`/`ask_shell` → `Deny`（`permissions/mod.rs:1003-1044`），因为 worker 没有 UI sink 来抛出权限弹窗。Deny 原因写入 worker transcript 而非 parent audit，保持 C4 审计纯净（RULE-A-016）。

### 🔴 关键缺陷：worker system_prompt 未生效

```rust
// chat_loop.rs:2052
let _worker_system_prompt = assemble_subagent_prompt(def, task);
// (run_chat_loop builds its own system prompt from the project /
// session row; the worker's replacement prompt would need to be
// threaded as a parameter...)
```

`SubagentDef.system_prompt`（如 `subagent.rs:163-175` 的 researcher prompt、`:185-196` 的 general-purpose prompt）虽然精心编写（含角色定位、可用 tool 提示、summary 产出指引），但**从未被传入嵌套的 `run_chat_loop`**。Worker 实际得到的是 parent 的 `assemble_system_prompt` 输出——包含 parent 的 mode_prefix 和 behavior_prompt。这导致：

1. Worker 不知道自己是一个 subagent——它认为自己是主 agent
2. Worker 的 system prompt 告诉它可以写文件（来自 parent mode_prefix），但实际上在 Edit/Plan mode 下写操作被 Tier 4 auto-deny——**prompt 与现实行为矛盾**
3. SubagentDef 中精心编写的 system_prompt **完全是 dead code**

代码中已标记为 "PR1b Deviation"（`chat_loop.rs:2053-2059`），但修复此问题需要给 `run_chat_loop` 加 `system_prompt_override: Option<String>` 参数。

### ⚠️ 次要问题

- `max_turns` 耗尽时 `stop_reason == "max_turns"` 仅设置 `was_cancelled=false`（`subagent.rs:818-830`），worker 状态为 `Completed`。LLM 无法从 tool_result 区分 "真正完成" 和 "被 turn budget 截断"。

---

## 3. SSE 流与通信 (SSE Streaming & IPC)

### ✅ 设计亮点

- **`SubagentBufferSink`**（`subagent.rs:541-581`）：干净的 `ChatEventSink` 实现（`:787-858`），同时累积 in-memory transcript 和 emit `subagent:event` IPC（`:657-699` 的 `record()` 方法）。
- **IPC 隔离正确**：worker 的 SSE 事件不流入 parent 的 `chat-event` 通道——parent 前端只看到 `dispatch_subagent` 的 tool_call/tool_result 对（`chat_loop.rs:1586-1596`）。
- **前端 debounce 合理**：200ms 的 `subagent:event` 批量窗口（`subagentRuns.ts:350-376`）防止 per-delta 重渲染。
- **Token usage 流式累积**：`add_token_usage` 于 `chat_loop.rs:967` 不受 `skip_persist` 限制（PR2 修复），确保 worker 每 turn 的 token 实时叠加到 parent session 总计——用户在 worker 运行期间可以看到 token 计数器持续增长。

### ⚠️ 问题

- **主聊天流零可见性**：Parent loop **同步阻塞**等待 worker 完成。主聊天面板对 worker 运行只显示一个 `dispatch_subagent` 卡片 + "running" 状态。用户需要**主动发现并打开** SubagentDrawer（侧边面板）才能看到进度。
- **SubagentDrawer 是分离 UI 表面**：如果用户不知道 drawer 存在，worker 的中间过程完全不可见。
- **无 worker wall-clock 超时**：只有 `max_turns=20` 边界（`chat_loop.rs:2192`），但如果 LLM API 每 turn 30s，worker 可运行 10 分钟。Parent 的 Cancel 传播有效（`child_token`，`:2075`），但没有自动 watchdog。

---

## 4. 与主 Agent 的衔接通讯 (Parent-Worker Communication)

### ✅ 设计亮点

- **清晰的单向数据流**：
  1. Parent → Worker：`dispatch_subagent({ subagent, task })` tool_use
  2. Worker 运行于独立 context（fresh messages，独立 `cache_control` breakpoint）
  3. Worker 最终 assistant text → 回填为 tool_result，带 `[status: completed/cancelled/error]` 前缀（`subagent.rs:999` `format_dispatch_result`）
- **Cancel 传播**：`parent_token.child_token()` + 共享 `cancellations` map（`chat_loop.rs:2075-2079`），用户 Stop 同时终止 parent 和 worker。
- **`skip_session_active=true`**（`chat_loop.rs:2196`）：worker 的 `CancellationGuard` Drop 不清除 parent 的 `session_active_request` 映射——正确保护 RULE-E-005 语义。
- **Worker rid 格式**：`{parent_rid}-sub-{tool_use_id}`（`chat_loop.rs:2074`）使 ToolCallCard 可反向关联到对应的 worker run。
- **`subagent_runs` 表**：完整的 worker 运行审计（status/summary/transcript/token_usage），独立于 parent 的 `messages` 表。

### 🔴 关键缺陷：parent 无法访问 worker 内部状态

当 worker 出错时（如 LLM stream error），tool_result 的 `is_error=true` 且内容为 `[status: error]\n<error text>`。但 **parent LLM 完全看不到 worker 在出错前做了什么**——只能看到错误消息。Worker 的 transcript（在 `subagent_runs` 表 + drawer 中）对 parent LLM 不可读。这意味着：

- 如果 worker 已成功完成 3 个 file edit 后在第 4 个 turn 出错，parent LLM 只知道 "worker errored"，不知道前 3 个 edit 已落地
- Parent 无法做补偿性修复——它缺乏 worker 状态的上下文

### ⚠️ 次要问题

- **同步阻塞无超时**：同上。
- **cancel 后无 "summarize before exit"**：worker 被 cancel 时直接终止，返回的部分文本可能不完整（mid-sentence）。
- **Worker 退出后 transcript 对 parent LLM 不可见**：parent LLM 的 context 中没有 worker 的 tool_call/tool_result 历史——这些在 drawer 和 DB 里但对 agent loop 不可达。

---

## 5. 其他架构观察

| 维度 | 状态 | 说明 |
|---|---|---|
| **Memory/指令文件** | ✅ 正确 | Worker 加载自己独立的 4 指令文件 + 独立 `cache_control` breakpoint（`build_worker_messages`，`subagent.rs:244-288`），与 parent prompt cache 正交 |
| **Worker context 复用** | ✅ 正确 | Task APPEND（不 prepend）保持 memory breakpoint 在 messages[0]，遵循 B12+L1a 踩过的坑 |
| **DB best-effort 模式** | ✅ 正确 | `insert_run` / `update_run_finished` 失败只 log warn（`chat_loop.rs:2099-2108`、`:2282-2287`），不影响 dispatch 结果——用户体验不依赖审计持久化 |
| **Transcript 4 MiB cap** | ✅ 合理 | `TRANSCRIPT_MAX_BYTES = 4 MiB`（`subagent.rs:878`），head+tail 截断（`truncate_transcript_for_persistence`，`:899`）保留诊断价值 |
| **前端 race condition 修复** | ✅ 正确 | `eagerFetchedRunIds` Set + `subagent:event` 触发 eager-fetch（`subagentRuns.ts:391-413`）解决 ToolCallCard 在 `insert_run` 提交前的竞态 |
| **无 model 覆盖** | ⚠️ OOS | Worker 始终复用 parent provider/model，无法用廉价模型跑 researcher |
| **无 context_window 覆盖** | ⚠️ OOS | Worker 与 parent 共享 context_window |

---

## 6. 代码位置索引

| 组件 | 文件 | 行号 |
|---|---|---|
| `dispatch_subagent` ToolDef | `app/src-tauri/src/agent/subagent.rs` | 74-113 |
| `DISPATCH_TOOL_NAME` 常量 | `app/src-tauri/src/agent/subagent.rs` | 117 |
| `SubagentDef` struct | `app/src-tauri/src/agent/subagent.rs` | 136-146 |
| `builtin_subagents()` — researcher | `app/src-tauri/src/agent/subagent.rs` | 157-177 |
| `builtin_subagents()` — general-purpose | `app/src-tauri/src/agent/subagent.rs` | 178-202 |
| `lookup_subagent()` | `app/src-tauri/src/agent/subagent.rs` | 209-211 |
| `assemble_subagent_prompt()` | `app/src-tauri/src/agent/subagent.rs` | 222-228 |
| `build_worker_messages()` | `app/src-tauri/src/agent/subagent.rs` | 244-288 |
| `STRUCTURALLY_DISABLED` | `app/src-tauri/src/agent/subagent.rs` | 304-310 |
| `filter_tools_for_subagent()` | `app/src-tauri/src/agent/subagent.rs` | 320-343 |
| `SubagentStatus` enum | `app/src-tauri/src/agent/subagent.rs` | 352-366 |
| `SubagentBufferSink` struct | `app/src-tauri/src/agent/subagent.rs` | 541-581 |
| `SubagentBufferSink::record()` (IPC emit) | `app/src-tauri/src/agent/subagent.rs` | 657-699 |
| `ChatEventSink` impl (emit_*) | `app/src-tauri/src/agent/subagent.rs` | 787-858 |
| `TRANSCRIPT_MAX_BYTES` | `app/src-tauri/src/agent/subagent.rs` | 878 |
| `truncate_transcript_for_persistence()` | `app/src-tauri/src/agent/subagent.rs` | 899 |
| `format_dispatch_result()` | `app/src-tauri/src/agent/subagent.rs` | 999 |
| `run_subagent()` | `app/src-tauri/src/agent/chat_loop.rs` | 1985 |
| `_worker_system_prompt` (dead code) | `app/src-tauri/src/agent/chat_loop.rs` | 2052 |
| `worker_rid` 格式 | `app/src-tauri/src/agent/chat_loop.rs` | 2074 |
| `worker_token = parent_token.child_token()` | `app/src-tauri/src/agent/chat_loop.rs` | 2075 |
| 嵌套 `run_chat_loop` 调用 | `app/src-tauri/src/agent/chat_loop.rs` | 2174-2218 |
| `skip_session_active: true` | `app/src-tauri/src/agent/chat_loop.rs` | 2196 |
| `is_worker: Some(true)` | `app/src-tauri/src/agent/chat_loop.rs` | 2213 |
| `max_turns: Some(20)` | `app/src-tauri/src/agent/chat_loop.rs` | 2192 |
| `add_token_usage` (ungated) | `app/src-tauri/src/agent/chat_loop.rs` | 967 |
| `is_worker` Tier 4 collapse | `app/src-tauri/src/agent/permissions/mod.rs` | 1003-1044 |
| `is_parallel_eligible()` | `app/src-tauri/src/agent/chat_loop.rs` | 1907 |
| dispatch_subagent 拦截点 | `app/src-tauri/src/agent/chat_loop.rs` | 1523-1604 |
| 前端 store | `app/src/stores/subagentRuns.ts` | 1-482 |
| 前端 drawer 组件 | `app/src/components/chat/SubagentDrawer.vue` | 1-923 |
| 前端 ToolCallCard dispatch 分支 | `app/src/components/chat/ToolCallCard.vue` | (含 special branch) |

---

## 7. 最高优先级修复建议

1. **将 `SubagentDef.system_prompt` 实际传入 worker 的 `run_chat_loop`**：需要给 `run_chat_loop` 加 `system_prompt_override: Option<String>` 参数，worker 路径传入 `assemble_subagent_prompt(def, task)`。这将解决：
   - Worker 不知道自己是谁（当前它认为自己是主 agent）
   - Prompt 与现实行为矛盾（prompt 说"可写"但权限系统 deny）
   - SubagentDef.system_prompt 成为 dead code 的问题

2. **考虑 worker error 后向 parent 传递部分 transcript**：在 `format_dispatch_result` 的 Error 分支中附加 worker 已执行的 tool_call/tool_result 摘要，让 parent LLM 了解 worker 已改变了什么。
