# feat(subagent): worker tool approval interactive (RULE-FrontSubagent-003 修复)

## Goal

修复 RULE-FrontSubagent-003 (P2 open)：让 subagent worker 的 tool_use 审批从"自动拒绝"改为真正走交互式 ask_path — worker 分配独立 permission session，按 workerRunId 路由到 SubagentDrawer 内嵌可交互卡片，顶部 banner 在 drawer 关闭时唤起用户。保留 parent 串行派发 worker 的并发约束，切 session 不取消 ask、主动 cancel 才取消。

## What I already know

### 代码侧（Explore agent 已扫过）
- `app/src-tauri/src/agent/permissions/mod.rs:1003-1044` — Tier 4 `ask_path` worker 分支单点硬截流：直接 `return Decision::Deny`，跳过 `register_ask` / `tokio::select!{oneshot, cancel, 120s timeout}` 完整循环。
- `app/src-tauri/src/agent/permissions/mod.rs:1075` — 父 agent oneshot 挂起点（`tokio::select!` 三臂）。
- `app/src-tauri/src/agent/chat_loop.rs:386-391` — `PermissionContext { session_id, mode, cwd, is_worker }` 构造。
- `app/src-tauri/src/agent/chat_loop.rs:1587` — `dispatch_subagent` 截流进 `run_subagent`（不走 `tools::execute_tool`）。
- `app/src-tauri/src/agent/chat_loop.rs:2079` — `run_subagent` 入口。
- `app/src-tauri/src/agent/chat_loop.rs:2276` — 嵌套 `run_chat_loop(is_worker: Some(true), max_turns: Some(SUBAGENT_MAX_TURNS=200))`。
- `app/src-tauri/src/agent/chat_loop.rs:2294` — `permission_asks.clone()` 共享 parent store（worker 复用 parent session_id 是 RULE-A-014 当年止血的关键）。
- `app/src/components/chat/SubagentDrawer.vue` + `DrawerPermissionAskCard.vue` — 前端 drawer 渲染通道。
- `app/src/components/chat/DrawerPermissionAskCard.vue:97-113` — 当前显示 "worker · 自动拒绝 · 不可交互"，PR6 R24 降级注记。
- `app/src/stores/permissions.ts` / `app/src/stores/audit.ts` — 前端 store。

### 文档侧
- `.trellis/reviews/DEBT.md:218-233` — RULE-FrontSubagent-003 P2 open，三条根因 + 修复路径。
- `.trellis/reviews/DEBT.md` RULE-A-014/A-016 closed — 历史止血（worker 不能 hang parent oneshot）。

### 用户已锁定决策（从上一轮 AskUserQuestion）
1. **Modal 不能全局**（多 session 竞态 + 区分困难）→ drawer 内嵌可交互 + 顶部 banner 唤起
2. **parent 串行派发 worker**（同 session 同时只有 1 个 worker 在跑）
3. **切 session 保留 / 主动 cancel 才取消**（精细化取消策略）

## Assumptions (temporary)

- A1: parent `run_subagent` 当前是 `await` 调用 worker 完成才返回 → 串行派发天然成立，不需改调度。
- A2: `permission_asks` store 是 AppState 级共享（`HashMap<session_id, oneshot>`），切 session 不丢 entry。
- A3: 前端 store 跨 mount/unmount 是单例（Pinia），drawer 重开仍能从 store 拉到 ask 状态。
- A4: 120s timeout 沿用 parent 的 hardcoded 值，不做可配置化。
- A5: 现有 AuditKind 10 类可容纳 worker ask 相关事件（如 AskResponded / AskTimedOut / WorkerAskCancelled），不新增 AuditKind。

## Open Questions

- Q1 ✅（A）: `permission:ask` 增加可选 `workerRunId: Option<String>` 字段，复用现有 IPC 通道
- Q2 ✅（A）: worker ask 三种终态写 audit，新增 4 个 AuditKind: `WorkerAskAllowed` / `WorkerAskDenied` / `WorkerAskTimedOut` / `WorkerAskCancelled`
- Q3: drawer 状态切换（串行派发下 worker run/complete 时 drawer 已有 transcript 渲染，ask 是 tool_use 子状态，无需单独 "loading / waiting" 区分）→ **derive，不再问**
- Q4: drawer 默认开关策略 — 待问
- Q5: cancellation token 派发细节 — implementation detail，跳过（implementation phase 自己定）

## Requirements (evolving)

### R1 后端 — worker 独立 permission session
- worker 分配 `permission_session_id = "worker:{workerRunId}"`，与 parent session_id 解耦
- Tier 4 worker 分支改为：emit `permission:ask` + `register_ask(worker_session_id, rid)` + `tokio::select!{oneshot, derived_cancel_token, 120s timeout}` → 任一臂命中 resolve(Deny/Allow)
- 超时仍 Deny（保留安全网）

### R2 后端 — Cancellation token 路由
- `run_subagent` 创建 `parent_token.child_token()`，worker 的 oneshot select 用此 derived token
- 父 session cancel → 触发 child_token → worker 等待 oneshot 收到 cancel 信号 → resolve(Deny)
- 父 session 切走（用户切 sidebar）→ 不触发 child_token → ask 继续等

### R3 IPC schema
- 新增 `workerRunId: string` 字段于 `PermissionAskPayload`
- 前端按 `workerRunId` 路由到具体 drawer 行的 tool_use

### R4 前端 — DrawerPermissionAskCard 切回可交互
- 保留现有 `mode` / `onRespond` props（PR6 R24 留下的脚手架）
- 去掉 "自动拒绝" 注记文案
- `onRespond` 调 `invoke('permission:respond', { rid, response, workerRunId })`
- 计时显示：从 IPC 收到 ask 到现在的 elapsed（timeout 提示）

### R5 前端 — 顶部 banner
- 新增 `<WorkerAskBanner>` 组件，紧贴 ChatPanel header
- `v-if="pendingCount > 0"`，显示 `⏳ N 个 worker 待审批 [展开]`
- 点击 → 打开对应 session 的 SubagentDrawer + 自动滚动到 ask 行
- pendingCount 来自 `usePermissionsStore` 或新增 `usePendingAskStore`（按 sessionId 隔离）

### R6 测试
- vitest: DrawerPermissionAskCard 可交互模式 + WorkerAskBanner 显示/隐藏/点击
- cargo test: worker ask_path 三臂（allow / timeout / cancel）— 加到 permissions/mod.rs 的 #[cfg(test)]
- integration: parent 派 worker → worker ask → user 允许 → worker 继续（mock provider）

### R7 DEBT.md
- 关闭 RULE-FrontSubagent-003（带 commit hash）
- 若实现中发现新债（如 banner 边界 / cancel 边缘），新增 RULE-WorkerAsk-*

## Acceptance Criteria (evolving)

- [ ] worker 在 tool_use 需要审批时，drawer 中渲染可交互 Allow/Deny 卡片（非历史只读）
- [ ] drawer 关闭时，header banner 显示待审批计数
- [ ] 点击 banner 打开对应 drawer 并滚动到 ask 行
- [ ] user 在 ask 等待期间切走 session 再切回，ask 仍在且可响应
- [ ] user 在 ChatPanel 主动 cancel parent session，所有等待中的 worker ask resolve Deny
- [ ] 120s 无响应 → worker ask resolve Deny（保留安全网）
- [ ] 多 session 并发时，每个 session 的审批 UI 互不干扰（无全局 modal 弹出）
- [ ] DEBT.md RULE-FrontSubagent-003 closed，新 commit 引用

## Definition of Done

- 后端 vitest + cargo test --lib 通过
- 前端 vue-tsc --noEmit + pnpm build 通过
- DEBT.md 关闭 RULE-FrontSubagent-003
- 四段式 commit: fix → docs(debt) → archive → journal

## Out of Scope (explicit)

- 全局 modal 改造（用户已否决）
- parent 并发派发 worker（用户已选串行）
- 切 session 即取消策略（用户已选精细化区分）
- AuditKind 新增（如 A5 假设不成立则扩 1 类）
- 120s timeout 可配置化
- worker ask 审批历史导出 / 多设备同步

## Technical Notes

### 关键文件
- 后端: `app/src-tauri/src/agent/permissions/mod.rs`, `app/src-tauri/src/agent/chat_loop.rs`, `app/src-tauri/src/commands/permissions.rs` (如有)
- 前端: `app/src/components/chat/SubagentDrawer.vue`, `app/src/components/chat/DrawerPermissionAskCard.vue`, `app/src/stores/permissions.ts`
- 新增: `app/src/components/chat/WorkerAskBanner.vue`, `app/src/stores/pendingAsk.ts` (或合并到 permissions store)

### 约束
- 跨 session 隔离（drawer + banner 都按 sessionId 路由）
- cancellation token 必须正确派发（避免 RULE-A-014/A-016 复现）
- oneshot drop guard（避免 leak / 死锁）

### 参考
- RULE-FrontSubagent-003 P2 open (DEBT.md:218-233)
- RULE-A-014/A-016 closed (历史止血)
- Claude Code Task tool 的 sub-agent 审批 UX（参考）