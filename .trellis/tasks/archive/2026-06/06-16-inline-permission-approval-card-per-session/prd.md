# 内联审批卡片 + 按 session 分区 + 拒绝并反馈

## Goal

把工具调用审批从「全局单例 modal」改造为「内联到 ToolCallCard 的待审批状态卡片」——审批 UI 跟在对应 session 的消息流里、按 session 渲染，解决多 session 并发审批时的串台/丢失/超时 deny 问题；同时新增「拒绝并对 agent 说」：用户 deny 时可附反馈文字，作为 `tool_result(is_error)` 回填给 LLM，让 agent 知道为什么被拒、该怎么改。

## Background（已确认的事实）

来源：本次 session 对 `app/src/stores/permissions.ts`、`app/src/components/chat/PermissionModal.vue`、`app/src/components/chat/ChatPanel.vue`、`app/src-tauri/src/agent/permissions/mod.rs` 的调研。

- **全局单例 modal**：`<PermissionModal/>` 挂在 `ChatPanel.vue:517`，但用 `<Teleport to="body">`（`PermissionModal.vue:270`）飘出 panel；状态来自全局单槽 `pendingPermission`（`permissions.ts:147`）。
- **并发审批的真实行为**：`setPending(B)` 直接覆盖 `pendingPermission`，**不对旧 rid 做任何 respond**（`permissions.ts:254`）；旧 ask 的 oneshot sender 留在后端 store，`ask_path` 的 `select!` 跑满 120s 超时分支 → `Decision::Deny`（`mod.rs:951-964`）。即「前一个被静默丢弃，120s 后超时拒」，且该 session 的 agent loop 卡 120s。
- **payload 无 session 标识**：`PermissionAskPayload`（`mod.rs:361`）只有 `rid/toolName/toolInput/risk/reason/path`，无 sessionId。modal 文案写死「当前项目」，path badge 用 `chatStore.currentCwd`（跨 session 时指向错误 session）。
- **deny reason 写死**：`PermissionResponse::Deny`（`mod.rs:997`）返回固定 `"user denied"`，作为 `tool_result(is_error: true)` 回填给 LLM——LLM 只知道被拒、不知原因。
- **后端存储本就支持并发**：`PermissionStore = HashMap<rid, oneshot::Sender>`（`mod.rs:285`），全局共享，能并发挂起多个 ask。瓶颈在前端单槽。
- **chat 区单实例切内容**：`MessageList` 只渲染 `currentSessionId` 的消息；`tokenUsageBySession`、`sessionTotalLatencyMs` 均为 `Map<sessionId, …>`，数据天然按 session 组织。
- **`PermissionResponse` 枚举**：`AllowOnce | AllowAlways | Deny`（`mod.rs:272`），wire 上 decision 字符串 `"allow_once"|"allow_always"|"deny"`。

## Assumptions（待验证）

- ✅ **形态 2 已确认可行**（见 Technical Notes）：后端在 `permission:ask` 发出时能直接带 `tool_use_id`，前端 `ToolCallInfo.id` 即 `tool_use_id`，可 100% 定位目标 ToolCallCard。
- payload 需新增 `sessionId`（复用 `request_id` → session 路由）+ `toolUseId`。

## Open Questions（仅 Blocking / Preference）

- **Q1（Preference）**：拒绝反馈的输入框形态——① 点「拒绝」一键 deny + 独立「拒绝并说明」展开输入框；② 「拒绝」总是展开输入框（可留空）；③ 卡片常驻反馈框。
- **Q2（Preference）**：切走 session 时未决 pending 的处理——保留后端 120s 超时 deny 不变 + SessionList 加「待审批」标记；还是改后端让 pending 切走时挂起不超时？
- **Q3（Preference）**：全局 `<PermissionModal/>` 彻底移除（单一来源）还是兜底保留？
- **Q4（Preference，可选）**：审批↔审计联动（卡片可跳审计日志）是否纳入本轮？

## Requirements

### 后端
- `PermissionAskPayload` 增加 `tool_use_id` + `session_id`（复用 request_id → session 路由）。
- `check()` / `ask_path()` 签名穿透 `tool_use_id`（当前仅传 tool_name/tool_input）。
- `PermissionResponse::Deny` 扩展带 `reason: String`；`permission_response` IPC 接收 `reason`。
- deny 反馈作为 `tool_result(is_error: true)` 内容回填 LLM（沿用现有 deny→tool_result 链路）。
- yolo/plan 模式判定逻辑不变。

### 前端 store
- `pendingPermission`(单槽) → `pendingBySession: Map<sessionId, PermissionAsk>`，listener 按 `session_id` 路由。
- 每 ask 独立 120s 计时器（按 rid 索引），互不影响。
- 切走 session 时后端超时语义不变（120s auto-deny）；前端在 SessionList/session tab 标「待审批」。
- `respond(rid, decision, reason?)` 透传 deny reason。

### 前端 UI（ToolCallCard）
- `call.id === pending.toolUseId` 时渲染「待审批」态：risk 标识 + path 范围行(path 工具) + 命令预览 + 四个操作。
- 操作：仅一次 / 始终允许 / 拒绝（一键 deny，无反馈）/ 拒绝并说明（展开输入框，填反馈后 deny）。
- 审批 pending 中用户点「停止」(cancel) → 卡片转 denied 态。
- **移除全局 `<PermissionModal/>`**（`ChatPanel.vue:517` + `PermissionModal.vue`），内联卡片为唯一入口。
- path badge 用发起 session 的 cwd（修正当前误用 `chatStore.currentCwd`）。

### SessionList
- 有未决 pending ask 的 session 显示「待审批」标记（红点/badge），切走可感知。

## Acceptance Criteria

- [ ] payload 携带 `tool_use_id` + `session_id`；前端能据此定位目标卡片与 session。
- [ ] 多 session 并发审批：各 session 审批卡片独立显示、互不覆盖、互不串台。
- [ ] 切换 session：当前 session 无 pending 不显示审批；有 pending 显示其卡片，归属/cwd 正确。
- [ ] 旧的「setPending 覆盖即丢失 + 120s 静默超时」路径消除（每 ask 独立计时 + 独立 deny）。
- [ ] 「拒绝」一键 deny；「拒绝并说明」的反馈作为 `tool_result(is_error)` 回填，下一轮 LLM 可见。
- [ ] SessionList 标记有待审批的 session。
- [ ] 审批 pending 中点「停止」→ 卡片转 denied。
- [ ] 全局 `PermissionModal` 移除，无残留引用（含 test）。
- [ ] yolo/plan 模式行为不变。
- [ ] `vue-tsc --noEmit` + `cargo test`（PKG_CONFIG_PATH）绿。

## Definition of Done

- 前后端单元测试覆盖（permissions store 按 session 路由、payload 序列化、deny 反馈回填、cancel 态）。
- `vue-tsc --noEmit` + `cargo test`（PKG_CONFIG_PATH）绿。
- spec / 决策日志更新（permission modal → 内联卡片的 ADR）。

## Technical Approach

内联审批卡片，以 `tool_use_id` 为关联键。后端在 `permission:ask` 发出时携带 `tool_use_id` + `session_id`（agent loop 已持有 tool_use_id，仅需签名穿透）；前端 store 按 `session_id` 分区存 pending，ToolCallCard 以 `call.id === pending.toolUseId` 渲染审批态。deny reason 经 `PermissionResponse::Deny{reason}` → IPC → agent loop 包成 `tool_result(is_error)` 回填 LLM。后端 120s 超时语义保持不变（不挂起 agent loop），由 SessionList 标记弥补「切走不可见」。

## Decision (ADR-lite)

**Context**: 全局单例 modal 在多 session 并发下串台、丢失、120s 静默 deny；deny 无反馈，LLM 不知为何被拒。
**Decision**: 审批内联到 ToolCallCard（tool_use_id 关联）+ 按 session 分区 + 拒绝并反馈（分离式输入框）+ 保留后端 120s 超时并加 SessionList 标记 + 彻底移除全局 modal。
**Consequences**: 彻底解决串台/丢失；deny 反馈提升 agent 纠错能力；代价是 ToolCallCard 复杂度上升、payload/IPC 契约变更（单用户应用可接受）。全局 modal 移除后无兜底，依赖 `tool:call` 必先于 `permission:ask`（agent loop 时序已保证，仍加时序防御）。

## Out of Scope（explicit）

- yolo / plan / edit 三档 mode 判定逻辑不变。
- 「始终允许」grant 持久化逻辑不变。
- 审批↔审计联动跳转（future）。
- 批量放行（future）。
- daemon 化 / 多窗口（未来架构）。

## Technical Notes

- 影响文件（预期）：`app/src/stores/permissions.ts`、`app/src/components/chat/PermissionModal.vue`（可能废弃）、`app/src/components/chat/ToolCallCard.vue`、`app/src/components/chat/ChatPanel.vue`、`app/src/stores/chat.ts`；后端 `app/src-tauri/src/agent/permissions/mod.rs`（payload + PermissionResponse + check 时序）、`app/src-tauri/src/agent/`（事件发射时序）、`app/src-tauri/src/commands/permissions.rs`。
- **rid↔tool_use 关联（命门，已确认可行）**：agent loop `for (id, name, input) in &tool_calls`（`agent/chat_loop.rs:612`）里 `id` 即 `tool_use_id`；`permission::check` 在执行 tool **之前**调用（`chat_loop.rs:619`），且串行 await（同 session 同一时刻最多一个 pending ask，跨 session 才并发）。`tool:call`（`chat_loop.rs:423`）早于 `permission:ask`，前端收到审批事件时目标 ToolCallCard 已渲染。方案：① `PermissionAskPayload` 加 `tool_use_id` + `session_id`；② `check()`/`ask_path()` 签名穿透 `tool_use_id`（当前只传 tool_name/tool_input）；③ 前端 `ToolCallInfo.id`(=tool_use_id, `chat.ts:48`) 匹配 pending 渲染审批态。
- **事件 session 路由**：现有 `tool:call`/`tool:result` 经 `request_id` → `activeRequests` 路由到 session（`streamController.ts`）。`permission:ask` 当前无 request_id/sessionId，需补。
- **deny 反馈回填**：`Decision::Deny { reason }` → agent loop 包成 `tool_result(is_error:true)` 回填 LLM（`chat_loop.rs` deny 分支）。`PermissionResponse::Deny` 扩展带 `reason: String`，wire 上 `"deny"` → `{decision:"deny", reason:"..."}`。
- **cancel 交互**：用户点「停止」走 `token.cancelled()` → `ask_path` select 的 cancel 分支 deny（`mod.rs:943`）。内联卡片需在 cancel 时转为 denied 态。
- WSL cargo test 注意 PKG_CONFIG_PATH（见 CLAUDE.md）。
