# PRD: Memory 入口重构 — AppHeader corner action + reka-ui Dialog modal

**Task ID**: `06-11-memory-modal-appheader-entry`
**创建**: 2026-06-11
**前置**: `06-10-b5-memory-user-project-2layer`(已 archive)
**优先级**: P1(B5 follow-up — 已知 UX bug)

---

## 1. 背景

B5(Memory User+Project 2 层)已交付,但前端 Memory dropdown 存在严重布局 bug:

- **现象**:`ProjectTabs.vue` 上的 Memory 触发按钮在所有项目 tab 的右侧、`+` 按钮的左侧。点击后 popover 用 `position: absolute; right: 0; min-width: 480px;` 锚定到触发按钮右边,向左展开 480–600px。当 Memory 触发按钮不在窗口最右端(例如 3 个项目 tab 时它在窗口中部),popover 向左展开会**溢出视窗左边界**,文字被屏幕边裁掉,且与 sidebar 视觉重叠。
- **截图证据**:用户提供的截图中,Memory 触发按钮在 tab 区第 4 位,popover 横向扩展后左侧约 480px 内容被切到视窗左侧外,看到的是"emory"/"ject CLAUDE.md"等被裁字串。
- **根因**:hand-rolled popover 无 viewport collision detection;`right: 0` 的锚点策略只在 Memory 触发按钮处于窗口最右端时安全。

此外有 3 个语义/UX 问题:

1. Memory 触发按钮长得像 tab(带 chevron),容易被误以为是"Memory 项目"。它不是项目,是"对当前项目 memory 的查看入口"。
2. popover 容器密度过高(header + chip + 刷新 + 4 张 layer 卡片 + footer 说明),在 480–600px 宽的小弹层里挤。
3. ProjectTabs 上和 Settings 上的 Memory 视图功能重叠,语义上 dropdown 应该轻量,Settings tab 应该是"管理台"——但目前两个长得几乎一样。

---

## 2. 目标

> **📍 2026-06-11 Pivot**:本 PRD 在 brainstorm 阶段定的位置是 "AppHeader corner action",实施过程中(实际打开 dev 看效果时)用户当面要求改放到 **ChatPanel header**(WorktreeChip 右侧)。最终实施按 ChatPanel header 走;本 §2 + §4 Step 4/5 的 "AppHeader" 字样作为 brainstorm 历史快照保留,**最终决策与 rationale 见 `.trellis/spec/frontend/memory-ui.md` 的 "Decision: Memory entry 改为 ChatPanel header Brain 按钮 + reka-ui Dialog modal (2026-06-11)"**。task 目录名 `06-11-memory-modal-appheader-entry` 是 pivot 前命名,出于 trellis 流程不可变性保留。

把 Memory 入口从 ProjectTabs 上的 hand-rolled popover **迁移到 AppHeader 的 corner action button + reka-ui Dialog modal**(brainstorm 决策;实际实施挪到 ChatPanel header,见上方 Pivot)。

**MVP 范围**:

- ✅ 新增 AppHeader corner action(纯 Brain 图标按钮,点击打开 modal)
- ✅ 新建 MemoryModal 组件(reka-ui `DialogRoot/DialogContent/DialogOverlay/DialogClose`),内嵌 `<MemoryPreview kind="project" />`
- ✅ 移除 ProjectTabs 上的 Memory dropdown(trigger + popover + 相关 state)
- ✅ Icon.vue 改造支持混用 heroicons + lucide-vue-next(为获取真正的 Brain 图标)
- ✅ 更新 `memory-ui.md` spec:旧 popover 决策标 obsoleted,加新决策

**不在范围(明确不做)**:

- ❌ Settings 里的 Memory Tab(留作下一轮"Memory 功能重构"的工作)
- ❌ MemoryPreview / MemoryLayerItem 内部布局(全部保留)
- ❌ Backend memory 加载逻辑、IPC、watcher(完全不动)
- ❌ User 层 modal(本期只取代 popover 的"项目层"职责)

---

## 3. 决策汇总(全部已锁定)

| # | 维度 | 决策 | 理由 |
|---|------|------|------|
| D1 | Modal 内容 | 仅 Project 层(`<MemoryPreview kind="project">`) | 取代 popover 当前职责,不引入新表达 |
| D2 | 触发位置 | AppHeader 右侧 corner action | 不再混在 tab 列里,语义清晰 |
| D3 | Settings Memory Tab | 不动(下一轮重构) | 用户明确表示稍后另做功能性重构 |
| D4 | Modal 底层 | reka-ui `Dialog*` 原语 | 与 SettingsModal 一致;reka-ui-usage.md 鼓励;自带 portal + focus trap + a11y |
| D5 | AppHeader 按钮形态 | 纯图标(无文字) | 与已有 corner action 风格统一,header 空间紧 |
| D6 | 图标选择 | lucide-vue-next 的 `Brain` 图标 | heroicons 无 brain;CpuChip/BookOpen 都不够精准 |
| D7 | Modal 尺寸 | min-width 640px / max-width 900px / max-height 80vh / 内部滚动 | 可伸缩适应窗口,CLAUDE.md 4-5K tokens 时不全屏幕但能舒适浏览 |
| D8 | MemoryPreview 内部 | header / chip / 刷新 / footer **全部保留** | 用户明确"全部保留" |

---

## 4. 实施清单

按 5 个 step 分,与 Claude Code TaskList 一一对应:

### Step 1 — Brainstorm + PRD(本 doc,Task #1 ✅)

### Step 2 — 移除 ProjectTabs 的 Memory dropdown(Task #2)

- 删除 `ProjectTabs.vue` 里:
  - `memoryMenuOpen` / `memoryMenuRoot` ref
  - `toggleMemoryMenu` / `onMemoryDocumentClick` / `onMemoryKeydown` 函数
  - `document.addEventListener` / `onUnmounted` 监听
  - `watch(memoryMenuOpen, ...)` 加载触发
  - template 里 `<div class="tabs__memory">...</div>` 整块
  - `<style scoped>` 里 `.tabs__memory*` 整段(`.tabs__memory` / `.tabs__memory-trigger*` / `.tabs__memory-popover*` / `@keyframes memory-popover-slide`)
- 保留:
  - `useMemoryStore` import 与初始化(modal 会用,但走 AppHeader 触发后调用 `loadForProject`)
  - `onTabClick` 里 `memoryMenuOpen.value = false` 行可移除(state 已删)
- 影响:`<MemoryPreview>` 仍被 `MemoryTab.vue`(Settings)使用,不动

### Step 3 — Icon.vue 支持 lucide + 加 Brain(Task #4 的前置)

- `cd app && pnpm add lucide-vue-next`(latest 稳定版)
- `Icon.vue` 修改:
  - 顶部 import:`import { Brain } from "lucide-vue-next";`(heroicons import 不变)
  - `map` 加 `"brain": Brain`
- **兼容性验证**:lucide 图标默认 `<svg width="24" height="24">`,Icon.vue 现有 `:deep(svg) { width: 100%; height: 100%; }` 已经覆盖。确认 build 通过即可。

### Step 4 — AppHeader 加 Memory corner action(Task #3)

- `AppHeader.vue` 改造:
  - 现在结构:`<header><TitleBar><ProjectTabs/></TitleBar></header>`
  - 改为:`<header><TitleBar><ProjectTabs/><div class="app-header__actions"><MemoryButton/></div></TitleBar></header>`
  - 或:直接放 trigger 按钮(`<button @click="openModal">Brain icon</button>`) + MemoryModal,以 v-model:open 状态联动
- 推荐方案:在 AppHeader 内拆出小子组件 `MemoryEntryButton.vue`(20-30 行),内含:
  - 一个 `<button>` (Brain icon 图标)
  - 触发后 `dialogOpen.value = true`
  - 内嵌 `<MemoryModal v-model:open="dialogOpen" />`
- 显示规则:仅 `useProjectsStore().currentProjectId` 存在时按钮可见(与 popover 时机一致)
- 位置:TitleBar slot 的最右侧,在 window controls(`min/max/close`)之前
- 视觉 token:与现有 hover 模式一致(`background: var(--color-bg-elevated)` on hover)

### Step 5 — 新建 MemoryModal 组件(Task #4)

- 新建 `app/src/components/memory/MemoryModal.vue`
- 用 reka-ui Dialog 五件套:`DialogRoot` / `DialogPortal` / `DialogOverlay` / `DialogContent` / `DialogClose`
- props:`open: boolean`(v-model 双向);emits:`update:open`
- 内嵌:`<MemoryPreview kind="project" :project-id="projectsStore.currentProjectId" />`
- 尺寸 CSS:
  ```css
  :deep(.memory-modal__content) {
    min-width: 640px;
    max-width: 900px;
    width: 80vw;
    max-height: 80vh;
    /* 内部滚动让 MemoryPreview 自管 */
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  ```
- 动画:沿用 popover-pattern.md §Animation 的 modal fade+scale(150ms enter / 100ms leave),通过 `[data-state="open|closed"]` 触发
- Portal 内 CSS **必须** 用 `:deep()`(reka-ui-usage.md gotcha)
- 关闭交互:reka-ui Dialog 默认支持 ESC + 点遮罩 + DialogClose 按钮。无需手动接入。

### Step 6 — 更新 memory-ui.md spec(Task #6)

- 找到 `.trellis/spec/frontend/memory-ui.md` 里"Design Decision: Memory dropdown 走 hand-rolled popover":
  - 标 `OBSOLETED 2026-06-11`
  - 注明被新决策替代
- 加新 Design Decision: "Memory entry 改为 AppHeader corner action + reka-ui Dialog modal"
  - Context: B5 dropdown 横向溢出 bug;语义混乱
  - Decision: AppHeader corner action + Dialog modal
  - Consequences:位置安全(reka-ui portal 自适应);视觉与 SettingsModal 统一;一处入口扩展性更好
- Anti-Patterns 章 "Don't 用 reka-ui Popover/DropdownMenu" 保留(那是 popover 约束),加注:**Modal 走 reka-ui Dialog 是合规的**
- 更新 Related 段加本任务路径

### Step 7 — 前后端编译 + trellis-check(Task #5)

- `cd app && pnpm build`(vue-tsc --noEmit + vite build)
- `cd app/src-tauri && PKG_CONFIG_PATH=... cargo check`(应无影响,但保险起见)
- 手动 `pnpm tauri dev` 在不同窗口宽度(640px / 1024px / 1920px)下验证 modal 居中、不溢出
- 跑 trellis-check 验 spec compliance / lint

---

## 5. 验收

| # | 验证步骤 | 期望结果 |
|---|---------|---------|
| A1 | AppHeader 右上角看到 Brain 图标按钮(仅有 active project 时) | 显示;hover 有 bg 变化 |
| A2 | 点击 Brain 按钮 | reka-ui Dialog 居中弹出,fade+scale 动画 |
| A3 | Modal 内显示 2 个 Project layer 卡片(CLAUDE.md / AGENTS.md) | 卡片完整、无字符截断 |
| A4 | 按 ESC | Modal 关闭 + 100ms leave 动画 |
| A5 | 点击 modal 外部遮罩 | Modal 关闭 |
| A6 | 窗口拖到 700px 宽 | Modal 仍居中,内容不溢出 |
| A7 | 窗口拖到 1920px 宽 | Modal 锁在 max-width 900px |
| A8 | ProjectTabs 上不再有 Memory dropdown | 移除干净,无残留 button / popover |
| A9 | Settings → Memory Tab | 仍可用(本期不动) |
| A10 | `pnpm build` | 通过,无 type error / warning |

---

## 6. 风险与回滚

| 风险 | 概率 | 缓解 |
|------|------|------|
| lucide-vue-next 与 heroicons 同时存在导致 bundle 增大 | 中 | lucide 走 tree-shake,只导 Brain 一个图标增量 ~2KB |
| reka-ui Dialog 与现有 SettingsModal 的 portal 抢 z-index | 低 | 都走 reka-ui 自带 z-index;若冲突在 modal CSS 加 `z-index: 2000 !important`(SettingsModal 是 1000) |
| Brain 图标视觉不对路 | 低 | 用户已选;不行换 CpuChip 是 5 分钟事 |

**回滚**:本任务只动前端 + 1 个 spec md。git revert 单 commit 即可。

---

## 7. 关联

- 前置:[`06-10-b5-memory-user-project-2layer/prd.md`](../archive/2026-06/06-10-b5-memory-user-project-2layer/prd.md)
- Spec:
  - `.trellis/spec/frontend/popover-pattern.md`(本任务不涉及 popover,但理解为何放弃 popover 路线)
  - `.trellis/spec/frontend/reka-ui-usage.md`(Dialog 用法 + `:deep()` gotcha)
  - `.trellis/spec/frontend/memory-ui.md`(本任务会更新这份 spec)
