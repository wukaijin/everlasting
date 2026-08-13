# S6a Design — 主聊天视图 + 消息流移动端打磨

> 对应 prd:`./prd.md`。本文件只写技术设计:改动边界、数据流、兼容性、取舍。

## 1. 架构与边界

**不做结构重构**。所有改动是"桌面为基线 + `@media (max-width: 767px)` 覆盖 + 新增 `<360px` 降级断点",不编辑任何桌面样式块(结构保证桌面零回归,spec:`frontend/responsive-mobile.md`)。

改动范围(全部 `app/src/`):

| 文件 | 改动 |
|---|---|
| `components/chat/ChatPanel.vue` | header 瘦身:移动端隐藏 cwd chip + WorktreeChip + git chip;标题行不再 wrap;4 图标按钮 touch 目标 44px + 紧凑间距 |
| `components/chat/ChatInput.vue` | 输入行重排:移动端 Edit/wf 标签缩小、输入框占主宽度;状态条折叠(只留模型选择) |
| `components/chat/ChatInputHintRow.vue` | 移动端隐藏 LLM 延迟 + token chip,保留 ModelSelect |
| `components/chat/MessageList.vue` | 悬浮 ↓ 按钮移动端避让/隐藏 |
| `components/chat/ThinkingBlock.vue` | 移动端紧凑样式(间距/字号) |
| `components/chat/ToolOutputBody.vue` + `ToolInputBody.vue` | 折叠块 summary 移动端紧凑 + 横向滚动容器 |
| `components/chat/MessageItem.vue` | 气泡间距/对比度微调(仅移动端) |
| `style.css` | 全局 Dialog 全屏化块旁追加:窄屏降级断点常量说明 + 移动端 `WordBreak` 全局规则(长 token/URL 不撑破布局) |

## 2. 断点方案(DEC-5)

- 沿用 `@media (max-width: 767px)` desktop-first overlay(S5 约定)。
- **新增 `<360px` 窄屏降级断点**:`@media (max-width: 359px)`(与 767 正交,只用于"隐藏次要元素"级降级)。不放 CSS 变量、不做多档 ladder(S5 V2 项,本任务只加一档)。

## 3. 各改动点设计

### 3.1 ChatPanel header 瘦身(A1/A2/D1/D2/D4/D5)

**模板层**——给现有元素加移动端隐藏,不改桌面渲染:

```vue
<span v-if="chatStore.simplifiedCwd" class="chat-panel__chip chat-panel__chip--cwd mobile-hide-cwd">…</span>
<WorktreeChip v-if="showWorktreeChip" … class="mobile-hide-worktree" />
```

- cwd chip:加 class `mobile-hide-cwd` → `@media (max-width: 767px) { .mobile-hide-cwd { display: none } }`
- WorktreeChip:在 `@media (max-width: 767px)` 内 `display: none`(WorktreeChip 根节点有 class `worktree-chip`,覆盖用 `:deep(.worktree-chip)` 或直接对组件根节点 class——Vue 组件根节点继承 class,scoped 下用 `:deep()`)
- git chip:同样隐藏(它也是 PC 向信息)
- 标题行 `flex-wrap`(ChatPanel.vue:1112-1119):移动端改 `flex-wrap: nowrap` + 标题 `text-overflow: ellipsis; overflow: hidden; white-space: nowrap`,标题不再被挤到行首贴边(D4 解)
- 4 图标按钮(memory/audit/trace/grants):移动端 44×44 touch 目标、gap 收紧;不改图标本身

**不动**:`showGitChip` / `showWorktreeChip` 的 script 逻辑(桌面还要用),只 CSS 隐藏。

### 3.2 底部输入区重排(A6/D6)

- `ChatInput.vue:721-731` 的 `.chat-input__row`(flex,gap 8px,padding 6px 6px 6px 14px):
  - 移动端:ModeSelect / PluginSelect chip 缩小(gap 4px、font-size 保持 xs、padding 收紧)
  - 输入框(flex:1)在窄屏获得主宽度
- **不做"标签折叠进 + 菜单"**(DEC-3 明确保留 Edit/wf,只缩小)

### 3.3 状态条折叠(A7/D7/DEC-3)

- `ChatInputHintRow.vue`:
  - 移动端隐藏 `ChatInputLatencyPopover`(LLM 延迟)与 token usage chip(两者都是开发者调试信息)
  - `ModelSelect` 保留 —— 但 ModelSelect 在 hint row 内,`justify-content: space-between` 会把它推到最右;移动端改 `justify-content: flex-end`(或让 ModelSelect 上移到 ChatInput 主行内——**倾向保留在 hint row 不动位置**,减少结构改动)
  - 若隐藏后整行只剩 ModelSelect,提示行高度塌缩 → 移动端 `margin-top: 4px` 收紧
- **取舍**:模型选择仍在下拉 popover 内,占位文案不换行;只处理行内布局

### 3.4 消息流组件(A3/A4/A5/C2/C3)

- **ToolOutputBody / ToolInputBody**(A3):
  - summary 行移动端紧凑(`padding` 收紧)
  - 内容容器加 `overflow-x: auto`(代码/长行横向滚动不撑破)
- **ThinkingBlock**(A4):
  - 移动端 `padding` 收紧、label 字号保持
  - **位置不动**(不动 MessageItem 挂载逻辑 —— 那是"thinking 状态指示器"的语义问题,V2 范畴;本任务只做紧凑)
- **MessageList 悬浮↓**(C2):
  - 移动端 `right: 8px; bottom: 64px`(避让滚动条+输入区)或 `display: none`
  - **取舍**:保留但下移(用户仍需要"跳到底部"能力,尤其流式输出时) — 决定:**保留 + 下移避让**
- **MessageItem 气泡**(C3):
  - 移动端微调气泡间距(`margin-bottom`)、正文行高(line-height 提到 1.6)
  - 对比度:不动设计 token(改全局配色风险大),只让"段落间距 + 行高"拉开视觉层次

### 3.5 窄屏降级(<360px)(D8/D9)

- 360px 以下追加隐藏:4 图标按钮里**非高频**的(audit/trace)折叠进…**不做**——DEC-2/3/5 的隐藏规则已解决主占位问题,360px 断点只做:
  - `.chat-panel__title` 更小字号
  - 消息气泡 padding 再收紧
- **简化**:`<360px` 断点不是新交互,只是 A 组已有改动的强化档

## 4. 兼容性 / 回归

- 桌面块零改动 → 桌面零回归(结构保证)
- `WorktreeChip` / `ModelSelect` / 折叠块等**桌面行为不变**,只移动端 CSS 叠加
- reka-ui portal 元素(Tooltip/Popover)不受 scoped 影响,不需要 `!important`(本任务不改 portal 内样式)
- 断点对齐:S6b 的 Settings tab 用同一 767px 约定(两子任务并行,互不重叠文件)

## 5. 风险

| 风险 | 缓解 |
|---|---|
| `<360px` 断点与 767px 规则叠加产生特异性冲突 | 只用单类选择器,360px 规则写在 767px 规则之后(后者优先,天然覆盖) |
| ChatPanel header 隐藏 worktree 后,`title` 行布局偏移 | 用 `flex-wrap: nowrap` + ellipsis,标题行稳定 |
| 悬浮↓ 下移后与输入区重叠 | `bottom: 64px` 显式避让 safe-area + 输入区高度 |
| 移动端隐藏 WorktreeChip 的 CSS 选择器命中组件根节点 | 用 `:deep(.worktree-chip)`(scoped 下组件根 class 继承) |
