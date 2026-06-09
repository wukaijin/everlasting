# fix SettingsModal reka-ui Select 层级 + 宽度 + 背景 — 第二轮（:deep() 修法）

## Goal

修复 SettingsModal 中 3 个 reka-ui `Select` 实例的视觉 bug。

## ⚠ 诊断重置

**第一轮错误诊断**：以为是 reka-ui inline `z-index: 0` 覆盖了 class `z-index: 3000`。
**第一轮实际无效的修复**：加 `!important` / 换 CSS 变量。

**第二轮（截图证据）**：用户截图显示 SelectContent 掉到 modal 下面的 document flow，items 渲染为裸文字。这不是 z-index 问题，是 **整个 CSS 块根本没应用**。

**真正的根因**：

> **Vue 3 `<style scoped>` 给选择器加 `data-v-xxx` 属性（如 `.models-tab__content[data-v-abc123]`），但 `<SelectPortal>` 把 SelectContent 渲染到 `<body>` 下，portal 出去的 DOM 节点不带 `data-v-xxx` 属性。所以整个 CSS 块的任何规则都不匹配。**

这同时解释了用户的三个症状：
- "层级问题" → 不是被覆盖，是 z-index 根本没设上去
- "宽度不够" → `width` / `min-width` 规则没匹配（但 `min-width: 240px` 写死的也可能被 reka-ui 默认 min-content 接管）
- "背景透明" → `background` 规则没匹配

第一轮加的 `!important` / 换 var 都没用，因为规则本身没应用。截图里 SelectContent 是 `position: static`（默认值），所以掉到 document flow。

## What I already know

### 受影响文件
- `app/src/components/settings/ProvidersTab.vue`（line 234–253 SelectRoot，line 529–539 CSS）
- `app/src/components/settings/ModelsTab.vue`（line 349–375 + 424–457 SelectRoot，line 812–822 CSS，line 436 死类 z-3001）

### 现状（错误状态）
```css
/* ProvidersTab.vue line 529-538 / ModelsTab.vue line 812-821 */
/* ⚠ 整个块不生效 —— scoped style 不匹配 portal 子树 */
.models-tab__content {
  position: fixed;
  background: var(--color-bg-surface);
  ...
  z-index: 3000 !important;  /* ← 没用，规则本身没应用 */
}
.models-tab__viewport { ... }
.models-tab__option { ... }
.models-tab__option[data-highlighted] { ... }
.models-tab__option[data-state="checked"] { ... }
```

### 修法：用 `:deep()` 穿透 scoped 边界
trigger 在组件内，scoped selector 能匹配——**trigger 的规则不动**。content / viewport / option 都在 portal 内，必须用 `:deep()`。

## Requirements

- [ ] `.providers-tab__content` / `.models-tab__content` 块用 `:deep()` 包裹
- [ ] `.xxx__viewport` / `.xxx__option` 块用 `:deep()` 包裹
- [ ] `.xxx__option[data-highlighted]` / `[data-state="checked"]` 用 `:deep()` 包裹
- [ ] `.xxx__trigger` / `.xxx__trigger:hover` / `.xxx__trigger[data-state="open"]` **不动**（trigger 在组件内）
- [ ] 删 `z-3001` dead class（line 436 ModelsTab）
- [ ] 视觉：背景、边框、box-shadow、动画保持不变
- [ ] z-index 实际生效（用 DevTools 验证 = 3000）
- [ ] SelectContent 实际 `position: fixed`（用 DevTools 验证）

## Acceptance Criteria

- [ ] DevTools 检查：SelectContent 元素的 `data-v-` 属性存在（或 `:deep` 规则匹配）
- [ ] DevTools 检查：计算 `z-index` = 3000（不是 0）
- [ ] DevTools 检查：计算 `position` = `fixed`（不是 static）
- [ ] DevTools 检查：计算 `background` = `rgb(19, 24, 34)`（即 `var(--color-bg-surface)`，不是 transparent）
- [ ] DevTools 检查：计算 `width` ≥ trigger 宽度
- [ ] 视觉：弹出的 SelectContent 锚定在 trigger 位置，有深色背景 + 边框 + box-shadow
- [ ] 视觉：弹出的 SelectContent 在 modal 内容之上（不被遮挡）
- [ ] ModelsTab line 436 不再有 `z-3001`
- [ ] `pnpm build` 通过

## Definition of Done

- [ ] 改动控制在 ProvidersTab.vue + ModelsTab.vue 两个文件
- [ ] 改动量 ≤ 20 行 CSS（`:deep()` 包裹 5 个规则 × 2 文件）
- [ ] 手动 dev 验证三个 Select 都正常弹出 + 样式正确
- [ ] 至少用 DevTools 截 1 个 SelectContent 计算样式的证据（z-index / position / background）
- [ ] `pnpm build` 通过
- [ ] journal 记录变更 + 诊断纠错

## Technical Approach

### 改动清单

**1. `app/src/components/settings/ProvidersTab.vue` line 529–563** —— 整个 SelectContent 相关 CSS 块加 `:deep()`

把：
```css
.providers-tab__content { ... }
.providers-tab__viewport { ... }
.providers-tab__option { ... }
.providers-tab__option[data-highlighted] { ... }
.providers-tab__option[data-state="checked"] { ... }
```

改为：
```css
:deep(.providers-tab__content) { ... }
:deep(.providers-tab__viewport) { ... }
:deep(.providers-tab__option) { ... }
:deep(.providers-tab__option[data-highlighted]) { ... }
:deep(.providers-tab__option[data-state="checked"]) { ... }
```

**trigger 的 3 条规则（`.providers-tab__trigger` / `:hover` / `[data-state="open"]`）保持原样**，因为 trigger 在组件内，scoped selector 能匹配。

**2. `app/src/components/settings/ModelsTab.vue` line 812–846** —— 同样改法

**3. `app/src/components/settings/ModelsTab.vue` line 436** —— 删 `z-3001` dead class

### 视觉对比

**前（错误）**：
- SelectContent 实际样式：`position: static`, `z-index: auto`, `background: transparent`, `width: auto`
- 结果：items 掉到 document flow，渲染为裸文字，无背景

**后（正确）**：
- SelectContent 实际样式：`position: fixed`, `z-index: 3000`, `background: var(--color-bg-surface)`, `width: var(--reka-select-trigger-width)`
- 结果：items 锚定在 trigger 位置，浮在 modal 之上，深色背景 + 边框

### z-index 不再需要 `!important`

第一轮加了 `!important` 是基于错误诊断。现在 `:deep(.xxx__content) { z-index: 3000 }` 应该够了（class selector 的 specificity 是 0,1,0，reka-ui 不会以更高特异性压它）。**但为了稳妥，可以保留 `!important` 作为防御**——也方便未来 reka-ui 改行为时不受影响。建议保留。

## Decision (ADR-lite)

**Context**: 用户报告 SettingsModal 里的 reka-ui Select 三个症状（下拉看不见 / 宽度不够 / 背景透明）。第一轮诊断为 reka-ui inline z-index 覆盖，加 `!important` 修复；用户截图证明修复无效，items 仍在 document flow 渲染为裸文字。

**第二轮诊断**：实际根因是 **Vue 3 `<style scoped>` 不会把规则应用到 portal 子树**。`<SelectPortal>` 把 SelectContent 渲染到 `<body>` 下，脱离组件 scope，所以 `.models-tab__content[data-v-xxx]` 这样的选择器完全不匹配。这是 Vue 3 + 任何 portal 类组件（Headless UI、Radix、Reka）的通用坑。

**Decision**:
- 修法：用 `:deep(.xxx__content)` 穿透 scoped 边界
- Trigger 规则保持原 scoped 形式（trigger 在组件内，scoped selector 能匹配）
- 删 z-3001 dead class（保留在改动里）

**Consequences**:
- 修复后 SelectContent 实际应用 position: fixed / z-index: 3000 / background / width
- 沉淀：必须把"scoped style + portal = :deep()"加进 `reka-ui-usage.md` 的 gotcha 列表（Phase 3.3 spec 更新）
- 沉淀：必须把"先验证 CSS 规则实际应用到元素，再判断 z-index / specificity"加进 `guides/break-loop.md` 之类的检查清单（避免再误诊）
- 改动量：~20 行 CSS（5 规则 × 2 文件 = 10 个选择器加 `:deep()` 前缀）
- 跨项目教训：所有用 portal 的 headless 库（reka-ui、Radix、Headless UI、Ark UI）都有这个问题

## Out of Scope

- 不换 UI 库（Naive UI 等不在 scope）
- 不动 reka-ui 版本
- 不动 ModelSelect.vue（手写 popover，spec 保护）
- 不动 ChatPanel.vue 的 worktree dropdown
- 不动 SettingsModal.vue（Dialog 容器）
- 不动 DefaultTab.vue（无 Select）
- 不改 trigger 的 CSS
- 不改 SelectPortal 包装

## Technical Notes

### 根因再确认

Vue 3 `<style scoped>` 编译时给选择器加 `data-v-xxx` 属性：
```css
/* 源 */
.models-tab__content { ... }

/* 编译后（带 data-v hash） */
.models-tab__content[data-v-models-tab-xxx] { ... }
```

trigger 是组件的子元素，编译时 Vue 给 trigger 元素加了 `data-v-models-tab-xxx` 属性，selector 匹配上。
SelectContent 通过 `<SelectPortal>` 渲染到 body，**Vue 编译时不会给 portal 出去的 DOM 加 data-v 属性**（它不在组件模板里），所以 selector 不匹配。

### reka-ui 2.9.9 渲染流程

1. `<SelectRoot>` 创建响应式状态
2. `<SelectPortal>` 用 Vue 3 `<Teleport to="body">` 把内容渲染到 body
3. `<SelectContent>` 在 body 下渲染，**没有** data-v 属性
4. 内容里的 `<SelectItem>` 等子元素也不带 data-v

### 触发等价于的 visual 现象

- SelectContent 的 `<div class="models-tab__content">` 在 body 下
- `.models-tab__content[data-v-models-tab-xxx]` 选择器要 `data-v-models-tab-xxx` 属性 → 找不到
- 整块规则不应用
- SelectContent 实际样式 = 浏览器默认 (`position: static`, `z-index: auto`, 无背景)
- items 在 document flow 中渲染（`display: block` 默认）

### Vue 3 修法

`:deep()` 是 Vue 3 scoped style 的官方 escape hatch：
- `:deep(.selector)` 编译为 `.selector[data-v-xxx]`，**但**不要求目标元素有 data-v 属性
- 实际上 `:deep()` 编译后会让内部选择器"穿过" scope 边界

### 验证路径

启动 dev (Tauri 或 Vite)，在 DevTools 里：
1. 点击 Thinking Effort trigger
2. 找到 SelectContent 元素（body 下直接子级，class="models-tab__content"）
3. 看 Elements 面板：该元素**没有** `data-v-xxx` 属性 ← 这就是根因
4. 看 Computed 面板：
   - `position` = static（错误）/ fixed（修复后）
   - `z-index` = auto（错误）/ 3000（修复后）
   - `background-color` = rgba(0,0,0,0) transparent（错误）/ rgb(19, 24, 34)（修复后）
5. 截图保存作为证据

### 文件
- `app/src/components/settings/ProvidersTab.vue`（CSS 改动 ~10 行）
- `app/src/components/settings/ModelsTab.vue`（CSS 改动 ~10 行 + 1 个 class 名修改）

### 不动
- `app/src/components/settings/SettingsModal.vue`
- `app/src/components/settings/DefaultTab.vue`
- `app/src/components/chat/ModelSelect.vue`
- reka-ui 版本
