# S5 移动端中限适配:单栏 tab 切换 + chat 输入框/弹窗移动端友好

> 架构决策见 [parent PRD](../08-11-remote-control-epic/prd.md)。本任务只细化执行。

## Goal

S4 做完后手机 PWA 技术上可用,但布局是为 PC 大屏设计的三栏(projects / sessions / chat 并排),手机宽度下挤死不可用。本任务做**中限适配**:三栏 → 移动端单栏 tab 切换 + 关键交互(输入框、弹窗)移动端可点。

**不做原生体验**(触屏手势、紧凑视图重设计、移动端专属导航)—— 那是 V2。本任务只让手机**真正可用**,不追求**好用**。

## Scope

先探查现有三栏布局结构(实施时读 `AppShell.vue` / 主 layout),再定具体改造点。预期改造:

### 响应式断点

- 桌面(≥768px):现有三栏布局不变
- 移动(<768px):单栏 + 顶部 tab 切换

### 三栏 → 移动端单栏 tab

参考 Slack/Discord/GitHub 移动 web 模式:

- 移动端顶部加 tab bar:`项目` / `会话` / `对话`
- 同一时间只显示一栏全屏
- 切 session 时自动跳到"对话" tab(点击 session item 触发)
- tab 状态持久化(切回时恢复)

### Chat 输入框移动端适配

- CodeMirror 编辑区移动端可点可输入(检查现有 CodeMirror 6 移动端行为)
- 输入框区域固定底部 + 软键盘弹起时不挡(iOS Safari viewport 处理)
- 工具栏(@文件 / /命令 / 发送)触摸友好(按钮够大)

### 弹窗移动端适配

现有弹窗(reka-ui Dialog):PermissionModal / AskUserQuestionCard / YoloConfirmModal / ConfirmDialog 等。

- 移动端弹窗居中全屏(而非桌面的小卡片)
- 按钮触摸友好
- 阻止背景滚动

### 流式内容移动端可读

- chat 消息区移动端字号/行距可读
- markdown 代码块横向滚动(不撑破布局)
- tool_use / tool_result 卡片移动端紧凑显示

## 依赖

- **强依赖 S4**(在能用的基础上做适配)

## 验收标准

实施前先做一次手机宽度冒烟(用 Chrome DevTools 移动端模拟 + 真机),记录所有挤死/挡死/不可点的点,作为本任务清单。预期验收:

- [ ] 手机宽度(<768px)下:三栏不并排,改成单栏 + 顶部 tab 切换,每栏全屏可用
- [ ] 切 session → 自动跳"对话" tab
- [ ] chat 输入框:移动端可点可输入,软键盘弹起输入框可见不被挡
- [ ] 发消息 → 看到流式输出,markdown 代码块横向滚动不撑破
- [ ] permission:ask / ask_user_question 弹窗在手机上居中全屏,按钮可点
- [ ] tool_use / tool_result 卡片手机宽度下可读
- [ ] 桌面(≥768px)布局回归零变化

## Notes

- **不做的事**(Q11 推后):触屏手势(swipe 切 session)、pull to refresh、紧凑消息视图、移动端专属导航、底部 tab bar 动效
- 家里电脑浏览器(大屏)不需要本任务 —— 它用现有三栏布局,零适配
- 实施前必须先做"手机宽度冒烟"记录问题清单,避免盲目改造
- CSS 优先(媒体查询),避免改组件结构;只在必要时加移动端专属组件
- iOS Safari 是主要测试目标(你的主力手机是 iPhone);Android 国产浏览器次要测(都是 webkit/blink 内核,CSS 兼容性差异小)
