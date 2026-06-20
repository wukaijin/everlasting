# FT-F-001 PR1 — ToolCallCard shared body 抽出(硬前置)

> **状态**:**planning → 准备 in_progress**(brainstorm 阶段完成 2026-06-20,Session 51+,8 个 Open Questions 全部 derive 收口,等用户 final confirmation 后 curate jsonl + `task.py start`)
>
> **Tracking**:`.trellis/reviews/DEBT.md` §FT-F-001(同源,本 task 不单独列 DEBT,作为 FT-F-001 阶段 2 的依赖)
>
> **Origin**:FT-F-001 prd R-HP1/R-HP2/R-HP3/R-HP4 + D1/D2 决策(2026-06-20)
>
> **依赖关系**:本 task 是 FT-F-001 typed-cards 阶段 2 的**硬前置** — 本 task 未合并前,FT-F-001 阶段 2 不能 `task.py start`(见 FT-F-001 prd "Definition of Done")

---

## Goal(一句话)

从 `ToolCallCard.vue`(995 行,outer + 3 body + store 依赖四合一)抽出 `ToolInputBody` / `ToolOutputBody` / `PermissionAskBody` 三个 shared body component(只拿 data + callback prop,无 store 依赖),让主面板与后续 drawer(FT-F-001 阶段 2)都消费同一组 body 组件,达成 UX 一致性 + 维护性双提升。

---

## What I already know(codebase 摸底 2026-06-20)

### `ToolCallCard.vue` 实际形态(995 行,4 个独立 block)

| Block | 行号 | 形态 | 共享给 drawer? |
|---|---|---|---|
| `tool-card__header` | 435-478 | outer wrapper(icon / name / path / status / duration / diff btn) | ❌ outer 各自管 |
| `tool-card__approval` | 486-525 | pending 权限 inline 4 按钮 + feedback | ✅ `PermissionAskBody` |
| `tool-card__subagent-preview` | 534-551 | dispatch_subagent collapsed 预览 | ❌ drawer 不需要(主面板 dispatch 卡片专用) |
| `tool-card__diff` | 559-576 | edit_file diff popover(DiffView 包装) | ❌ drawer 不用 diff |
| `tool-card__details` | 578-586 | input `<details>` + output `<details>`(2 个 `<pre>` 调 helper) | ✅ `ToolInputBody` + `ToolOutputBody` |

### store 依赖(`ToolCallCard.vue:25-46`)

- `useChatStore`:`currentSessionId` / `currentCwd` / `getFileDiff` / `sessions` / `fetchDiff`
- `usePermissionsStore`:`getPending` / `hasPending` / `respond` / `clearPending`
- `useSubagentRunsStore`:`getSummaryByToolUseId` / `openDrawer` / `fetchForSession`

→ 全部在 **outer wrapper**(`ToolCallCard.vue` 本身)持有,body 不接触 store。

### helper signatures(`messageFormat.ts`)

- `formatToolInput(tc: ToolCallInfo): string` — 内部只 `JSON.stringify(tc.input, null, 2)`
- `truncateOutput(s: string, max = 500): string`
- `extractToolResultDisplay(content: string): string` — 拆 cwd envelope

### type defs(`chat.ts:49-67`)

- `ToolCallInfo { id, name, input }` — id 是 tool_use_id,body 渲染不需要
- `ToolResultInfo { toolUseId, content, isError, durationMs? }`

### drawer 数据形状(`subagentRuns.ts:111-114`)

- `TranscriptEntry { kind, payload_json: Record<string, unknown> }`
- 4 kind:chat_event / tool_call / tool_result / permission_ask
- payload_json snake_case(DB 形状,与 live `subagent:event` payload camelCase 不同)

### 现有 test 模式(`ToolCallCard.test.ts`,14 个)

- **全部 `shallow: true`**(line 67 / 204)— 子 component 不真渲染,直接 mock
- 断言集中在 **outer 行为**(approval UI 出现/隐藏 / click 触发 IPC / store 状态)
- **几乎零 body 内容渲染断言** — textContent 检查只针对按钮文字
- → 抽出 body 后 14 test 几乎**零改动**

### reka-ui 用了没

- `ToolCallCard.vue` **不用** reka-ui `DialogContent`(plain div + details)
- drawer 才用(`SubagentDrawer.vue`)
- → memory 3 坑笔记在本 task **不直接触发**,但 `Icon` stub + `useFakeTimers` 仍适用(新 body 内部如用 Icon 或定时器,test 仍需遵守)

---

## Requirements

### R1:`ToolInputBody.vue` 新增(纯渲染)

- **Props**:`name: string`, `input: Record<string, unknown>`
- **职责**:渲染 `<details>` + `<summary>input</summary>` + `<pre>`(内容 `JSON.stringify(input, null, 2)`,走 `<pre>` 视觉)
- **不读 store**;不接 callback
- drawer 端使用:`<ToolInputBody :name="payload_json.name as string" :input="payload_json.input as Record<string, unknown>" />`
- 主面板端使用:替换 `ToolCallCard.vue:578-581` 的 `<details>` block

### R2:`ToolOutputBody.vue` 新增(纯渲染)

- **Props**:`content: string`, `isError: boolean`, `durationMs?: number`
- **职责**:
  - 内部 `display = extractToolResultDisplay(content)`,`truncated = truncateOutput(display, 500)`
  - 渲染 `<details>` + `<summary>output · {{ sizeLabel }}</summary>` + `<pre>`(isError 时 pre 边框变红)
  - `durationMs` 存在时 summary 行加 ` · {{ abbreviateDuration(durationMs) }}`(沿用主面板 statusText 旁的 duration 风格)
- **不读 store**
- drawer 端使用:`<ToolOutputBody :content="payload_json.content as string" :isError="payload_json.is_error as boolean" :durationMs="payload_json.duration_ms as number | undefined" />`
- 主面板端使用:替换 `ToolCallCard.vue:583-586` 的 output `<details>` block

### R3:`PermissionAskBody.vue` 新增(渲染 + 可选回调)

- **Props**:`mode: 'interactive' | 'historical'`, `ask: PermissionAsk`, `onRespond?: (decision: PermissionDecision, reason?: string) => void`
- **mode === 'interactive'**:渲染现有 4 按钮(仅一次 / 始终允许 / 拒绝 / 拒绝并说明)+ feedback textarea;点按钮调 `onRespond(decision, reason?)`
- **mode === 'historical'**:渲染 info-only 行(worker wanted X at path, denied [worker context, ask collapsed] / worker wanted X, auto-allowed [low risk]);**不渲染按钮**;`onRespond` 不调
- **不读 store** — store 依赖由 parent 注入(`ToolCallCard.vue` 提供 `onRespond` → `respondApproval`;drawer 端不传 `onRespond`)
- drawer 端 `historical` mode:`<PermissionAskBody mode="historical" :ask="synthesizeAsk(payload_json)" />`
- 主面板端 `interactive` mode:替换 `ToolCallCard.vue:486-525` 的 approval block

### R4:`ToolCallCard.vue` 重构

- 删除现有 input/output `<details>` block(行 578-586)和 approval block(行 486-525)的内联模板
- 改为 `<ToolInputBody :name="call.name" :input="call.input" />` / `<ToolOutputBody :content="result.content" :isError="result.isError" :durationMs="result.durationMs" />` / `<PermissionAskBody mode="interactive" :ask="pendingAsk" :onRespond="respondApproval" />`(v-if 条件保留)
- `formatToolInput` import 保留(可能用不到 — body 内部自己 stringify)— 待实现时确定
- `truncateOutput` / `extractToolResultDisplay` import 保留(仅 `displayContent` 仍用)
- store 依赖**零变化**
- **diff popover**(行 559-576)**留 inline** — drawer 不用 diff(FT-F-001 D6 决定)
- **dispatch_subagent preview**(行 534-551)**留 inline** — 主面板 dispatch 卡片专用
- `subagent-preview` 内的 `extractToolResultDisplay` import 仍用

### R5:测试拆分

- **`ToolCallCard.test.ts` 14 test 零改动** — `shallow: true` 仍 mock 掉新 body
- 新增 `ToolInputBody.test.ts`:~5 test(空 input 不渲染 / 字符串 input 渲染 / 对象 input 渲染 / 嵌套对象渲染 / 空对象不渲染)
- 新增 `ToolOutputBody.test.ts`:~6 test(普通 content / cwd envelope 自动拆 / truncate 长 content / isError 视觉 / durationMs 显示 / 无 result 不渲染)
- 新增 `PermissionAskBody.test.ts`:~8 test(interactive: 4 按钮渲染 / 仅一次/始终允许/拒绝/拒绝并说明 触发 onRespond / 拒绝并说明 提交 feedback / 缺 onRespond 不调 / historical: 渲染 info-only / 不渲染按钮 / 显示 risk / 显示 path)
- 14 + 5 + 6 + 8 = **33 test**(从 14 增到 33,基线 +19)

### R6:文档同步

- `docs/HACKING-markdown.md` 段(如提到 ToolCallCard 内联渲染)— 不需改(本 task 不动 markdown)
- `docs/IMPLEMENTATION.md` §4 决策日志 — 加 1 条 2026-06-20 shared body 抽出决策
- `.trellis/reviews/DEBT.md` FT-F-001 段 — `Blocked by` 引用保留(本 task 合并后改 closed)

---

## Acceptance Criteria(可勾选)

- [ ] **AC1**:`app/src/components/chat/ToolInputBody.vue` 存在,Props 形状 = `{ name: string, input: Record<string, unknown> }`,不读 store
- [ ] **AC2**:`app/src/components/chat/ToolOutputBody.vue` 存在,Props 形状 = `{ content: string, isError: boolean, durationMs?: number }`,不读 store
- [ ] **AC3**:`app/src/components/chat/PermissionAskBody.vue` 存在,Props 形状 = `{ mode, ask, onRespond? }`,interactive 模式调 onRespond,historical 模式不调
- [ ] **AC4**:`ToolCallCard.vue` 编译通过,store 依赖零变化(input/output/approval 三处内联 block 替换为 body component 调用)
- [ ] **AC5**:`pnpm vitest run app/src/components/chat/ToolCallCard.test.ts` → 14 test 全 pass(零改动)
- [ ] **AC6**:`pnpm vitest run app/src/components/chat/ToolInputBody.test.ts` → ~5 test 全 pass
- [ ] **AC7**:`pnpm vitest run app/src/components/chat/ToolOutputBody.test.ts` → ~6 test 全 pass
- [ ] **AC8**:`pnpm vitest run app/src/components/chat/PermissionAskBody.test.ts` → ~8 test 全 pass
- [ ] **AC9**:`pnpm vitest run` 全集 → pass 数 ≥ 基线 232(14 + 19 新增 = 33,基线不变或 +19)
- [ ] **AC10**:`pnpm vue-tsc --noEmit` → 0 error
- [ ] **AC11**:`app/src/components/chat/ToolCallCard.vue` 行数 ≤ 600 行(原 995,抽出 ~400 行 body 模板 + CSS)
- [ ] **AC12**:`git grep -nE "JSON\.stringify\(.*input" app/src/components/chat/ToolCallCard.vue` → 0 hit(确认内联 stringify 已迁出)
- [ ] **AC13**:`git grep -nE "extractToolResultDisplay" app/src/components/chat/ToolCallCard.vue` → ≤ 1 hit(仅 dispatch_subagent preview 用)
- [ ] **AC14**:drawer(FT-F-001 阶段 2)可直接消费 3 个 body(本 task 不实施 drawer,只验 API 形状 + 类型 import 兼容)

---

## Technical Approach

### 架构(改后)

```
ToolCallCard.vue (outer + store + 4 形态逻辑)
├── header                 ── 留 inline
├── PermissionAskBody      ── 抽出 (mode='interactive')
├── subagent-preview       ── 留 inline (drawer 不用)
├── DiffView popover       ── 留 inline (drawer 不用)
├── ToolInputBody          ── 抽出 (通用 tool call)
└── ToolOutputBody         ── 抽出 (通用 tool result)

SubagentDrawer.vue (FT-F-001 阶段 2,本 task 不改)
├── kind badge + outer wrapper per kind
├── tool_call entry → <ToolInputBody>
├── tool_result entry → <ToolOutputBody>
├── permission_ask entry → <PermissionAskBody mode='historical'>
└── chat_event entry → <WorkerTextTimeline> (drawer 独立 component)
```

### Component Props 详细

```ts
// ToolInputBody.vue
defineProps<{
  name: string;
  input: Record<string, unknown>;
}>();

// ToolOutputBody.vue
defineProps<{
  content: string;
  isError: boolean;
  durationMs?: number;
}>();

// PermissionAskBody.vue
defineProps<{
  mode: 'interactive' | 'historical';
  ask: PermissionAsk;  // from stores/permissions.ts
  onRespond?: (decision: PermissionDecision, reason?: string) => void;
}>();
// emits: 内部不需要 emit(直接调 onRespond)
```

### 数据流

**主面板**(`ToolCallCard.vue`):
```
chat.ts (ToolCallInfo + ToolResultInfo)
  └─→ ToolInputBody :name :input
  └─→ ToolOutputBody :content :isError :durationMs

permissions.ts (PermissionAsk) + 现有 respondApproval
  └─→ PermissionAskBody mode='interactive' :ask :onRespond=respondApproval
```

**drawer**(`SubagentDrawer.vue`,FT-F-001 阶段 2,本 task 不实施):
```
subagentRuns.ts (TranscriptEntry.payload_json)
  └─→ ToolInputBody :name=...(payload_json.name) :input=...(payload_json.input)
  └─→ ToolOutputBody :content=...(payload_json.content) :isError=...(payload_json.is_error) :durationMs=...(payload_json.duration_ms)
  └─→ PermissionAskBody mode='historical' :ask=synthesizeAsk(payload_json)
       synthesizeAsk = (p) => ({rid: '', sessionId: '', toolUseId: p.tool_use_id, toolName: '', toolInput: {}, risk: p.risk, reason: p.reason, path: p.path})
```

### 改动文件清单

| 文件 | 动作 | 行数估算 |
|---|---|---|
| `app/src/components/chat/ToolInputBody.vue` | **新增** | +60 行(模板 ~25 / script ~10 / CSS ~25) |
| `app/src/components/chat/ToolOutputBody.vue` | **新增** | +80 行(模板 ~30 / script ~15 / CSS ~35) |
| `app/src/components/chat/PermissionAskBody.vue` | **新增** | +200 行(模板 ~80 / script ~30 / CSS ~90,沿用现有 approval 视觉) |
| `app/src/components/chat/ToolCallCard.vue` | **重构** | -400 行(995 → ~600,删 3 个内联 block,加 3 个 component import + 调用) |
| `app/src/components/chat/ToolInputBody.test.ts` | **新增** | +100 行(~5 test) |
| `app/src/components/chat/ToolOutputBody.test.ts` | **新增** | +130 行(~6 test) |
| `app/src/components/chat/PermissionAskBody.test.ts` | **新增** | +180 行(~8 test) |
| `app/src/components/chat/ToolCallCard.test.ts` | 零改 | 0 行 |
| `docs/IMPLEMENTATION.md` §4 | +1 条决策 | +10 行 |
| `.trellis/reviews/DEBT.md` FT-F-001 段 | `Blocked by` 改 `Resolved` | 0 行(状态更新) |

**总估算**:+760 / -400 = 净 +360 行(test 占大头,production code 净减少)

---

## Decision (ADR-lite)— 8 D 决策全档

### D1 (2026-06-20):Body 数量 = 3 独立(排除 1 variant prop / 4th 抽 diff)

**Context**:`ToolCallCard.vue` 内 3 个内联 block(input / output / approval)结构上完全解耦;FT-F-001 D3 已决定"Body 纯,outer 各起,variant prop 排除"。

**Decision**:**3 独立 body component** — `ToolInputBody` / `ToolOutputBody` / `PermissionAskBody`;**diff 留 inline** 在 `ToolCallCard.vue`(drawer 不用 diff,抽 4th 收益小);**dispatch_subagent preview 留 inline**(drawer 不需要)。

**Consequences**:
- ✅ 每个 body 接口纯净,test 粒度匹配
- ✅ drawer 未来只接 3 body,API surface 小
- ⚠️ `PermissionAskBody` 单 component ~200 行(包含 4 按钮 + feedback textarea + historical 模式行)
- ❌ 排除 1 个 variant prop `<ToolCardBody>` 方案(FT-F-001 D3 已排除)
- ❌ 排除抽 `DiffBody` 4th(收益小,drawer 不用)

### D2 (2026-06-20):Props 形状 = decoupled data props(方案 A,排除方案 B)

**Context**:body 应同时被主面板(`ToolCallInfo` / `ToolResultInfo` / `PermissionAsk`)和 drawer(`payload_json: Record<string, unknown>`)消费。

**Decision**:**方案 A — decoupled data props**:
- `ToolInputBody { name, input }`
- `ToolOutputBody { content, isError, durationMs? }`
- `PermissionAskBody { mode, ask, onRespond? }`

**Consequences**:
- ✅ drawer 端只需 `as` 类型断言,无需合成 `ToolCallInfo`/`ToolResultInfo` wrapper
- ✅ body 内聚(`name + input` 就够,不需要 `id` / `toolUseId` 等 id 字段)
- ✅ test 简单(直接传 plain object,无需 mock ToolCallInfo 完整 shape)
- ⚠️ `PermissionAskBody` 接 `ask: PermissionAsk`(完整类型,因 6+ 字段,decouple 收益小)— 折中
- ❌ 排除方案 B(typed wrapper):drawer 端需写 `synthesizeToolCallInfo` / `synthesizeToolResultInfo`,重复 boilerplate

### D3 (2026-06-20):Store 注入 = callback prop + outer 持 store

**Context**:`ToolCallCard.vue` 当前在 outer 持 3 个 store;body 应不接触 store 以便 drawer 复用。

**Decision**:**callback prop 模式**:
- `ToolInputBody` / `ToolOutputBody` 完全不读 store(纯渲染)
- `PermissionAskBody` 接受 `onRespond?: (decision, reason?) => void` callback,parent(`ToolCallCard.vue` 现有 `respondApproval` 函数)负责调 `permStore.respond`

**Consequences**:
- ✅ 3 body 都不 import Pinia,drawer 端无需 provide/inject setup
- ✅ `PermissionAskBody` test 可纯 `vi.fn()` mock `onRespond`,不需 setActivePinia
- ⚠️ `ToolCallCard.vue` 仍持 3 store(没减少)— 但 store 依赖"集中"是好事
- ❌ 排除 provide/inject(过度抽象,3 body 都集中于一个 parent)
- ❌ 排除 body 直接读 store(body 绑死 store,drawer 复用需 provide)

### D4 (2026-06-20):PermissionAskBody mode = 显式 prop(排除 provide/inject)

**Context**:`PermissionAskBody` 需区分 interactive(主面板式"仅一次/始终允许/拒绝"按钮)和 historical(drawer 式 info-only 行)。

**Decision**:**显式 prop** `<PermissionAskBody mode="interactive" :ask :onRespond />` / `<PermissionAskBody mode="historical" :ask />` — 内部 if-else 切换。

**Consequences**:
- ✅ 模板/调用点清晰,parent 决定 mode
- ✅ historical mode 不传 `onRespond`,TS 编译时强制(图 1:`onRespond?: optional`)
- ❌ 排除 provide/inject(过度抽象,显式 prop 足够)

### D5 (2026-06-20):测试拆分 = ToolCallCard 14 test 零改 + 新增 19 test

**Context**:现有 14 test 在 `ToolCallCard.test.ts` 全部 `shallow: true`,断言在 outer 行为,不在 body 内容渲染上。

**Decision**:
- `ToolCallCard.test.ts` **零改动** — shallow mount 仍 mock 掉新 body,test 行为锁保持
- 新增 `ToolInputBody.test.ts` / `ToolOutputBody.test.ts` / `PermissionAskBody.test.ts` — deep mount,各自验内部渲染

**Consequences**:
- ✅ 14 + 5 + 6 + 8 = 33 test(基线 14 → 33,净 +19)
- ✅ 行为锁零回退风险(14 outer test 零改)
- ⚠️ 新 body test 需 deep mount,启动时间略增(33 test 启动 ~3-5s 增量,可接受)
- ❌ 排除"重写 ToolCallCard.test.ts"(无关 test 浪费 review)

### D6 (2026-06-20):diff 渲染 = 留 inline(排除抽 4th `DiffBody`)

**Context**:`edit_file` 的 diff popover(line 559-576)只用于主面板,drawer 不用 diff(FT-F-001 D6 决定后端零改动,drawer 不引入 diff)。

**Decision**:**留 inline** 在 `ToolCallCard.vue` — 内部继续调 `DiffView` 组件(已独立)。

**Consequences**:
- ✅ 抽出收益小(div popover 容器 ~10 行 CSS + DiffView 调用)
- ✅ drawer 端无需 import `DiffBody`(drawer 完全不渲染 diff)
- ⚠️ `ToolCallCard.vue` 仍含 diff 模板段,行数减少受影响(原 995 → 预估 600)
- ❌ 排除抽 `DiffBody` 4th component(对称性收益 < 维护成本)

### D7 (2026-06-20):CSS 边界 = scoped SFC + CSS vars for theming

**Context**:3 body 内部需保持视觉一致(背景 / 字体 / padding rhythm),同时 outer wrapper 可控主题色。

**Decision**:**scoped CSS** + **CSS variables** for theming:
- 3 body 各自 `<style scoped>`,class 名 BEM(`tool-input-body__*` / `tool-output-body__*` / `permission-ask-body__*`)
- 颜色 / padding 走 CSS vars(`var(--color-bg-elevated)` / `var(--color-tool-error)` / `var(--color-bg-border)`)— 沿用 `ToolCallCard.vue` 已有 vars
- 不引入新 CSS var(避免 var surface 爆炸)

**Consequences**:
- ✅ scoped 隔离防 class 冲突
- ✅ 沿用项目已有 CSS var 体系,theme 切换(如未来 dark mode)零成本
- ⚠️ 3 body 各自 ~30-90 行 CSS,重复略多(可接受,避免抽 common CSS)
- ❌ 排除 unscoped + 全局 class(BEM 冲突风险)
- ❌ 排除 unscoped + inline style(失去 CSS cascade 优势)

### D8 (2026-06-20):导出位置 = `app/src/components/chat/` 同级(排除 `cards/` 子目录)

**Context**:项目结构无 `cards/` 子目录;3 body 是 `ToolCallCard` 的拆解产物,语义上同属 chat 组件。

**Decision**:`app/src/components/chat/ToolInputBody.vue` / `ToolOutputBody.vue` / `PermissionAskBody.vue` — 跟 `ToolCallCard.vue` / `SubagentDrawer.vue` 平级。

**Consequences**:
- ✅ 跟现有项目结构一致
- ✅ import path 短(`./ToolInputBody.vue` 同 `import { formatToolInput } from "../../utils/messageFormat"`)
- ⚠️ 3 文件 + 现有 chat 组件可能让 `app/src/components/chat/` 目录略乱(~12 个文件,可接受)
- ❌ 排除 `app/src/components/chat/cards/` 子目录(项目无子目录传统)

---

## Definition of Done

- AC1-AC14 全 ✅
- 8 D 决策全部执行到位
- `pnpm vitest run` 全集 232+(实际 232 + 19 新增 = 251)
- `pnpm vue-tsc --noEmit` 0 error
- `git grep` 验证 AC12 / AC13 通过
- `IMPLEMENTATION.md` §4 加 1 条决策
- `DEBT.md` FT-F-001 段 `Blocked by` 改 `Resolved` 引用本 task

---

## Out of Scope(明确不做)

- **不动 drawer 任何文件**(`SubagentDrawer.vue` 改造 = FT-F-001 阶段 2)
- **不动 `ToolCallCard.vue` outer wrapper 视觉与行为**(dispatch_subagent 整卡可点击 / diff popover / 状态色全保持)
- **不动 store 任何文件**(`subagentRuns.ts` / `chat.ts` / `permissions.ts`)
- **不动后端任何文件**(frontend-only 任务)
- **不做 body 自己的 markdown 渲染增强**(input 已经是格式化 JSON/text,本 task 不嵌套新 markdown)
- **不做 `DiffBody` 独立 component**(D6 决定留 inline)
- **不做跨 body 共享 sub-component**(如 `<JsonBlock>` / `<KeyValueList>`);只接受现有 helper 复用(`extractToolResultDisplay` / `truncateOutput` / `abbreviateDuration`)
- **不做 permission_ask 的 store 改造**(保持 `permissions.ts` 现状)
- **不做 `formatToolInput` helper 改造**(本 task 不删 — body 内部自己 `JSON.stringify` 即可,helper 留着以备其他调用方)

---

## 启动 checklist(进入 in_progress 时)

- [x] 走 `trellis-brainstorm` skill 答 8 个 Q(本轮完成)
- [x] 决定 3 body 命名 + props 形状(D1/D2)
- [x] 决定 store 注入方式(D3)
- [x] 决定 PermissionAskBody mode prop 位置(D4)
- [x] 决定测试拆分粒度(D5)— 14 零改 + 19 新增
- [x] 决定 diff 归属(D6)— 留 inline
- [x] 决定 CSS 边界(D7)— scoped + vars
- [x] 决定导出位置(D8)— 同级
- [ ] Phase 1.3:curate `implement.jsonl` + `check.jsonl`(workflow.md 要求,spec + research 文件)
- [ ] `python3 ./.trellis/scripts/task.py start .trellis/tasks/06-20-06-20-frontend-tool-call-card-shared-body-extract/`
- [ ] 决定是否开 worktree(推荐:本 task 涉及 3 新文件 + 1 重构 + 3 新 test,worktree 较安全)
- [ ] 跑 `pnpm vitest run app/src/components/chat/ToolCallCard.test.ts` 验证 14 test 全 pass(抽出后行为锁保持)
- [ ] 跑 `pnpm vue-tsc --noEmit` 验证 0 error
- [ ] 跑 `git grep -nE "JSON\.stringify\(.*input" app/src/components/chat/ToolCallCard.vue` 验证 0 hit(AC12)

---

## Implementation Plan(1 PR)

### PR1:`feat(frontend): extract shared body components from ToolCallCard`(约 +760 / -400 行)

**commits 拆分**(1 个 commit / 1 个文件 / 1 个 phase,合并时 squash):

1. **commit 1**:`feat(chat): add ToolInputBody + ToolOutputBody components`
   - 新增 `ToolInputBody.vue` + `ToolInputBody.test.ts`
   - 新增 `ToolOutputBody.vue` + `ToolOutputBody.test.ts`
   - 不动 `ToolCallCard.vue`(并行的 spec-only 文件)
2. **commit 2**:`feat(chat): add PermissionAskBody with interactive + historical modes`
   - 新增 `PermissionAskBody.vue` + `PermissionAskBody.test.ts`
3. **commit 3**:`refactor(chat): replace inline blocks in ToolCallCard with shared body components`
   - 改 `ToolCallCard.vue`(删 3 个内联 block,加 3 个 body 调用)
   - 跑 vitest 验证 14 test 零回退 + 19 新增 pass
   - 跑 vue-tsc 0 error
4. **commit 4**:`docs: record shared body extract decision in IMPLEMENTATION §4`
   - `docs/IMPLEMENTATION.md` §4 加 1 条决策 + 回填本 commit hash
5. **commit 5**:`docs(debt): mark FT-F-001 Blocked by as Resolved`
   - `.trellis/reviews/DEBT.md` FT-F-001 段 `Blocked by` → `Resolved (PR1 merged {hash})`
   - 本 task 加 `Closed At: {commit hash}` 行

**PR 标题**:`feat(frontend): extract shared body components from ToolCallCard (FT-F-001 PR1)`

**PR 描述**:
```
抽 ToolInputBody / ToolOutputBody / PermissionAskBody 3 个 shared body component,
让主面板 (ToolCallCard.vue) 与后续 drawer (FT-F-001 阶段 2) 共用同一组 body 组件,
UX 一致性 + 维护性双提升。

改动:
- 新增 ToolInputBody / ToolOutputBody / PermissionAskBody 3 个 .vue
- 重构 ToolCallCard.vue (995 → ~600 行),用 3 个 body 替换 3 个内联 block
- 新增 19 个 test (5 + 6 + 8),14 个 ToolCallCard.test.ts 零改 (shallow: true)

deps: 无
后端: 零改动
store: 零改动
drawer: 零改动 (FT-F-001 阶段 2 才接入)

Refs: FT-F-001 prd R-HP1/R-HP2/R-HP3/R-HP4
Closes: FT-F-001 Blocked by (本 task = 阶段 1)
```

---

## 测试注意(避免踩上次 FT-F-005 的坑)

实施本 task 时,新 body 内部如用 `Icon` / 定时器 / reka-ui 组件,先读 `~/.claude/projects/-usr-local-code-github-everlasting/memory/subagentdrawer-banner-test-gotchas.md` 避 3 坑:

1. **`Icon` stub `textContent` 不含字符**:`stubs: { Icon: true }` → 渲染 `<icon-stub>`,断言 emoji 字符永远 fail,用 `querySelector` 拿 class 内部 textContent
2. **reka-ui `DialogContent` Teleport DOM leak 跨 test**:`mount(..., { attachTo: document.body })` + `w.unmount()` 不彻底,下一个 test `document.body.querySelector` 找到上次的 dialog,`beforeEach` 手动清
3. **`useFakeTimers` + `setSystemTime` 影响 `Date` 解析**:`vi.setSystemTime` 让 `new Date("...")` 也走 fake clock,fixture 时间相对 system clock 也要设对

**本 task 实际触及面**:
- `ToolInputBody` / `ToolOutputBody` 内部用 `Icon`? 不用(仅 `<details>` + `<pre>`)— Icon stub 坑不触发
- `PermissionAskBody` 内部用 `Icon`? 也不用(approval dot 用 inline `background: var(--color-...)`,text only)— Icon stub 坑不触发
- reka-ui `DialogContent`? 不用(plain div)— Teleport 坑不触发
- `useFakeTimers`? `PermissionAskBody` 无定时器(主面板 outer 才有 `setTimeout` 链)— 不触发

→ 本 task test 实际**安全**,3 坑笔记仅作 FT-F-002 / FT-F-005 / drawer follow-up 时的提醒

---

## 关联

- **上游决策**:`.trellis/tasks/06-20-06-20-frontend-drawer-typed-cards/prd.md`(FT-F-001,R-HP1/R-HP2/R-HP3/R-HP4 + D1/D2/D3/D4/D5/D6/D7 决策 7 条)
- **下游**:本 task 合并后,FT-F-001 阶段 2 才能 `task.py start`
- **DEBT.md**:`.trellis/reviews/DEBT.md` §FT-F-001 段(`Blocked by: 06-20-06-20-frontend-tool-call-card-shared-body-extract/`)
- **关键文件**(本 task 预计改动):
  - `app/src/components/chat/ToolCallCard.vue`(995 → ~600 行)
  - `app/src/components/chat/ToolInputBody.vue`**新增**
  - `app/src/components/chat/ToolOutputBody.vue`**新增**
  - `app/src/components/chat/PermissionAskBody.vue`**新增**
  - `app/src/components/chat/ToolCallCard.test.ts`(14 test,0 改)
  - `app/src/components/chat/ToolInputBody.test.ts` / `ToolOutputBody.test.ts` / `PermissionAskBody.test.ts`**新增**
  - `docs/IMPLEMENTATION.md` §4 决策日志(+1 条)
  - `.trellis/reviews/DEBT.md` FT-F-001 段(状态更新)
- **同源 family**:
  - FT-F-001(drawer typed-cards,本 task 是它的 PR1)
  - FT-F-002 / FT-F-003 / FT-F-004(其他 drawer 独立 polish,本 task 完成后部分会被更易实施)
  - FT-F-005(已 closed,本 task 测试策略参考其 vitest 3 坑笔记)
