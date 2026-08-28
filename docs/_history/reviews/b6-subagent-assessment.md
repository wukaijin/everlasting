# B6 Subagent 系统评估

> 评估日期: 2026-06-20 | 最后更新: 2026-06-21（行号重验证，纳入 R1-R3 变更） | 评估范围: B6 PR1+PR2+PR3 + 2026-06-21 fix 全部落地代码

## 评估概览

| 角度 | 评级 | 关键问题 |
|---|---|---|
| 工具配给 | 🟢 合理 | 粒度两档（OOS 已知），structural-disabled 列表充分 |
| Mode 结合 | 🟢 合理 | ✅ 2026-06-21 已修复 `system_prompt_override` |
| SSE/流 | 🟢 合理 | 主聊天流零可见性，但 IPC 实现正确 |
| 衔接通讯 | 🟡 有缺陷 | **parent 无法访问 worker 内部状态**，error 后 parent 盲目 |
| 整体架构 | 🟢 合理 | MVP 范围内设计决策审慎，已知 OOS 均有文档 |

---

## 1. 工具配给 (Tool Allocation)

### ✅ 设计亮点

**双层过滤**：`SubagentDef.tools` allowlist + 无条件 `STRUCTURALLY_DISABLED` 剥离（`subagent.rs:321-327`），即使未来支持 frontmatter 自定义工具集也不会意外开启嵌套 dispatch 或 background shell。

**空 allowlist = 自维护全集**：`general-purpose` 的 `tools: &[]`（`subagent.rs:201`）被 `filter_tools_for_subagent`（`:341-345`）解释为 "builtin_tools() 减去禁用集"，未来新增 tool 时 general-purpose 自动可用而不需改定义。

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

- **Mode 继承干净**：worker 通过 `PermissionContext` 继承 parent 的 Mode（Yolo/Edit/Plan），`run_subagent` 传入 `is_worker: Some(true)` 作为 `run_chat_loop` 的第 21 个 override 参数（`chat_loop.rs:2319`）。
- **Tier 4 collapse**：`is_worker=true` 时 `ask_path`/`ask_shell` → `Deny`（`permissions/mod.rs:1003-1044`），因为 worker 没有 UI sink 来抛出权限弹窗。Deny 原因写入 worker transcript 而非 parent audit，保持 C4 审计纯净（RULE-A-016）。
- **`system_prompt_override`**（`chat_loop.rs:235`）：`run_chat_loop` 第 23 个参数。Worker 路径传入 `Some(assemble_subagent_prompt(def, task))`（`:2338`），`run_chat_loop` 内部 short-circuit parent 的 `assemble_system_prompt`（`:478-480`），使 `SubagentDef.system_prompt` 完全替换 parent 的 `mode_prefix + base_prompt`。**2026-06-21 修复**（修复前 worker 获得 parent prompt，导致 worker 角色错位 + prompt/权限矛盾——见 `docs/review/b6-subagent-assessment.md` v1 §2）。

### ⚠️ 次要问题

- `max_turns` 耗尽时 worker 状态为 `Incomplete`（`chat_loop.rs:2358-2366`），返回 `[status: incomplete]` + `[INCOMPLETE_MARKER]`，`is_error=true`。这已比最初版本的"一律 Completed"有了改善（2026-06-21 R2），但 parent LLM 仍需从 marker 推断而非从结构化信息获知。

---

## 3. SSE 流与通信 (SSE Streaming & IPC)

### ✅ 设计亮点

- **`SubagentBufferSink`**（`subagent.rs:570-631`）：干净的 `ChatEventSink` 实现（`:853-927`），同时累积 in-memory transcript 和 emit `subagent:event` IPC（`:714-759` 的 `record()` 方法）。
- **IPC 隔离正确**：worker 的 SSE 事件不流入 parent 的 `chat-event` 通道——parent 前端只看到 `dispatch_subagent` 的 tool_call/tool_result 对（`chat_loop.rs:1587-1596`）。
- **前端 debounce 合理**：200ms 的 `subagent:event` 批量窗口（`subagentRuns.ts:350-376`）防止 per-delta 重渲染。
- **Token usage 流式累积**：`add_token_usage` 于 `chat_loop.rs:1031` 不受 `skip_persist` 限制（PR2 修复），确保 worker 每 turn 的 token 实时叠加到 parent session 总计——用户在 worker 运行期间可以看到 token 计数器持续增长。
- **Per-tool duration**（2026-06-21 R3）：`SubagentBufferSink` 记录每个 `tool_call→tool_result` 的 wall-clock 间隔（`:929-958`），前端 drawer 渲染 per-tool 延迟。

### ⚠️ 问题

- **主聊天流零可见性**：Parent loop **同步阻塞**等待 worker 完成。主聊天面板对 worker 运行只显示一个 `dispatch_subagent` 卡片 + "running" 状态。用户需要**主动发现并打开** SubagentDrawer（侧边面板）才能看到进度。
- **SubagentDrawer 是分离 UI 表面**：如果用户不知道 drawer 存在，worker 的中间过程完全不可见。
- **无 worker wall-clock 超时**：只有 `max_turns=200` 边界（`chat_loop.rs:2076`，2026-06-21 从 20 升至 200），但如果 LLM API 每 turn 30s，worker 理论上可运行 100 分钟。Parent 的 Cancel 传播有效（`child_token`，`:2166`），但没有自动 watchdog。

---

## 4. 与主 Agent 的衔接通讯 (Parent-Worker Communication)

### ✅ 设计亮点

- **清晰的单向数据流**：
  1. Parent → Worker：`dispatch_subagent({ subagent, task })` tool_use
  2. Worker 运行于独立 context（fresh messages，独立 `cache_control` breakpoint）
  3. Worker 最终 assistant text → 回填为 tool_result，带 `[status: completed/cancelled/error/incomplete]` 前缀（`subagent.rs:1278` `format_dispatch_result`）
- **Cancel 传播**：`parent_token.child_token()` + 共享 `cancellations` map（`chat_loop.rs:2166-2170`），用户 Stop 同时终止 parent 和 worker。
- **`skip_session_active=true`**（`chat_loop.rs:2302`）：worker 的 `CancellationGuard` Drop 不清除 parent 的 `session_active_request` 映射——正确保护 RULE-E-005 语义。
- **Worker rid 格式**：`{parent_rid}-sub-{tool_use_id}`（`chat_loop.rs:2165`）使 ToolCallCard 可反向关联到对应的 worker run。
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
| **Memory/指令文件** | ✅ 正确 | Worker 加载自己独立的 4 指令文件 + 独立 `cache_control` breakpoint（`build_worker_messages`，`subagent.rs:261-305`），与 parent prompt cache 正交 |
| **Worker context 复用** | ✅ 正确 | Task APPEND（不 prepend）保持 memory breakpoint 在 messages[0]，遵循 B12+L1a 踩过的坑 |
| **Worker system_prompt** | ✅ 已修复 | 2026-06-21：`system_prompt_override` 参数落地（`chat_loop.rs:235`），`SubagentDef.system_prompt` 全文替换 parent prompt |
| **max_turns 语义** | ✅ 已改善 | 2026-06-21 R2：从 20→200，新增 `Incomplete` 状态区分 budget 耗尽 vs 自然完成 |
| **DB best-effort 模式** | ✅ 正确 | `insert_run` / `update_run_finished` 失败只 log warn（`chat_loop.rs:2189-2199`、`:~2400`），不影响 dispatch 结果——用户体验不依赖审计持久化 |
| **Transcript 4 MiB cap** | ✅ 合理 | `TRANSCRIPT_MAX_BYTES = 4 MiB`（`subagent.rs:1083`），head+tail 截断（`truncate_transcript_for_persistence`，`:1104`）保留诊断价值 |
| **前端 race condition 修复** | ✅ 正确 | `eagerFetchedRunIds` Set + `subagent:event` 触发 eager-fetch（`subagentRuns.ts:391-413`）解决 ToolCallCard 在 `insert_run` 提交前的竞态 |
| **无 model 覆盖** | ⚠️ OOS | Worker 始终复用 parent provider/model，无法用廉价模型跑 researcher |
| **无 context_window 覆盖** | ⚠️ OOS | Worker 与 parent 共享 context_window |

---

## 6. 代码位置索引

| 组件 | 文件 | 行号 |
|---|---|---|
| `dispatch_subagent` ToolDef | `app/src-tauri/src/agent/subagent.rs` | 76-113 |
| `DISPATCH_TOOL_NAME` 常量 | `app/src-tauri/src/agent/subagent.rs` | 117 |
| `SubagentDef` struct | `app/src-tauri/src/agent/subagent.rs` | 136-146 |
| `builtin_subagents()` — researcher | `app/src-tauri/src/agent/subagent.rs` | 157-177 |
| `builtin_subagents()` — general-purpose | `app/src-tauri/src/agent/subagent.rs` | 178-202 |
| `lookup_subagent()` | `app/src-tauri/src/agent/subagent.rs` | 209-211 |
| `assemble_subagent_prompt()` | `app/src-tauri/src/agent/subagent.rs` | 239-245 |
| `build_worker_messages()` | `app/src-tauri/src/agent/subagent.rs` | 261-305 |
| `STRUCTURALLY_DISABLED` | `app/src-tauri/src/agent/subagent.rs` | 321-327 |
| `filter_tools_for_subagent()` | `app/src-tauri/src/agent/subagent.rs` | 337-360 |
| `SubagentStatus` enum (含 Incomplete) | `app/src-tauri/src/agent/subagent.rs` | 379-384 |
| `SubagentBufferSink` struct | `app/src-tauri/src/agent/subagent.rs` | 570-631 |
| `SubagentBufferSink::record()` (IPC emit) | `app/src-tauri/src/agent/subagent.rs` | 714-759 |
| `ChatEventSink` impl (emit_*) | `app/src-tauri/src/agent/subagent.rs` | 853-927 |
| `TRANSCRIPT_MAX_BYTES` | `app/src-tauri/src/agent/subagent.rs` | 1083 |
| `truncate_transcript_for_persistence()` | `app/src-tauri/src/agent/subagent.rs` | 1104 |
| `format_dispatch_result()` | `app/src-tauri/src/agent/subagent.rs` | 1278-1290 |
| `run_chat_loop` 函数签名 | `app/src-tauri/src/agent/chat_loop.rs` | 129 |
| `system_prompt_override` 参数 | `app/src-tauri/src/agent/chat_loop.rs` | 235 |
| `system_prompt_override` 分支 | `app/src-tauri/src/agent/chat_loop.rs` | 478-480 |
| `add_token_usage` (ungated) | `app/src-tauri/src/agent/chat_loop.rs` | 1031 |
| dispatch_subagent 拦截点 | `app/src-tauri/src/agent/chat_loop.rs` | 1587-1668 |
| `is_parallel_eligible()` | `app/src-tauri/src/agent/chat_loop.rs` | 1987 |
| `SUBAGENT_MAX_TURNS` (=200) | `app/src-tauri/src/agent/chat_loop.rs` | 2076 |
| `run_subagent()` | `app/src-tauri/src/agent/chat_loop.rs` | 2079-2390 |
| `worker_rid` 格式 | `app/src-tauri/src/agent/chat_loop.rs` | 2165 |
| `worker_token = parent_token.child_token()` | `app/src-tauri/src/agent/chat_loop.rs` | 2166 |
| 嵌套 `run_chat_loop` 调用 (Box::pin) | `app/src-tauri/src/agent/chat_loop.rs` | 2280-2339 |
| `max_turns: Some(SUBAGENT_MAX_TURNS)` | `app/src-tauri/src/agent/chat_loop.rs` | 2298 |
| `skip_session_active: true` | `app/src-tauri/src/agent/chat_loop.rs` | 2302 |
| `is_worker: Some(true)` | `app/src-tauri/src/agent/chat_loop.rs` | 2319 |
| `system_prompt_override` (worker) | `app/src-tauri/src/agent/chat_loop.rs` | 2338 |
| status picker (含 was_incomplete) | `app/src-tauri/src/agent/chat_loop.rs` | 2354-2369 |
| `is_worker` Tier 4 collapse | `app/src-tauri/src/agent/permissions/mod.rs` | 1003-1044 |
| 前端 store | `app/src/stores/subagentRuns.ts` | 1-482 |
| 前端 drawer 组件 | `app/src/components/chat/SubagentDrawer.vue` | 1-923 |
| 前端 ToolCallCard dispatch 分支 | `app/src/components/chat/ToolCallCard.vue` | (含 special branch) |

---

## 7. 修复建议

1. ✅ **`SubagentDef.system_prompt` 传入 worker** — 已于 2026-06-21 修复（`system_prompt_override` 参数，见 §2 + `chat_loop.rs:2338`）。

2. **考虑 worker error 后向 parent 传递部分 transcript**：在 `format_dispatch_result` 的 Error/Incomplete 分支中附加 worker 已执行的 tool_call/tool_result 摘要，让 parent LLM 了解 worker 已改变了什么。
