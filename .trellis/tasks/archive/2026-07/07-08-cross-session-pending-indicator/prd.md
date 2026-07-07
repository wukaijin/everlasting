# 跨 session 待处理交互 UI 提醒

## Goal

用户停留在 session A 时,session B(同 project 或另一 project)的 LLM 发起 `request_mode_change` / `ask_user_question` 申请,用户在 session A 的视图里能**视觉感知到**——不用主动切到 session B 才发现。消除"另一个 session 有待处理申请,视觉上完全不知道"的盲区。

三档提醒(用户已确认"都一起做"):
- **A 档 持续·定位**:sidebar session 条目角标(精确到 session)
- **B 档 持续·汇总**:AppHeader 顶栏全局徽章(跨 project 总数)
- **C 档 瞬时·push**:新申请到达时全局 toast(主动提醒)

## 背景与现状(已确认,带 file:line)

- **后端全局广播**:`emit_mode_change_request` = `self.app.emit("mode:change:request", payload)`(`state.rs:670`,非定向);`tool:question`(`state.rs:657`)、`permission:ask`(`state.rs:646`)同理。前端所有 listener 都收到。
- **前端已跨 session 全量缓存**:`useQuestionCardsStore.pendingBySession` = `reactive(Map<sessionId, PendingInteraction>)`(`questionCards.ts:81`),`mode:change:request` / `tool:question` 监听器已接(`streamController.ts:1696` / `1683`),按 payload `session_id` 路由进 Map。
- **`PendingInteraction`** = `{ kind: "question" | "mode_change"; payload }` tagged union(`questionCards.types.ts:309`)。A 档角标按 `kind` 分发图标。
- **权限审批另有独立缓存**:`usePermissionsStore.pendingBySession`(`permissions.ts:177`),已有 `hasPending(sessionId)`(`permissions.ts:306`)。**本任务不并入**(Q1 决策)。
- **展示层缺口**:`SessionList.vue` 已有 `session-item__streaming`(`SessionList.vue:435/514`)与 `session-item__pending-approval`(读 `permStore.hasPending`,`SessionList.vue:441/520`,shield-check + 脉冲)两种跨 session 角标范式,但**未 import `useQuestionCardsStore`** → mode/question pending 无任何跨 session 视觉。inline card 只在切到该 session 后由 MessageList 渲染。
- **sidebar 只显示当前 project 的 sessions**:`chat store` 的 `sessions` 随 `currentProjectId` `loadSessions(projectId)`(`chat.ts:534`)→ **A 档对"另一 project 的 session B"天然失效**(sidebar 不渲染它)。
- **sidebar 在用户场景下总可见**:`showSidebar = currentProjectId !== null`(`AppShell.vue:18`)。
- **现成可复用 toast**:`projectsStore.showToast(message, kind)` + AppShell 渲染(fixed bottom-center,单条 TTL,带 transition,`AppShell.vue:35-43` / `projects.ts:71`);`ToastMessage = { message, kind }`(`projects.ts:43`)。
- **`useErrorBus` 是 stub**(全 `console.warn`,`useErrorBus.ts:158`)——不碰。
- **AppHeader 结构**:`<TitleBar>` slot 包 `<ProjectTabs>` + `<HiddenProjectsMenu>`(`AppHeader.vue:38-45`)——顶栏徽章挂载点。
- **Icon 可用**:`refresh`(mode 切换)、`info`(ask_user_question)、`shield-check`(已被权限审批占用)、`circle-dot`/数字(徽章)。

## 范围决策(已锁定)

| # | 决策 | 取值 |
|---|---|---|
| Q1 | 覆盖种类 | mode change + ask_user_question(`useQuestionCardsStore`);**权限审批不并入**(维持现有 sidebar 角标) |
| Q2 | B 档徽章落点 | AppHeader 顶栏全局徽章,数字 = `pendingBySession.size`,跨 project 天然成立,**零后端改动** |
| Q3 | C 档 toast 触发 | **仅当新 pending 属于"非当前 session"** 时弹一次;当前 session 的 inline card 已可见,不弹 toast 打扰 |
| Q4 | 点击跳转范围 | **仅同 project 跳转**(toast/徽章点击 → 切到当前 project 内目标 session);跨 project 不跳(只提示),**零后端改动**(不引入 session→project 映射) |
| Q5 | 视觉区分(自决) | A 档角标按 `kind` 分图标:mode_change=`refresh`+target_mode 配色,question=`info`;B 档徽章单一数字不区分;C 档 toast 文案按 kind 区分 |

## Requirements

- **R1(A 档)**.`SessionList.vue` import `useQuestionCardsStore`,在 session 条目 `session-item__title-row` 内(与 `session-item__pending-approval` 并列)渲染新角标:`v-if="questionCards.getPending(s.id)"`,按 `entry.kind` 选图标(mode_change→`refresh`+target_mode 边色 / question→`info`)+ 复用脉冲动画。两处渲染点同步(flat 模式 `:435` 区 + grouped 模式 `:520` 区)。
- **R2(B 档)**.AppHeader 顶栏新增全局徽章组件(或内联),`v-if="count > 0"`,`count = computed(() => questionCards.pendingBySession.size)`;脉冲 + accent 色;`title="N 个会话有待处理交互"`。点击 → 若当前 project 内有 pending session,`switchSession` 到最近一个(按 `ts` 最大);否则 no-op。
- **R3(C 档)**.扩展 `showToast` 支持可选 `sessionId`:`showToast(message, kind, opts?: { sessionId?: string })`;`ToastMessage` 加可选 `sessionId` 字段(向后兼容现有调用)。AppShell 的 toast 点击 handler:若 `toast.sessionId` 存在且该 session 属于当前 project(chat store `sessions` 内),`switchSession(sessionId)`;否则仅 `dismissToast`。
- **R4(C 档触发)**.`streamController` 的 `handleModeChangeRequest` / `handleToolQuestion` 在 `addPending` 之后:若 `payload.session_id !== chatStore.currentSessionId`,调 `projectsStore.showToast(文案, "info", { sessionId })`。文案:mode_change→`「会话」申请切换到 {target_mode} 模式`(同 project 带标题,跨 project 用"另一项目的会话");question→`「会话」有问题等你回答`。
- **R5(视觉)**.R1 角标配色:mode_change 沿用 `RequestModeChangeCard` 的 target_mode 映射(edit=蓝 `--color-accent`/plan=青 `--color-tool-read`/yolo=红 `--color-tool-error`);question 用中性 accent。pending 解决(`removePending`)后角标/徽章自动消失(reactive Map 驱动)。

## Acceptance Criteria

- [ ] **AC1**.session B 发起 `request_mode_change`(B ≠ 当前 session A,同 project):A 视图内 SessionList 的 B 条目出现 `refresh` 角标 + 顶栏徽章数字 ≥1 + 一条 toast 弹出。
- [ ] **AC2**.**当前** session 发起 mode change:不弹 toast(Q3);顶栏徽章计数含当前 session(数字≥1);SessionList 当前条目角标显示。
- [ ] **AC3**.session B 发起 `ask_user_question`(B ≠ 当前):B 条目出现 `info` 角标 + 徽章 + toast。
- [ ] **AC4**.角标按 `kind` 正确分发(mode_change≠question 图标),与权限审批 `shield-check` 角标可并存(同一 session 两种 pending 时两个角标并列不溢出)。
- [ ] **AC5**.用户在 B 的 inline card 上允许/拒绝 → B 角标消失 + 顶栏徽章数字减 1(或归 0 隐藏)。
- [ ] **AC6**.顶栏徽章 `count === 0` 时 `v-if` 隐藏;`count ≥ 1` 显示且脉冲。
- [ ] **AC7**.跨 project:session B 属另一 project,当前 project 的 sidebar 看不到 B,但**顶栏徽章仍显示总数**;toast 仍弹(文案为"另一项目的会话")。
- [ ] **AC8**.toast 点击(同 project 目标):`switchSession` 切到目标 session,inline card 可见;跨 project 目标点击:仅关闭 toast,不跳转。
- [ ] **AC9**.切 session / 切 project 不丢 pending(`pendingBySession` 跨 session 保留,回归现有 ensureLoaded 机制)。
- [ ] **AC10**.vitest:store computed(count)、SessionList 角标渲染(按 kind)、toast sessionId 跳转逻辑(同 project 切/跨 project 不切)均有用例。

## Out of Scope

- `useErrorBus` 真实 toast UI(独立 follow-up)。
- 权限审批并入统一计数(Q1 已定不并入;若后续顶栏徽章需反映"全部待处理"再开 follow-up)。
- 跨 project 点击跳转(Q4 已定不做;需 session→project 映射 = 后端 payload 改动)。
- 标题栏窗口闪烁(`requestUserAttention`)——WSLg/Weston 不可靠(`TitleBar.vue:35`)。
- 声音提醒。
- toast 去重/防抖(同一 pending 只弹一次由"pending 是单次 register 事件"自然保证;短时多 session 连发各自弹一条,可接受)。
