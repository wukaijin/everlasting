# 全局键盘焦点可见性基线：:focus-visible 焦点环 + outline:none 治理

## Goal

让键盘用户在全应用内"看得见 Tab 焦点在哪"：建立全局 `:focus-visible`
焦点环基线（复用既有 `--shadow-ring` token），并治理散落各组件的
35 处 `outline: none`——每处要么被全局环覆盖、要么有明确的自定义
替代、要么删除冗余压制。

**范围界定**：本任务是全局 CSS 基线 + 存量压制点清理；不引入共享
Button 组件、不做 66+ 按钮文件的逐一改造（那是后续独立任务，本任务
完成后其收益自动覆盖所有未定制焦点的按钮）。

## Background（2026-08-22 实测核验）

- 全项目 **69 个文件含 `<button`**，仅 **4 个**有 `:focus-visible`
  样式（MemoryPreview / SearchModal / MessageImages / MessageActionsMenu）。
- **35 处 `outline: none`** 散布在 ProjectTabs / SearchModal(×3) /
  MessageActionsMenu(×2) / RemoteTab / GroupChatConfigModal(×3) 等
  ——压掉 UA 默认焦点环，多数没有等价替代。
- `--shadow-ring`（`0 0 0 3px color-mix(accent 20%)`，design-tokens
  已文档化的焦点环约定）全项目只有 **4 个消费者**（ChatInput /
  MessageItemEdit / AuditLogModal / PairingView）。
- z-index 实测 21 个散值（2→9999），佐证后续 P1 任务清单。

键盘导航当前实际不可见：Tab 遍历侧栏会话项 / header 图标 / modal
按钮时无任何视觉指示。这是可用性缺陷（WCAG 2.4.7 Focus Visible,
AA），不只是美化。

## Requirements

- **R1 全局基线规则**：在 `app/src/style.css` 增加 `:focus-visible`
  基线——`outline` 保持 UA 默认或显式置 none 的同时提供
  `box-shadow: var(--shadow-ring)` 替代。作用面收敛到交互元素选择器组
  （`button, a, select, input, textarea, summary, [role="button"],
  [tabindex]:not([tabindex="-1"])`），避免波及 modal 容器等已有阴影的
  非交互节点。已自带焦点处理的组件（ChatInput focus-within、4 个
  focus-visible 文件）不受破坏：全局规则用低优先级层（非 !important，
  具体策略实现时定，倾向 `:where()` 包裹降零特异性）。
- **R2 opt-out 机制**：提供一个 `.no-focus-ring` 类（命名实现时定）
  给确需自定义焦点呈现或鼠标场景专用控件的元素；凡使用处必须注释原因。
- **R3 outline:none 治理**：逐个核对 35 处——
  (a) 元素已被全局环覆盖 → 删除该 `outline: none` 或保留但确认不冲突；
  (b) 有自定义替代（如 SearchModal 结果项的高亮底） → 保留压制 +
  注释指向替代样式；(c) 纯历史遗留 → 删除。
- **R4 不新增 token**：焦点环一律 `--shadow-ring`；如真机验证发现
  accent 20% 在个别底色上不可辨，先在任务内记录证据再议新档，
  不静默改值。
- **R5 reduced-motion 与现有动效零回归**：焦点环是静态 box-shadow，
  不得引入 transition 冲突；`:where()` 方案不得改变既有 hover/active
  行为。

## Acceptance Criteria

- [ ] Playwright 键盘冒烟：对代表性界面（sidebar 会话项、AppHeader
      图标钮、SettingsModal 内按钮/表单、ChatInput 工具行、搜索结果）
      逐个 Tab，截图证明每个落点有可见环；对照修复前（无环）。
- [ ] `grep -rn "outline:\s*none" app/src --include="*.vue"` 的每一条
      命中都能归类到 R3 的 (a)/(b)/(c) 并在代码里留有注释或删除。
- [ ] 既有 4 个 focus-visible 组件与 ChatInput 焦点样式视觉不变
      （截图对照）。
- [ ] `cd app && pnpm test` 全绿；`vue-tsc --noEmit` 零错误。
- [ ] Tab 遍历不出现"焦点环被 overflow:hidden 裁剪半截"类观感问题
      （如有，记入 research 作为 Button 组件任务的输入）。

## Non-goals

- 共享 Button/BaseButton 组件及其在 66+ 文件的机械替换（后续任务；
  本任务的全局环是其安全网）。
- z-index 阶梯 token 化（另立任务，证据已备）。
- Spinner/Skeleton 原语抽取（另立任务）。
- 鼠标点击不显示环的 `:focus { outline:none }` 老式处理——本项目
  直接用 `:focus-visible` 天然区分键鼠，不需要 JS polyfill。

## Notes

- reka-ui 弹窗家族（Dialog/Select/Tooltip）自带 roving tabindex 与
  焦点管理；全局环只负责"可见"，不干预其行为。实现后重点回归
  SettingsModal / SearchModal / 各下拉。
- Tailwind v4 preflight 对 outline 有自己的 baseline（`outline-color`
  相关），实现前先在 devtools 确认 UA 默认行为，避免双重环。
- 验证脚本可沿用 `~/.cache/everlasting-ui-review/` scratch 环境，
  新增 keyboard Tab 遍历取证脚本即可。
