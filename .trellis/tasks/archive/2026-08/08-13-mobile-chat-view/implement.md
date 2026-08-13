# S6a Implement — 主聊天视图 + 消息流移动端打磨

> 对应 prd + design。执行顺序按改动面从小到大,每步可独立验证。

## 实施清单(有序)

### 1. ChatPanel header 瘦身(A1/A2/D1/D2/D4/D5)

- [ ] `ChatPanel.vue` 模板:cwd chip / WorktreeChip / git chip 各加移动端隐藏 class
- [ ] `ChatPanel.vue` CSS:`.chat-panel__title-row` 在 767px 内 `flex-wrap: nowrap`;标题 `ellipsis`;cwd/worktree/git chip `display: none`
- [ ] 4 图标按钮在 767px 内 touch 目标 44×44 + gap 收紧

### 2. 底部输入区重排(A6/D6)

- [ ] `ChatInput.vue` `.chat-input__row` 移动端:chip 缩小(gap/padding),输入框占主宽
- [ ] 确认占位文案不再换行(CM placeholder 由 `white-space` 控制)

### 3. 状态条折叠(A7/D7)

- [ ] `ChatInputHintRow.vue` 移动端:隐藏 latency chip + token chip,保留 ModelSelect,行高收紧
- [ ] 验证空状态 + 有会话两种态下底部都不占整行

### 4. 消息流组件(A3/A4/A5/C2/C3)

- [ ] `ToolOutputBody.vue` / `ToolInputBody.vue`:summary 紧凑 + 内容 `overflow-x: auto`
- [ ] `ThinkingBlock.vue`:移动端 padding 收紧
- [ ] `MessageList.vue` 悬浮↓:移动端 `right: 8px; bottom: 64px` 避让
- [ ] `MessageItem.vue`:移动端气泡间距 + 行高 1.6

### 5. 窄屏降级(<360px)(D8/D9)

- [ ] `@media (max-width: 359px)` 档:标题字号缩小、气泡 padding 收紧
- [ ] 若 360px 以下仍有横向溢出元素,记录并处理(长 token 全局 WordBreak 兜底)

## 验证命令

```bash
cd app
pnpm test            # 全量 Vitest(移动端 CSS 改动不涉逻辑,但跑一遍防回归)
pnpm build           # vue-tsc --noEmit + vite build(类型 + 构建)
```

## 验证检查单(dev server + DevTools 移动模拟)

- [ ] 320 / 360 / 393 / 430px 四宽度下:无横向滚动、无元素裁剪、无溢出
- [ ] 手机宽:header 无 worktree 路径/attach worktree;标题完整显示
- [ ] 输入框占主宽;Edit/wf 缩小但可点;模型选择可见
- [ ] 状态条不占整行(延迟/token 已折叠)
- [ ] 折叠块 summary 紧凑、长代码横向滚动不撑破
- [ ] 悬浮↓ 不挡滚动条
- [ ] 消息气泡在 320px 可读(段落间距/行高)
- [ ] 桌面(≥768px)逐个对比:header 无缺失、输入区布局不变、状态条完整

## 冒烟脚本

手动走:空状态 → 发消息(流式)→ 折叠块展开 → 悬浮↓ 滚动。手机宽度 + 桌面宽度各一遍。

## 风险回滚点

- 若移动端隐藏 worktree 影响 `title` 行布局 → 回退 `flex-wrap: nowrap`,只隐藏 chip
- 若 `<360px` 档引入特异性问题 → 删 359px 块,保留 767px 规则(先保 393/430 主流宽度)

## 收尾

- [ ] 跑 `pnpm test` + `pnpm build` 全绿
- [ ] 桌面回归截图对比(零变化)
- [ ] 更新 spec:`frontend/responsive-mobile.md` 补"窄屏降级 + 隐藏类命名"约定
