# 设计:跨 session 待处理交互 UI 提醒

## 1. 边界

**纯前端,零后端改动。** 全部复用已有数据与机制:

| 复用项 | 位置 |
|---|---|
| 跨 session pending 数据(已全量) | `useQuestionCardsStore.pendingBySession`(`questionCards.ts:81`) |
| IPC 监听(已接) | `streamController` `handleModeChangeRequest`(`:1448`)/ `handleToolQuestion`(`:1422`) |
| 跨 session 角标范式(CSS + 脉冲) | `SessionList` `session-item__pending-approval`(`:441/520`) |
| 全局 toast(单条 TTL + transition) | `projectsStore.showToast`(`projects.ts:71`)+ AppShell 渲染(`AppShell.vue:35-43`) |
| tagged union 分发依据 | `PendingInteraction.kind`(`questionCards.types.ts:309`) |

不新增 IPC、不改后端 payload、不引入 session→project 映射(Q2/Q4 已规避)。

## 2. 三档数据流

```
后端 emit("mode:change:request" | "tool:question", payload)   ← 全局广播(已存在)
        │
        ▼
streamController.handleModeChangeRequest / handleToolQuestion
        │
        ├─ addPending(session_id, {kind, payload})   ← 已存在
        │
        └─ [新] if payload.session_id !== chatStore.currentSessionId:
                  projectsStore.showToast(文案, "info", { sessionId })   ← C 档触发
                                                                        │
                                                                        ▼
                                          AppShell toast 渲染(已存在)+ [新] 点击 → switchSession(同 project)

questionCards.pendingBySession (reactive Map)
        │
        ├─ SessionList: getPending(s.id)?.kind → 角标(mode=refresh / question=info)   ← A 档
        │
        └─ AppHeader PendingBadge: pendingBySession.size → 数字徽章                     ← B 档

pending resolve/removePending (已有) → Map 变更 → 角标/徽章 reactive 消失              ← 自动
```

## 3. 改动清单(文件 → 改动)

| 文件 | 改动 | 档 |
|---|---|---|
| `app/src/stores/projects.ts` | `ToastMessage` 加可选 `sessionId?: string`;`showToast(message, kind, opts?: {sessionId?})` 第三参数(可选,向后兼容) | C |
| `app/src/components/layout/AppShell.vue` | toast 点击 handler:若 `toast.sessionId` 且属当前 project(`chatStore.sessions.some(s=>s.id===sid)`)→ `switchSession(sid)`;否则 `dismissToast` | C |
| `app/src/stores/streamController.ts` | `handleModeChangeRequest` / `handleToolQuestion` 在 `addPending` 后加 toast 触发判断(`payload.session_id !== currentSessionId`) | C |
| `app/src/components/SessionList.vue` | import `useQuestionCardsStore`;两处 `session-item__title-row` 加角标 `v-if="qc.getPending(s.id)"`,按 `entry.kind` 选 Icon + 配色 | A |
| `app/src/components/layout/AppHeader.vue`(或新建 `PendingBadge.vue`) | 新增顶栏徽章:`count = pendingBySession.size`,`v-if="count>0"`,脉冲 + 点击 switchSession(当前 project 最近 pending) | B |
| `app/src/utils/*.test.ts` 或 `app/src/stores/*.test.ts` | vitest:count computed、SessionList 角标 kind 分发、toast 同 project 跳转/跨 project 不跳 | 测 |

无 Rust 改动 → 不需要 `cargo check/test`。

## 4. 关键设计决策

### 4.1 A 档角标 kind 分发 + 配色
- `entry.kind === "mode_change"`:`<Icon name="refresh" :size="12" />`,边色按 `payload.target_mode`(edit=蓝 `--color-accent`/plan=青 `--color-tool-read`/yolo=红 `--color-tool-error`,沿用 `RequestModeChangeCard` 的 `.mode-card--{edit,plan,yolo}` 左边栏映射)。
- `entry.kind === "question"`:`<Icon name="info" :size="12" />`,中性 accent。
- 复用 `session-item__pending-approval` 的 `pulseDot` 动画(`SessionList.vue:802`)。新增 class `session-item__pending-interaction`(或复用 pending-approval 的视觉,仅图标不同)。

### 4.2 B 档顶栏徽章位置
**推荐**:挂在 `TitleBar` 的 `titlebar__content` slot 内末尾(`HiddenProjectsMenu` 之后,`AppHeader.vue:44`),CSS `margin-left: auto / flex` 推到 content 区右侧(视觉落在 ProjectTabs 与窗口控件之间的 `titlebar__spacer` 边缘)。slot 是 `data-tauri-drag-region="false"`(`TitleBar.vue` slot 注释),可点击。
**备选**(若 content flex 不便推右):PendingBadge 放 `AppHeader` 的 `absolute top-right`,Win/Linux 用 `right` 让出 `titlebar__controls` 宽度(~120px),macOS 贴右边。z-index 高于 drag region。
徽章形态:accent 圆角小 chip + 数字 + `pulseDot` 脉冲;`title="N 个会话有待处理交互"`。

### 4.3 C 档 toast 扩展(向后兼容)+ 跳转
- `ToastMessage = { message: string; kind: ToastKind; sessionId?: string }`(`sessionId` 可选 → 现有调用不受影响)。
- `showToast(message, kind = "info", opts?: { sessionId?: string })`。
- AppShell 点击:同 project(`chatStore.sessions` 含该 sid)→ `chatStore.switchSession(sid)` + dismiss;跨 project(不在 sessions)→ 仅 dismiss(Q4)。`projectsStore.toast` 的 click 现状是 `dismissToast`(`AppShell.vue:39`),改为带跳转判断的 handler。

### 4.4 角标并存 + 不溢出
后端 single-pending mutex 只在 QuestionStore 内(mode/question 互斥);permission pending(PermissionStore)与之独立 → 同一 session 最多 1 个 shield-check + 1 个 refresh/info,`session-item__title-row`(flex)两图标并列。compact 密度下需验证不溢出(测试 + 必要时 `flex-shrink:0` + 间距)。

### 4.5 pending=0 隐藏 + reactive 消失
- 徽章 `v-if="count > 0"`(count=0 不渲染,无副作用)。
- 角标 `v-if="qc.getPending(s.id)"`。
- 用户在 inline card 允许/拒绝 → `resolveModeChange` / `resolveToolQuestion` → `removePending` → Map 删除 → reactive 自动让角标/徽章消失(`count--` 或归零隐藏)。**无需额外清理逻辑**。

### 4.6 跨 project 文案降级
toast 文案需要 session 标题;跨 project session 不在 `chatStore.sessions`:
- 同 project:`「${title}」申请切换到 ${target_mode} 模式` / `「${title}」有问题等你回答`。
- 跨 project(session 不在 sessions):`另一项目的会话申请切换到 ${target_mode} 模式` / `另一项目的会话有问题等你回答`。
- 标题取:`chatStore.sessions.find(s=>s.id===sid)?.title`。

## 5. 兼容性与回滚

- **向后兼容**:`showToast` 第三参数可选、`ToastMessage.sessionId` 可选 → 所有现有 toast 调用(`projects.ts:170/191/204` 等)零改动。
- **新组件/角标 v-if 守卫**:count=0 / getPending=undefined 时不渲染 → 无 pending 时零视觉/行为变化。
- **回滚**:单 PR 涉及 5 个前端文件 + 测试;回滚 = revert 该 PR,无 DB/IPC/后端残留状态(pending 状态本就在 QuestionStore,不受 UI 改动影响)。
- **回归风险点**:
  - toast 点击 handler 改动影响**所有** toast(含项目操作 toast)→ 这些 toast 无 `sessionId`,走 `else dismissToast` 分支,行为不变(测试覆盖)。
  - `handleModeChangeRequest`/`handleToolQuestion` 加 toast 不改 `addPending` 既有逻辑(只在其后追加)。
