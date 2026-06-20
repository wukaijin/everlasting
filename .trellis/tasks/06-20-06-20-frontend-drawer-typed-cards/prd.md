# FT-F-001 — SubagentDrawer typed-cards 重做(deferred from B6 PR3b)

> **状态**:**planning** — brainstorm 阶段完成(2026-06-20),7 个 Open Questions 全部答完,等用户 final confirmation 后进 `task.py start` 之前的 Phase 1.3 (curate jsonl)
>
> **Tracking**:`.trellis/reviews/DEBT.md` §Feature Follow-ups / FT-F-001
>
> **Origin**:Session 49 B6 PR3b brainstorm D1(commit `186e500`),journal 末尾 "Next Steps" 段

---

## Goal(一句话)

把 `SubagentDrawer` 当前**统一 `payload` 字符串**渲染(`formatPayload = JSON.stringify(payload_json, null, 2)` 走 `<pre>`)重做为 typed-cards —— 按 `TranscriptKind`(`chat_event` / `tool_call` / `tool_result` / `permission_ask`)路由到对应组件,drawer 与 chat 主面板共用同一组卡片组件,UX 一致性 + 维护性双提升。

---

## What I already know(codebase 摸底,2026-06-20)

### Drawer 渲染现状(`SubagentDrawer.vue:374-409`)

```vue
<ol class="subagent-drawer__list">
  <li
    v-for="(entry, i) in visibleTranscript"
    :key="i"
    :class="`subagent-drawer__entry--${entry.kind}`"
  >
    <span class="subagent-drawer__kind">{{ KIND_META[entry.kind].label }}</span>
    <pre class="subagent-drawer__payload">{{ formatPayload(entry) }}</pre>
  </li>
</ol>
```

- `KIND_META` 给 4 个 kind 各一个 label + color:`chat` / `call` / `result` / `perm`
- `formatPayload()` 全部走 `JSON.stringify(entry.payload_json, null, 2)` → `<pre>`
- 唯一差异化 = kind badge 颜色 + label 文案

### 主面板卡片现状(`ToolCallCard.vue`)

`ToolCallCard` 已经是 4 形态合一的多形态卡片:
- 通用 tool call:input `<details>` + output `<details>`(line 578-586)
- `dispatch_subagent` 特殊分支:整卡可点击 → 开 drawer(input/output `<details>` 被 `v-if="!isDispatchSubagent"` 抑制)
- pending 权限:inline approval UI(line 487-525,replaces 旧全局 PermissionModal)
- `edit_file`:inline diff popover(line 559-576)

主面板的 `ToolCallInfo { id, name, input }` 与 drawer 的 `TranscriptEntry { kind, payload_json }` **形状不对齐**:
- 主面板 `call.input` 是 dispatch_subagent 自己的 args(`{ subagent, prompt, ... }`)
- drawer `payload_json` 是 worker 实际 emit 的 tool call input(可能是 `{path: "..."}` for read_file)
- 因此 `TranscriptEntry` 直接喂给 `ToolCallCard` 不能用 —— 需要 adapter 或抽出共享 body component

### Store 与类型(`subagentRuns.ts:42-117`)

```ts
export type TranscriptKind =
  | "chat_event"
  | "tool_call"
  | "tool_result"
  | "permission_ask";

export interface TranscriptEntry {
  kind: TranscriptKind;
  payload_json: Record<string, unknown>;
}
```

- payload 是 `Record<string, unknown>` —— 强类型缺失,需要按 kind 缩窄
- `chat_event` payload 形如 `{ type: "delta" | "start" | "stop" | ..., text?: string, ... }`(SSE delta 流)
- `tool_call` payload 形如 `{ id, name, input }`(worker 实际发起的 tool_use)
- `tool_result` payload 形如 `{ tool_use_id, content, is_error, duration_ms? }`
- `permission_ask` payload 形如 `{ tool_use_id, risk, path?, reason? }`(对应 `pendingAsk`)

### 现有测试覆盖

- `SubagentDrawer.test.ts`:12 个,基于"统一 payload 字符串"渲染假设
- `ToolCallCard.test.ts`:14 个,覆盖 dispatch_subagent + 通用 tool call + permission + diff
- `subagentRuns.test.ts`:26 个,store 行为

---

## Requirements(从 7 个 Q&A 收敛)

### 硬前置(独立 PR / 独立 task,FT-F-001 不做)

**R-HP1**:从 `ToolCallCard.vue` 抽出 `ToolCallBody` / `ToolResultBody` / `PermissionAskBody` 三个 shared body component,store 依赖由 parent 注入。
**R-HP2**:shared body **不接 variant prop**,只拿 data,主面板与 drawer 各包一层 outer wrapper。
**R-HP3**:`PermissionAskBody` 接 `mode: 'interactive' | 'historical'` prop(只有它接)。
**R-HP4**:阶段 1 task 需另起 `.trellis/tasks/...-frontend-tool-call-card-shared-body-extract/`,挂为 FT-F-001 的依赖。

### FT-F-001 本 task 范围

**R1**:`SubagentDrawer.vue` 顶部循环改为按 `entry.kind` 路由到 4 个 typed-card component:
- `tool_call` → `<ToolCallBody :input=...>`(复用主面板 `ToolCallBody`)
- `tool_result` → `<ToolResultBody :content=... :isError=...>`(复用主面板 `ToolResultBody`)
- `permission_ask` → `<PermissionAskBody mode="historical" :payload=...>`(复用主面板,传 historical mode)
- `chat_event` → `<WorkerTextTimeline :events=...>`(drawer 内新做,只显示 start/stop lifecycle)

**R2**:drawer 新增 4 个 outer wrapper(内联在 `SubagentDrawer.vue` 模板,非独立 component):
- `kind` badge + narrow padding
- 与主面板 outer 视觉一致但宽度收窄(480px)
- 不复用主面板 outer 样式 class

**R3**:`WorkerTextTimeline` 接收 `chat_event[]` 数组,内部按 `type` 过滤:
- `type: 'start'` → "agent starting response"
- `type: 'stop'` → "agent finished N.Xs"
- 其他(`delta` / `content_block` 等)→ 忽略(已聚合)

**R4**:`SubagentDrawer` 现有 8 个无关 test(live timer / jump-to-latest / auto-scroll / truncated banner / empty state / kind badge colors / filter toggle)**零改动**;4 个 JSON 假设的 test 改写为 `findComponent(ToolCallBody)` 等 typed-card 断言。

**R5**:后端零改动(`subagent:event` IPC schema / `subagent_events` 表 schema 不动;`payload_json` 保持不透明)。

---

## Acceptance Criteria(可勾选)

- [ ] AC1:drawer 显示 transcript 时,`tool_call` entry 渲染为 `<ToolCallBody>`,内容是 worker 实际 tool 的 input(path / line range / 等),不再是 `<pre>{{ JSON.stringify(...) }}</pre>`
- [ ] AC2:`tool_result` entry 渲染为 `<ToolResultBody>`,含 `is_error` 红条 + content(经 `extractToolResultDisplay` 剥离 cwd envelope)
- [ ] AC3:`permission_ask` entry 渲染为 `<PermissionAskBody mode="historical">`,显示 "worker wanted X at path, denied (worker context, ask collapsed)" 信息,**不**渲染 interactive 按钮
- [ ] AC4:`chat_event` entry 渲染为 `<WorkerTextTimeline>` 的 lifecycle 行(start / stop),**不**渲染 delta 噪声
- [ ] AC5:drawer outer wrapper 视觉与主面板对应卡片**完全一致**(accent 色 / 字体 / padding rhythm)
- [ ] AC6:`pnpm vitest run` → 12 个 `SubagentDrawer.test.ts` 全 pass(4 改写 + 8 零改动)
- [ ] AC7:`pnpm vitest run` → 14 个 `ToolCallCard.test.ts` 全 pass(无变化;依赖 R-HP1 阶段 1 已合并)
- [ ] AC8:`pnpm vue-tsc --noEmit` → 0 error
- [ ] AC9:`pnpm vitest run` 全集 → pass 数不减少(基线 232)
- [ ] AC10:drawer 在生产模式跑通完整 worker 流程(dispatch_subagent → worker 跑 read_file / shell / permission / text)→ 各 kind 都正确 typed-card 化显示

---

## Technical Approach(摘要)

### 架构

```
SubagentDrawer (outer wrapper per kind)
  ├── tool_call → <ToolCallBody :input=...>     ← shared body(来自阶段 1)
  ├── tool_result → <ToolResultBody :content=... :isError=...>
  ├── permission_ask → <PermissionAskBody mode="historical" :payload=...>
  └── chat_event → <WorkerTextTimeline :events=...>  ← drawer 新做
```

### 数据流(本 task 改动部分)

`TranscriptEntry` (`subagentRuns.ts:107-114`) 在 `SubagentDrawer.vue` 内按 `kind` 分支:
- `tool_call`:从 `payload_json` 抽 `{id, name, input}` → 喂 `<ToolCallBody :name :input>`(不传 id,body 不需要)
- `tool_result`:从 `payload_json` 抽 `{tool_use_id, content, is_error, duration_ms?}` → 喂 `<ToolResultBody :content :isError :durationMs>`
- `permission_ask`:从 `payload_json` 抽 `{tool_use_id, risk, path?, reason?}` → 喂 `<PermissionAskBody mode="historical" :payload>`
- `chat_event`:聚合相邻 `start`/`stop` 为 lifecycle 行 → 喂 `<WorkerTextTimeline :events>`

### 改动文件清单(估算)

- `app/src/components/chat/SubagentDrawer.vue`:+80 / -30(refactor 渲染分支;加 4 个 outer wrapper;新增 `WorkerTextTimeline` 内联或独立 component)
- `app/src/components/chat/WorkerTextTimeline.vue`:**新增**(~50 行 component + ~30 行 CSS)
- `app/src/components/chat/SubagentDrawer.test.ts`:+40 / -20(4 个 JSON 测试改写为 typed-card 断言)

### 不改

- 后端:Rust 任何文件 0 改动
- 主面板:`ToolCallCard.vue` 由阶段 1 改,本 task 不动
- Store:`subagentRuns.ts` 不动(`TranscriptEntry` / `TranscriptKind` / `KIND_META` 已是本 task 直接消费)
- 其他 drawer 文件:0 改动

---

## Definition of Done

- AC1-AC10 全 ✅
- 7 个 D 决策全部执行到位
- 阶段 1 task 已合并(`06-20-frontend-tool-call-card-shared-body-extract/` 必须先合)
- DEBT.md FT-F-001 状态更新为 closed + Closed At 回填 commit
- journal Session 49 "Next Steps" FT-F-001 段加 commit hash

---

## Decision (ADR-lite) — 7 决策全档

### D1 (2026-06-20):硬前置形态 = 抽 shared body component

**Context**:`SubagentDrawer` 当前 4 种 `TranscriptKind` 走统一 `JSON.stringify` 渲染,主面板 `ToolCallCard` 已有多形态(input / output / permission / diff)。两者要共用卡片逻辑,但数据形状不对齐(`TranscriptEntry.payload_json: Record<string, unknown>` vs `ToolCallInfo { id, name, input }`)。

**Decision**:**A. 抽 shared body component** —— 从 `ToolCallCard.vue` 抽出 `ToolCallBody` / `ToolResultBody` / `PermissionAskBody` 等纯渲染 component,store 依赖由 parent 注入,drawer 和主面板都消费。

**Consequences**:
- ✅ 零 adapter 层,零 id 合成,store 边界天然清晰
- ✅ 后续加新 tool 时,新 component 自动被 drawer 复用(无需双写)
- ⚠️ ToolCallCard 内部要拆成多 component,refactor 量大(估算 ~50-80 行 抽出,~100 行测试更新)
- ⚠️ 抽出的 component 命名 / props 形状要敲定,可能需要 round-trip 一两轮 review
- ❌ 排除 adapter 路径(合成的 `id` 语义弱;`ToolCallCard` 的 result / permission 路径假设了主面板 store,drawer 接进来要么全局共享要么再 adapter,两层都不净)

### D2 (2026-06-20):PR 拆分 = 独立 PR 先做硬前置

**Context**:D1 决定抽 shared body component 后,工作量分两段:(1) refactor `ToolCallCard` 抽出 shared body + 调出测试;(2) drawer 接入 shared body 改 typed-cards。

**Decision**:**A. 独立 PR 先做硬前置** —— 本 task (FT-F-001) 只负责阶段 2;阶段 1 (硬前置 refactor) 另起 task,等阶段 1 合并后本 task 才能开始。

**Consequences**:
- ✅ 每个 PR scope 小,review 独立
- ✅ 硬前置 API 可被未来其他组件复用(新 tool / 新视角)
- ⚠️ 实际开工需 2 个 task setup + 等阶段 1 合并
- ⚠️ 阶段 1 需独立 brainstorm(命名 / props 形状 / 测试粒度)
- 📋 **Action**:阶段 1 task 需另起,挂为 FT-F-001 的依赖(见 prd.md 末尾 "Action Items")

### D3 (2026-06-20):样式边界 = Body 纯,outer wrapper 各起

**Context**:D1 决定抽 shared body,主面板与 drawer 数据形状不同(`ToolCallInfo` vs `TranscriptEntry`),主面板与 drawer header 也不一致(主面板 icon+name+path vs drawer kind badge)。

**Decision**:**A. Body 纯,outer wrapper 各起** —— shared body 包含 input/output/diff 渲染,主面板与 drawer 各给 body 包一层 outer wrapper(主面板 = tool-card header,drawer = kind badge + narrow padding + 自定色)。body 不接 variant prop,只拿 data。

**Consequences**:
- ✅ shared body 接口纯净(只吃 data,不关心 container)
- ✅ 主面板与 drawer outer wrapper 独立迭代(后续 drawer 加新视角如 inline expand 不影响主面板)
- ⚠️ drawer outer wrapper 仍需 ~30-50 行模板 + ~50 行 CSS(与主面板 outer 解耦)
- ❌ 排除 variant prop 方案(避免 body 变胖,variant 多变体爆炸)

### D4 (2026-06-20):Drawer 测试 = 保留 + 适配

**Context**:现有 12 个 `SubagentDrawer.test.ts` 中,8 个与 typed-cards 无关(live timer / jump-to-latest / auto-scroll / truncated banner / empty state / kind badge colors / filter toggle),4 个假设"统一 JSON payload 渲染"。

**Decision**:**A. 保留 + 适配** —— 保留 8 个无关 test 不动;4 个 JSON 假设的 test 改写为 'render typed-card component' 断言(用 vue-test-utils `findComponent(ToolCallBody)` 等)。typed-card 子 component 的内部 behavior 测由阶段 1 硬前置 task 负责。

**Consequences**:
- ✅ 8 个 test 零改动,行为锁保持
- ✅ 4 个 test 改写后能锁定 typed-cards 路由逻辑
- ⚠️ 改写需要 phase 1 抽出的 component 名(依赖阶段 1 task 命名)
- ❌ 排除重写全部 test(无关 test 浪费 review)

### D5 (2026-06-20):持久化层 = 保持现状

**Context**:FT-F-001 是 frontend 任务,后端 `subagent_events` 表保持 `kind` enum + `payload_json` TEXT 不透明已能撑住 typed-cards。frontend 在 mount/render 时 `JSON.parse` 拿内部字段。

**Decision**:**A. 保持现状** —— 本 task 零后端 schema migration,后端代码零改动。

**Consequences**:
- ✅ Scope 最小,frontend-only 任务
- ✅ typed-cards 在 frontend 解析开销可忽略(每个 transcript entry 一次 JSON.parse)
- ❌ 排除 typed columns 方案(over-engineering,本 task 不需要)
- ❌ 排除 schema_version 方案(本 task 不需 forward compat)

### D6 (2026-06-20):Permission 模式 = Body 接 mode prop

**Context**:drawer 内 `permission_ask` 是 worker 命中权限边界的**历史记录**(worker 路径上 ask 被 collapse,见 RULE-A-016 / FT-A-016 PR3a 实施);不是主面板式的 interactive 待审批。

**Decision**:**A. Body 接 `mode: 'interactive' | 'historical'` prop** —— shared `PermissionAskBody` 内部按 mode 分支:interactive = 主面板式"仅一次/始终允许/拒绝"按钮;historical = info-only "worker wanted X, denied (worker context, ask collapsed)"。tool_call / tool_result body 不接此 prop(无需区分)。

**Consequences**:
- ✅ 单一 `PermissionAskBody` 组件覆盖两种 context,零 component 数量膨胀
- ✅ drawer outer wrapper 传 `mode="historical"`,主面板 outer wrapper 传 `mode="interactive"`
- ⚠️ shared body 接 prop,接口略复杂(只 1 个 prop 区分)
- ❌ 排除 drawer 独立 component 方案(permission 维护成本翻倍)
- ❌ 排除现状(违反 typed-cards 初衷)

### D7 (2026-06-20):chat_event 渲染 = 只显示 start/stop lifecycle

**Context**:drawer 现状默认隐藏 chat_event,展开后是原始 JSON 噪音。worker text delta payload 含 `type: 'delta' | 'start' | 'stop' | ...` 多个事件。

**Decision**:**A. 只显示 start/stop lifecycle** —— drawer 抽 `WorkerTextTimeline` component(或更轻量在 drawer 内):接一轮 `message_start` = "agent starting response";接 `message_stop` = "agent finished N.Xs, used X tokens"。`delta` / `content_block` 不单独显示(已聚合进 start/stop 间隔)。

**Consequences**:
- ✅ drawer chat_event 区像 log timeline,一行一状态,不展开噪声
- ✅ 与 typed-cards 形态一致(kind badge 'chat' + lifecycle line,非 JSON)
- ⚠️ 用户看不到 worker 中间文本(只看到聚合的 start/stop 状态)—— 这是有意的:worker text 在主面板 dispatch_subagent 卡片展开后已显示(见 `extractToolResultDisplay` 走 `format_dispatch_result`)
- ❌ 排除复用 MessageItem(drawer 480px 宽展开会变 scrolly hell)
- ❌ 排除原始 JSON(违反 typed-cards 初衷)

---

## Out of Scope(明确不做)

- **后端 wire shape / schema** 改动(0 字节,见 D5)
- **`dispatch_subagent` tool_def** 改动
- **drawer 内做新子视图**(如 inline expand/collapse tool_call input):用卡片组件原生逻辑
- **message_start / message_stop 之间的中间 delta 渲染**:D7 决定只显示 lifecycle
- **drawer outer wrapper 的 variant 化**(D3 排除)
- **typed-card 自身的 markdown 渲染**(`ToolCallBody` 内部 input 已经是格式化 JSON/text,不再嵌套 markdown)
- **drawer 内做 chat_event 的 delta 流可视化**(D7 决定不显示)
- **drawer 支持 multiple worker transcripts 并排比较**(单 drawer 一次一个 runId,见 `store.openRunId` 注释)

---

## Why deferred(为什么不在 B6 PR3b 做)

| 理由 | 细节 |
|---|---|
| **scope 爆炸** | B6 PR3b 已是 race fix + 3 polish,叠 typed-cards → review 困难 |
| **硬前置未定** | shared body vs adapter 路径需先决策,直接做 typed-cards = 赌一边回滚风险 |
| **缺真实使用反馈** | drawer 刚上线,真实使用反馈可作 typed-cards 设计的 input |

---

## 启动 checklist(进 `task.py start` 前)

- [x] 走 `trellis-brainstorm` skill 答 Open Questions 7 项(本轮 2026-06-20 完成)
- [x] 决定 PR 拆分(D2)
- [x] 决定 hard prefactor 形态(D1)
- [x] 决定 4 个 typed-card 的具体行为(D3 / D4 / D6 / D7)
- [ ] **待用户 final confirmation**(本回合需 approve)
- [ ] 阶段 1 task 需另起(`.trellis/tasks/...-frontend-tool-call-card-shared-body-extract/`)并 `task.py start`(D2 Action)
- [ ] 阶段 1 task 合并后,本 task 才能 `task.py start`
- [ ] Phase 1.3:curate `implement.jsonl` + `check.jsonl`(workflow.md 要求)

---

## Action Items(进 in_progress 前必做)

1. **AI 必做(等用户 confirm 后)**:起阶段 1 task skeleton `06-20-frontend-tool-call-card-shared-body-extract/` + 同 FT-F-001 占位策略(不立即 brainstorm,等用户要启动时再走)
2. **AI 必做**:DEBT.md FT-F-001 entry 加阶段 1 依赖引用(类似 "Blocked by: `06-20-frontend-tool-call-card-shared-body-extract/`")
3. **User 决定**:
   - (A) 立即开始阶段 1 task(先 brainstorm 它,再走 PR1 硬前置)
   - (B) 暂缓阶段 1,先 close 当前 session,等下次启动再做
   - (C) 用户改了某个 D 决策 → 重新 brainstorm

---

## 关联

- **DEBT.md**:`.trellis/reviews/DEBT.md` §FT-F-001(open)
- **journal**:`.trellis/workspace/Carlos-home/journal-2.md` Session 49 "Next Steps"
- **B6 PR3b commit**:`186e500`
- **关键文件**:
  - `app/src/components/chat/SubagentDrawer.vue`(681 行,待重构)
  - `app/src/components/chat/ToolCallCard.vue`(995 行,阶段 1 抽 shared body)
  - `app/src/stores/subagentRuns.ts`(482 行,`TranscriptEntry` 类型源)
  - `app/src/stores/chat.ts`(`ToolCallInfo` / `ToolResultInfo` 类型)
  - `app/src/stores/permissions.ts`(`PermissionAsk` 形状参考)
- **依赖(待开)**:阶段 1 task `06-20-frontend-tool-call-card-shared-body-extract/`
- **同源 follow-up**:FT-F-002 / FT-F-003
