# FT-F-001 — SubagentDrawer typed-cards 重做(deferred from B6 PR3b)

> **状态**:**planning,prd 已 sync PR1 实施结果**(2026-06-20 Session 53)。
> PR1 硬前置(`06-20-frontend-tool-call-card-shared-body-extract/`,commit `9b685c8`)已于 Session 52 合并并 archived;
> 本 task(unblocked)接 3 个 shared body component 进 drawer,把统一 JSON payload 渲染改为 typed-cards。
> 下一步:curate jsonl(Phase 1.3)→ `task.py start` → Phase 2 dispatch implement。
>
> **Tracking**:`.trellis/reviews/DEBT.md` §Feature Follow-ups / FT-F-001
>
> **Origin**:Session 49 B6 PR3b brainstorm D1(commit `186e500`),journal 末尾 "Next Steps" 段

---

## Goal(一句话)

把 `SubagentDrawer` 当前**统一 `payload` 字符串**渲染(`formatPayload = JSON.stringify(payload_json, null, 2)` 走 `<pre>`)重做为 typed-cards —— 按 `TranscriptKind`(`chat_event` / `tool_call` / `tool_result` / `permission_ask`)路由到对应组件,drawer 复用 PR1 抽出的 3 个 shared body component + drawer 新做 1 个 `WorkerTextTimeline`,UX 一致性 + 维护性双提升。

---

## What I already know(codebase 摸底,2026-06-20 Session 53 sync PR1)

### Drawer 渲染现状(`SubagentDrawer.vue`,~681 行)

```vue
<ol class="subagent-drawer__list">
  <li v-for="(entry, i) in visibleTranscript" :key="i"
      :class="`subagent-drawer__entry--${entry.kind}`">
    <span class="subagent-drawer__kind">{{ KIND_META[entry.kind].label }}</span>
    <pre class="subagent-drawer__payload">{{ formatPayload(entry) }}</pre>
  </li>
</ol>
```

- `KIND_META` 给 4 个 kind 各一个 label + color:`chat` / `call` / `result` / `perm`
- `formatPayload()` 全部走 `JSON.stringify(entry.payload_json, null, 2)` → `<pre>`
- store 依赖:`useSubagentRunsStore`(openRunId / openRun / liveTranscript / parseTranscriptJson / coerceStatus)。**无 chatStore 依赖**(本 task 要加 `useChatStore().currentCwd` 作 PermissionAskBody repoRoot)
- 现有 test:`SubagentDrawer.test.ts`(基于统一 payload 字符串假设)

### PR1 已落地的 shared body component(commit `9b685c8`,2026-06-20 Session 52)

| 组件 | Props(decoupled data,D2) | store 依赖(D3) | 用途 |
|---|---|---|---|
| `ToolInputBody.vue`(81 行) | `{ name: string, input: Record<string, unknown> }` | 无 | tool_call 的 input `<details>` + `<pre>`(JSON.stringify) |
| `ToolOutputBody.vue`(140 行) | `{ content: string, isError: boolean, durationMs?: number }` | 无 | tool_result 的 output `<details>`,内部 `extractToolResultDisplay` 剥 cwd envelope + `truncateOutput(500)` + size label + durationMs chip(有才显示) |
| `PermissionAskBody.vue`(323 行) | `{ mode: "interactive" \| "historical", ask: PermissionAsk, onRespond?, repoRoot? }` | 无 | interactive = 4 按钮;historical = info-only 行 `worker wanted {toolName} at {path}, ask collapsed (worker context)`,**无按钮、无 denied 字样** |

- 3 body 完全不读 store(D3),callback prop 模式(`onRespond`)
- `PermissionAskBody` 的 path badge(仓库内/外)需 `repoRoot` prop —— 主面板传 `chatStore.currentCwd`,**drawer 端也传 `chatStore.currentCwd`**(Q2 决策:假设 worker 在同一 project root 下跑,多数成立;边缘情况 worker 跑别的 cwd 时 badge 不准,可接受)
- historical mode 实际文案:`worker wanted {{ ask.toolName || "this tool" }} at {{ ask.path }}, ask collapsed (worker context)` —— 故 synthesizeAsk 需填 `toolName` + `path`

### payload_json 实际结构(drawer 端解析基础,后端 serde 验证)

`payload_json = serde_json::to_value(payload)`,4 kind 对应 4 个 payload struct(全部 snake_case,Rust 原生 serde):

| kind | payload_json 字段 | → body props 映射 |
|---|---|---|
| `tool_call` | `{ request_id, id, name, input }` | `ToolInputBody :name=p.name :input=p.input` |
| `tool_result` | `{ request_id, tool_use_id, content, is_error }`(**无 duration_ms**) | `ToolOutputBody :content=p.content :isError=p.is_error`(durationMs 永远 undefined,chip 不显示) |
| `permission_ask` | `{ rid, session_id, tool_use_id, tool_name, tool_input, risk, reason?, path? }` | `PermissionAskBody mode="historical" :ask=synthesizeAsk(p) :repoRoot="chatStore.currentCwd"` |
| `chat_event` | `{ request_id, kind: "start"\|"delta"\|"thinking_delta"\|"signature_delta"\|"redacted_thinking_delta"\|"tool_call"\|"tool_result"\|"done"\|"error"\|..., text?/stop_reason?/usage?/... }` | `WorkerTextTimeline :events=[相邻 chat_event 聚合]`,过滤 `kind==="start"` / `kind==="done"` |

**chat_event 关键**:无 `stop` kind(D7 原文笔误,实际是 `done`);`done` 含 `stop_reason` + `usage`(input/output tokens);`start` 无字段。

### TS 类型(`subagentRuns.ts:71-74`)

```ts
export interface TranscriptEntry {
  kind: TranscriptKind;
  payload_json: Record<string, unknown>;  // snake_case,DB 存储形状
}
```

drawer 端按 kind 用 `as` 断言解 payload_json 字段(D2 decoupled data props 的代价 —— body 不接 typed wrapper,drawer 自己做 snake_case→camelCase 映射 + synthesizeAsk 合成 PermissionAsk)。

---

## Requirements

### 硬前置(PR1,已完成 commit `9b685c8`,Session 52)

- **R-HP1** ✅:从 `ToolCallCard.vue` 抽出 `ToolInputBody` / `ToolOutputBody` / `PermissionAskBody`(PR1 实际命名,非原 brainstorm 的 `ToolCallBody`/`ToolResultBody`)。store 依赖由 parent 注入。
- **R-HP2** ✅:shared body 不接 variant prop,只拿 decoupled data props。
- **R-HP3** ✅:`PermissionAskBody` 接 `mode: "interactive" | "historical"` prop。
- **R-HP4** ✅:独立 task `06-20-frontend-tool-call-card-shared-body-extract/` 已合并 + archived。

### FT-F-001 本 task 范围

**R1**:`SubagentDrawer.vue` 顶部循环改为按 `entry.kind` 路由到 4 个 typed-card:
- `tool_call` → `<ToolInputBody :name="p.name as string" :input="p.input as Record<string, unknown>" />`
- `tool_result` → `<ToolOutputBody :content="p.content as string" :isError="p.is_error as boolean" />`(无 durationMs,符合 ToolOutputBody 设计)
- `permission_ask` → `<PermissionAskBody mode="historical" :ask="synthesizeAsk(p)" :repoRoot="chatStore.currentCwd" />`
- `chat_event` → `<WorkerTextTimeline :events="aggregatedChatEvents" />`(drawer 新做,见 R3)

**R2**:drawer 新增 4 个 outer wrapper(内联在 `SubagentDrawer.vue` 模板,非独立 component):
- 保留现有 `kind` badge(chat/call/result/perm)+ 颜色
- body component 嵌在 badge 下,narrow padding(适配 480px)
- **不**复用主面板 outer 样式 class(D3:outer 各起)

**R3**:`WorkerTextTimeline.vue` **新增**(独立 component,~50 行 + CSS):
- props:`{ events: TranscriptEntry[] }`(已过滤为 kind=chat_event 的子集,或内部过滤)
- 内部按 `payload_json.kind` 过滤:
  - `kind === "start"` → "agent 开始响应"(或首个 delta 到达标记)
  - `kind === "done"` → "agent 完成"(附 `stop_reason`,如 end_turn / max_turns / cancelled)
  - 其他(`delta` / `thinking_delta` / `signature_delta` / `redacted_thinking_delta` / `tool_call` / `tool_result` / `error`)→ **忽略**(已聚合进 start/done 间隔,或 error 由 drawer banner FT-F-005 处理)
- **不显示 token usage**(Q3 决策):drawer header 已有 `tokenUsageJson` 汇总,timeline 行不重复
- 多轮 chat_event(start/done 对)渲染为多行 timeline,每行一个状态点

**R4**:`SubagentDrawer` 现有无关 test(live timer / jump-to-latest / auto-scroll / truncated banner / empty state / kind badge colors / filter toggle)**零改动**;JSON payload 假设的 test 改写为 `findComponent(ToolInputBody)` / `findComponent(ToolOutputBody)` / `findComponent(PermissionAskBody)` / `findComponent(WorkerTextTimeline)` 断言。

**R5**:后端零改动(`subagent:event` IPC schema / `subagent_events` 表 schema / payload struct 全不动;`payload_json` 保持不透明)。

**R6**(drawer 端 synthesizeAsk 合成 helper,内联在 `SubagentDrawer.vue` script):
```ts
// payload_json snake_case → PermissionAsk camelCase 合成
const synthesizeAsk = (p: Record<string, unknown>): PermissionAsk => ({
  rid: String(p.rid ?? ""),
  sessionId: String(p.session_id ?? ""),
  toolUseId: String(p.tool_use_id ?? ""),
  toolName: String(p.tool_name ?? ""),
  toolInput: (p.tool_input ?? {}) as Record<string, unknown>,
  risk: p.risk as Risk,                 // risk 枚举,Rust Risk serde 透传
  reason: p.reason as string | undefined,
  path: p.path as string | undefined,
});
```

---

## Acceptance Criteria(可勾选)

- [ ] **AC1**:drawer `tool_call` entry 渲染为 `<ToolInputBody>`,内容是 worker 实际 tool 的 name + input,不再是 `<pre>{{ JSON.stringify }}</pre>`
- [ ] **AC2**:drawer `tool_result` entry 渲染为 `<ToolOutputBody>`,`is_error` 时 pre 红边框;content 经 `extractToolResultDisplay` 剥 cwd envelope;display 的是实际 tool 输出
- [ ] **AC3**:drawer `permission_ask` entry 渲染为 `<PermissionAskBody mode="historical">`,显示 `worker wanted {toolName} at {path}, ask collapsed (worker context)` 信息行(实际 PR1 historical 文案,**无 "denied" 字样**),**不**渲染 interactive 按钮;path badge 用 `chatStore.currentCwd` 作 repoRoot
- [ ] **AC4**:drawer `chat_event` entry 渲染为 `<WorkerTextTimeline>`,显示 start/done lifecycle 行(附 stop_reason),**不**渲染 delta / thinking_delta 噪声,**不**显示 token usage
- [ ] **AC5**:[2026-06-20 sync,原"完全一致"自相矛盾已校准] drawer 各 typed-card 的 **body 内容**(ToolInputBody 的 input / ToolOutputBody 的 output / PermissionAskBody 的 ask 信息)与主面板对应卡片渲染一致;**outer wrapper**(kind badge / header)按 D3 各自管,不强制视觉统一
- [ ] **AC6**:`pnpm vitest run src/components/chat/SubagentDrawer.test.ts` → 全 pass(JSON payload test 改写为 typed-card 断言,无关 test 零改动)
- [ ] **AC7**:`pnpm vitest run` → 主面板 + 3 body test 全 pass(PR1 已锁,本 task 不动它们)
- [ ] **AC8**:`pnpm vue-tsc --noEmit` → 0 error
- [ ] **AC9**:[2026-06-20 sync] `pnpm vitest run` 全集 → pass 数 ≥ 基线 272(PR1 后;本 task 新增 WorkerTextTimeline test + 改写 drawer test,总数不减少)
- [ ] **AC10**:`WorkerTextTimeline.vue` 新文件存在,props `{ events: TranscriptEntry[] }`,内部过滤 start/done,忽略 delta 噪声,不显示 token
- [ ] **AC11**:drawer 在生产模式跑通完整 worker 流程(dispatch_subagent → worker 跑 read_file / shell / permission / text)→ 各 kind 都正确 typed-card 化显示(手动验证)

---

## Technical Approach

### 架构(改后)

```
SubagentDrawer.vue (outer wrapper per kind, +chatStore.currentCwd 依赖)
  ├── tool_call → <ToolInputBody :name :input>           ← PR1 shared body
  ├── tool_result → <ToolOutputBody :content :isError>    ← PR1 shared body
  ├── permission_ask → <PermissionAskBody mode="historical" :ask=synthesizeAsk :repoRoot>  ← PR1 shared body
  └── chat_event → <WorkerTextTimeline :events>           ← 本 task 新做
```

### 数据流

`TranscriptEntry.payload_json`(snake_case,`subagentRuns.ts:71-74`)在 `SubagentDrawer.vue` 内按 `kind` 分支(见 R1/R6 payload 字段映射表):

- `tool_call`:`{ name, input }` → `ToolInputBody`
- `tool_result`:`{ content, is_error }` → `ToolOutputBody`(无 duration_ms → chip 不显示,符合设计)
- `permission_ask`:`synthesizeAsk(p)`(snake_case→camelCase + PermissionAsk 合成,R6)→ `PermissionAskBody mode="historical"`,repoRoot 取 `chatStore.currentCwd`
- `chat_event`:相邻 kind=chat_event 聚合 → `WorkerTextTimeline`,内部再过滤 start/done

### 改动文件清单(估算)

| 文件 | 动作 | 行数估算 |
|---|---|---|
| `app/src/components/chat/SubagentDrawer.vue` | **重构** | +80 / -30(渲染分支改 typed-card;4 个 outer wrapper 内联;加 `useChatStore` + synthesizeAsk;删 formatPayload) |
| `app/src/components/chat/WorkerTextTimeline.vue` | **新增** | ~50 行 component + ~30 行 CSS |
| `app/src/components/chat/SubagentDrawer.test.ts` | **改写** | +40 / -20(JSON payload test → typed-card findComponent 断言) |

### 不改(Out of Scope 同步)

- **后端**:Rust 任何文件 0 改动(D5)
- **PR1 的 3 body component**:`ToolInputBody` / `ToolOutputBody` / `PermissionAskBody` 0 改动(本 task 只消费,不改 API)
- **主面板**:`ToolCallCard.vue` 0 改动
- **Store**:`subagentRuns.ts` 0 改动(`TranscriptEntry` / `TranscriptKind` / `KIND_META` 已是本 task 直接消费);`chat.ts` 只读 `currentCwd`
- **其他 drawer 文件**:0 改动

---

## Definition of Done

- AC1-AC11 全 ✅
- 7 个 D 决策全部执行到位
- 阶段 1 task 已合并(`06-20-frontend-tool-call-card-shared-body-extract/`,commit `9b685c8`,✅ Session 52)
- DEBT.md FT-F-001 状态更新为 closed + Closed At 回填 commit
- journal Session 53 加 FT-F-001 实施记录

---

## Decision (ADR-lite) — 7 决策(原 Session 51 brainstorm + Session 53 sync 校准)

### D1 (2026-06-20):硬前置形态 = 抽 shared body component ✅ 已执行

**Context**:`SubagentDrawer` 4 种 `TranscriptKind` 走统一 JSON 渲染,主面板 `ToolCallCard` 已有多形态。两者共用卡片逻辑需对齐数据形状。

**Decision**:从 `ToolCallCard.vue` 抽出纯渲染 shared body component,store 依赖由 parent 注入,drawer 和主面板都消费。

**Consequences**:零 adapter 层;后续加新 tool 时新 component 自动被 drawer 复用。PR1(Session 52)已抽出 `ToolInputBody` / `ToolOutputBody` / `PermissionAskBody`。

### D2 (2026-06-20):PR 拆分 = 独立 PR 先做硬前置 ✅ 已执行

**Decision**:本 task(FT-F-001 阶段 2)只负责 drawer 接入;阶段 1(硬前置 refactor)独立 task。阶段 1 已合并(`9b685c8`)。

### D3 (2026-06-20):样式边界 = Body 纯,outer wrapper 各起

**Context**:主面板与 drawer header 不一致(主面板 icon+name+path vs drawer kind badge)。

**Decision**:shared body 只含 input/output/permission 渲染,主面板与 drawer 各给 body 包一层 outer wrapper。body 不接 variant prop。

**Consequences**:shared body 接口纯净;outer 独立迭代。**AC5 已据此校准**(Session 53):只验 body 一致,outer 各自管。

### D4 (2026-06-20):Drawer 测试 = 保留 + 适配

**Decision**:保留无关 test 不动;JSON 假设的 test 改写为 `findComponent(ToolInputBody)` 等 typed-card 断言。

### D5 (2026-06-20):持久化层 = 保持现状

**Decision**:本 task 零后端 schema migration,后端代码零改动。payload_json 保持不透明 Record,frontend 在 render 时按 kind 解析。

### D6 (2026-06-20):Permission 模式 = Body 接 mode prop ✅ PR1 已实现

**Decision**:`PermissionAskBody` 接 `mode: "interactive" | "historical"`。interactive = 主面板式按钮;historical = info-only 行。drawer 传 `mode="historical"`。

**Session 53 sync**:historical 实际文案 `worker wanted {toolName} at {path}, ask collapsed (worker context)`(PR1 实现),**无 "denied" 字样**(原 AC3 措辞已校准)。path badge 需 `repoRoot`,drawer 传 `chatStore.currentCwd`(Q2 决策)。

### D7 (2026-06-20):chat_event 渲染 = 只显示 start/done lifecycle

**Context**:drawer chat_event 默认隐藏,展开后是原始 JSON 噪声。

**Decision**:drawer 抽 `WorkerTextTimeline` component,接 chat_event 数组,过滤 `kind === "start"` / `kind === "done"`(done 附 stop_reason)。

**Session 53 sync**:原笔误"start/stop"应为"start/done"(ChatEvent enum 无 Stop variant,结束信号是 Done);done 含 usage 但 timeline 行**不显示 token**(Q3 决策:drawer header 已有 tokenUsageJson 汇总,避免重复)。worker 中间文本 delta 不显示(已在主面板 dispatch_subagent 卡片展开后显示)。

---

## Out of Scope(明确不做)

- **后端 wire shape / schema** 改动(D5)
- **PR1 的 3 body component** 改动(本 task 只消费)
- **`dispatch_subagent` tool_def** 改动
- **drawer 内做新子视图**(inline expand/collapse tool_call input):用卡片组件原生 `<details>` 逻辑
- **chat_event 的 delta / thinking_delta 渲染**:D7 决定只显示 start/done lifecycle
- **WorkerTextTimeline 显示 token usage**:Q3 决定(header 已有汇总)
- **drawer outer wrapper 的 variant 化**(D3 排除)
- **typed-card 自身的 markdown 渲染**(ToolInputBody 内部 input 已是格式化 JSON)
- **drawer 支持 multiple worker transcripts 并排比较**(单 drawer 一次一个 runId)
- **worker 跑别的 cwd 时 path badge 精确化**(Q2 接受 currentCwd 近似)

---

## Why deferred(为什么不在 B6 PR3b 做)

| 理由 | 细节 |
|---|---|
| **scope 爆炸** | B6 PR3b 已是 race fix + 3 polish,叠 typed-cards → review 困难 |
| **硬前置未定** | shared body vs adapter 路径需先决策(现已 D1 定案 + PR1 落地) |
| **缺真实使用反馈** | drawer 上线后反馈已作 typed-cards 设计 input |

---

## 启动 checklist(进 `task.py start` 前)

- [x] 走 `trellis-brainstorm` 答 Open Questions(Session 51 完成 7 项 + Session 53 sync PR1)
- [x] 决定 PR 拆分(D2)+ 阶段 1 已合并(`9b685c8`)
- [x] 决定 hard prefactor 形态(D1)+ 已执行(PR1)
- [x] 决定 4 个 typed-card 具体行为(D3 / D4 / D6 / D7)
- [x] Session 53 sync prd:组件名 / props shape / AC3/AC5/AC9 校准 / payload 字段表 / repoRoot 决策
- [ ] **Phase 1.3:curate `implement.jsonl` + `check.jsonl`**(当前还是 seed `_example`,待 curate)
- [ ] `task.py start` 进 Phase 2

---

## 关联

- **DEBT.md**:`.trellis/reviews/DEBT.md` §FT-F-001(open,PR1 已 Resolved)
- **journal**:`.trellis/workspace/Carlos-home/journal-2.md` Session 52(PR1)+ Session 53(本 task)
- **PR1 commit**:`9b685c8`(shared body 抽出,Session 52 archived)
- **关键文件**:
  - `app/src/components/chat/SubagentDrawer.vue`(~681 行,待重构)
  - `app/src/components/chat/ToolInputBody.vue` / `ToolOutputBody.vue` / `PermissionAskBody.vue`(PR1 产出,本 task 消费)
  - `app/src/stores/subagentRuns.ts`(`TranscriptEntry` / `TranscriptKind` / `KIND_META` 类型源)
  - `app/src/stores/chat.ts`(`currentCwd` —— PermissionAskBody repoRoot 来源)
  - `app/src/stores/permissions.ts`(`PermissionAsk` / `Risk` / `PermissionDecision` 类型源)
- **同源 follow-up**:FT-F-002(toast fallback)/ FT-F-003(workerWaiting ref leak)
