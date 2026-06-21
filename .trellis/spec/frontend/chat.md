# Chat Components Frontend Spec

> 主 chat panel + subagent drawer 组件的前端执行性规范。目前覆盖 SubagentDrawer（重构 PR1-6, 2026-06-21）；主 panel 的 MessageItem / ToolCallCard 等待后续补充。

---

## SubagentDrawer (重构 PR1-6, 2026-06-21)

worker subagent 的右侧 drawer。reka-ui `Dialog*` 组合实现（@2.9.9 无 `Sheet` primitive，CSS 成右侧 panel）。**5 段分组折叠视图**，数据源是 store accumulator 的 `liveSections`（不是 raw `liveTranscript`）。

### 文件清单

| 文件 | 职责 |
|---|---|
| `app/src/components/chat/SubagentDrawer.vue` | 顶层容器 + 5 段组装 + header + 边界态 |
| `app/src/components/chat/DrawerSection.vue` | 通用折叠容器（thinking/tools/reply 共用），折叠态 lazy render |
| `app/src/components/chat/DrawerPromptCard.vue` | `run.task` prompt 卡片（120 截断 + View full） |
| `app/src/components/chat/DrawerThinkingBlock.vue` | `ThinkingSection` → 共享 `ThinkingBlock` 适配器 |
| `app/src/components/chat/DrawerToolCallCard.vue` | tool call 卡片（复用 ToolInputBody/ToolOutputBody，**不 wrap ToolCallCard**） |
| `app/src/components/chat/DrawerPermissionAskCard.vue` | permission ask 卡片（historical only） |
| `app/src/utils/transcriptPairing.ts` | `pairSections` section 级配对（snake→camel） |
| `app/src/stores/subagentRuns.ts` | `RunAccumulator` + `liveSections` Map + `TranscriptSection` 类型 |

### 5 段布局

```
DrawerHeader        (status pill + name + duration + FT-F-005 failure banner + timestamps)
DrawerPromptCard    (run.task, 120 截断, null 则隐藏)
❌ ErrorCard        (v-if status==='error', prompt 下方, R25)
DrawerSection type="thinking"  (默认折叠, DrawerThinkingBlock × N)
DrawerSection type="tools"     (默认展开, DrawerToolCallCard + DrawerPermissionAskCard)
DrawerSection type="reply"     (默认展开, live Text / FinalText, 280 截断)
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

`v-if status==='error'`，❌ card 在 DrawerPromptCard 下方（复用 `.drawer-tool-card` chrome + `--color-tool-error` 3px 左 border + `shield-x` icon）。`errorMessage` computed **4 级 fallback**：

1. `parseTranscriptJson(run.transcriptJson)` **反向扫描**末位 `kind==='chat_event'` 且 inner `payload_json.kind==='error'`，读 `payload_json.message`（对应 Rust `ChatEvent::Error { message, category }`，`llm/types.rs:407`）
2. `run.finalText`（error 时 `format_final_text` 返回 worker_text verbatim）
3. `run.summary`
4. `"(no error text captured)"`

> **Gotcha**：accumulator 的 `routeChatEvent` 把 inner `kind==='error'` **drop** 了（`case "error": return`，不贡献 text），所以 error message **不在 `liveSections` 里**，必须独立 `parseTranscriptJson(run.transcriptJson)`。与 header 的 FT-F-005 banner（80 字符截断）并存——banner 简短提示，❌ card 详细 message。

> **Gotcha**：discriminator 必须**双 `===` 严格**（outer `chat_event` + inner `error`），**不能**用 `.includes("error")` / `.indexOf` —— delta 事件 text 可能含 "Error:" 字样但不是 error inner kind。有专门测试 lock。

#### R23 cancelled（降级 wall-clock）

Reply 段顶部 `⊘ Cancelled · at X.Xs` chip，用 `terminalDurMs`（wall-clock = `finishedAt - startedAt`）。`cancelled && replyText 空` → 只 chip；`cancelled && replyText 非空` → chip 在上 + reply 在下（保留 worker 中断前输出）。

> **已知限制（DEBT）**：PRD R23 字面是 "at turn N"，但 `subagent_runs` **无 turn 列**（schema 只有 started_at/finished_at + PR1 的 task/final_text，`db/migrations.rs:515`），`SUBAGENT_MAX_TURNS=20` 是常量不持久化。用 wall-clock 降级。未来加 turn 列 + worker 持久化实际 turn 后可改回 turn N。DEBT 见 `.trellis/reviews/DEBT.md`。

#### R24 permission_ask（降级 historical）

`DrawerPermissionAskCard` 保持 `mode="historical"`（只读，视觉「worker · 自动拒绝」+ auto-denied note）。

> **已知限制（DEBT）**：worker 的 `PermissionContext.is_worker=true`（`permissions/mod.rs:287`）让 Tier 4 `ask_path`/`ask_shell` 直接 collapse `Decision::Deny`（`mod.rs:1003-1045`），**从不 emit `permission:ask` IPC**；transcript 里的 historical permission_ask 用 synthetic rid（`Uuid::new_v4()`），不在 `permission_asks` oneshot map 中，`permission:response` IPC 无法路由（`commands/permissions.rs:197-234`）；worker 复用 `parent_session_id`（`subagent.rs:597`）无独立 permission session。未来需 worker 独立 permission session + worker ask 事件带 workerRunId + Tier 4 collapse 改 emit 才能 interactive。DEBT 见 `.trellis/reviews/DEBT.md`。

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
