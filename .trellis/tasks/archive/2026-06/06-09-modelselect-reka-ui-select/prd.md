# ModelSelect 迁移到 reka-ui Select (评估中)

## Goal

修复 `app/src/components/chat/ModelSelect.vue` 的下拉"看不见 + 宽度对不齐"问题。
**初判方案**：迁移到 reka-ui `SelectRoot/Trigger/Content`。
**⚠ 关键约束**（刚刚发现，需要决策）：项目 spec 在 2026-06-09
**显式禁止**用 reka-ui `Popover` / `DropdownMenu` 替换手写 popover
（见 `.trellis/spec/frontend/popover-pattern.md` 第 320-344 行
"Don't: Use `reka-ui` `DropdownMenu` for New Dropdowns in This Project"，
以及 `.trellis/spec/frontend/reka-ui-usage.md` 第 231-244 行
"Don't: Use reka-ui's `Popover` primitive for project popovers"）。
Spec 预先给出的修法是 `<Teleport to="body">` + `position: fixed`
+ JS 计算坐标（见 `popover-pattern.md` 第 379-405 行）。

## What I already know

- 当前 `ModelSelect.vue` 是**手写 popover**（line 142-209），
  不是 reka-ui `Select`。参考的是 `ChatPanel.vue` 的 worktree dropdown
  （line 127-149 close logic，line 750-812 CSS）。
- z-index 100 写在 popover 元素自身上（`.model-select__menu` line 292）。
- 宽度问题：popover 用 `right: 0` + `min-width: 220px` 锚定
  （line 282-289），trigger 用 `max-width: 220px`（line 236）。
  当 trigger 实际宽度 < 220px 时，popover 向左溢出且和 trigger 不对齐。
- 上游 stacking context 排查：
  - `style.css` 全局：无 `transform` / `filter` / `isolation`
  - `ChatWindow.vue`：无 transform/filter/fixed
  - `ChatPanel.vue`：line 821 有 `position: fixed` (diff-modal-backdrop)；
    line 949 有 `transform: scale(0.96)` (popover 动画自身)
  - `ChatInput.vue`：line 258 有 `transform: rotate(360deg)` (图标自身)
  - `style.css` line 75：`html, body, #app { overflow: hidden }`
    → **这是关键**：#app 整体 `overflow: hidden`，所以 trigger
    紧贴 #app 边缘时，向上开的 popover 可能被裁掉
- 项目里 reka-ui **已经在用**的场景（4 个 settings 文件）：
  - DialogRoot/Content/Title/Close、Overlay（SettingsModal）
  - TabsRoot/List/Trigger/Content（SettingsModal inner tabs）
  - SelectRoot/Trigger/Content/Item/Value（ProvidersTab/ModelsTab/DefaultTab）
  - CheckboxRoot/Indicator
  - RadioGroupRoot/Item/Indicator
  - Label（包裹表单字段）
- reka-ui 版本 pin: `2.9.9`（`.trellis/spec/frontend/reka-ui-usage.md`）
- 动画规范（`popover-pattern.md` 第 416-501 行）：
  - 150ms enter / 100ms leave
  - Popover 滑出方向 = 弹出方向（向下弹 → translateY(-4px → 0)；
    向上弹 → translateY(4px → 0)）
  - Modal 用 fade + scale 0.96 → 1
  - reka-ui DialogContent 用 `[data-state="open|closed"]` 选 CSS
    （无需 Vue `<Transition>` wrapper）

## Assumptions (temporary)

- 假设 bug 真实存在（用户已确认"select 不可用，下拉项看不到"）。
- 假设用户希望保留视觉设计（深色 Prussian blue、动画、chevron、
  group-by-provider 分组、streaming 禁用、isPlaceholder 灰色斜体）。
- 假设用户希望保留 PR5 的 per-session 持久化逻辑
  （`invoke("update_session_model_id", ...)`）。
- 假设 spec 决策可以重审（这是技术债 / 选型决策的常规重审窗口）。

## Open Questions (按优先级)

1. **【Blocking】要走 spec-prescribed fix（Teleport）还是要 override
   spec（迁 reka-ui Select）？** 见下方 Decision 段
2. 【Preference】scope：是否同时审计 / 迁移 StatusBar dropdown、
   ProjectTabs 中的同类手写 popover？
3. 【Blocking，需明确】用户报的"看不见"具体是哪种：
   a) 完全不可见（被裁掉 / 被遮挡）
   b) 渲染了但宽度 / 位置错
   c) 两者都是

## Requirements (evolving, 待 Q1 决策后细化)

### 若选 Spec-prescribed fix (Teleport)
- [ ] `ModelSelect.vue` 改用 `<Teleport to="body">` + `position: fixed`
- [ ] 用 trigger 的 `getBoundingClientRect()` 计算 popover 位置
- [ ] 改用 `min-width: max(220px, 100%)` 让 popover 至少和 trigger 等宽
- [ ] 关闭时还原 trigger 位置计算
- [ ] z-index 提至 9999
- [ ] 保留 isStreaming 禁用、isPlaceholder 灰色斜体、group-by-provider
- [ ] 保留 `update_session_model_id` IPC 持久化
- [ ] 保留 150ms/100ms enter/leave 动画（向上弹 → translateY(4px → 0)）
- [ ] Esc + outside-click 关闭（已实现，需保留）

### 若选 Override spec (reka-ui Select)
- [ ] `ModelSelect.vue` 改用 `SelectRoot` + `SelectTrigger` + `SelectContent`
      + `SelectItem` / `SelectGroup`（如有）
- [ ] 主题对齐：用 `.model-select__trigger` / `.model-select__menu` /
      `.model-select__item` 等 BEM 类包裹 reka-ui primitive
      （按 `reka-ui-usage.md` "Convention: Wrap reka-ui primitives in
      project-scoped CSS classes"）
- [ ] 动画：用 `[data-state="open|closed"]` 选 CSS
      （按 `reka-ui-usage.md` "Convention: Theming via data-state"）
- [ ] 宽度：`SelectContent` 用 `position="popper"` + width strategy
- [ ] 保留 isStreaming 禁用（`:disabled` + `data-disabled`）
- [ ] 保留 isPlaceholder 灰色斜体
- [ ] 保留 group-by-provider（reka-ui 2.9.9 有 `SelectGroup`）
- [ ] 保留 `update_session_model_id` IPC
- [ ] **必须**更新 `.trellis/spec/frontend/popover-pattern.md` 加一个
      "Deviation: ModelSelect now uses reka-ui Select" 小节
      （按 spec 第 343-344 行规定："document the deviation"）
- [ ] **必须**更新 `.trellis/spec/frontend/reka-ui-usage.md` 移出
      "Reka-ui is **not** used for the project popovers (ModelSelect,
      worktree dropdown)" 段

## Acceptance Criteria (evolving)

- [ ] 打开下拉后，popover **完全可见**，不被父级裁掉
- [ ] popover 宽度 ≥ trigger 宽度，**视觉上与 trigger 对齐**
- [ ] z-index 9999，不被 `ChatPanel.vue` 的 `diff-modal-backdrop`
      （z-index 1000）以外的任何元素遮挡
- [ ] 150ms enter / 100ms leave 动画保留
- [ ] Esc / outside-click 关闭保留
- [ ] 切换模型触发 IPC，per-session override 持久化
- [ ] streaming 中禁用 + tooltip 提示保留
- [ ] type-check (`pnpm build`) 通过

## Definition of Done (team quality bar)

- [ ] `pnpm build` 通过
- [ ] 手动验证：在 dev server 下点击 trigger、选择、Esc、outside-click
      四个路径都正确
- [ ] 若走 override 路径：spec 文档已同步更新（标注 deviation）
- [ ] Journal 记录变更

## Out of Scope (explicit)

- 不重做 ChatPanel.vue 的 worktree dropdown（除非用户后续单独开任务）
- 不动 StatusBar / ProjectTabs（除非用户扩 scope）
- 不动 reka-ui 版本（仍 pin 2.9.9）
- 不动 settings/ 4 个文件（已经用 reka-ui Select，不需要变）

## Technical Notes

### Spec 引用
- `.trellis/spec/frontend/popover-pattern.md`（Filled 2026-06-09，PR5）
- `.trellis/spec/frontend/reka-ui-usage.md`（Filled 2026-06-09，UI polish）
- `.trellis/spec/frontend/design-tokens.md`（Filled 2026-06-09，UI polish）
- `.trellis/spec/frontend/component-guidelines.md`（To fill）

### 文件路径
- `app/src/components/chat/ModelSelect.vue`（待改，375 行）
- `app/src/components/chat/ChatPanel.vue`（参考 line 127-149, 750-812）
- `app/src/components/chat/ChatInput.vue`（父组件，line 143 mount）
- `app/src/stores/{config,models,chat}.ts`（store 读源，不动）

### 已知类似 popover（scope 决定时参考）
- `app/src/components/chat/ChatPanel.vue` worktree dropdown
  （line 127-149 close logic + line 750-812 CSS）
- StatusBar（在 App.vue 或 ChatWindow.vue 顶部 / 底部，需 grep 确认）
- ProjectTabs.vue（看是否有 dropdown）
