# 拆分 SubagentDrawer.vue — 抽 Header + ErrorCard

## Goal

把 `app/src/components/chat/SubagentDrawer.vue`(1257 行)拆为 3 个文件:

- `SubagentDrawer.vue` — 主编排器(orchestrator,~900 行)
- `SubagentDrawerHeader.vue` — header 子组件(~250 行)
- `SubagentDrawerErrorCard.vue` — R25 错误卡子组件(~100 行)

主抽屉继续负责 reka-ui Dialog 容器、ticker、scroll 编排、sections / thinking / tools / reply 5 段 group view。Header 与 ErrorCard 是纯展示组件(无 store 调用、无 ticker)。

## What I already know

### 文件现状

`SubagentDrawer.vue` 1257 行,结构(行号依 `Read` 结果):

| 区块 | 行号范围 | 说明 |
|---|---|---|
| `STATUS_META` | 163-182 | 状态枚举 → {label, color} 映射 |
| `statusDisplay` computed | 383-421 | 含 suffix(running/completed/error/cancelled/incomplete 5 分支) |
| `bannerText` computed | 430-447 | 失败/警告 banner 文本(error/warning/cancelled/incomplete) |
| `errorMessage` computed | 484-520 | R25 4 级 fallback:transcriptJson → finalText → summary → canned |
| `cancelledSuffix` computed | 533-537 | R23 ⊘ Cancelled · at X.Xs |
| `nowTick` ticker + `watch(openRunId)` | 327-328, 539-561 | 100ms cadence,header 计时 + body pairSections 共享 |
| Header 模板 | 678-745 | title row + banner + meta + summary + truncated(含 jump-latest) |
| Error card 模板 | 767-779 | v-if 守卫 `status === "error" && errorMessage !== null` |
| Body 5 段 + auto-follow | 748-889 | sections / prompt / thinking / tools / reply + autoFollow + newCount |
| Header scoped CSS | 950-1095 | status badge / title row / banner / close / jump-latest / new-events / meta / summary / truncated |
| Error card scoped CSS | 1188-1232 | error card + header + icon + title + message |

### Cross-cut 决策(已与用户确认 A 方案)

`jumpToLatest` 按钮原本在 `<header>` 区块,但 visible 条件 (`!autoFollow && sections.length > 0`) + click handler (`jumpToLatest()`) 都依赖 body 状态 (`autoFollow` / `newCount` / `bodyEl` / `onBodyScroll`)。**A 方案**:Header 精简,jump-latest 按钮 + autoFollow + newCount + onBodyScroll + bodyEl 全部下移到 body(`subagent-drawer__body` 顶部 sticky)。

→ Header 不需要从 main drawer 接 `autoFollow` / `sectionsCount` 任何 prop。Header 只读:status / run / statusDisplay / bannerText / truncated。

### Sibling 约定

`DrawerPromptCard.vue` 是参考:独立组件 + `defineProps` + scoped style。Header / ErrorCard 沿用同模式。

### 测试面

`SubagentDrawer.test.ts` 1225 行,挂载 `SubagentDrawer` 整体测试。Split 是内部重构,DOM 输出必须不变 → tests 不改。

## Requirements

### 行为不变(硬约束)

- 渲染 DOM 与重构前 1:1 等价(类名 / 文本 / 嵌套结构一致)
- Header / ErrorCard scoped CSS 选择器全部保留(`[data-v-*]` 由 Vue 3.5 自动注入,不需要手动加)
- reka-ui DialogClose 在 Header 内仍是原生 primitive(无需 emit,reka-ui 自管 close)
- `nowTick` ticker 仍由 main drawer 持有(`statusDisplay` 依赖 → Header 通过 prop 收 `statusDisplay` / `bannerText` 即可,不需要 ref 直传)

### 子组件接口

**SubagentDrawerHeader.vue** (`defineProps`):
```ts
{
  run: SubagentRun | undefined;             // 来自 store.openRun
  status: SubagentStatus;                   // 已 coerce
  statusDisplay: { label: string; color: string; suffix: string };
  bannerText: { kind: "error" | "warning"; text: string } | null;
  truncated: boolean;
}
```

**SubagentDrawerErrorCard.vue** (`defineProps`):
```ts
{
  errorMessage: string;
}
```

无 emit(纯展示)。

### 主 drawer 编排逻辑

- `nowTick` ticker / `watch(openRunId)` / `onUnmounted` 全部留在 main
- `STATUS_META` 留在 main(Header 通过 `statusDisplay` prop 拿渲染好的 text/color)
- `statusDisplay` / `bannerText` / `errorMessage` / `cancelledSuffix` 留在 main(它们的依赖 `run` / `status` / `terminalDurMs` / `elapsedMs` 都在 main scope)
- body 顶部新增 `<button v-if="showJumpLatest" ...>` (从 header 搬下来,加 sticky 定位 → 复用现有 `.subagent-drawer__new-events` 样式 + 新加 `.subagent-drawer__jump-latest` 复用)
- main drawer 同时 mount Header + ErrorCard:
  ```html
  <SubagentDrawerHeader
    :run="run"
    :status="status"
    :status-display="statusDisplay"
    :banner-text="bannerText"
    :truncated="truncated"
  />
  <SubagentDrawerErrorCard
    v-if="status === 'error' && errorMessage !== null"
    :error-message="errorMessage"
  />
  ```

### 行数目标

| 文件 | 当前 | 目标 |
|---|---|---|
| SubagentDrawer.vue | 1257 | ~900 |
| SubagentDrawerHeader.vue | — | ~250 |
| SubagentDrawerErrorCard.vue | — | ~100 |

## Acceptance Criteria

- [ ] `SubagentDrawer.vue` ≤ 950 行(允许 ±50 行抖动)
- [ ] 新建 `SubagentDrawerHeader.vue` 存在,header template + status badge + name + close + banner + meta + summary + truncated 全部齐全,**不含** jump-latest 按钮
- [ ] 新建 `SubagentDrawerErrorCard.vue` 存在,error card 模板 + scoped CSS 完整
- [ ] `SubagentDrawer.test.ts` 1225 行测试**零修改**通过(`pnpm vitest run components/chat/SubagentDrawer.test.ts`)
- [ ] `pnpm vue-tsc --noEmit` 通过(`app/` 下)
- [ ] `pnpm vitest run` 全量无新增失败
- [ ] 渲染 DOM 与重构前等价(visual snapshot / test snapshot 不变)
- [ ] 任何迁移到子组件的 scoped CSS 选择器(如 `.subagent-drawer__header`、`.subagent-drawer__status`、`.subagent-drawer__error-card`)在新文件中以同样前缀保留

## Definition of Done

- 三个文件存在并 import 关系正确
- `pnpm build` (`vue-tsc --noEmit` + `vite build`)绿
- `pnpm vitest run` 全绿,SubagentDrawer test 通过(0 修改)
- 手工 review:Header 在 `<DialogContent>` 内位置不变、ErrorCard 在 prompt 卡片下位置不变
- 不引入新依赖、不改后端 IPC、不改 store

## Decision (ADR-lite)

**Context**: SubagentDrawer.vue 1257 行,主编排 + Header 渲染 + Error card + 5 段 group view + scroll 编排 + ticker 全部塞在一个文件。test 1225 行已锁定 DOM 结构,任何 render 等价性破坏会立刻翻车。

**Decision**: 走 A 方案(Header 精简 + jump-latest 下移 body),把 jump-latest 按钮从 header 搬到 body 顶部 sticky。Header 只接 `status` / `run` / `statusDisplay` / `bannerText` / `truncated` 5 个 prop,无 cross-cut。

**Consequences**:
- (+) Header 与 main 完全解耦,只读 prop,未来 Header 改动不需碰 main
- (+) `autoFollow` / `newCount` / scroll 编排全部留在 main,与 body scroll 自然耦合,无 emit 上传
- (-) jump-latest 按钮从 header 顶部跑到 body 顶部 sticky,视觉位置微调;功能(↗ 跳到最新 + "↓ N new" 提示)等价
- (-) 用户已确认接受该视觉调整

## Out of Scope

- 不重构 body 5 段 group view(Thinking / Tools / Reply) — 那部分与 DrawerSection / DrawerThinkingBlock / DrawerToolCallCard / DrawerPermissionAskCard 4 个 sibling 强耦合,本次不碰
- 不动 scroll 编排(`autoFollow` / `newCount` / `jumpToLatest` 逻辑保持原样)
- 不修 DEBT.md RULE-FrontSubagent-001 / 002(`.tool-card` header CSS 重复 / `pairSections` 隐式状态) — 与本次 split 无关
- 不做跨组件 CSS 共享(ToolCallHeader 抽取) — 那是另一项 task
- 不改 `STATUS_META` / 状态文案 / 测试用例

## Technical Notes

### 文件位置

```
app/src/components/chat/
├── SubagentDrawer.vue                  (改:减 ~350 行)
├── SubagentDrawerHeader.vue            (新)
├── SubagentDrawerErrorCard.vue         (新)
└── SubagentDrawer.test.ts              (不动)
```

### 类型导入

Header 需要 `SubagentRun` 类型(从 `app/src/stores/subagentRuns.types` 取)。SubagentStatus 同源。

### CSS 迁移

- Header scoped CSS 块搬迁:`.subagent-drawer__header`、`.subagent-drawer__title-row`、`.subagent-drawer__status`、`.subagent-drawer__name`、`.subagent-drawer__close`、`.subagent-drawer__banner` + `__banner--error` / `__banner--warning`、`.subagent-drawer__banner-text`、`.subagent-drawer__meta`、`.subagent-drawer__meta-time`、`.subagent-drawer__summary`、`.subagent-drawer__truncated`
- 留下 main drawer 的:`.subagent-drawer__jump-latest`(搬到 body 后位置)、`.subagent-drawer__new-events`、`subagent-drawer__body` / `__empty` / `__segments` / `__reply-*`
- Error card scoped CSS 块搬迁:`.subagent-drawer__error-card` / `__error-header` / `__error-icon` / `__error-title` / `__error-message`

### 参考

- Sibling: `DrawerPromptCard.vue` 的独立组件 + defineProps + scoped style 模式
- DEBT.md RULE-FrontSubagent-001(已知 `.tool-card` header CSS 重复 — 不在本 task 范围)