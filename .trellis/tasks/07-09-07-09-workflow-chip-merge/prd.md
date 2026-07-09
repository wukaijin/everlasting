# 合并 WorkflowToggle + PluginSelect 为单 chip + popover

## Goal

把 `WorkflowToggle` (chat/WorkflowToggle.vue) 和 `PluginSelect` (chat/PluginSelect.vue) 两个 chip 合并成**单 chip + popover**：
- 触发器统一为一个：`Wf ▾`（OFF）或 `Wf · dev ▾`（ON）
- 点开 popover：顶部一行 workflow toggle、下面列 plugin（OFF 时整段灰显但保留可见，方便用户看到有哪些 plugin 可选）
- 沿用 `ModeSelect.vue` 现成的 popover 模板（outside-click、Esc、slide 动画都已有，无新基础设施）
- 无 store action / IPC / 后端改动（`requestSetWorkflowEnabled` + `requestSetPluginName` 维持现状）
- 删 `WorkflowToggle.vue` 文件，`ChatInput.vue` 移除其引用

## 背景

`ChatInput.vue:583-590` 同 flex row 渲染两个 chip：
- OFF 时只有 `WorkflowToggle`（约 28px）
- ON 时 `WorkflowToggle` + `PluginSelect` 并排（约 28px + 80px + 8px = 116px）

`PluginSelect` 的可见性完全受 `workflowEnabled` 控制（`PluginSelect.vue:157` `v-if="hasSession && workflowEnabled"`），纯条件从属。两者都用 `Wf` 前缀当 brand marker，语义上是**同一个概念的两次表达**——独立渲染浪费横向空间，也违反"一个语义概念 = 一个控件"原则。

## 决策

1. **形态**:单 chip + popover，对标 `ModeSelect` 模板。OFF 灰 chip，ON accent chip + chevron。
2. **OFF 时 plugin 列表保留但灰显 + 不可点击**:让用户能看到有哪些 plugin 可选（onboarding 信号）。如果隐藏,ON 切换后还要再点一次 popover 才看到 dev plugin,体验更差。
3. **toggle 行**:`ModeSelect` 的 popover 是菜单项数组;本任务用「label + 右侧 switch/checkbox 形态」的异质行(top row = toggle,bottom rows = plugin 列表)。用一个小 `<button role="switch" aria-checked>` 表达 toggle 即可,不必引新组件库。
4. **不引入 TriggerMenu**:`ModeSelect` 自己手写了 popover(`popover-pattern.md` spec 里有),保持一致。
5. **不动 store / IPC / 后端**:`requestSetWorkflowEnabled` 和 `requestSetPluginName` 是两个独立 IPC,popover 内部两个 UI 元素只是绑到同一个 store action。

## 影响范围

| 文件 | 操作 |
|---|---|
| `app/src/components/chat/WorkflowToggle.vue` | 整文件删除 |
| `app/src/components/chat/PluginSelect.vue` | 改:去 `v-if` 的 `workflowEnabled` 分支、加 toggle 行、改 chip 形态 |
| `app/src/components/chat/ChatInput.vue` | 删 `<WorkflowToggle />` import + 用法,保留 `<PluginSelect />` |

无 test 文件(确认过 `WorkflowToggle.test.ts` / `PluginSelect.test.ts` 均不存在),无 store / 后端改动。

## Acceptance Criteria

- [ ] **OFF 状态**:单 chip `Wf ▾`(ghost 形态),点击弹 popover,popover 内 toggle 在 OFF 位置、plugin 列表灰显但可见
- [ ] **ON 状态**:单 chip `Wf · dev ▾`(accent),点开 popover,toggle 在 ON 位置、plugin 列表可选、当前 plugin 带 check
- [ ] **点 toggle 行**:chip 立即 flip ON↔OFF(同当前 toggle 语义),plugin 列表立即可点/灰显
- [ ] **点 plugin 行**:chip 立即切到新 plugin 名 + 选中项带 check + popover 关闭
- [ ] **outside-click / Esc 关闭 popover**:与 ModeSelect 行为一致
- [ ] **streaming 期间可点击**:与 ModeSelect 一致(chip 立即 flip,IPC 在下个 turn 边界生效)
- [ ] **vitest 跑通**:`pnpm -C app test` 不因引用 `WorkflowToggle.vue` 失败
- [ ] **type-check 跑通**:`pnpm -C app build` 不报引用错误
- [ ] **WorkflowToggle.vue 已删**,全仓 grep 不到 WorkflowToggle 字样(除文档/历史 commit)