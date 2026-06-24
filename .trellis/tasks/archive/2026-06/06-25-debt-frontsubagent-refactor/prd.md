# 前端重构清债 — FrontSubagent-001（tool-card header CSS 重复）+ FrontSubagent-002（pairing Map 隐式状态）

> **状态**：Plan phase（2026-06-25），research 完成，待 Final Confirmation → Phase 2。
>
> **来源**：`.trellis/reviews/DEBT.md` §RULE-FrontSubagent-001 / §RULE-FrontSubagent-002
>
> **前置约束已解除**：两条债都源自 B6 subagent-drawer redesign（`06-21-refactor-redesign-...`），redesign 时为"主 panel `ToolCallCard` 本体 0 改动"约束推迟。redesign PR1-6 现已全部落地，约束解除 —— `DrawerToolCallCard.vue:42-52` 头注释原文："The right time to extract is a follow-up ... redesign 收尾后可做"。

## Goal

消掉 DEBT.md 两条 P3 前端债：
- **FrontSubagent-001**：`.tool-card` header CSS 在 3 个组件 1:1 镜像（~150 行重复）
- **FrontSubagent-002**：`pairTranscript`/`pairSections` 第三参 `pendingFirstSeenAt` Map 既是输入又是输出，签名隐式，新调用方易踩坑

## 现状（research 2026-06-25）

### 001 — header CSS 三处 1:1 镜像

| 组件 | class 前缀 | CSS 行 | markup 行 | 来源 |
|---|---|---|---|---|
| `ToolCallCard.vue`（主 panel） | `.tool-card*` | 642-757 | 475-517 | 原始 source |
| `DrawerToolCallCard.vue`（drawer） | `.drawer-tool-card*` | 196-320 | 147-171 | PR4 镜像 |
| `DrawerPermissionAskCard.vue`（drawer） | `.drawer-permission-ask-card*` | 191-270 | 168-179 | PR6 镜像（+ interactive 变体） |

三者 header markup 共同骨架：`title(icon + name + [path|suffix])` + `status([icon] + text + [duration] + [extra])`。差异：
- **ToolCallCard**：path + status-icon + duration + **diff-btn**（header 内 `<button>`）
- **DrawerToolCallCard**：path + status-icon + duration，无 diff-btn
- **DrawerPermissionAskCard**：**suffix**（非 path），无 status-icon、无 duration，有 `--interactive` card 变体

error 颜色当前靠容器 `--error` class 的后代选择器（`.tool-card--error .tool-card__name`），抽组件后需改 prop 驱动。

### 002 — pairing Map 隐式状态

`pairTranscript(entries, now, pendingFirstSeenAt)` / `pairSections(sections, now, pendingFirstSeenAt)`：第三参 Map 被 `.set/.delete/.get` —— 既是输入又是输出。**调用方必须跨调用保持同一 Map 引用**，否则 30s pending timeout 永不推进（每次 `new Map()` → 永远 pending）。

生产调用方**仅 `SubagentDrawer.vue`**（`:192` module-level Map、`:203` computed、`:522` 切 run `.clear()`），已踩对。其余 30+ 处是测试。**当前无 bug，债在签名隐式 + 未来调用方**。

**load-bearing 约束**（`SubagentDrawer.vue:178-192` 注释）：Map **必须是 plain（非 reactive）** —— reactive Map 会让 `toolEntries` computed 在 `pairSections` 内部 `.set/.delete` 时触发自身依赖 → 递归 re-invalidation → 100ms nowTick × 大量 sections → 100× 递归 re-eval → **webview OOM 崩溃**（已踩过并修复的真实 bug）。任何方案必须保留 plain Map。

## 选定方案

### 002 — `useTranscriptPairing()` composable（DEBT 方向 a）

`transcriptPairing.ts` 新增 composable，闭包持有 plain Map，返回绑定好的 pair 函数 + reset：

```ts
export function useTranscriptPairing() {
  // plain Map — 非响应式，避免 computed 递归 re-invalidation（OOM bug，见 SubagentDrawer 注释）
  const pendingFirstSeenAt = new Map<string, number>();
  return {
    pairEntries: (entries, now) => pairTranscript(entries, now, pendingFirstSeenAt),
    pairSections: (sections, now) => pairSections(sections, now, pendingFirstSeenAt),
    reset: () => pendingFirstSeenAt.clear(),
  };
}
```

- **保留**纯函数 `pairTranscript`/`pairSections`（测试 30+ 处依赖 + 未来 raw-list consumer），composable 是薄包装
- `SubagentDrawer.vue` 改造：删 `:192` Map、`:203` computed 改调 `pairToolSections(sections.value, nowTick.value)`、`:522` `.clear()` 改 `reset()`
- composable 注释继承 plain-Map 约束（防 OOM 回归）

**否决备选**（DEBT 方向 b，Map 移 module-level 单例）：破坏测试隔离（现每 test `new Map()` 独立实例）；多 run 并发时单例串扰风险。composable 实例隔离更安全。

### 001 — `<ToolCallHeader>` 共享组件（DEBT 方向 a）

新建 `components/chat/ToolCallHeader.vue`：
- props：`iconName` / `name` / `filePath?` / `suffix?` / `statusText` / `statusIconName?` / `durationLabel?` / `isError?` / `isRunning?`
- `#status-extra` slot（ToolCallCard 的 diff-btn 走这里）
- 内置全部 `.tool-card*` header CSS（单一来源）；error/running 颜色改 `isError`/`isRunning` prop 驱动

三个调用方改造：
- **ToolCallCard**：header markup 替换为 `<ToolCallHeader>` + diff-btn 走 slot；删 header CSS（保留 `.tool-card` 容器 + `--error/--running/--subagent` 容器变体 + `__approval/__diff/__diff-btn` + dispatch preview 等非 header 规则）
- **DrawerToolCallCard**：header markup 替换为 `<ToolCallHeader>`（无 slot）；删 header CSS（保留 `.drawer-tool-card` 容器 + `--error/--running`）
- **DrawerPermissionAskCard**：header markup 替换为 `<ToolCallHeader :suffix="'权限询问'">`（无 icon/duration）；删 header CSS（保留 `.drawer-permission-ask-card` 容器 + `--interactive` 变体）

**slot scoped CSS 确认**：diff-btn 是 ToolCallCard template 内定义的 slot 内容，带 ToolCallCard scope id → ToolCallCard 的 scoped `.tool-card__diff-btn` 仍命中，无需迁移。

**否决备选**（DEBT 方向 b，抽 `style.css` 全局工具类）：全局 class 跨组件泄漏风险；项目已有 `ToolInputBody`/`ToolOutputBody` 共享子组件先例（FT-F-001），组件封装更一致。

## PR 划分

- **PR1 = 002**（低风险先落地）：composable + SubagentDrawer 改造 + composable 测试。纯封装，行为零变化。
- **PR2 = 001**（中风险需视觉验证）：ToolCallHeader 组件 + 3 调用方改造 + CSS 迁移。

两条相互独立，可只做其中一条（建议至少做 002，风险低收益清晰）。

## Acceptance Criteria

- [ ] 002：`useTranscriptPairing()` composable 落地 + 单测（共享 Map → timeout 跨调用推进；`reset()` 清空）
- [ ] 002：`pairTranscript`/`pairSections` 纯函数保留，现有 30+ 测试不动
- [ ] 002：`SubagentDrawer.vue` 改用 composable，pending timeout / clear 语义零变化
- [ ] 001：`ToolCallHeader.vue` 新建，3 个调用方改造完成
- [ ] 001：header CSS 单一来源（3 处重复 header CSS 删除）
- [ ] `pnpm vitest run` 全绿（518 现有 + 新增 composable 测试）
- [ ] `vue-tsc --noEmit` 通过
- [ ] 视觉零回归（手动对照 tool card error/diff-btn、drawer tool card、permission ask interactive 三态 header）
- [ ] DEBT.md：两条 finding 删除（闭合后 git log 追溯），优先级分布表 4→2

## Definition of Done

- PR1（002）+ PR2（001）各自独立 commit
- 全测试绿 + 类型通过 + 视觉零回归
- DEBT.md 4→2 open（剩 B-007 / C-008 决策类）
- journal 记录 + task archive

## 风险

- **001 视觉回归**：ToolCallCard 886 行老组件，header error 颜色靠容器 `--error` 后代选择器，抽组件后改 prop 驱动 —— 需仔细对照。缓解：`ToolCallCard.test.ts` 锁 class + 手动视觉对照 error/diff/permission 三态。
- **002 plain Map 约束**：composable 内部必须 plain Map，注释防 OOM 回归。缓解：注释继承 + composable 单测。
