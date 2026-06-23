# Chat Components Frontend Spec

> 主 chat panel + subagent drawer 组件的前端执行性规范。目前覆盖 SubagentDrawer（重构 PR1-6, 2026-06-21）；主 panel 的 MessageItem / ToolCallCard 等待后续补充。

---

## SubagentDrawer (重构 PR1-6, 2026-06-21)

worker subagent 的右侧 drawer。reka-ui `Dialog*` 组合实现（@2.9.9 无 `Sheet` primitive，CSS 成右侧 panel）。**5 段分组折叠视图**，数据源是 store accumulator 的 `liveSections`（不是 raw `liveTranscript`）。

### 文件清单

| 文件 | 职责 |
|---|---|
| `app/src/components/chat/SubagentDrawer.vue` | 顶层容器 + 5 段编排 + ticker + scroll 编排 + 边界态(06-23 拆后 ~900 行,拆分自 1257 行) |
| `app/src/components/chat/SubagentDrawerHeader.vue` | ★ (06-23 拆)header 子组件:status badge + name + close + banner + meta + summary + truncated(无 jump-latest,跳转按钮下移 body) |
| `app/src/components/chat/SubagentDrawerErrorCard.vue` | ★ (06-23 拆)R25 ❌ 错误卡:v-if `status==='error'`、4 级 fallback 的 `errorMessage` |
| `app/src/components/chat/ChatInput.vue` | 主输入框(06-23 拆后 ~712 行,拆分自 1834 行;留 props/emits + 提交编排 + ModeSelect) |
| `app/src/components/chat/ChatInputLatencyPopover.vue` | ★ (06-23 拆)自包含 chip + popover + open state + onDocumentClick + Esc + Transition(0 store import) |
| `app/src/components/chat/ChatInputHintRow.vue` | ★ (06-23 拆)embed `<ChatInputLatencyPopover>` + token reka-ui Tooltip + `<ModelSelect>` |
| `app/src/utils/chatInputCodeMirror.ts` | ★ (06-23 拆)composable:~564 行,封装 CM 6 生命周期 + keymap + IME + 触发器检测(0 store import) |
| `app/src/components/chat/DrawerSection.vue` | 通用折叠容器(thinking/tools/reply 共用),折叠态 lazy render |
| `app/src/components/chat/DrawerPromptCard.vue` | `run.task` prompt 卡片(120 截断 + View full) |
| `app/src/components/chat/DrawerThinkingBlock.vue` | `ThinkingSection` → 共享 `ThinkingBlock` 适配器 |
| `app/src/components/chat/DrawerToolCallCard.vue` | tool call 卡片(复用 ToolInputBody/ToolOutputBody,**不 wrap ToolCallCard**) |
| `app/src/components/chat/DrawerPermissionAskCard.vue` | permission ask 卡片(live interactive + historical outcome badge) |
| `app/src/components/chat/MessageItem.vue` | 主消息项(06-23 拆后 ~770 行,拆分自 1099 行) |
| `app/src/components/chat/MessageItemEdit.vue` | ★ (06-23 拆)user 消息 inline edit 模式(textarea + Save/Cancel + inline error) |
| `app/src/components/chat/MessageItemFooter.vue` | ★ (06-23 拆)assistant/user 通用底部两联(error footer + F5 latency chip) |
| `app/src/utils/transcriptPairing.ts` | `pairSections` section 级配对(snake→camel) |
| `app/src/stores/subagentRuns.ts` | store 主体 + `coerceStatus`(06-23 拆后 ~547 行) |
| `app/src/stores/subagentRuns.types.ts` | ★ (06-23 拆)~354 行类型 + `SUBAGENT_EVENT_DEBOUNCE_MS` |
| `app/src/stores/runAccumulator.ts` | ★ (06-23 拆)~537 行 `RunAccumulator` + `parseTranscriptJson`(打破循环依赖唯一解) |

### 5 段布局

```
<SubagentDrawerHeader>          ← 独立组件：status pill + name + close + banner + meta + summary + truncated
<SubagentDrawerErrorCard>       ← 独立组件（R25）：v-if status==='error'，prompt 下方
<DrawerPromptCard>              ← run.task, 120 截断, null 则隐藏
<DrawerSection type="thinking"> ← 默认折叠, DrawerThinkingBlock × N
<DrawerSection type="tools">    ← 默认展开, DrawerToolCallCard + DrawerPermissionAskCard
<DrawerSection type="reply">    ← 默认展开, live Text / FinalText, 280 截断
```

`isEmpty` gate：`sections.length === 0 && status !== 'cancelled' && status !== 'error'` 时显示 "Worker is starting..."。cancelled/error 即使空 transcript 也放开 gate（让 chip/card 渲染）。

### 数据流契约

```
dispatch_subagent(task="...")
  → 后端写 subagent_runs.task (PR1 列)
worker 启动 → emit subagent:event { kind, payload } (store 200ms debounce)
  → routeEvent → RunAccumulator.feed (O(1) per event, R20/R21 markRaw)
    chat_event.thinking_delta → ThinkingSection (in-place text +=)
    chat_event.delta          → TextSection
    chat_event.error/done/start → DROP (不贡献 text)
    tool_call                 → ToolCallSection
    tool_result               → ToolResultSection
    permission_ask            → PermissionAskSection
  → publishAccumulator → liveSections Map<runId, TranscriptSection[]>
drawer 读 store.liveSections.get(openRunId) → 5 段渲染
subagent:finished → fetchRun → rebuildFromCache(transcriptJson, finalText)
  → 权威 transcript 替换内存 (R22) → FinalText section → Reply 段
```

**契约**：drawer 数据源是 `liveSections`（accumulator 输出），**不是** `liveTranscript`（raw entries，仅旧 pairing 路径残留用）。读 `liveSections.get(rid) ?? []`。空数组 = openDrawer 与 fetchRun 之间的瞬态 → empty state。

### Design Decision: 视觉原语复用边界（不 wrap ToolCallCard）

**Context**：drawer 要和主 panel 视觉一致，但数据结构不同（drawer 渲染 `TranscriptSection`，主 panel 渲染 `ChatMessage`）。`ToolCallCard.vue` 不是纯视觉组件——它读 `useChatStore` / `usePermissionsStore` / `useSubagentRunsStore` 3 个 store（diff popover / inline approval / dispatch_subagent drawer 触发）。

**Decision**：共享视觉子组件，但 drawer 维护自己的渲染路径。
- `ThinkingBlock`（纯视觉，0 store）→ `DrawerThinkingBlock` 直接 wrap（适配 `ThinkingSection` → `ThinkingBlockInfo[]`）
- `ToolCallCard`（**3 store 耦合**）→ `DrawerToolCallCard` **不 wrap**，改用已抽取的 `ToolInputBody` + `ToolOutputBody`（FT-F-001 PR1，纯 props）+ 重声明 header CSS

**Why not wrap ToolCallCard**：drawer 渲染 worker transcript，wrap `ToolCallCard` 会把父 session store 上下文带进 worker 渲染：(a) permission ask mis-resolve（worker ask 不挂父 session）、(b) dispatch_subagent 递归开 drawer、(c) diff popover 依赖父 worktree。违反 PRD R7「不耦合 ChatMessage 数据结构」。

**CSS 复用**：`DrawerToolCallCard` 的 `.drawer-tool-card*` 1:1 镜像 `.tool-card*`（class 改名避 scoped 碰撞，0 hex，全 design token）。**不**抽 `ToolCallHeader.vue` 共享组件——PRD Risk 表锁「主 panel ToolCallCard 本体 0 改动」，抽取会触犯此约束。

### Design Decision: Header / ErrorCard 子组件 + jump-latest 下移 body（split refactor 2026-06-23）

**Context**：`SubagentDrawer.vue` 长到 1257 行(header template + error card + 5 段编排 + ticker + scroll 编排 + 跨层 drift 注释),需要拆分降复杂度(header template 里原本挂了一个 `↗` "跳到最新" 按钮(`jumpToLatest`),但它的 visible 条件 `!autoFollow && sections.length > 0` + click handler 全部依赖 body 状态(`autoFollow` / `newCount` / `bodyEl` / `onBodyScroll`))。**2026-06-23 拆分完成**:主文件缩到 ~900 行,新出 `SubagentDrawerHeader.vue` (~250 行) + `SubagentDrawerErrorCard.vue` (~100 行),jump-latest 按钮按 A 方案下移 body 顶部 sticky。

**Decision**：拆出 2 个纯展示子组件 + 1 个 cross-cut 移位：
- `SubagentDrawerHeader.vue`（5 prop: `run` / `status` / `statusDisplay` / `bannerText` / `truncated`，**无 emit**，无 cross-cut）—— 仅渲染 status badge / name / close / banner / meta / summary / truncated
- `SubagentDrawerErrorCard.vue`（1 prop: `errorMessage`，**无 emit**）—— R25 详细错误卡
- **`jumpToLatest` 按钮从 header 搬到 body 顶部 sticky**（A 方案，2026-06-23 与用户确认）—— 按钮 visible 条件 + handler 与 body scroll 编排自然耦合，下移后零 cross-cut

**Why A over B/C**：
- (B) Header 保留按钮 + body emit `autoFollow`/`newCount` 上行 → 2 个 emit + Header 多 2 个 prop，多余耦合
- (C) Header 接 `autoFollow` / `sectionsCount` 作为 prop → main drawer 要 expose 状态，同样耦合
- (A) Header 完全解耦，只读 prop；body 顶部 sticky 按钮（与现有 `.subagent-drawer__new-events` 同 sticky 模式）保留全部 UX——"↓ N new" 提示本就在 body 底部，按钮在 body 顶部对称放置

**测试 0 修改**：1225 行 `SubagentDrawer.test.ts` 不动作为 DOM 等价性硬约束（类名 / 文本 / 嵌套结构 1:1 保留）—— 拆分只动 component 边界，不动 user-visible 结构。

**Extensibility**：未来想把 Header / ErrorCard 移到独立 subpackage、或加 `<DrawerHeaderAction>` slot、或 body 顶部 sticky 区做更多 affordance（"pause auto-follow" 等），A 方案 0 重构成本。

### Design Decision: ChatInput split — composable + LatencyPopover + HintRow（split refactor 2026-06-23）

**Context**：`ChatInput.vue` 长到 1834 行,承载 4 个独立关注点(CM 6 宿主 + `/` `@` 触发器检测 + LLM 累计耗时 popover + 底部 hint row 编排),需要拆分降复杂度,同时公共 API(`sending` / `placeholder` + emit `send` / `stop`)必须不变(`ChatPanel.vue` 零修改)。**2026-06-23 拆分完成**:主文件缩到 ~712 行,新出 `ChatInputLatencyPopover.vue` (~365 行) + `ChatInputHintRow.vue` (~251 行) + `app/src/utils/chatInputCodeMirror.ts` composable (~564 行)。

**Decision**：拆出 1 个 composable + 2 个纯展示子组件：

- **`app/src/utils/chatInputCodeMirror.ts` composable**（~564 行，0 store import）—— 封装 CM 6 生命周期 + keymap + IME + 触发器检测（`currentSlashToken` / `currentAtToken` / `detectCommandTrigger` / `detectFileTrigger` / `syncCommandPalette` / `syncFilePalette` / `closeCommandPalette` / `closeFilePalette` / `replaceDoc` / `submit`）。内部管理 `commandPaletteOpen` / `commandItems` / `commandFilter` / `filePaletteOpen` / `fileItems` / `fileFilter` + `commandsLoaded` / `filesLoaded` flags。父组件只通过 `opts.commandItemsSource?` / `opts.fileItemsSource?` 回调拉取最新 items（**单向回调 + panel state 内置**，避免双向 watch stale state）。dispatch handler（`onCommandSelect` / `onFileSelect`）留在主组件（碰 Tauri `invoke` + `chatStore.send`，不能进 composable）。
- **`ChatInputLatencyPopover.vue`**（~365 行，0 store import，0 emit）—— 自包含 chip + popover + open state + `onDocumentClick` + Esc + Transition。严格遵循 `popover-pattern.md`（root ref / typeof document SSR guard / `onUnmounted` 清理）。HintRow 只 `<ChatInputLatencyPopover :total-ms :turns />` 一行 embed。
- **`ChatInputHintRow.vue`**（~251 行，0 store import，0 emit）—— embed `<ChatInputLatencyPopover>` + token reka-ui Tooltip（4 行 breakdown + "升级前未统计" fallback）+ `<ModelSelect>`。reka-ui TooltipPortal `:deep(.chat-input__token-tooltip*)` 选择器全部 wrap 在 scoped CSS 内（避免 portal DOM 逃逸）。

**关键 ADR**：

- **ADR-1 composable 范围 = B 方案（完整）** —— 收 CM host + keymap + IME + 触发器检测；**dispatch handler 留主组件**（碰 Tauri + store，不能进 composable）；**0 store import**（composable 可独立测试 + 未来 AppShell Cmd+K 复用）。主组件从 1834 → 712 行（-61%）。
- **ADR-2 composable ↔ 主组件面板状态通信 = 单向回调 + panel 状态内置** —— composable 内部管 panel state，父只传 source 回调（`commandItemsSource?: () => TriggerMenuItem[]` / `fileItemsSource?: () => TriggerMenuItem[]`）。避免双向 watch 的 stale state 风险。
- **ADR-3 Latency 拆分 = A 方案（自包含 chip+popover）** —— chip 与 popover 共享 root ref + open state + onDocumentClick listener，不能拆开。LatencyPopover ~365 行（CSS 占大头），超任务描述「80 行」但用户已 confirm。

**Composable 接口形状**（锁定）：
```ts
export function useChatInputCodeMirror(opts: {
  host: Ref<HTMLDivElement | null>;
  sending: Ref<boolean>;
  placeholder: Ref<string | undefined>;
  onSubmit: () => void;
  commandItemsSource?: () => TriggerMenuItem[];
  fileItemsSource?: () => TriggerMenuItem[];
}): {
  view: ShallowRef<EditorView | null>;
  input: Ref<string>;
  replaceDoc: (newDoc: string, caret?: number) => void;
  currentSlashToken: () => { line, from, to, slashOffset, tokenEnd } | null;
  currentAtToken: () => { line, from, to, atOffset, tokenEnd } | null;
  detectCommandTrigger: () => { trigger: boolean; filter: string };
  detectFileTrigger: () => { trigger: boolean; filter: string };
  syncCommandPalette: () => void;
  syncFilePalette: () => void;
  closeCommandPalette: () => void;
  closeFilePalette: () => void;
  submit: () => boolean;
  commandMenuRef: Ref<InstanceType<typeof TriggerMenu> | null>;
  fileMenuRef: Ref<InstanceType<typeof TriggerMenu> | null>;
  commandPaletteOpen: Ref<boolean>;
  commandItems: Ref<TriggerMenuItem[]>;
  commandFilter: Ref<string>;
  filePaletteOpen: Ref<boolean>;
  fileItems: Ref<TriggerMenuItem[]>;
  fileFilter: Ref<string>;
}
```

**生命周期安全**：
- composable onMounted: 创建 EditorState + EditorView 挂到 `host.value`
- composable onUnmounted: `view.value?.destroy(); view.value = null;`
- watch(sending): `editableCompartment.reconfigure([EditorView.editable.of(!sending.value)])`
- watch(placeholder): `placeholderCompartment.reconfigure([cmPlaceholder(placeholder.value ?? "")])`
- IME: `submit()` 检查 `view.composing` → true 时拦截；否则调 `opts.onSubmit()`

**测试 0 修改**（既有 ChatInput 测试 = 0，所以天然满足）。**可选新增**：`ChatInputLatencyPopover.test.ts`（chip 渲染 / open-close / outside-click / Esc / empty state）+ `chatInputCodeMirror.test.ts`（composable 单元测：currentSlashToken / currentAtToken / detect* / submit 拦截）—— 留 follow-up。

**Extensibility**：未来 Composable 可直接复用给 AppShell Cmd+K / 其他输入框（0 store import + 触发器检测可配置触发字符）。

### Convention: section 级配对（pairSections）

```ts
pairSections(sections: TranscriptSection[], now: number, pendingFirstSeenAt: Map<string, number>): SectionToolEntry[]
```

- 输入：`TranscriptSection[]`（accumulator 输出），**不是** raw `TranscriptEntry[]`
- 配对 `ToolCallSection` + `ToolResultSection` by `payload_json.tool_use_id`
- snake→camel 转换：`tool_use_id→id`（ToolCallInfo）/ `tool_use_id→toolUseId, is_error→isError, duration_ms→durationMs`（ToolResultInfo）
- 30s pending timeout（`PENDING_TIMEOUT_MS`），跨调用持久化 via `pendingFirstSeenAt` Map（drawer 100ms nowTick ticker 驱动 age-out）
- 旧 `pairTranscript`（raw `TranscriptEntry[]` 输入）保留向后兼容，**新代码用 `pairSections`**

### Modal / 截断复用

- `MarkdownDetailModal.vue`（PR3）：DrawerPromptCard / Reply 段的 "View full →" 入口，`source ∈ {prompt, reply}`
- `useTruncate.ts`（PR3）：`truncate(text, maxChars)`，纯函数（markdown-aware，代码块边界回退）。task 用 120 / reply 用 280

### 3 边界态（R23/R24/R25）

#### R25 error（必做）

`v-if status==='error'`，❌ card 在 DrawerPromptCard 下方，独立组件 `SubagentDrawerErrorCard.vue` 接 `errorMessage: string` 单 prop。chrome：复用 `.drawer-tool-card` chrome + `--color-tool-error` 3px 左 border + `shield-x` icon。`errorMessage` computed（留在 main drawer）**4 级 fallback**：

1. `parseTranscriptJson(run.transcriptJson)` **反向扫描**末位 `kind==='chat_event'` 且 inner `payload_json.kind==='error'`，读 `payload_json.message`（对应 Rust `ChatEvent::Error { message, category }`，`llm/types.rs:407`）
2. `run.finalText`（error 时 `format_final_text` 返回 worker_text verbatim）
3. `run.summary`
4. `"(no error text captured)"`

> **Gotcha**：accumulator 的 `routeChatEvent` 把 inner `kind==='error'` **drop** 了（`case "error": return`，不贡献 text），所以 error message **不在 `liveSections` 里**，必须独立 `parseTranscriptJson(run.transcriptJson)`。与 header 的 FT-F-005 banner（80 字符截断）并存——banner 简短提示，❌ card 详细 message。

> **Gotcha**：discriminator 必须**双 `===` 严格**（outer `chat_event` + inner `error`），**不能**用 `.includes("error")` / `.indexOf` —— delta 事件 text 可能含 "Error:" 字样但不是 error inner kind。有专门测试 lock。

#### R23 cancelled（now 显示 turn，已 resolved）

Reply 段顶部 `⊘ Cancelled · at turn N` chip,优先读 `run.turnCount`(非 null 时);`turnCount === null`(pre-PR2 老行)降级显 wall-clock `at X.Xs`(`terminalDurMs = finishedAt - startedAt`)。`cancelled && replyText 空` → 只 chip;`cancelled && replyText 非空` → chip 在上 + reply 在下(保留 worker 中断前输出)。

> **Resolved 2026-06-22 (RULE-FrontSubagent-004)**:PRD R23 字面 "at turn N" 已实现 —— `subagent_runs` 加 `turn_count INTEGER` 列(幂等 `add_subagent_runs_column_if_missing`),`SubagentBufferSink::turns_completed()` 在真实 per-turn `Done` 时 `fetch_add(1)`(`stop_reason != "cancelled"` && `!= "max_turns"` 守卫,合成 terminal 不 increment),`run_subagent` 终态 `update_run_finished(..., Some(turns))` 写入。DEBT 见 `.trellis/reviews/DEBT.md` RULE-FrontSubagent-004(已 close via `06-22-subagent-drawer-historical-ask-outcome-and-cancelled-turn-count` task)。

#### R24 permission_ask（live interactive + historical outcome badge）

`<DrawerPermissionAskCard>` 模式由 `isPermissionAskLive(rid)` 协调:
- **Live (pending)**:transcript `PermissionAsk` entry + `usePermissionsStore.pendingWorkerByRunId` 还有这个 rid → `interactive = true` → `<PermissionAskBody mode="interactive" hideAllowAlways>`(隐藏「始终允许」,worker 端 AllowAlways 当 AllowOnce,避免跨权限边界)
- **Historical (resolved)**:permissions store rid 移除 + transcript 里有配对的 `PermissionAskResolved` entry → `interactive = false` → `<PermissionAskBody mode="historical" :outcome>` → 显 ✓已允许 / ✗已拒绝 / ⏱已超时 / ⊘已取消 badge

> **已 evolved 2026-06-22 (RULE-FrontSubagent-003 + RULE-WorkerAsk-001)**:原 R24 "已知限制(DEBT)" 描述的 worker Tier 4 collapse + synthetic rid + 父复用 session_id 三个 blocker **全部解决**:
> - Session 62 `89e5ba1` (RULE-FrontSubagent-003):worker Tier 4 `ask_path` 改完整 `register_ask + tokio::select!{cancel, timeout, oneshot}` round-trip(不再 collapse to auto-Deny);oneshot key 改 composite `worker:{runId}` 隔离(避免覆盖 parent 主 chat 槽)
> - Session 63 (RULE-WorkerAsk-001):worker ask resolve outcome 写 transcript `PermissionAskResolved` entry;`pairSections` 按 `rid` 配对;`<PermissionAskBody>` historical 分支显 outcome badge
> - DEBT 见 `.trellis/reviews/DEBT.md` RULE-FrontSubagent-003(closed `89e5ba1`)+ RULE-WorkerAsk-001(closed via `06-22-...task`)

### Common Mistakes

#### Mistake: drawer 读 liveTranscript 而非 liveSections
**Symptom**：drawer 显示 raw chat_event delta 流（PRD 原痛点：6963 chat_event 暴露）。
**Cause**：用旧的 `store.liveTranscript` + `pairTranscript`。
**Fix**：读 `store.liveSections.get(openRunId)`（accumulator 已 collapse chat_event 成 Thinking/Text 段）。

#### Mistake: DrawerThinkingBlock 传 ThinkingBlockInfo[]
**Symptom**：类型不匹配 / 渲染空。
**Cause**：accumulator 产出的是 `ThinkingSection { text, chars, closed }`（拼接纯文本），**不是** `ThinkingBlockInfo[]`（数组，每元素有 `signature`）。
**Fix**：`DrawerThinkingBlock` 接 `ThinkingSection`，内部转 `[{ text: section.text, signature: "" }]` 喂 `ThinkingBlock`（单元素数组，`thinkingDisplayText` 的 `.join("\n\n")` 是 no-op）。

#### Mistake: Vue 3 boolean casting 吃掉 undefined
**Symptom**：`showStreamingHint` 等 override prop 永远是 false（override 失效）。
**Cause**：bare `?: boolean` prop 在未传时被 Vue coerce 成 `false`（[Boolean Casting 规则](https://vuejs.org/guide/components/props.html#boolean-casting)）。
**Fix**：`withDefaults(defineProps<{ showStreamingHint?: boolean | undefined }>(), { showStreamingHint: undefined })` + `typeof === "boolean"` 判断区分 absent vs explicit-false。

#### Mistake: DrawerSection 折叠态渲染 20000 entry
**Symptom**：冷启动卡顿。
**Fix**：`DrawerSection` 折叠态 `<div v-if="open"><slot/></div>`（lazy render，折叠不挂 DOM）。accumulator 已把 20000 chat_event 聚合成少量 sections，实际渲染压力小；但折叠态仍必须 lazy。

### Tests Required

- `SubagentDrawer.test.ts`：5 段分组 / 默认折叠展开 / 边界态（error 4 级 fallback + discriminator 严格 / cancelled chip 空+非空 reply / permission historical 不回归）
- `transcriptPairing.test.ts`：`pairSections` 配对 / pending timeout / snake→camel 转换 / orphan call+result
- `subagentRuns.test.ts`：accumulator 累加 / markRaw 不被 reactive / **20000 events `rebuildFromCache` <500ms benchmark**（实测 13.4ms）
- `useTruncate.test.ts`：截断边界（空 / 超长 / 代码块不破坏）
- `DrawerToolCallCard.test.ts`：**lock「无 store 耦合」**（断言不渲染 diff-btn / approval UI / dispatch-preview / role=button）
- `DrawerThinkingBlock.test.ts`：ThinkingSection → ThinkingBlock 适配 + boolean casting 修复

### Wrong vs Correct

#### Wrong — 直接 wrap 主 panel ToolCallCard
```vue
<!-- drawer 渲染 worker transcript，但 ToolCallCard 读父 session store -->
<ToolCallCard :call="callInfo" :result="resultInfo" />
<!-- 后果：worker 的 permission ask 去查父 session permStore.getPending → mis-resolve；
     worker 内 dispatch_subagent 触发 openDrawer 覆盖当前 drawer → 递归 -->
```

#### Correct — 复用纯 props body 子组件 + 重声明 header
```vue
<!-- DrawerToolCallCard.vue: 复用 ToolInputBody/ToolOutputBody (0 store) + 自己的 header -->
<DrawerToolCallCard :call="callInfo" :result="resultInfo" />
<!-- 内部: <ToolInputBody :name :input /> + <ToolOutputBody :content :is-error :duration-ms /> -->
```
