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
| `app/src/components/chat/DrawerToolCallCard.vue` | tool call 卡片(复用 ToolCallHeader + ToolInputBody/ToolOutputBody,**不 wrap ToolCallCard**) |
| `app/src/components/chat/DrawerPermissionAskCard.vue` | permission ask 卡片(复用 ToolCallHeader + PermissionAskBody,live interactive + historical outcome badge) |
| `app/src/components/chat/ToolCallHeader.vue` | ★ (RULE-FrontSubagent-001, 2026-06-25) 共享 tool-card header(纯展示,0 store);ToolCallCard / DrawerToolCallCard / DrawerPermissionAskCard 三处复用,props 驱动差异(filePath/suffix/statusIconName/durationLabel/isError/isRunning/statusVariant) + `#status-extra` slot(ToolCallCard diff-btn) |
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

**CSS 复用（RULE-FrontSubagent-001, 2026-06-25 更新）**：原 `DrawerToolCallCard` / `DrawerPermissionAskCard` 的 header CSS 1:1 镜像 `ToolCallCard` 的 `.tool-card*`(class 改名避 scoped 碰撞)。**现已抽 `<ToolCallHeader>` 共享组件** —— redesign PR1-6 收尾后,原 PR4「主 panel ToolCallCard 本体 0 改动」约束解除,三处 header markup + CSS 合并为单一来源(**推翻本节旧决策**「不抽 ToolCallHeader.vue」)。card 容器 chrome(背景/边框/3px left bar/`--error`/`--running` 容器变体)仍各自保留;header 内 error/running 颜色改 ToolCallHeader 的 `isError`/`isRunning` prop 驱动(不再靠 card root 后代选择器)。ToolCallCard 的 diff-btn 走 `#status-extra` slot —— slot 内容带父 scope id,`.tool-card__diff-btn` CSS 留 ToolCallCard scoped 仍命中。DrawerPermissionAskCard 的 interactive status accent 由 `statusVariant="accent"` prop 驱动;header 与 body 的 4px gap 用 `:deep(.tool-call-header)` 注入。

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
- **RULE-FrontSubagent-002 (2026-06-25)**：第三参 `pendingFirstSeenAt` Map 既是输入又是输出(被 `.set`/`.delete`),签名隐式 —— 新调用方易踩「忘传/传新 Map → 30s timeout 永不推进」。改用 `useTranscriptPairing()` composable 封装:闭包持 plain Map(非响应式,避免 `toolEntries` computed 在 pairing 内部 `.set`/`.delete` 触发自身依赖 → 递归 re-invalidation → 100ms nowTick × 大量 sections → webview OOM 崩溃,**plain Map 是 load-bearing 约束**),返回 `{ pairEntries, pairSections, reset }`。SubagentDrawer 用 `pairToolSections(sections, now)` 两参签名 + 切 run `reset()`。纯函数 pairTranscript/pairSections 保留(测试 30+ 处 + raw-list consumer)。

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

### L3b PR4 (2026-06-27) — SubagentDrawer merge / discard UI

闭合 L3b PR3 backend `merge_worker` / `discard_worker` IPC 在前端的可见/可控环。新增 2 个组件 + 1 个 util,SubagentDrawer footer 渲染 Merge / Discard 按钮(完成 worker 带保留 branch 时)。

#### 新增文件

| 文件 | 职责 |
|---|---|
| `app/src/components/chat/WorkerBranchBadge.vue` | 派生 badge:`status + worktreePath` → 隔离中 / 已完成·保留分支(已 destroy 隐藏) |
| `app/src/components/chat/WorkerMergeControls.vue` | Merge / Discard 按钮 + ConfirmDialog 二次确认 + 冲突 inline 文件列表 |
| `app/src/components/chat/WorkerMergeControls.test.ts` | 27 单测(store actions + util + parser + 9 组件场景) |
| `app/src/utils/workerBranch.ts` | `formatWorkerBranchLabel(worker/<run_id> 或 worktree_path)` → `Worker <8-char hash>` |
| `app/src/stores/subagentRuns.ts` | `mergeWorker(runId)` / `discardWorker(runId)` actions + `mergeStateByRunId: reactive Map` per-run spinner |
| `app/src/stores/subagentRuns.types.ts` | `MergeResult` / `DiscardResult` / `MergeState` 类型 + `parseConflictFiles(errStr)` 纯函数 |

#### 严格可见门(STRICT — 不是单字段)

```ts
// WorkerMergeControls.vue visible-gate
const visible = computed(
  () => worktreePath.value !== null && status.value === 'completed',
);
```

**严格双条件**:worktreePath 非空(branch + worktree 保留) **且** status === 'completed'。cancelled / error / incomplete worker **不显示按钮**,即便 disk 上 worktree_path 残留。原因:worker exit-state 才是「user-actionable」权威信号,disk presence 不可靠(L3b PR3 sweep 会清)。

派生规则(WorkerBranchBadge 三态):
- `status === 'running'` → 隔离中(amber `--color-tool-shell`)
- `status === 'completed' && worktreePath != null` → 已完成 · 保留分支(emerald `--color-tool-write`)
- 其他(worktreePath null)→ hidden

#### store actions 契约

```ts
// useSubagentRunsStore
mergeWorker(runId: string): Promise<MergeResult>
  // ↪ invoke("merge_worker_run", { rid: "merge-pr4", runId })
  // ↪ 成功 → getRunCache.set(runId, {...row, worktreePath: null}) → 按钮自动消失
  // ↪ 失败 → parseConflictFiles 命中 → { kind: "conflict", files }
  //            未命中 → { kind: "error", message }
discardWorker(runId: string): Promise<DiscardResult>
  // ↪ invoke("discard_worker_run", { rid: "discard-pr4", runId })
  // ↪ 成功/失败 (无 conflict 路径)
```

**Per-run spinner 隔离**: `mergeStateByRunId = reactive(new Map<runId, MergeState>())`,key 是 runId。多 drawer(不同 runId)同时打开互不阻塞。`finally` 清 spinner guard,二次 click 短路(`{ kind: "error", message: "another action is already in flight" }`)做防御性兜底(按钮 `:disabled` 已防双击)。

#### Conflict 跨层契约(cross-layer)

后端 `merge_worker` 冲突路径返 `Err(String)` 形式 `"merge conflict: [<file1>, <file2>, ...]. The worker branch 'worker/<run_id>' and parent branch 'session/<id>' both modified these files. Resolve manually, then call merge_worker again (or discard_worker to drop the changes)."`

前端 `parseConflictFiles(errStr)` 正则 `/^merge conflict: \[([^\]]*)\]/` 提取 `[...]` 内文件列表(逗号+空格 split)。**branch + worktree 保留**(backend 冲突路径已 hard-reset 到 parent tip 但保留 branch),drawer Merge/Discard 按钮保持可见,用户 git resolve 后可点 Merge 重试。conflict 文件列表 inline 渲染(`role="alert"` + `--color-tool-error` left-border),引导用户到 git CLI。

#### Store cache 单源模式(关键决策)

```ts
// WorkerMergeControls.vue 不接 worktreePath prop,只接 runId
const props = defineProps<{ runId: string }>();
const worktreePath = computed(() => store.getRunCache.get(props.runId)?.worktreePath ?? null);
const status       = computed(() => store.getRunCache.get(props.runId)?.status ?? null);
```

**SubagentDrawer 父不传 `:worktree-path` 给 MergeControls**(只传 `:run-id`)。理由:`getRunCache` 是 single source of truth,`mergeWorker` 成功后 `.set(runId, {...row, worktreePath: null})` → computed reactive → `v-if="visible"` 自动 false → 按钮消失,**无需父组件 re-thread prop**。WorkerBranchBadge 接 prop(纯展示,无 store),可保留。

#### 设计决策 / 反模式

- **ConfirmDialog(非 `window.confirm`)** — Tauri webview 静默 no-op `window.confirm()`,必须走 in-app ConfirmDialog,见 `popover-pattern.md` 二次确认段
- **不用 i18n key** — 全部中文硬编码(项目惯例,zh-CN 优先,en-US 留 follow-up)
- **不接 DiffView 联动** — PRD 显式 out-of-scope,「点 Merge 前想看 diff」留 follow-up
- **不暴露锁按钮的 `:disabled` 派生状态给父** — 按钮 `disabled` 由组件内 `mergeState` 派生,父不需要 know
- **C5b regression test** — `worktreePath set but status=cancelled → hidden`,锁严格双条件门,防未来 refactor 退化成单字段

#### Tests Required (PR4 新增)

- `WorkerMergeControls.test.ts`: 27 测(6 store + 5 util + 4 parser + 12 组件含 C5b 严格门)
- `SubagentDrawer.test.ts`: 0 改(PR4 不动 drawer 既有 5 段布局),baseline fixture `worktreePath: null`
- `ToolCallCard.test.ts` + `subagentRuns.test.ts`: baseline fixture `worktreePath: null`(PR1 列新增后 fixture 跟齐)

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

---

## B9 生成式 UI — `use_ui` primitive registry (2026-07-02)

`use_ui` tool 的 primitive 由前端 `uiPrimitiveRegistry.ts` 的 `type → Component` Map 派发。`<UiCard>` 读 `call.input.primitives` 遍历，按 `primitive.type` 从 registry 取组件，未知 type 走 fallback（不崩）。

### 注册条目

| type | 组件 | child |
|---|---|---|
| `diff` | `DiffPrimitive.vue`（`parsePatch` 拆多文件 → 复用 `DiffView` 只读 + 复制） | C |
| `code_block` | `CodeBlockPrimitive.vue`（hljs 高亮 + 复制） | B |
| (unknown) | `MockPrimitive`（fallback，渲染 type + JSON dump） | A |

### MessageItem dispatch（`tool_name` 路由，仿 ask_user_question 对称结构）

`<UiCard>` 作为 `<ToolCallCard>` 的 sibling 挂在 `visibleToolCalls` v-for 内（同 `AskUserQuestionCard` 模式，不 portal）：
```vue
<template v-for="tc in visibleToolCalls" :key="tc.id">
  <ToolCallCard :call="tc" :result="..." />
  <AskUserQuestionCard v-if="askCardPropsFor(tc) !== undefined" v-bind="askCardPropsFor(tc)!" />
  <UiCard v-if="tc.name === USE_UI_TOOL_NAME" :call="tc" />
</template>
```

### 加新 primitive 的契约（registry 可扩展性）

加新 type = 改 `uiPrimitiveRegistry.ts` 一行条目 + 后端 `use_ui.rs` 的 `KNOWN_TYPES` + schema `enum` + description 字段说明。`<UiCard>` / MessageItem dispatch 零改动（Child B/C 实证：各只改 registry 一行）。后端 `definition_schema_type_enum_matches_known_types` 测试锁定 schema `enum` 与 `KNOWN_TYPES` 同步。

### 数据源

`<UiCard>` 直接读 `call.input.primitives`（tool_use 输入）—— non-blocking tool 无需独立 IPC 事件（不像 `ask_user_question` 的 `tool:question` channel）。`ToolCallInfo.input: Record<string, unknown>`，`primitives` 用 `Array.isArray` narrow，非数组/缺 → 不渲染（防御 stale 消息）。

### hljs 共享（D6）

`utils/highlight.ts` 的 `renderCodeHtml(code, language)` 两个入口共用：markdown 管线（`marked-highlight` 接 marked 18，现有 markdown 代码块顺带高亮）+ `<CodeBlockPrimitive>`。语言集 `highlight.js/lib/common`（非 full ~900KB）。**注意**：hljs 改变代码块 HTML 输出（`<span class="hljs-*">`），含代码块 substring 的测试断言要适配（见 markdown.test.ts / MarkdownDetailModal.test.ts）。

### Tests required

- `UiCard.test.ts`：registry dispatch（type→组件）+ 未知 type fallback + 空/缺 primitives 守卫
- `CodeBlockPrimitive.test.ts` / `DiffPrimitive.test.ts`：各自渲染 + 复制 + 边界
- 后端 `use_ui::tests`：definition schema + execute 校验

### DiffPrimitive / DiffView raw fallback contract (RULE-FrontDiff-001, 2026-07-02)

`use_ui` 的 `diff` primitive 的 LLM 输出有**两种合法形态**,渲染器必须都能兜住(LLM 风格 +/- 片段是默认行为,标准 unified-diff 是升级形态):

| 形态 | 例子 | 渲染路径 |
|---|---|---|
| 标准 unified-diff(首选) | `--- a/foo\n+++ b/foo\n@@ ... @@\n-x\n+y` | `jsdiff parsePatch` 拆出 hunks → `<DiffView>` 走 `diff-file__hunks` 分支(行级 +/- 染色 + 双 gutter 行号 + 折叠) |
| LLM 风格 +/- 片段(无 `---`/`+++` 头) | ` foo\n-x\n+y\n bar` | **raw fallback** — `DiffPrimitive.files` 按"原 patchToText round-trip 后无内容"路径返回原始 `diff_text` → `<DiffView>` 走 `diff-file__raw` 分支(每行 div 按首字符分类 add/del/ctx/other,绿/红背景,无双 gutter) |

#### 关键 invariant(b00dde2 + b5073ea 落地)

1. **`parsePatch` 空 hunks 检测**:`jsdiff` 对"无 `---`/`+++` 头但有 `+`/`-` 行"的输入返回 **`[{ hunks: [] }]`(`length === 1` 但 hunks 空)**,**不是** `[]`。`patches.length === 0` 守卫会失效。`DiffPrimitive.files` computed 必须额外检查 `patches.length > 0 && patches.every(p => p.hunks.length === 0)` 才触发 raw fallback。
2. **raw fallback 必须保留原文**:`DiffPrimitive` 走 raw fallback 时,`FileDiff.diff_text` 字段填**原始文本**(不是 `patchToText(p)` 重新打包)。DiffView re-parse 这段文本得到同一 `[{hunks:[]}]` 形态,触发自身的 raw fallback `<div>` 分支。round-trip 会丢内容(`patchToText({hunks:[]}) === "--- a\n+++ b"`),永远不要走。
3. **DiffView 防御性兜底**:`out.parsed` 仅在 `out.hunks.length > 0` 时置 true。原 `out.parsed = true` 隐含 `parsed-but-empty` 静默空白根因(模板 `v-for pf.hunks` 走 0 迭代)。如果上游某天绕过 `DiffPrimitive` 直接传 `FileDiff[]` 进 DiffView,这一守卫保证不出现空 body。
4. **计数从原文派生**:raw fallback 的 `added`/`removed` 字段必须按 `text.split("\n")` 重数 `+`/`-` 行(不能从空 hunks 派生,否则永远 0),保证文件 header 的 `+N / −M` badge 显示真实值。
5. **行级染色复用既有 token**:`.diff-raw-line--add / --del` 用 `rgba(16, 185, 129, 0.12)` / `rgba(239, 68, 68, 0.12)`(与 `diff-line--add / --del` 同色,不引入新 token);`--ctx` 与 `--other` 走 `--color-text-secondary` 普通色。

#### Wrong vs Correct

**Wrong**:`DiffPrimitive.files` 只用 `patches.length === 0` 守卫,空 hunks 形态漏判:

```ts
const patches = parsePatch(text);
if (patches.length === 0) return [rawFallback(text)];  // ❌ 漏 catch [{hunks:[]}]
return patches.map(p => ({ diff_text: patchToText(p), ... }));  // 进去后 round-trip 空
// 后果: DiffView 拿 "--- a\n+++ b" → parsePatch 又得 0 hunks
//       → out.parsed = true + hunks = [] → 模板静默空白
```

**Correct**:双守卫 + 原文保留 + DiffView 防御:

```ts
// DiffPrimitive.vue
const allHunksEmpty =
  patches.length > 0 && patches.every(p => p.hunks.length === 0);
if (patches.length === 0 || allHunksEmpty) {
  let added = 0, removed = 0;
  for (const line of text.split("\n")) {
    if (line.startsWith("+") && !line.startsWith("+++")) added++;
    else if (line.startsWith("-") && !line.startsWith("---")) removed++;
  }
  return [{ path: "diff", status: "modified", added, removed, diff_text: text }];
}
return patches.map(p => ({ ... patchToText(p) }));
```

```ts
// DiffView.vue
out.hunks = patch.hunks.map(hunk => /* lines */);
out.parsed = out.hunks.length > 0;  // ❌→✅ 防御 parsed-but-empty
```

#### Common Mistakes / Gotchas

- **jsdiff 空 hunks 不是空数组**:`patches.length === 0` 看着像合理守卫,实际只 catch 0 patch 的输入;LLM-style 不齐的 +/- 进去都是 1 个 patch + 0 hunks。**Lock**:测试两种空 fallback 输入("just some prose" → `[]`、LLM-style → `[{hunks:[]}]`)都得触发 raw 路径。
- **`patchToText({hunks:[]})` 是字符串陷阱**:返回的 `"--- a\n+++ b"` 看似合法(unified-diff 头),但下游 re-parse 后得到 0 行,body 空白。永远不要从空 hunks round-trip。
- **不在 frontend 强制 LLM 格式**:Schema 仅校验 `type`,不校验 `diff_text` 字段是不是真 unified-diff(避免误拒 + 与 `additionalProperties: true` 一致);渲染器承担兜底责任,LLM 契约写在 tool description 里(给 LLM 看,见 `tool-contract.md` §use_ui + `use_ui.rs` definition 的 `description`)。

#### Tests required(RULE-FrontDiff-001 锁定)

- `DiffPrimitive.test.ts`:"falls back to raw text for LLM-style +/- fragments" — 断言(1)`+N / −M` header 计数正确,(2)`add`/`del`/`ctx` 行级 class 各自分配正确次数,(3)`(1..=n).product()` 与 `match n {` 等关键文本可见。
- `DiffPrimitive.test.ts`:"does not crash on non-diff text" 保留(`parsePatch returns []` 单字串分支,断言 wrapper 存在)。
- 防回归锚点:session `1b469d93-84d3-49b0-a4c5-eefc34b1bf58` — prompt "调 use_ui 输出一个 code_block(rust 代码)和一个 diff(两段对比)",该次 LLM 输出是 LLM-style `+/-` 片段,该 prompt 下必须不再出现"打开 diff 卡片是空白"。
