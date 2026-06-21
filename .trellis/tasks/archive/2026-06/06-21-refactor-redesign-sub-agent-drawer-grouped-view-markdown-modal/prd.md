# refactor: redesign sub-agent drawer (grouped view + markdown modal)

## Goal

把 SubagentDrawer 从"时间序平铺 + 无意义 chat_event 流暴露"重构为"分组折叠视图 + Markdown 详情 modal",与主 chat panel 视觉一致,同时支持主 LLM 拉起 sub-agent 的提示词可见、最终回复可放大查看。

**Why**:当前 SubagentDrawer 暴露 6963 个 SSE 字节流级 chat_event 给用户(无意义)、不显示主 LLM 给 worker 的提示词、不分段(Thinking/Tools/Reply)、不提供 markdown 详情入口 — 与主 chat panel 设计规范严重不一致。

## Background(用户痛点 — 来自截图)

1. "Show chat events 98 events · +6963 chat hidden" 整段 UI 暴露 SSE chunk 流,主 panel 不暴露,sub-agent 暴露 = 设计不一致
2. 没有显示主 LLM 拉起 sub-agent 的提示词(`dispatch_subagent.task` 字段),只有完成态 `completed` 标签
3. 没有"思考"分段和 text 回复分段的视觉分组,与主 panel 的 `[Thought]/[Text]/[Tool]` 分段规范不一致
4. 最终回复"我来逐步执行这个任务..."被压在 `dispatch_subagent` 卡片里,没有独立"最终回复"段,也没有"查看详情 modal"入口

## Requirements

### 数据层(Q1 + Q7)

- **R1**: `subagent_runs` 表加 `task` 列(string,nullable)— 主 LLM 拉起 sub-agent 的完整指令
- **R2**: `subagent_runs` 表加 `final_text` 列(string,nullable)— worker 退出时的纯文本最终回复(去掉 `[status: ...]\n` 前缀)
- **R3**: `transcriptJson` 字段不动,仍存原始 SSE event 流(含 chat_event)
- **R4**: 后端 `dispatch_subagent` 写 task 列;worker exit hook 写 final_text 列
- **R5**: 前端 `SubagentRunSummary` / `SubagentRunRow` TS 类型同步加 `task: string | null` / `finalText: string | null`

### 渲染策略(Q2)

- **R6**: Drawer 共享主 panel 视觉原语 `ThinkingBlock` / `ToolCallCard` / `renderMarkdown()`,**不**整体复用 `MessageItem`
- **R7**: Drawer 视觉子组件(`DrawerThinkingBlock` / `DrawerToolCallCard`)只接 props,不耦合 `ChatMessage` 数据结构

### Chat_event 处理(Q3)

- **R8**: 前端 store 加 `runAccumulator`,订阅 chat_event 时聚合成 `Thinking` / `Text` / `FinalText` 3 个新 `TranscriptEntryKind`
- **R9**: 删除 `showChatEvents` toggle / `hiddenChatCount` 计算属性 / chat_event 渲染分支
- **R10**: 后端 `chat_event` emit 不动,继续写入 `transcriptJson` 保持完整时序

### Modal 封装(Q4)

- **R11**: 新建通用 `MarkdownDetailModal.vue`(`reka-ui DialogRoot`,props: `{ open, title, markdown, source? }`)
- **R12**: 新建 `useTruncate(text, n)` composable(纯字符串截断,不破坏 markdown)
- **R13**: Drawer 默认截断:`finalText` 280 字符 / `task` 120 字符,各带 "View full →" 触发 modal

### Drawer 顶层布局(Q5)

- **R14**: Drawer 改为分组折叠视图:`header` / `prompt` / `thinking` / `tools` / `reply` 5 段
- **R15**: 每段 chip 显示 summary(类型图标 + 段名 + 数字 + 耗时);展开后内部时间序平铺 entry
- **R16**: 默认折叠 `Thinking`,默认展开 `Tools` + `Reply`

### Live streaming 视觉(Q6)

- **R17**: 每段右上角显示 live 指示器(spinner + 数字 + 耗时)
- **R18**: 无 entry 级 cursor(避免视觉碎);光标粒度统一在段级
- **R19**: `subagent:finished` 触发后全段 live 指示器消失,转 `✓ Completed · 118.6s`

### Accumulator 性能(Q8)

- **R20**: Accumulator 累加式实现 — 每个新 event 只更新最后 segment 字段(O(1)),不重建数组
- **R21**: 原始 events 数组用 `markRaw()` 包裹,跳过 Vue 3 reactivity proxy
- **R22**: Live 阶段不跑全量 accumulator(只累加);worker finished 时 fetch 拿权威 `transcriptJson`,跑一次全量 accumulator 替换内存 transcript

### 边界态(Q9)

- **R23**: `cancelled` 终态 — `FinalText` 段替换为 `⊘ Cancelled · at turn N` chip
- **R24**: `permission_ask` 中间态 — Tools 段展开时 PendingAsk ToolCallCard 顶部 Allow/Deny 按钮 + 监听 `permission:response` IPC 回填
- **R25**: `error` 终态 — 顶部 prompt 卡片下方插入 ❌ 卡片,error message 从 transcriptJson 末位 error event 提取

## Acceptance Criteria

- [ ] DB migration 加 `task` + `final_text` 列成功,新写入 sub-agent run 在 DB 里能看到两列
- [ ] 后端 dispatch_subagent 调用时把 task 字段写入 `subagent_runs.task`
- [ ] 后端 worker exit hook 时把 `[status: ...]\n` 前缀剥掉后的纯文本写入 `subagent_runs.final_text`
- [ ] Drawer 顶部 prompt 卡片显示主 LLM 拉起 worker 的提示词(120 字符截断 + "View full →")
- [ ] Drawer 不再有 "Show chat events" toggle / "+N chat hidden" 字样
- [ ] Drawer 按 5 段分组:header / prompt / thinking / tools / reply,每段 chip 显示 summary
- [ ] Drawer Thinking 段默认折叠,Tools + Reply 默认展开;展开后内部 entry 按时间序平铺
- [ ] FinalText 在 Reply 段显示,默认 280 字符 + "View full →" 弹 modal
- [ ] Worker 跑中时每段右上角显示 live spinner + 数字;finished 后转 `✓ Completed`
- [ ] cancelled / error / permission_ask 三种边界态都能正确渲染
- [ ] 20000 events transcript 冷启动 <500ms(实测:JSON.parse + 全量 accumulator + Vue render)
- [ ] Live 阶段每 200ms debounce 更新一次,GC 压力 0(累加式 + markRaw)
- [ ] 不破坏主 panel `MessageItem` / `ToolCallCard` 现有功能(主路径 0 改动)
- [ ] 单元测试覆盖:accumulator 累加逻辑 / markRaw 不被 reactive / useTruncate 边界 / MarkdownDetailModal open/close
- [ ] 前端 `vue-tsc --noEmit` + `vitest run` 全绿;后端 `cargo test` 全绿
- [ ] 文档更新:`.trellis/spec/frontend/chat.md` 加入 SubagentDrawer 章节;ROADMAP.md 更新;DEBT.md 回填 grill-me 决策

## Definition of Done

- DB migration 文件写好并测试过(新机器 + 已有 DB 都能正常 upgrade)
- 前端 store / drawer / modal / composable 全部新增完成
- 后端 dispatch + worker exit hook 改动完成
- 单元测试覆盖核心逻辑(accumulator / truncate / modal)
- `vue-tsc --noEmit` + `vitest run` + `cargo test` 全绿
- `pnpm tauri build` 产物 OK,手动 smoke test 一次 happy path + 一次 cancelled + 一次 error
- `.trellis/spec/frontend/chat.md` 加入 SubagentDrawer 章节,说明 5 段布局 + 数据流 + Modal 复用
- ROADMAP.md 第二档 5 项剩余里如果有匹配的本任务,标记 ✅
- DEBT.md 回填 grill-me 决策记录(Q1-Q10 关键结论 + 链接 prd.md)
- 任务 archive 到 `.trellis/tasks/archive/2026-06/06-21-refactor-redesign-sub-agent-drawer-grouped-view-markdown-modal/`
- Journal entry 写入 session 41+ 总结本次重构

## Technical Approach

### 数据流

```
主 LLM dispatch_subagent(task="...")
   ↓ (后端 dispatch hook 写)
subagent_runs.task = "..."
   ↓
Worker 启动 → emit subagent:event { kind: chat_event | tool_call | tool_result | permission_ask, payload }
   ↓ (前端 subagentRuns.start() 监听,200ms debounce)
runAccumulator 累加式更新 transcript ref
   ↓
Drawer transcript computed → 5 段折叠 + 视觉子组件渲染
   ↓ (worker finished)
fetchRun → DB.subagent_runs + transcriptJson
runAccumulator 全量替换 transcript 为权威版本
final_text → Reply 段
```

### 关键实现细节

#### Accumulator 接口草案

```ts
// subagentRuns.ts
type SegmentState = {
  kind: 'Thinking' | 'Text';
  chars: number;
  /** 累加式 update — mutate 字段,不重建对象 */
  append(event: ChatEventPayload): void;
};

class RunAccumulator {
  private rawEvents = shallowRef<ChatEventPayload[]>(markRaw([]));
  private thinkingSegment: SegmentState | null = null;
  private textSegment: SegmentState | null = null;
  private transcript = ref<TranscriptEntry[]>([]);

  /** live 阶段:累加式,O(1) per event */
  feed(event: ChatEventPayload): void {
    this.rawEvents.value = markRaw([...this.rawEvents.value, event]);
    this.routeToSegment(event);
  }

  /** worker finished:全量重跑,fetch 后调用 */
  rebuildFromCache(transcriptJson: string, finalText: string | null): void {
    const raw = markRaw(JSON.parse(transcriptJson));
    this.rawEvents.value = raw;
    this.thinkingSegment = null;
    this.textSegment = null;
    this.transcript.value = this.buildTranscriptFromRaw(raw, finalText);
  }
}
```

#### Drawer 5 段组件结构

```
<SubagentDrawer>
  <DrawerHeader />              <!-- status badge + name + duration -->
  <DrawerPromptCard task={...} /> <!-- 顶部 prompt,120 截断 + View full -->
  <DrawerSection type="thinking" :entries :live>
    <DrawerThinkingBlock v-for="t in entries" :blocks />
  </DrawerSection>
  <DrawerSection type="tools" :entries :live>
    <DrawerToolCallCard v-for="tc in entries" :call :result />
  </DrawerSection>
  <DrawerSection type="reply" :text :live>
    <DrawerMarkdownBody :text + "View full →" />
  </DrawerSection>
</SubagentDrawer>

<MarkdownDetailModal v-model:open :title :markdown :source />
```

## Decision (ADR-lite)

### Decision 1:Drawer 独立数据层,共享视觉原语

**Context**:主 `MessageItem` 渲染 `ChatMessage`,drawer 渲染 `TranscriptEntry[]`,数据结构完全不同。如果整体复用 MessageItem 要写大量 adapter mock ChatMessage 字段(id/role/latency/streaming 等),会脏。

**Decision**:共享 `ThinkingBlock` / `ToolCallCard` 视觉子组件 + `renderMarkdown()` 工具函数,但 drawer 维护自己的 `TranscriptEntry[] → 5 段 sections → 视觉组件` 渲染路径。

**Consequences**:Drawer 与主 panel 数据层独立,改动不互相影响;视觉一致性靠子组件复用保证。Drawer 内部多段折叠 / 截断 / live 指示器是 drawer 独有需求,不会被主 panel 拖累。

### Decision 2:transcriptJson 存原始 event,前端聚合

**Context**:Drawer 需要显示 Thinking / Text 分段,但后端只有 chat_event SSE chunk 流。后端聚合需要后端写一份 accumulator 逻辑,与前端重复;前端聚合简单但 20000 events 时性能压力大。

**Decision**:transcriptJson 存原始 event 流(后端零改动),前端 `runAccumulator` 用累加式 + `markRaw` + live 不跑全量,实测 20000 events 冷启动 <500ms。

**Consequences**:前端 accumulator 实现需小心性能(markRaw + 累加 + 不重建数组);后端零改动避免 wire shape 风险。未来如果 transcriptJson 超过 100k events,记 DEBT 改为后端聚合。

### Decision 3:Drawer 折叠段,默认展开 Tools + Reply

**Context**:Worker 一个 turn 可能 5-10 个 tool,全部时间序平铺 drawer 滚动条爆掉;但完全折叠用户看不到任何内容。

**Decision**:分组折叠 5 段,默认展开 Tools + Reply(用户主要看的内容),折叠 Thinking(节省空间,按需展开)。

**Consequences**:Drawer 首屏视觉清爽;Thinking 内容可按需展开;折叠状态不持久化,重开 drawer 恢复默认(简单)。

## Implementation Plan(按依赖排序,6 个 PR)

### PR1:DB + 后端 hook(1 PR,~150 lines Rust + migration)

- 新建 migration `m_2026_06_21_add_subagent_runs_task_final_text.sql`
- `SubagentRunRow` / `SubagentRunSummary` 加 `task` / `finalText` 字段(`#[serde(rename_all = "camelCase")]`)
- `dispatch_subagent` 调用时(`run_subagent` 入口)把 input.task 写入 `subagent_runs.task`
- `worker exit hook` 把 `worker_sink.final_text()` 去掉 `[status: ...]\n` 前缀后写入 `subagent_runs.final_text`
- 单元测试:dispatch → DB 有 task;worker completed → DB 有 final_text;cancelled → DB 有 final_text + cancelled 状态
- **依赖**:无

### PR2:前端 store accumulator + 删除 chat_event 暴露(1 PR,~200 lines TS)

- `subagentRuns.ts` 加 `RunAccumulator` 类 + `thinkingSegment` / `textSegment` 累加式 state
- `TranscriptEntry` 加 `kind: "Thinking" | "Text" | "FinalText"`(3 个新 kind)
- `routeEvent` 内:chat_event 不直接 append,喂给 accumulator segment
- transcriptJson fetch 后调 `rebuildFromCache` 全量重跑
- 单元测试:累加逻辑 / markRaw 不被 reactive / 20000 events <500ms
- **依赖**:无(可与 PR1 并行)

### PR3:`MarkdownDetailModal` + `useTruncate`(1 PR,~120 lines Vue + TS)

- 新建 `app/src/components/common/MarkdownDetailModal.vue`(reka-ui DialogRoot,仿 `MemoryModal.vue`)
- 新建 `app/src/composables/useTruncate.ts`(纯字符串截断,不破坏 markdown)
- 单元测试:open/close 状态 / 边界字符(空 / 超长 / 含代码块)
- **依赖**:无

### PR4:Drawer 视觉子组件封装(1 PR,~200 lines Vue)

- 新建 `app/src/components/chat/DrawerThinkingBlock.vue`(props: `blocks: ThinkingBlock[]`)
- 新建 `app/src/components/chat/DrawerToolCallCard.vue`(props: `call: ToolCall`, `result: ToolResult | null`)
- 内部直接复用主 panel 的 `ThinkingBlock` / `ToolCallCard` 视觉 + props,不重写视觉
- 单元测试:props 渲染
- **依赖**:无

### PR5:SubagentDrawer 分组折叠重写(1 PR,~400 lines Vue)

- `SubagentDrawer.vue` 改为 5 段分组折叠布局
- header / prompt / thinking / tools / reply 各为 `DrawerSection` 子组件
- 默认折叠 Thinking,展开 Tools + Reply
- live 指示器:每段右上角 spinner + 数字
- 替换 `transcript computed` 为 `accumulator.transcript`
- 手动 smoke test:happy path + cancelled + error + 20000 events 冷启动
- **依赖**:PR2 / PR3 / PR4

### PR6:边界态(cancelled / permission_ask / error)+ 文档(1 PR,~150 lines Vue + docs)

- `cancelled` 终态:Reply 段替换为 `⊘ Cancelled · at turn N` chip
- `permission_ask` 中间态:Tools 段展开时 PendingAsk ToolCallCard 顶部 Allow/Deny 按钮 + `permission:response` 监听
- `error` 终态:prompt 卡片下方 ❌ 卡片 + error message 提取逻辑(末位 error event grep)
- `.trellis/spec/frontend/chat.md` 加 SubagentDrawer 章节(5 段布局 + 数据流 + Modal 复用 + 边界态)
- ROADMAP.md / DEBT.md 更新
- **依赖**:PR5

## Risks

| Risk | 严重度 | 缓解 |
|---|---|---|
| **Q8 accumulator 性能实测不达标**(20000 events >500ms) | 高 | PR2 强制 benchmark(20000 events 冷启动 <500ms 写进 AC);不达标回退到后端聚合(DEBT 记) |
| **Q9 permission_ask 交互兼容性**(现有 permission 系统是否兼容 dispatcher 上下文) | 中 | PR6 实施前 review `app/src-tauri/src/permissions/*`;若不兼容,scope 缩小到只显示 PendingAsk 状态,Allow/Deny 按钮留 TODO |
| **主 panel `ToolCallCard` 改造连带影响**(R6 抽视觉子组件时改动) | 中 | PR4 只抽不改动主 panel `ToolCallCard` 本体;通过 props 传入保证主路径 0 改动 |
| **transcriptJson 增长导致 IPC 传输慢**(20000 events = 4MB JSON) | 中 | PR2 加 IPC 传输 benchmark;若慢,后端按 turn 切 chunk + lazy load(DEBT) |
| **drawer 在 worker 启动前的瞬态**(fetchRun 还在飞,transcript 为空) | 低 | PR5 已有 "Worker is starting..." empty state;保留 |
| **6 个 PR 之间的 feature branch 冲突**(store + drawer 同步改) | 中 | PR1/PR2/PR3/PR4 各自独立可并行;PR5 串行接 PR2/3/4;PR6 串行接 PR5 |
| **DB migration 在已有 DB 上失败** | 低 | PR1 写 ALTER TABLE IF NOT EXISTS 兼容模式;测试新机器 + 已有 DB 两条路径 |

## Out of Scope(明确不做 — Q10)

1. **后端聚合 transcriptJson**(Q7-C 不做)— YAGNI,前端累加式够用
2. **transcriptJson 切分存储** — 仅 100k+ events 才需要,记 DEBT
3. **多 drawer / tab 并发** — 单例 drawer(主 panel ToolCallCard 切回历史 sub-agent)
4. **Drawer 折叠状态持久化** — 重开 drawer 默认重置(简单)
5. **Auto-open on dispatch** — 仅点击 ToolCallCard trigger 才打开,不打扰用户
6. **ThinkingBlock modal 化** — 不在 Q4 范围,留给后续
7. **Drawer 内 sub-agent 历史操作**(复制 transcript / 导出 JSON)— 完全 YAGNI
8. **PermissionAsk 历史回放**(已完成或拒绝的 PermissionAsk 不再可交互)— 当前 store 已支持,留作既有功能

## Technical Notes

### 关键文件

- 后端 dispatch + transcript: `app/src-tauri/src/agent/subagent.rs`
- 后端 DB migrations: `app/src-tauri/src/db/migrations.rs`
- 后端 DB types: `app/src-tauri/src/db/types.rs` / `app/src-tauri/src/db/models.rs`
- 前端 SubagentDrawer: `app/src/components/chat/SubagentDrawer.vue`(待重写,~400 lines)
- 前端 store: `app/src/stores/subagentRuns.ts`(待加 accumulator)
- 前端 types: `app/src/stores/subagentRuns.ts` line 81-94(`SubagentRunSummary` / `SubagentRunRow`)
- 通用 modal 参考: `app/src/components/memory/MemoryModal.vue`(reka-ui Dialog 模式)
- 主 panel ThinkingBlock / ToolCallCard: `app/src/components/chat/MessageItem.vue`(视觉原语复用源)

### 现有约束

- reka-ui 版本 pin: `2.9.9`,无 `Sheet` 原语,继续用 `Dialog*` 组合 drawer
- vue-tsc 严格模式,所有新增 TS 类型必须通过 `--noEmit`
- vitest 覆盖 `app/src/**/*.test.ts`,新增 `app/src/components/chat/Drawer*.test.ts` / `app/src/composables/useTruncate.test.ts` / `app/src/stores/subagentRuns.test.ts`
- 后端 `#[serde(rename_all = "camelCase")]`,SQLite 列名 snake_case(`task` / `final_text`)
- transcriptJson IPC 已有 wire shape contract: `.trellis/tasks/06-20-b6-pr3-frontend-expand/wire-shape-contract.md`

### 既有 sub-agent 历史(不破坏)

- 当前 B6 PR3 drawer(B6 PR3 已合并 main)保留作为本任务参考 baseline
- 主 panel `MessageItem` / `ToolCallCard` 路径完全不动
- 现有测试 `app/src/utils/transcriptPairing.test.ts` 保留

### 决策参考

- Grill-me session 40:Q1-Q10 决策记录在 journal-2.md(待 PR6 完成后回填)
- 截图证据:`/home/carlos/.claude/image-cache/3e77c3c1-ff8b-4ea5-b9e2-6e9ed6fdce0f/2.png`

### 后续参考

- 主 panel MessageItem 渲染规范:`app/src/components/chat/MessageItem.vue` line 425-550(Thinking → Tools → Bubble 顺序)
- reka-ui Dialog 使用:`app/src/components/memory/MemoryModal.vue` line 21-59(DialogRoot + Portal + Overlay + Content)
- Pinia store 模式:`app/src/stores/subagentRuns.ts`(defineStore + reactive + computed)