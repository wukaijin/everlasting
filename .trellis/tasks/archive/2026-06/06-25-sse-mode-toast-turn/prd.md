# 前端:放开 SSE 流中 mode 切换 + toast 提示下一 turn 生效

## Goal

把 mode 切换器（Edit / Plan / Yolo）从"流中禁用"改成"流中也可点击",**仅在流中**弹 toast 明示模式将在下一轮 turn 生效(非流中切 mode 立即生效,不需要 toast)。后端零改动。Yolo 仍走 confirm modal 安全门。

## What I already know

- 6 个前端 guard 全部集中在 `app/src/components/chat/ModeSelect.vue` + `app/src/stores/chat.ts`:
  - `ModeSelect.vue:117` `toggleMenu()` early-return
  - `ModeSelect.vue:187` `:disabled="isStreaming"` on button
  - `ModeSelect.vue:182` CSS class `--disabled`
  - `ModeSelect.vue:191-194` title 文案 "Streaming 中,无法切换 Mode"
  - `ModeSelect.vue:242` `YoloConfirmModal` 的 `:disabled="isStreaming"` (Yolo 选项 trigger)
  - `chat.ts:1242` `requestSetMode()` store guard
  - `chat.ts:1278` `confirmYolo()` store guard
  - (实际 6 处: 4 处 ModeSelect.vue + 2 处 chat.ts;YoloConfirmModal 的 :disabled 算第 4 处 ModeSelect)
- 后端 `set_session_mode` 命令全文无 streaming 检查,照单全收
- `app/src-tauri/src/agent/chat_loop.rs:396` 每 turn 开头读 `loaded_session.session.mode`,整 turn 复用同一 mode(影响 system prompt 前缀 / tool defs 过滤 / 每次 `permissions::check` 时的 Tier 3/4 决策)
- 现成 toast 系统已存在:
  - API: `projectsStore.showToast(message, kind?, durationMs?)` — kind = `"info"|"warn"|"error"`, 默认 3500ms
  - 实现: `app/src/stores/projects.ts:70-83`(单例 + timer 重置)
  - 模板: `app/src/components/layout/AppShell.vue:35-42`(`<transition name="toast">`,底部居中浮层)
  - 样式: `AppShell.vue:73-100` 三档 `.toast--warn / --error / --info`
- 现存调用点:`ChatWindow.vue:48`、`chat.ts:611,634,655`、`permissions.ts:234` — store 名为 `projects` 但实际承担"全局轻量通知"角色(注释 "lightweight, no UI library")

## Requirements

- 用户在 SSE 流进行中也能点击 mode 切换器打开 popover
- popover 三档都可点击,无 disabled 状态
- **仅当 SSE 流进行中** mode 切换成功后弹 toast(非流中切 mode 立即生效,trigger button 文字立刻变就是反馈,toast 是噪音)
- toast 文案: "Mode 已切换,将在下一轮 turn 生效"(明示 turn-boundary 语义)
- Yolo 切换仍走 `YoloConfirmModal` 确认门(modal 整体保留);仅删 Yolo 选项 trigger 的 `:disabled`,让流中点 Yolo 也能弹 modal 走确认
- 现有后端 turn-boundary 语义保留(mode 在每 turn 开头读,不在流中途推送) — 不动 `chat_loop.rs`
- toast 用现有 `projectsStore.showToast` 系统,不引第三方库

## Acceptance Criteria

- [ ] 流中点击 mode 切换器 trigger button → popover 正常打开
- [ ] 流中点击 popover Edit / Plan → IPC 透传到后端 + store 乐观更新 + toast 弹出 "Mode 已切换,将在下一轮 turn 生效"
- [ ] 流中点击 Yolo → 弹 `YoloConfirmModal` 走确认门 → 确认后 IPC 透传 + toast 弹出
- [ ] **非流中**点击 popover 任意一档 → IPC 透传 + store 乐观更新,**不弹** toast
- [ ] `vue-tsc --noEmit` 通过
- [ ] 现有 vitest 通过(无回归)
- [ ] 后端零改动(无新 commit 进 `app/src-tauri/`)

## Implementation Plan

单一小改动,无需拆 PR:

1. **`app/src/components/chat/ModeSelect.vue`**:
   - 删 `toggleMenu()` (line 116-119) 的 `if (isStreaming.value) return;` early-return
   - 删 `:disabled="isStreaming"` (line 187) on button
   - 删 `:class` 中 `'mode-select__trigger--disabled': isStreaming` (line 182)
   - 删 `.mode-select__trigger--disabled` CSS 块 (line 304-308 附近)
   - 改 `title` 文案: 不再按 `isStreaming` 分支;统一为 "点击切换当前 session 的 Mode(Shift+Tab 循环)"
   - 删 `YoloConfirmModal` 的 `:disabled="isStreaming"` (line 242)
   - inject `useProjectsStore`
   - `onModePick` 改为: `const ok = await chatStore.requestSetMode(sid, mode); if (ok && isStreaming.value) projectsStore.showToast("Mode 已切换,将在下一轮 turn 生效", "info", 3000);`
   - Yolo 分支: `pendingYoloConfirm` 路径下不弹 toast(等 modal 确认完由 `confirmYolo` 成功后弹)
2. **`app/src/stores/chat.ts`**:
   - `requestSetMode` (line 1237) 删 `if (isCurrentSessionStreaming.value) return false;` (line 1242)
   - `confirmYolo` (line 1274) 删 `if (isCurrentSessionStreaming.value) return;` (line 1278)
   - 在 `confirmYolo` IPC 成功路径后调用 `projectsStore.showToast(...)`(只在流中弹)— 但 chat.ts 不应直接 import projects store,改成由 `ModeSelect.vue` 监听 `pendingYoloConfirm` 从 true → false 的 transition 来弹 toast
3. **`docs/IMPLEMENTATION.md` §4 决策日志**: 补一条 ADR 记录这次放开 + "下一 turn 生效" 的语义边界 + "toast 仅流中弹" 的设计意图

无新文件、无新依赖、无新测试文件。

## Decision (ADR-lite)

**Context**: 前端之前故意在 SSE 流中禁用 mode 切换(`ModeSelect.vue` 注释写明 "matches the backend's 'mode applies on next turn boundary' rule"),但用户希望放开,以便流中也能预切下一轮的 mode。Yolo 仍走 confirm modal 安全门。

**Decision**:
- 放开 UI + store guard,后端不动
- mode 仍按 turn-boundary 生效(读 `chat_loop.rs:396` 的旧语义)
- **toast 仅在流中弹**: 非流中切 mode 立即生效(trigger button 文案变化本身就是反馈);只有流中切 mode 才会有"已切换但不立即生效"的预期差,toast 是必要的 UX 锚点
- Yolo 走 `YoloConfirmModal` 安全门保留,仅放开 trigger 的 disabled

**Consequences**:
- 同一 turn 内所有 tool_use 仍按旧 mode 走(不会因为中途切 yolo 立刻 bypass 弹窗)
- toast 仅在流中弹 — 非流中模式切换视觉反馈直接靠 button 文案变化,零额外噪音
- 后续如果真的需要"流中途立即生效",需要改 `chat_loop.rs` 把 `session_mode` 改成每次 tool_use 重新读 — 当前不做

## Out of Scope

- 改后端 `chat_loop.rs` 让 mode 在同一 turn 内也能即时生效
- 改 `set_session_mode` IPC 添加并发控制 / 版本号 / 乐观锁
- 给 `projectsStore` 改成更通用的 `notificationStore`(命名问题先不动)
- 加新 toast 库(继续用现有轻量实现)
- 把 `YoloConfirmModal` 整体移除(Yolo 进入仍必须二次确认)

## Resolved Questions

**Q1**: chat.ts 的两个 store guard 怎么处理?
- **决议**: 全删(`requestSetMode:1242` 流中 guard 删,`confirmYolo:1278` 流中 guard 删)。store 不在前端偷偷吞 IPC,IPC 透传到后端照单全收 + 持久化。

**Q2**: toast 触发条件?
- **决议**: **仅在流中弹**。非流中切 mode 立即生效(button 文案变化即反馈),弹 toast 是噪音。Yolo 路径(走 modal)也在 modal 确认后由 `ModeSelect.vue` 监听 `pendingYoloConfirm` 翻转来弹,而不是在 `chat.ts` 内调 projects store。

**Q3**: Yolo confirm modal 删不删?
- **决议**: 整体保留(安全门),仅删 Yolo 选项 trigger 的 `:disabled="isStreaming"`,让流中点 Yolo 也能弹 modal 走确认。

## Technical Notes

- 文件: `app/src/components/chat/ModeSelect.vue` + `app/src/stores/chat.ts` + `docs/IMPLEMENTATION.md`
- `ModeSelect.vue` 当前在 chat input 行内,不是 settings tab
- `projectsStore` 在 `ModeSelect.vue` 中需要 inject(目前未引入,需加 `import { useProjectsStore } from '@/stores/projects'`)
- Toast 文案: "Mode 已切换,将在下一轮 turn 生效",kind=info,duration=3000ms
- Yolo modal 路径的 toast 触发: 在 `ModeSelect.vue` 用 `watch(() => chatStore.pendingYoloConfirm, ...)` 监听 false 翻转;或更简单 — 让 `confirmYolo` 返回一个 promise 让 `ModeSelect.vue` await 后再决定是否 toast。后者更显式,采纳后者。
- toast 调用集中放在 `ModeSelect.vue`(不进 `chat.ts` store),避免 store 间的隐式耦合