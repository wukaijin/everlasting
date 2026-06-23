# 拆分 ChatInput.vue — 抽 CodeMirror composable + LatencyPopover + HintRow

## Goal

`app/src/components/chat/ChatInput.vue` 当前 **1834 行**,承载 4 个独立关注点:
1. CodeMirror 6 宿主 + keymap + IME 编排(host / view / compartments / updateListener / Enter / Shift+Tab / 提交 hook)
2. `/` 命令 + `@` 文件 触发器检测与状态机(currentSlashToken / currentAtToken / detect* / sync* / close* / replaceDoc / panel state)
3. LLM 累计耗时 chip + click popover(state / toggle / onDocumentClick / Esc / body 模板)
4. 底部 hint row 编排(ModeSelect / LatencyChip / token chip reka-ui Tooltip / ModelSelect)

本次按 **B 方案 + A 方案**(已与用户 confirm)拆出 3 个独立单元:

- **`chatInputCodeMirror.ts` composable** — 接收 `host` + `props.sending` + `props.placeholder`,返回 `{ view, input, replaceDoc, currentSlashToken, currentAtToken, detectCommandTrigger, detectFileTrigger, syncCommandPalette, syncFilePalette, closeCommandPalette, closeFilePalette }`,内部封装 CM 6 状态机 + 命令/文件触发器检测(state machine)。主组件调用 dispatch handler(`onCommandSelect` / `onFileSelect` 留主组件碰 store/Tauri)。
- **`ChatInputLatencyPopover.vue` 子组件** — 自包含:chip 按钮 + 浮层 body + open state + onDocumentClick + Esc + body + Transition。0 store import,纯 props 渲染(`currentSessionLatencyTotal` / `currentSessionLatencyTurns` 由父传)。
- **`ChatInputHintRow.vue` 子组件** — 接收 `currentSessionId`(决定是否渲染 LatencyPopover)+ `currentSessionTokenUsage` + `currentModelContextWindow` + `usageLevel`,内部 embed `<ChatInputLatencyPopover />` + 自带 token chip(reka-ui TooltipProvider/Root/Trigger/Portal/Content/Arrow)+ `<ModelSelect />`。

主组件保留:
- `defineProps<{ sending, placeholder }>()` + `defineEmits<{ send, stop }>()` — 公共 API 不变
- `inputRowStyle` computed(消息框 style)
- `onSubmit` / `onStop` / `sendDisabled` / `onEscKeydown` — 提交/stop/Esc 编排
- `currentModelContextWindow` / `usageLevel` — 计算后传给 HintRow(子组件不读 store)
- `<ModeSelect />`(已在 hint row 之外,留在 input row 左)
- `<TriggerMenu />` × 2(命令/文件 palette)— 命令面板状态由 composable 内部管,模板里依然 render
- CM 宿主 `<div ref="host">` + send/stop button + 消息框 row 容器
- `displayContent` 闸门(如存在 — 删,见 ADR-4 镜像 split-message-item)

主组件行数目标:**~1000 行**(从 1834 减 ~830 行)。

## What I already know

### 文件现状(实测 `wc -l`)

- `app/src/components/chat/ChatInput.vue` = **1834 行**(超 1833 任务描述 +1 行差异,可忽略)
- `app/src/components/chat/chatInputTokens.ts` = 171 行(既存编辑器扩展,纯 Decoration plugin,本次不动)
- `app/src/components/chat/ModeSelect.vue` = 411 行(自包含,本次不动)
- `app/src/components/chat/ModelSelect.vue` = 374 行(自包含,本次不动)
- `app/src/components/chat/TriggerMenu.vue` = 既有,本次不动
- **无 ChatInput 既存测试**(`grep -r "ChatInput" app/src/**/*.test.ts` 为空)

### ChatInput.vue 行号映射(锚点)

| 区块 | 行号 | 行数 | 备注 |
|---|---|---|---|
| 头注释 + imports | 1-107 | 107 | 顶部注释 PR1.5/PR5/F5 链路 + 13 个 import |
| props/emits + initial state | 109-153 | 45 | input ref / host ref / view shallowRef / 2 个 Compartment |
| stores + computeds | 155-204 | 50 | chatStore / modelsStore / projectsStore / currentModelContextWindow / usageLevel / inputRowStyle |
| **Latency popover state** | 205-264 | 60 | latencyPopoverOpen / latencyPopoverRoot / toggle / onDocumentClick / onKeyDown / onUnmounted |
| latencyTurns + latencyAverage | 265-304 | 40 | store 派生 computed |
| **CM setup** | 305-475 | 170 | onEditorUpdate / handleEnter / buildKeymap / onMounted 创建 view / onUnmounted 销毁 / watch editable / watch placeholder |
| onSubmit / onStop | 477-521 | 45 | 提交/停止 |
| `// --- command palette ---` 分隔 | 523 | 1 | 视觉锚点 |
| **Command palette state** | 547-852 | 306 | triggerMenu ref / commandPaletteOpen / commandItems / commandFilter / commandsLoaded / onCommandSelect / currentSlashToken / detectCommandTrigger / closeCommandPalette / syncCommandPalette / replaceDoc(共享给 file) |
| `// --- file palette ---` 分隔 | 854 | 1 | 视觉锚点 |
| **File palette state** | 855-1017 | 163 | fileTriggerMenu / filePaletteOpen / fileItems / fileFilter / filesLoaded / onFileSelect / currentAtToken / detectFileTrigger / closeFilePalette / syncFilePalette |
| submit / sendDisabled / onEscKeydown | 1019-1045 | 27 | 提交辅助 |
| `<template>` | 1047-1321 | 275 | 含 ModeSelect / 2 个 TriggerMenu / CM host / send+stop / Latency 块 / token Tooltip / ModelSelect |
| `<style scoped>` | 1323-1834 | 512 | chat-input 全套样式 |

### 外部消费清单

- `app/src/components/chat/ChatPanel.vue:43` import + `:490` mount `<ChatInput :sending="..." :placeholder="..." @send="..." @stop="..." />`
- 公共 API(`sending` / `placeholder` / `send` / `stop`)**不变**;不动 ChatPanel
- 无后端 IPC / store / 路由 API 变更

### 依赖边界

- **composable `chatInputCodeMirror.ts`**:
  - 输入:`host: Ref<HTMLDivElement | null>` + `props: { sending: Ref<boolean>; placeholder: Ref<string | undefined> }`
  - 输出:`{ view: ShallowRef<EditorView | null>; input: Ref<string>; replaceDoc: (newDoc: string, caret?: number) => void; currentSlashToken: () => { line, from, to } | null; currentAtToken: () => { line, from, to } | null; detectCommandTrigger: () => { trigger: boolean; filter: string }; detectFileTrigger: () => { trigger: boolean; filter: string }; syncCommandPalette: () => void; syncFilePalette: () => void; closeCommandPalette: () => void; closeFilePalette: () => void; submit: () => boolean /* true if Enter was consumed by CM */ }`
  - **无 store import**(composable 不碰 chatStore / modelsStore / projectsStore)
  - **dispatch handler 不在内**:`onCommandSelect` / `onFileSelect` 由主组件编排(碰 Tauri `invoke` + `chatStore.send`)
  - 副作用:onMounted 时创建 EditorView,onUnmounted 时 `view.destroy()`(生命周期安全)
- **`ChatInputLatencyPopover.vue`**:
  - props:`{ totalMs: number | null; turns: LatencyTurn[] | null }`
  - **无 emit / 无 store import**(纯展示;chip 点击 + outside-click + Esc 全内部)
  - 复用 `utils/duration.abbreviateDuration`(既存)
- **`ChatInputHintRow.vue`**:
  - props:`{ tokenUsage: TokenUsage | null; contextWindow: number; usageLevel: TokenUsageLevel | null; currentSessionId: string | null }`
  - 内部 embed `<ChatInputLatencyPopover :total-ms="..." :turns="..." />` + 自带 token tooltip(reka-ui Tooltip* 系列,跟随 chatInputTokens.ts 注释里说的「reka-ui Tooltip 也可消费」)+ `<ModelSelect />`
  - **无 emit**

### 既有 precedent

- `06-23-06-23-split-message-item` PRD:本任务复用其「行号映射 + ADR-lite + 子组件无 store import」骨架
- `06-23-06-23-split-subagent-drawer` PRD:本任务复用其「子组件 defineProps 锁定 + 行为不变硬约束」
- `.trellis/spec/frontend/popover-pattern.md`:**LatencyPopover 自包含 chip+popover 完全对应**「hand-rolled popover pattern」第 1-4 节(root ref / toggle / outside-click / Esc)
- `.trellis/spec/frontend/reka-ui-usage.md`:`<style scoped>` + portal `:deep()` gotcha — LatencyPopover 不需要(reka-ui 不用),HintRow 的 token tooltip portal 走 `:deep(.chat-input__token-tooltip*)` 规则集
- `chatInputTokens.ts`:命名约定 `chatInput*` 既有先例,`chatInputCodeMirror.ts` 与之对齐

## Requirements

### 必抽(明确外移)

| 块 | 源行号 | 去向 | 行数预算 |
|---|---|---|---|
| Latency popover state(toggle / onDocumentClick / onKeyDown / onUnmounted) | 205-264 | `ChatInputLatencyPopover.vue` | 60 |
| Latency chip button + popover 模板 + scoped CSS(`chat-input__latency*`) | 1158-1251 + 1524-1653(估) | `ChatInputLatencyPopover.vue` | 90 |
| CM setup + keymap + IME | 305-475 | `chatInputCodeMirror.ts` | 170 |
| Command palette state(currentSlashToken / detectCommandTrigger / close/sync) | 547-740(切片,不含 onCommandSelect) | `chatInputCodeMirror.ts` | 195 |
| File palette state(currentAtToken / detectFileTrigger / close/sync) | 855-1017(切片,不含 onFileSelect) | `chatInputCodeMirror.ts` | 160 |
| Hint row 模板(ModeSelect 之后的 Latency + token + ModelSelect) | 1152-1319(估) | `ChatInputHintRow.vue` | 170 |
| Hint row scoped CSS(`chat-input__hint*` / `chat-input__token*`) | 1500-1700(估) | `ChatInputHintRow.vue` | 200 |
| `replaceDoc` 函数(共享给命令/文件面板) | 730-740(估) | `chatInputCodeMirror.ts` | 12 |

### 抽后留主组件

- `<script setup>`:`input` ref(被 composable 内化,主组件不再声明)、`props`、`emit`、`chatStore` / `modelsStore` / `projectsStore` 三 store 调用、`currentModelContextWindow` / `usageLevel` / `inputRowStyle` 三个 computed、`onSubmit` / `onStop` / `sendDisabled` / `onEscKeydown` 编排函数
- `<template>`:`<ModeSelect />` + `<TriggerMenu ref="triggerMenu" .../>` + `<TriggerMenu ref="fileTriggerMenu" .../>` + CM 宿主 `<div ref="host">` + send/stop button + row 容器 + `<ChatInputHintRow ... />`(替换原 hint row 内联)
- `<style scoped>`:`chat-input` / `chat-input__row` / `chat-input__field` / `chat-input__action` / `chat-input__send` / `chat-input__stop` 等输入框相关类(留下 ~280 行 CSS)

### 测试面

- **既有测试 = 0**(ChatInput 没有 .test.ts),所以「不改既有测试」自动满足
- **新增测试**(可选,见 ADR-3):
  - `app/src/components/chat/ChatInputLatencyPopover.test.ts`(chip 渲染 / popover open-close / outside-click / Esc / empty state)
  - `app/src/utils/chatInputCodeMirror.test.ts`(composable 单元测:JSDOM + CodeMirror,触发器检测 currentSlashToken / currentAtToken;CM 集成测优先级低,留 follow-up)

## Acceptance Criteria

- [ ] `app/src/utils/chatInputCodeMirror.ts` 存在,~340 行,封装 CM 6 宿主 + keymap + IME + 触发器检测
- [ ] `app/src/components/chat/ChatInputLatencyPopover.vue` 存在,~150 行,自包含 chip + popover + open state + outside-click + Esc,0 store import
- [ ] `app/src/components/chat/ChatInputHintRow.vue` 存在,~370 行(170 模板/逻辑 + 200 CSS),内嵌 `<ChatInputLatencyPopover />` + token tooltip + `<ModelSelect />`,0 store import
- [ ] `ChatInput.vue` 行数 ≤ 1050 行(目标 ~1000,允许 ±50 抖动)
- [ ] `ChatInput.vue` 公共 API 不变(`sending` / `placeholder` / `send` / `stop`),`ChatPanel.vue` 零修改
- [ ] CM 生命周期安全:composable onUnmounted 调用 `view.destroy()`;`host` ref 在 onMounted 时才挂 view(view 未就绪时 `currentSlashToken` 等调用安全返回 null/empty)
- [ ] IME 安全:composable 内 Enter 仍走 CM keymap + `view.composing` 守卫(沿用 line 328-355 `handleEnter` 现有逻辑)
- [ ] 触发器检测不变:`/` 命令面板触发条件(line 654 `detectCommandTrigger`)与 `@` 文件面板触发条件(line 945 `detectFileTrigger`)1:1 保留
- [ ] 视觉零回归:
  - Latency chip 位置(hint row 左)+ popover 浮层位置(从 chip 底部展开,向上)不变
  - Token chip 位置(hint row 中)+ tooltip 内容(4 行 breakdown / "升级前未统计")不变
  - ModelSelect 位置(hint row 右)不变
  - 输入框 row 容器 + CM 宿主 + send/stop button 视觉不变
- [ ] testid 全部保留(若有;grep 确认无遗漏)
- [ ] `pnpm --filter app exec vue-tsc --noEmit` 全绿
- [ ] `pnpm --filter app exec vitest run` 全绿(包括新增的 .test.ts,若有)
- [ ] `pnpm --filter app build` 全绿
- [ ] 不引入新依赖;不修改 `chatInputTokens.ts` / `TriggerMenu.vue` / `ModeSelect.vue` / `ModelSelect.vue`
- [ ] `chatInputCodeMirror.ts` 不 import `useChatStore` / `useModelsStore` / `useProjectsStore`(composable 0 store 依赖)

## Definition of Done

- 3 个新文件就位 + 主组件降体成功
- 公共 API 不变,`ChatPanel.vue` 零修改
- 类型检查 + vitest + 构建全绿
- 视觉零回归(手动核对 3 chip 位置 + popover 展开方向)
- commit message:`refactor(chat): extract chatInputCodeMirror composable + LatencyPopover + HintRow from ChatInput.vue`
- 不引入新依赖 / 不改后端 IPC / 不改 store

## Technical Approach

### Composable 形状(锁定)

```ts
// app/src/utils/chatInputCodeMirror.ts
export interface ChatInputCodeMirrorApi {
  view: ShallowRef<EditorView | null>;
  input: Ref<string>;
  /** Replace the entire CM doc; optionally set caret. Used by command/file panel selection handlers. */
  replaceDoc: (newDoc: string, caret?: number) => void;
  /** Read the `/` token at the caret; null if not on a `/` trigger line. */
  currentSlashToken: () => { line: number; from: number; to: number } | null;
  /** Read the `@` token at the caret; null if not on an `@` trigger line. */
  currentAtToken: () => { line: number; from: number; to: number } | null;
  /** Cheap trigger check used by the parent's watch loop. */
  detectCommandTrigger: () => { trigger: boolean; filter: string };
  detectFileTrigger: () => { trigger: boolean; filter: string };
  /** Pull the current slash/at token into the parent's panel items + filter. */
  syncCommandPalette: () => void;
  syncFilePalette: () => void;
  /** Force-close panels (e.g. on Esc). */
  closeCommandPalette: () => void;
  closeFilePalette: () => void;
  /** Enter handler; returns true if Enter was consumed (composing or submit). */
  submit: () => boolean;
}

export function useChatInputCodeMirror(opts: {
  host: Ref<HTMLDivElement | null>;
  sending: Ref<boolean>;
  placeholder: Ref<string | undefined>;
  onSubmit: () => void;
}): ChatInputCodeMirrorApi
```

Composable 内部:
- `input` ref + `view` shallowRef + 2 个 Compartment(沿用 line 151-153)
- `onMounted`:创建 EditorState(extensions = keymap + IME + updateListener + tokenHighlightPlugin + editable + placeholder Compartment)+ EditorView 挂载到 `host.value`
- `onUnmounted`:`view.value?.destroy()` + 清理 listener
- `watch(sending)`:editableCompartment.reconfigure([EditorView.editable.of(!sending.value)])
- `watch(placeholder)`:placeholderCompartment.reconfigure([cmPlaceholder(placeholder.value ?? "")])
- `submit()`:检查 `view.composing`,true 则返回 true 拦截;否则调 `opts.onSubmit()`(主组件的 onSubmit 读 `input.value.trim()` 后 emit send)
- `currentSlashToken` / `currentAtToken`:`view.value?.state.doc.lineAt(head)` + 手动 regex(沿用 line 599-654 + 884-944 逻辑)
- `detectCommandTrigger` / `detectFileTrigger`:基于 `currentSlashToken` / `currentAtToken` 派生 filter(沿用 line 654-697 + 945-975 逻辑)
- `syncCommandPalette` / `syncFilePalette`:emit `update:command-trigger` / `update:file-trigger`(由主组件 watch,拉 `commandItems` / `fileItems` 然后 panel 自己更新)— 或更简单:composable 直接管理 panel state(commandPaletteOpen / commandItems / commandFilter),主组件只传 items source(回调)。**采用后者**(避免双向 watch),详见 ADR-2。
- `closeCommandPalette` / `closeFilePalette`:`commandPaletteOpen.value = false`(沿用 line 697-712 + 976-985 逻辑)
- `replaceDoc`:`view.value?.dispatch({ changes: { from: 0, to: view.value.state.doc.length, insert: newDoc }, selection: caret !== undefined ? { anchor: caret } : undefined })`(沿用 line 730-740)

### 子组件 props 形状(锁定)

```ts
// ChatInputLatencyPopover.vue
defineProps<{
  totalMs: number | null;
  turns: LatencyTurn[] | null;
}>();
// 无 emit;0 store import

// ChatInputHintRow.vue
defineProps<{
  tokenUsage: TokenUsage | null;
  contextWindow: number;
  usageLevel: TokenUsageLevel | null;
  currentSessionId: string | null;
}>();
// 无 emit;0 store import
```

### 行数目标

| 文件 | 当前 | 目标 |
|---|---|---|
| ChatInput.vue | 1834 | ~1000 |
| chatInputCodeMirror.ts | — | ~340 |
| ChatInputLatencyPopover.vue | — | ~150 |
| ChatInputHintRow.vue | — | ~370 |
| ChatInputLatencyPopover.test.ts | — | ~80(可选) |
| chatInputCodeMirror.test.ts | — | ~120(可选) |

### CSS 命名

- Latency 相关:`chat-input__latency` / `__latency-chip` / `__latency-chip--open` / `__latency-label` / `__latency-value` / `__latency-popover` / `__latency-popover-*` — 全部跟搬到 LatencyPopover.vue
- Hint row 相关:`chat-input__hint` / `__hint-left` / `__hint-center` / `__hint-right` / `__token-usage` / `__token-usage--*` / `__token-tooltip` / `__token-tooltip-row` / `__token-tooltip-empty` / `__token-tooltip-arrow` — 搬到 HintRow.vue(reka-ui Tooltip portal `:deep()` 仍需)
- 留主组件:`chat-input` / `__row` / `__field` / `__field--disabled` / `__action` / `__send` / `__stop` / `__stop-glyph`

### 测试隔离

- LatencyPopover 测试:mount + props 注入 + 模拟 click / document.click / keydown(Esc);无需 mock store(子组件 0 store import)
- composable 测试:JSDOM + 真实 CM 6;触发器检测可用假 doc(`EditorState.create({ doc: "/foo bar" })`)验证 `currentSlashToken` / `detectCommandTrigger` 输出

### PR 策略

单 PR + 多 commit:
1. 抽 `ChatInputLatencyPopover.vue`(最小风险,纯 props)+ 主组件减码 + 视觉回归确认
2. 抽 `ChatInputHintRow.vue`(把 token tooltip + ModelSelect 也搬)+ 主组件减码 + 视觉回归确认
3. 抽 `chatInputCodeMirror.ts` composable(最大块,CM 集成)+ 主组件减码 + CM 生命周期确认
4. (可选)新增 2 个 .test.ts

## Decision (ADR-lite)

**Context**: 4 个连续边界决策需要锁定(已与用户 confirm)。

**Decision**:

* **ADR-1 composable 范围 = B 方案(完整)** — composable 收 CM host + keymap + IME + 触发器检测(currentSlashToken / currentAtToken / detect* / sync* / close* / replaceDoc)+ `submit` Enter 拦截。**onCommandSelect / onFileSelect dispatch handler 留主组件**(碰 Tauri `invoke` + `chatStore.send`)。**0 store import**。主组件 → ~1000 行命中目标。
* **ADR-2 composable ↔ 主组件面板状态通信 = 单向回调 + panel 状态内置** — composable 内部管理 `commandPaletteOpen` / `commandItems` / `commandFilter` / `filePaletteOpen` / `fileItems` / `fileFilter`;主组件通过 `opts.commandItemsSource?: () => TriggerMenuItem[]` 回调(命令)/ `opts.fileItemsSource?: () => TriggerMenuItem[]` 回调(文件)在 `syncCommandPalette` / `syncFilePalette` 时拉取最新 items。这样避免双向 watch,且 composable 仍是 0 store import。`commandsLoaded` / `filesLoaded` flag 也内置。
* **ADR-3 Latency 拆分 = A 方案(自包含 chip+popover)** — `ChatInputLatencyPopover.vue` 自带 chip 按钮 + popover 浮层 + open state + onDocumentClick + Esc + Transition。组件边界清晰。HintRow 只 `<ChatInputLatencyPopover :total-ms="..." :turns="..." />` 一行 embed。LatencyPopover 实际 ~150 行(超任务描述「80 行」,但用户已 confirm A 方案)。
* **ADR-4 测试覆盖 = 新增 2 个 vitest(可选,但推荐)** — `ChatInputLatencyPopover.test.ts`(chip 渲染 / open-close / outside-click / Esc / empty state)+ `chatInputCodeMirror.test.ts`(composable 单元测:currentSlashToken / currentAtToken / detect* / submit 拦截;CM 集成测留 follow-up)。不强制,实施时间允许则加。

**Consequences**:

- (+) 主组件从 1834 → ~1000 行,命中目标,降低单文件复杂度
- (+) Composable 0 store import,可独立测试 + 未来 AppShell Cmd+K 复用
- (+) 子组件纯 props,无 emit,边界清晰,无 store 副作用污染
- (+) 公共 API(`sending` / `placeholder` / `send` / `stop`)不变,`ChatPanel.vue` 零修改
- (+) `chatInputTokens.ts` / `ModeSelect.vue` / `ModelSelect.vue` 零改动
- (+) Composable 内置 panel state,避免双向 watch 的 stale state 风险
- (-) Composable 暴露 API 表面大(~10 个方法),但都是已存在的命令/文件面板所需
- (-) LatencyPopover 实际 ~150 行,超出任务描述「80 行」,但 A 方案下无法更短(已 confirm)
- (-) 单 PR 4 commit,顺序需谨慎:LatencyPopover → HintRow → composable 风险递增

## Open Questions

*(已全部收敛 — 4 项决策见 ADR-lite)*

## Out of Scope

- 改 `chatInputTokens.ts`(既存编辑器扩展,本次不动)
- 改 `TriggerMenu.vue` / `ModeSelect.vue` / `ModelSelect.vue`(自包含子组件)
- 改 `ChatPanel.vue` 的 `<ChatInput>` 调用方式(公共 API 不变)
- 改 command panel 的 items 数据源(仍由 `list_panel_items` IPC 提供)
- 改 file panel 的 items 数据源(仍由 `list_files` IPC 提供)
- 改 `chatStore.send` / `chatStore.cancel` / `currentSessionLatencyTotal` / `currentSessionLatencyTurns` API
- 改 latency chip 视觉位置 / popover 浮层方向(留 hint row 左 + 向上展开)
- 改 token chip 视觉位置 / tooltip 内容(留 hint row 中 + reka-ui Tooltip 4 行 breakdown)
- 改 ModelSelect 视觉(留 hint row 右 + ModelSelect.vue 0 改动)
- 引入 `*.types.ts` 之类的命名约定到其他组件(本任务只动 chat 组件层)
- Composable 复用给 AppShell Cmd+K / 其他输入框(留 follow-up,本任务只服务 ChatInput)
- 改 Composable 单元测试覆盖到 CM 集成测(留 follow-up)

## Technical Notes

### 文件位置

```
app/src/
├── components/chat/
│   ├── ChatInput.vue                        (改:1834 → ~1000)
│   ├── ChatInputLatencyPopover.vue          (新:~150)
│   ├── ChatInputHintRow.vue                 (新:~370)
│   ├── chatInputTokens.ts                   (不动)
│   ├── TriggerMenu.vue / ModeSelect.vue / ModelSelect.vue  (不动)
│   └── SubagentDrawer.vue 等其他兄弟         (不动)
├── utils/
│   ├── chatInputCodeMirror.ts               (新:~340)
│   ├── chatInputCodeMirror.test.ts          (新可选:~120)
│   └── duration.ts / tokenUsage.ts / colorTag.ts / useKeyboard.ts  (不动)
└── stores/
    └── chat.ts / models.ts / projects.ts     (不改 API)
```

### 类型导入

- `chatInputCodeMirror.ts` 需要:Vue(`ref` / `shallowRef` / `watch` / `onMounted` / `onUnmounted` / `computed` / `nextTick`)+ CM 6(`EditorState` / `EditorView` / `Compartment` / `keymap` / `placeholder` / `Prec`)+ `tokenHighlightPlugin`(本地相对路径 `./components/chat/chatInputTokens` 或提取到 utils 后改 `./chatInputTokens`)
- `ChatInputLatencyPopover.vue` 需要:Vue(`ref` / `onUnmounted` / `Transition`)+ `Icon` + `abbreviateDuration`(utils/duration)+ `LatencyTurn` 类型(`stores/chat` export)
- `ChatInputHintRow.vue` 需要:Vue + `Icon` + reka-ui `Tooltip*` 系列 + `ModelSelect` + `ChatInputLatencyPopover` + `abbreviateTokens` + `TokenUsage` / `TokenUsageLevel` 类型(`utils/tokenUsage` export)

### CSS 迁移要点

- `chat-input__latency*`(≈ 130 行)— 整体搬到 LatencyPopover.vue
- `chat-input__hint*` + `chat-input__token*`(≈ 200 行)— 整体搬到 HintRow.vue
- `chat-input__latency-popover` Transition 的 4 条 `@keyframes` / `.chat-input-latency-popover-enter-from` / `-enter-active` / `-enter-to` / `-leave-to` 跟搬到 LatencyPopover.vue
- reka-ui Tooltip portal `:deep(.chat-input__token-tooltip*)` 规则仍需(随 HintRow 一起搬,scoped 范围一致)

### 测试坑提醒

- reka-ui `Tooltip` portal 跨 test leak(参考 `subagentdrawer-banner-test-gotchas.md`):HintRow 若加 .test.ts,务必 `afterEach` 里 unmount + cleanup,避免 popup DOM 跨用例污染
- Icon stub textContent 陷阱:同样适用
- `vi.useFakeTimers` 影响 Date 解析:LatencyPopover 不用时间格式,但若加时间相关断言需注意
- CodeMirror 6 JSDOM 集成:CM 6 在 JSDOM 下可工作(已有 `useCodeMirror` 类似先例参考),但 `view.dom` 需要 `document.body` append;composable 单元测用 `EditorState.create({ doc: "..." })` 跳过 host 挂载,只测纯函数(`currentSlashToken` / `currentAtToken` / `detect*`)