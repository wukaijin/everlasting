# PRD — UI polish: reka-ui forms + cog-6-tooth + worktree chip border + popup animations + text-muted brighten

> Task: `06-09-ui-polish-reka-forms-cog6-worktree-anim-muted`
> Status: planning
> Created: 2026-06-09
> Base branch: `main` (PR1+PR2+PR3+PR4+PR5 follow-up 已 merge)

## Background

After PR5 follow-up merged (Settings 入口到 sidebar footer / model 选择到 chat input 旁), 用户 dev 体验后指出 5 处 UI 优化需求:

1. **Settings 里所有表单控件仍是裸 HTML** (`<input>`, `<select>`, `<textarea>`, `<input type="checkbox">`, radio) — 跟项目其他位置用 reka-ui 风格不统一 (SettingsModal 主体已经用 reka-ui Dialog + Tabs)
2. **Sidebar footer "设置" 图标是 12px cog (8 齿)** — 用户想要 cog-6-tooth (FontAwesome 6 风格, 6 齿) + 加大 + 颜色变浅
3. **ChatPanel worktree chip 右侧 chevron toggle 按钮缺右边框和右圆角** — 跟主 chip 接缝不齐, 视觉割裂
4. **Modal / Dropdown / Popup 全部 instant 显示/隐藏** — 无淡入/淡出动画, 体验生硬 (AppShell 的 toast 是例外, 已经用了 transition)
5. **全局 CSS var `--color-text-muted: #64748b` (slate-500)** — 太暗, 视觉权重不够, 11px mono 文字几乎要消失了

## Goal

5 项 polish 一次性合并成一个 PR, 提升 multi-model UI 整体质感:
- 表单控件统一 reka-ui primitives + 主题色适配
- Sidebar 设置图标换 cog-6-tooth + 加大 + 颜色变浅
- worktree chip 视觉接缝
- 所有 modal/dropdown/popup 加淡入/淡出/缩放过渡
- `--color-text-muted` 加亮

## Non-goals

- 不改表单的 schema / 提交逻辑 / 验证 (只是把 `<input>` 换成 reka-ui `InputRoot` 之类的包装, v-model 还是双向绑定, submit 逻辑不动)
- 不动后端 (纯前端 CSS/Vue)
- 不重构 Icon.vue 注册表结构 (只加新 icon)
- 不改其他 color var (`--color-text-primary` / `--color-text-secondary` 等) — 只动 `--color-text-muted`
- 不加新依赖 (reka-ui 已经引入了; cog-6-tooth 看情况是否需要加 iconify, 见 D1)
- 不动 form 的 field label / 错误提示样式 (只是把控件本身换皮)
- 不重做 toast (AppShell 已 transition)
- 不改 worktree 主 chip 样式 (只补右侧 chevron 的接缝)

## Requirements (5 项 user 决策, 2026-06-09 收敛)

### R1. Settings 表单控件 → reka-ui + 主题色适配

- 涉及文件:
  - `app/src/components/settings/ProvidersTab.vue` (1 select + 3 input)
  - `app/src/components/settings/ModelsTab.vue` (2 select + 4 input + 1 checkbox)
  - `app/src/components/settings/DefaultTab.vue` (1 radio group)
- reka-ui primitives:
  - `TextField` (替代 `<input type="text|password">`) — ProvidersTab displayName/baseUrl/apiKey, ModelsTab modelName/displayName/maxTokens/contextWindow
  - `Select` (替代 `<select>`) — ProvidersTab protocol, ModelsTab providerId/thinkingEffort
  - `Checkbox` (替代 `<input type="checkbox">`) — ModelsTab supportsThinking
  - `RadioGroup` + `RadioGroupItem` (替代 `<input type="radio">`) — DefaultTab default model
- 主题色适配: reka-ui 默认 chrome 是中性色, 需用项目 CSS var 重写:
  - 背景: `var(--color-bg-elevated)` (跟现有 input 背景一致)
  - 边框: `var(--color-bg-border)` / focus 时 `var(--color-accent)`
  - 文字: `var(--color-text-primary)` (输入) / `var(--color-text-muted)` (placeholder)
  - 选中态: `var(--color-accent)` (checkbox check / radio dot)
  - Select dropdown: `var(--color-bg-surface)` 背景 + `var(--color-bg-border)` 边框 + 6px 圆角 (跟 ModelSelect / worktree popover 一致)
- v-model 行为保持: `useProvidersStore` / `useModelsStore` 的 reactive state 不动, 只把 `<input v-model="form.x">` 换成 `<TextFieldRoot v-model="form.x">` 之类
- 错误态 / 禁用态: 沿用现有 disabled 属性 (form.saving 时 disable)
- 保留 `<label>` 标签文本 (form-group__label), 跟现在视觉一致

### R2. Sidebar 设置图标 → cog-6-tooth + 加大 + 颜色变浅

- 涉及文件:
  - `app/src/components/Icon.vue` (注册新 icon)
  - `app/src/components/layout/Sidebar.vue` (替换 + 调大)
- cog-6-tooth 来源 (D1 决, **事实修正**):
  - **heroicons 24/outline 实际有 `Cog6ToothIcon`** (在 `@heroicons/vue/24/outline/Cog6ToothIcon.js` 验证), 跟项目已在用的 `CogIcon` (8 齿) 是不同组件
  - **D1 决策: 用 heroicons `Cog6ToothIcon`**, 0 依赖, 跟项目其他 heroicons 一致, 之前 prd 列的 A (iconify)/ B (内联)/ C (调 heroicons `AdjustmentsHorizontalIcon`) 都作废
  - 在 Icon.vue map 加 `import { Cog6ToothIcon } from "@heroicons/vue/24/outline"` + `"cog-6-tooth": Cog6ToothIcon`
- 大小: 12px → 18px (跟 worktree chip 12px document icon 形成层次, 18px 是 footer 主操作)
- 颜色: 当前 `var(--color-text-muted)` (#64748b) → **加亮** (用 R5 加亮后的新值, 跟 "设置" 文字同色或稍浅)
- 视觉布局: 齿轮 + 文字 "设置" 11px mono, 跟现在一致, 但齿轮本身大一号

### R3. worktree chip 右边 chevron 按钮 → 补右边框和右圆角

- 涉及文件: `app/src/components/chat/ChatPanel.vue` (`.chat-panel__chip--worktree-toggle` CSS)
- 当前现状 (line 413-425): 主 "attach worktree" 按钮 + 右侧 chevron toggle 按钮并排, 主按钮有左+中+右 圆角, chevron toggle 按钮只有左边框, 缺右边框和右圆角
- 修复: 给 `.chat-panel__chip--worktree-toggle` 加 `border-right: 1px solid var(--color-bg-border)` + `border-top-right-radius: 4px` + `border-bottom-right-radius: 4px`
- 注意: 整个 `.chat-panel__worktree` 容器 (line 399-402) 已经有 `border: 1px solid var(--color-bg-border); border-radius: 4px`, 内部两个按钮共享容器边框
- 当前 main chip 跟 chevron 之间有 `border-right` (在 main chip 上), chevron 缺 `border-right` 跟容器接缝
- 修复: chevron 自己加 `border-right: none` (跟容器右边框重复) + 重叠处理 (or `border-right: 1px solid transparent` 占位避免跳动)
- 实际实现看代码现状再定具体 CSS

### R4. Modal / Dropdown / Popup 加动画

- 涉及组件 (按出现位置):
  - `app/src/components/settings/SettingsModal.vue` — reka-ui DialogContent, 加 fade + scale
  - `app/src/components/chat/ModelSelect.vue` — 手写 popover, 向上弹, 加 fade + slight slide-down
  - `app/src/components/chat/ChatPanel.vue` — 手写 worktree popover, 向下弹, 加 fade + slight slide-down
  - `app/src/components/chat/DeleteWorktreeConfirm.vue` — 如果是 modal, 加 fade + scale
  - `app/src/components/ProjectTabs.vue` — 如果有 popover/modal, 加
- 动画风格 (待 D2 决):
  - **A. fade-only**: `opacity: 0 → 1`, 150ms ease-out
  - **B. fade + scale**: `opacity 0→1` + `scale(0.96→1)`, 150ms ease-out (modal 风格)
  - **C. fade + slide**: popover 风格 — `opacity 0→1` + `translateY(4px→0)` 配合弹方向 (向下 popover `+4px`, 向上 popover `-4px`), 150ms ease-out
  - **D. 混合**: modal 用 B, popover 用 C
- 决策: 待用户选
- 退出动画: 100ms ease-in, 同方向反向
- 实现方式: Vue `<Transition name="...">` 包裹, scoped style 定义 enter/leave 关键帧
- Toast (AppShell) 已有, 不动
- v-if 控制显示的 (ModelSelect / worktree popover) 用 v-if + Transition 自动 hook
- reka-ui DialogContent 自带 data-state attribute, 可用 CSS 选择器 (但项目目前没 data-state 动画, 需加)

### R5. `--color-text-muted` 加亮

- 涉及文件: `app/src/style.css` (line 37, `--color-text-muted: #64748b`)
- 当前值: `#64748b` (slate-500)
- 加亮到 (待具体决定, 推荐 `#7d8aa3` 或 `#8694ad` — slate-400 / slate-350 等价)
- 不能过亮 (会跟 `--color-text-secondary: #8b95a7` 打架)
- 推荐值: `#7c8aa0` (slate-500 → slate-450 等价, 提亮 ~6% luminance)
- 决定: 1 行 CSS 改

## Acceptance Criteria

### 视觉 / 交互

- [ ] Settings 三个 tab (Providers / Models / Default) 的所有表单控件渲染为 reka-ui 风格 (背景 / 边框 / focus 环 / 选中态都用项目 CSS var)
- [ ] Sidebar footer "设置" 图标换成 cog-6-tooth (具体来源 D1 决), 18px 大小, 颜色用 R5 加亮后的 `--color-text-muted`
- [ ] worktree chip 右侧 chevron toggle 按钮视觉接缝齐整 (右圆角 + 跟容器右边框不重复)
- [ ] SettingsModal 打开有 fade + scale 动画 (从中心 scale 0.96→1 + opacity 0→1, 150ms)
- [ ] ModelSelect popover 打开有 fade + slide 动画 (从下方 slide-down 4px, 150ms)
- [ ] ChatPanel worktree popover 打开有 fade + slide 动画 (从上方 slide-down 4px, 150ms)
- [ ] DeleteWorktreeConfirm 打开 (如 modal) 有 fade + scale
- [ ] `--color-text-muted` 加亮, 全局所有用此 var 的位置 (sidebar__title / status-bar / chat-input__hint 等) 视觉权重微增

### 文件改动

- [ ] `app/src/components/settings/ProvidersTab.vue` — 1 select + 3 input 换 reka-ui
- [ ] `app/src/components/settings/ModelsTab.vue` — 2 select + 4 input + 1 checkbox 换 reka-ui
- [ ] `app/src/components/settings/DefaultTab.vue` — radio 换 reka-ui RadioGroup
- [ ] `app/src/components/Icon.vue` — 加 cog-6-tooth 图标 (来源 D1 决)
- [ ] `app/src/components/layout/Sidebar.vue` — 换 cog-6-tooth + 加大 + 颜色
- [ ] `app/src/components/chat/ChatPanel.vue` — worktree chip 边框 + popover 动画
- [ ] `app/src/components/chat/ModelSelect.vue` — popover 动画
- [ ] `app/src/components/settings/SettingsModal.vue` — modal 动画 (transition name + scoped CSS)
- [ ] `app/src/components/chat/DeleteWorktreeConfirm.vue` — modal 动画 (如 modal)
- [ ] `app/src/style.css` — `--color-text-muted` 改值
- [ ] 不改 `app/src/components/chat/ChatInput.vue` (chat-input__hint 已经用 `--color-text-muted`, R5 自动影响)

### 验证

- [ ] `pnpm exec vue-tsc --noEmit` (cd app) 全 pass
- [ ] `pnpm build` (cd app) 全 pass, 0 warning
- [ ] `cargo check` + `cargo test --lib` 全 pass (后端理论上不破, 但跑一遍保险)
- [ ] 手动验证 5 项 user flow (dev 跑 `pnpm tauri dev`):
  1. 打开 Settings → 切到 ProvidersTab → 加 provider form 渲染风格统一
  2. 切到 ModelsTab → 加 model form 渲染风格统一 (含 checkbox 选中态)
  3. 切到 DefaultTab → 选 default model radio 风格统一
  4. 看 Sidebar footer 齿轮是 cog-6-tooth, 18px, 颜色浅
  5. 打开 chat → 看 worktree chip 接缝齐
  6. 打开任一 modal / popover, 看淡入+scale/slide 动画
  7. 全局扫一眼 11px mono 灰色文字 (sidebar / status / chat hint) 微变亮

## Out of Scope

- ❌ 后端 schema / IPC / 测试 (纯前端 polish)
- ❌ 表单的验证逻辑 / 错误提示样式 (R1 只换控件皮, 不动 v-model 逻辑)
- ❌ 改其他 color var (`--color-text-primary` / `--color-text-secondary` / `--color-text-tool-*` 等) — R5 只动 `--color-text-muted`
- ❌ 引入新的 UI 库 (除 D1 决议可能加 iconify)
- ❌ Toast 动画 (AppShell 已有, 不动)
- ❌ 重做 worktree chip 整体样式 (R3 只补右边接缝, 不动 main chip 边框/圆角)
- ❌ ModelSelect / worktree popover 的"行为"改动 (R4 只加动画, 不动 open/close 逻辑)
- ❌ 删除 worktree 按钮样式 (R3 不涉及)
- ❌ Sidebar 整体 layout / 宽度 (R2 只动 icon)
- ❌ SettingsModal 内部 Tabs 切换动画 (不在 R4 范围, 是 horizontal tab transition, 复杂)

## Technical Notes

### 关键文件改动

| 文件 | 改动 |
|---|---|
| `app/src/components/settings/ProvidersTab.vue` | 4 表单控件 (1 select + 3 input) 换 reka-ui TextFieldRoot / SelectRoot |
| `app/src/components/settings/ModelsTab.vue` | 7 表单控件 (2 select + 4 input + 1 checkbox) 换 reka-ui |
| `app/src/components/settings/DefaultTab.vue` | radio 换 reka-ui RadioGroup |
| `app/src/components/Icon.vue` | 加 cog-6-tooth (来源 D1 决) |
| `app/src/components/layout/Sidebar.vue` | Icon name="cog-6-tooth" size=18 + 颜色改 |
| `app/src/components/chat/ChatPanel.vue` | `.chat-panel__chip--worktree-toggle` 加右圆角 + 跟容器右 border 协调; popover 加 Transition |
| `app/src/components/chat/ModelSelect.vue` | popover 加 Transition (slide-down fade-in) |
| `app/src/components/settings/SettingsModal.vue` | DialogContent 包 Transition (scale + fade) |
| `app/src/components/chat/DeleteWorktreeConfirm.vue` | (如 modal) 包 Transition |
| `app/src/style.css` | `--color-text-muted: #64748b` → `#7c8aa0` (具体值 D3 决) |

### 关键复用点

- 项目已有 reka-ui 2.9.9, primitives 已用过: `DialogRoot` / `DialogContent` (SettingsModal), `TabsRoot` (SettingsModal)
- 新增 reka-ui 用法: `TextFieldRoot` / `SelectRoot` / `CheckboxRoot` / `RadioGroupRoot` —— 跟 DialogRoot 同样 `import { ... } from "reka-ui"`
- 主题色适配: 把 reka-ui 暴露的 class / data-attribute 映射到项目 CSS var (reka-ui 2.x 默认 unstyled primitives, 完全可控)
- 动画: Vue `<Transition name="...">` 是项目内通用模式 (AppShell toast 已经用), 不引入新依赖
- PR5 popover pattern (`.trellis/spec/frontend/popover-pattern.md`) 是 R4 popover 动画的 starting point

### Anti-patterns (避免)

- ❌ 引入整个 `@iconify/vue` 当 D1 决定内联 SVG 时 (保持依赖最小)
- ❌ 在 reka-ui TextFieldRoot 上写 inline style (用项目 CSS class 跟其他 input 风格统一)
- ❌ 改 `--color-text-secondary` (会跟现有次要文字视觉冲突)
- ❌ 给 Toast 加动画 (已经有, 别动)
- ❌ 重构 SettingsModal 的 Tabs (R1 范围内是 form 控件, 不是 Tabs)
- ❌ 在 worktree chip 主按钮加新边框 (R3 只补 chevron 接缝)

## Definition of Done

- [ ] 5 项 user flow 视觉验证通过 (dev 跑 `pnpm tauri dev`)
- [ ] R1-R5 全部 acceptance criteria 勾选
- [ ] `vue-tsc --noEmit` + `pnpm build` + `cargo check` + `cargo test --lib` 全 pass
- [ ] commit message: `style(ui): polish — reka-ui form primitives + cog-6-tooth + worktree chip + popup animations + text-muted`
- [ ] trellis-check 通过
- [ ] docs/IMPLEMENTATION.md 不动 (UI polish 不入路线图)

## Decision (ADR-lite)

### D1. cog-6-tooth 图标来源 — 用 heroicons `Cog6ToothIcon` (2026-06-09 修正)

**Context**: 用户要求 cog-6-tooth (FontAwesome 6 风格, 6 齿), 我之前 prd 错误地判断 heroicons 没有 6-tooth cog。事实: heroicons 24/outline 实际**有 3 个 cog variant**:

- `CogIcon` (8-tooth) — 项目已在 Icon.vue map 用
- `Cog6ToothIcon` (6-tooth) — 没人用, **正是用户要的**
- `Cog8ToothIcon` (8-tooth alt) — 没人用

**Decision**: 用 heroicons 现有的 `Cog6ToothIcon`, 0 依赖, 跟项目其他 heroicons 一致. 之前 prd 列的 A (iconify) / B (内联 SVG) / C (换 `AdjustmentsHorizontalIcon`) 都作废.

**Consequences**:
- ✅ 0 依赖 (1 行 import + 1 行 map entry)
- ✅ 跟项目其他 heroicons 风格一致 (项目以 heroicons 为主, 不引入 iconify 之类新源)
- ✅ heroicons 是 heroicons 2.x 官方维护, 不需要 license attribution
- ⚠️ heroicons 的 `Cog6ToothIcon` 跟 FA6 `cog-6-tooth` SVG 路径可能略不同 (e.g. stroke 风格 / 几何精度), 视觉"几乎一样"但不像素级一致 — 接受

### D2. 动画风格 (待用户决)

**Context**: modal 和 popover 风格不同 — modal 从中心缩放 (scale 0.96→1) + fade, popover 从触发点 slide (4px) + fade. 4 个 trade-off:

- **A. fade-only**: 最轻量, 视觉变化小, 不抢戏
- **B. fade + scale**: 适合 modal (SettingsModal), 中心感强
- **C. fade + slide**: 适合 popover (ModelSelect / worktree), 跟触发方向呼应
- **D. 混合**: modal 用 B, popover 用 C — **推荐**, 跟常见 UX 模式一致 (Material Design / Radix UI / shadcn/ui 都用 D)

**Decision**: 待用户选 (推荐 D)

**Consequences**:
- A: 1 套 CSS, 简单但无差别
- B / C: 单一风格, 适合只做 modal 或只做 popover
- D: 2 套 CSS keyframes, 但视觉更精细

### D3. `--color-text-muted` 加亮目标值 (待用户决)

**Context**: 当前 `#64748b` (slate-500), 用户说"加亮一点点" — 模糊, 需要具体 hex。

- **A. `#7c8aa0`**: slate-500 → slate-450, ~6% luminance 提升
- **B. `#8694ad`**: slate-500 → slate-400, ~10% luminance 提升
- **C. `#7d8aa3`**: 中间值

**Decision**: 待用户选 (推荐 A, 跟 `--color-text-secondary: #8b95a7` 保持差距)

**Consequences**:
- A/B/C 都是 "加亮", 差异在视觉权重微调
- 不能选过亮 (e.g. `#a0aec0` slate-300) — 会跟 secondary 打架
