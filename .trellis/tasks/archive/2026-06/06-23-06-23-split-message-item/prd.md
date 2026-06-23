# 拆分 MessageItem.vue — edit mode + footer

## Goal

`components/chat/MessageItem.vue` 当前 **1099 行**,承载三类独立 UI 模式(streaming / edit / static)+ 三个独立 UI 段(bubble / tool cards / footer)。本次拆出两块耦合度低、状态自包含的子组件:

1. **`MessageItemEdit.vue`**:user 消息的 inline 编辑模式(textarea + Save / Cancel + inline error)
2. **`MessageItemFooter.vue`**:assistant / user 消息通用的底部两联(error footer + F5 latency chip;`(edited)` 标签留在主组件内,见 ADR)

主组件降为 ~770 行的 orchestrator(li 容器 + bubble + tool cards + thinking 块 + FileInjectionsHint + 编排三种子视图)。

## What I already know

* **`MessageItem.vue` 行号映射**(实测):
  - 行 1–57:注释 + import
  - 行 59–62:`defineProps<{ message: ChatMessage }>()`
  - 行 67–89:`hasVisibleBubble` / `showBubble` / `showStreamingHint` 三个 computed(主组件留)
  - 行 91–105:`VIRTUAL_TOOLS` + `visibleToolCalls`(主组件留,跟 thinkingBlock 一起)
  - 行 115–125:`isStreaming` computed(主组件留)
  - 行 127–271:**D3 PR2 inline edit 状态机全部**:editingMessageSeq / isEditingThisMessage / editBuffer / isSaving / editError + watch + onEdit / onResend / cancelEdit / saveEdit
  - 行 273–311:markdown pipeline(createDebouncedRenderer + watch + flush + dispose)
  - 行 285–287:`displayContent` computed —— **拆分后删除(冗余闸门,见 ADR)**
  - 行 313–389:**F5 latency footer 全套** + D3 PR3 `(edited)` label 状态(edited 部分主组件留)
  - 行 393–615:模板(li + MessageActionsMenu + ThinkingBlock + redacted notice + tool cards + editor block + bubble + FileInjectionsHint + error footer + latency)
  - 行 617–1099:scoped CSS(msg__editor 全套 ~110 行 / msg__error ~14 行 / msg__edited ~22 行 / msg__latency + tooltip ~70 行)
* **行数预算**(粗):
  - 抽 A(edit):`MessageItemEdit.vue` ~180 行(70 状态机 + 30 template + 80 CSS)
  - 抽 B(footer):`MessageItemFooter.vue` ~120 行(error + latency + tooltip,无 `(edited)`)
  - 主组件余下:`MessageItem.vue` ~770 行(注释 30 + 状态 60 + bubble + tool cards + thinking + 模板 200 + 残余 CSS 480;`displayContent` 删 2 行)
* **外部消费清单**:`grep "from.*MessageItem" app/src` 仅 `MessageList.vue` 一处(`<MessageItem :message="m" />`);不涉及 store / IPC / 路由 API 变更。
* **依赖边界**:
  - edit 子组件:**仅父传 props + emits**(无 store import),`MessageItemEdit.vue` 内部不调 `chatStore.editMessage` / `chatStore.resendMessage`,全部由 `MessageItem.vue` 编排
  - footer 子组件:接 props 渲染,内部 `abbreviateDuration` 来自 `utils/duration`(已有单测)
* **既有 precedent**:`06-23-06-23-split-chat-types` PRD 模板(已复用其 `## What I already know` / `## Decision (ADR-lite)` / `## Technical Notes` 三段结构)。

## Requirements

### 必含(明确要抽)

* 行 127–271 的全部 edit 状态机 + 4 个 handler(**外移后由父组件编排**)
* 行 477–518 的 `.msg__editor` 模板块 → `MessageItemEdit.vue`
* 行 762–871 的 `.msg__editor*` CSS → `MessageItemEdit.vue`
* 行 313–389 的 footer 状态(5 个 computed)→ `MessageItem.vue` 算好后传 prop 给 `MessageItemFooter.vue`
* 行 575–578 的 `.msg__error` 模板块 → `MessageItemFooter.vue`
* 行 591–613 的 `.msg__latency` + TooltipProvider 模板块 → `MessageItemFooter.vue`
* 行 873–885 + 902–964 的 `.msg__error*` / `.msg__latency*` CSS → `MessageItemFooter.vue`(`:deep()` tooltip 仍需)
* 两个新组件的 vitest 测试文件:`MessageItemEdit.test.ts` + `MessageItemFooter.test.ts`

### 主组件留

* `hasVisibleBubble` / `showBubble` / `showStreamingHint` / `visibleToolCalls` / `isStreaming` 五个 computed(bubble / tool cards / thinking 的编排需要)
* `displayContent` 闸门 **删除**(见 ADR-4)
* markdown pipeline(createDebouncedRenderer + watch + flush + dispose)直接监听 `props.message.content`
* `showLatency` / `editedAt` / `showEditedLabel` / `latencyTotalLabel` / `latencyRows` 计算后传给 footer
* `<ThinkingBlock>` / `<ToolCallCard>` / `<FileInjectionsHint>` / `<MessageActionsMenu>` 调用
* `(edited)` 标签 JSX 留在 bubble 内(行 542–549,testid `msg-edited-label` 不动)
* `.msg__edited` CSS(行 751–760)留在主组件
* li 容器 + bubble 渲染 + `:hover` 触发 actions
* store 调用 `chatStore.editMessage` / `chatStore.resendMessage` / `chatStore.editingMessageSeq` / `chatStore.currentSessionId` 仍归主组件

## Acceptance Criteria

* [ ] 新文件 `components/chat/MessageItemEdit.vue` 存在,~180 行,只接收 props + emit,不 import store
* [ ] 新文件 `components/chat/MessageItemFooter.vue` 存在,~120 行,只接收 props 渲染,无 emit
* [ ] `MessageItem.vue` 行数降到 ≤ 800 行(目标 ~770 行)
* [ ] `MessageItem.vue` 内 `displayContent` 计算删除,markdown pipeline watcher 直接监听 `props.message.content`
* [ ] `MessageItem.vue` 模板不再有 inline `.msg__editor` / `.msg__error` / `.msg__latency` 模板块
* [ ] 既有 testid 全部保留:`msg-editor-textarea` / `msg-editor-cancel` / `msg-editor-save` / `msg-editor-error` / `msg-edited-label`
* [ ] `app/src/components/chat/MessageItemEdit.test.ts` 覆盖:save emit / cancel emit / resend emit / trim 空内容拒绝 / trim 空白 / same-content no-op 走 cancel / editError 渲染 / disabled 态(`isSaving`)/ streaming 守卫
* [ ] `app/src/components/chat/MessageItemFooter.test.ts` 覆盖:role × streaming × latency × error × editedAt 条件渲染矩阵 / latency tooltip 行数(ttfb/gen/total 各自可选)/ `abbreviateDuration` 集成
* [ ] `pnpm --filter app exec vue-tsc --noEmit` 全绿
* [ ] `pnpm --filter app exec vitest run` 全绿(包括新增的 2 个 .test.ts)
* [ ] `pnpm --filter app build` 全绿
* [ ] 视觉零回归:(`(edited)` 标签仍在 bubble 右下 / latency chip 仍在 bubble 右下 / error footer 仍在 bubble 下方)
* [ ] 内存 / 卸载无回归:`MessageItem.vue` 的 `onUnmounted(() => dispose())` 保留,新组件无新监听器需要 dispose

## Definition of Done

* 两个新子组件就位,主组件降体成功
* 新增 2 个 vitest 测试文件全绿
* 类型检查 + 完整构建全绿
* commit message:`refactor(chat): extract MessageItemEdit + MessageItemFooter from MessageItem.vue`
* 视觉回归:`(edited)` / latency / error 三块位置与拆分前像素级一致
* 不引入新的 store import 到子组件(ADR-1 锁定)

## Technical Approach

* **零 store 行为变更**:所有 `chatStore.editMessage` / `resendMessage` / `showToast` 调用保留在 `MessageItem.vue`;子组件只承担 render + emit
* **Props shape(锁定)**:
  - `MessageItemEdit` props:`{ seq: number; content: string; isStreaming: boolean; currentSessionId: string | null; isEditingThisMessage: boolean }`
  - `MessageItemEdit` emits:`{ save: [trimmed: string]; cancel: []; resend: [] }`
  - `MessageItemFooter` props:`{ role: 'user' | 'assistant'; streaming: boolean; latency?: Latency | undefined; error?: ChatError | undefined }`
* **CSS 命名**:保留既有 `msg__editor*` / `msg__error*` / `msg__latency*` 类名,仅做物理迁移(grep 确认无外部引用,迁移零回归)
* **Markdown 闸门简化**:删 `displayContent` computed,watcher 直接监听 `props.message.content`;bubble `v-if` 已经保证 edit 模式时 bubble 不挂载,行为等价
* **reka-ui portal `:deep()`**:latency tooltip 的 4 个 `:deep(.msg__latency-tooltip*)` 规则迁移到 footer 的 scoped CSS(spec `reka-ui-usage.md` 已有 gotcha 覆盖)
* **测试隔离**:新组件无副作用,可独立 mount + props 测;无需 mock Pinia store
* **PR 策略**:单 PR + 多 commit,commit 顺序:(1) 抽 `MessageItemEdit.vue` + 主组件减码 + 测试 (2) 抽 `MessageItemFooter.vue` + 主组件减码 + 测试 (3) `displayContent` 闸门删除(可在 (2) 合并做)

## Out of Scope

* 改 MessageItem.vue 的 bubble / tool cards / thinking 渲染(本次只搬 footer 和 editor)
* 拆 `MessageActionsMenu.vue`、`<ThinkingBlock>`、`<ToolCallCard>`、`<FileInjectionsHint>` 等其他子组件
* 改 `chatStore.editMessage` / `resendMessage` / `editingMessageSeq` 的 API
* 改 `(edited)` 标签的视觉位置(留 bubble 内,见 ADR-2)
* 改 latency chip 的 tooltip 内容或样式
* 改 store 与 IPC 交互(`editMessage` / `resendMessage` 后端路径不在本任务范围)
* 改 `utils/duration.ts`(`abbreviateDuration` 已有单测,本任务纯消费)
* 引入 `*.types.ts` 之类的命名约定到其他组件(本任务只动 chat 组件层)

## Decision (ADR-lite)

**Context**:四个连续边界决策需要锁定。

**Decision**:

* **ADR-1 API 形状 = Option A(纯展示,parent 调 store)** — 子组件不直接 import store;`MessageItemEdit` 接 `seq / content / isStreaming / currentSessionId / isEditingThisMessage`,emit `save(trimmed) / cancel / resend`;`MessageItemFooter` 接 `role / streaming / latency / error`,纯展示无 emit;主组件仍是 store 唯一消费方。
* **ADR-2 `(edited)` 标签位置 = 留在 bubble 内** — `MessageItemFooter.vue` 只装 error + latency;`(edited)` 标签继续在 `MessageItem.vue` 模板里、bubble div 内部;视觉零回归,testid `msg-edited-label` 不动。
* **ADR-3 测试覆盖 = 两个组件都加完整 vitest** — `MessageItemEdit.test.ts`(save / cancel / trim / empty / same-content no-op / editError 渲染 / disabled 态 / streaming 守卫)+ `MessageItemFooter.test.ts`(role × streaming × latency × error × editedAt 条件渲染矩阵 / latency tooltip 行数)。
* **ADR-4 markdown 闸门 = 删除 `displayContent` computed** — bubble `v-if` 已保证 edit 模式时 bubble 卸载,markdown pipeline 的 `rendered` 输出无处显示,gate 冗余;watcher 直接监听 `props.message.content`。

**Consequences**:
- ✅ 与 `MessageActionsMenu.vue` 既有"parent 编排、child 渲染"约定一致
- ✅ 子组件无副作用、可独立测,测试无需 mock Pinia store
- ✅ `MessageItemFooter` ~120 行(error + latency + CSS,不含 `(edited)`);`MessageItemEdit` ~180 行
- ✅ 视觉零回归(`(edited)` 位置、latency tooltip、error footer 三块都像素级一致)
- ✅ 既有 testid 全保留,无 e2e 选择器回归
- ✅ Markdown pipeline 简化 2 行,意图更清晰
- ⚠️ 主组件 template 多保留 ~20 行 `(edited)` JSX + 22 行 `.msg__edited` CSS 不搬到 footer
- ⚠️ 测试基础设施新增 2 个 .test.ts(~250 行);覆盖在 vitest.config.ts 默认 glob 内,无需配置改动
- ⚠️ 单 PR 多 commit 顺序需谨慎:editor 与 footer 都在 template 里改 v-if 分支,合并冲突风险低,但要在 commit message 写清拆分边界

## Open Questions

*(已全部收敛 — 4 项决策见 ADR-lite)*

## Technical Notes

* 主文件:`app/src/components/chat/MessageItem.vue`(1099 行 → ~770 行)
* 新增文件:`app/src/components/chat/MessageItemEdit.vue`(~180 行)+ `app/src/components/chat/MessageItemFooter.vue`(~120 行)+ 2 个 .test.ts(~250 行)
* 外部消费:`app/src/components/chat/MessageList.vue` 仅一处(MessageItem 公共 API 不变)
* 关联 store:`stores/chat.ts`(editMessage / resendMessage / editingMessageSeq / currentSessionId;**仍在主组件调用**)、`stores/projects.ts`(showToast;**仍在主组件调用**)
* 关联 util:`utils/duration.ts`(abbreviateDuration,已有单测 — 子组件直接消费)
* 既有 precedent:`.trellis/tasks/archive/2026-06/06-23-06-23-split-chat-types/prd.md`(本任务 PRD 复用其结构)
* spec 索引:`.trellis/spec/frontend/`(组件拆分 + scoped CSS + reka-ui portal `:deep()` 都有覆盖)
* 测试坑提醒:reka-ui `Tooltip` portal 跨 test leak(memory `subagentdrawer-banner-test-gotchas.md` 中提到的 Icon stub textContent 陷阱、`vi.useFakeTimers` 影响 Date 解析);`MessageItemFooter.test.ts` 涉及 tooltip 内容断言,务必在 `afterEach` 里 unmount + cleanup,避免 popup DOM 跨用例污染