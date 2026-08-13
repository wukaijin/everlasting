# S6a 主聊天视图 + 消息流打磨:header / 输入区+状态条 / 消息流组件 / 窄屏降级

> parent(S6 总任务)见 `../08-13-mobile-polish/prd.md`(痛点 SoT)。本任务细化执行。

## Goal

在 S5 "可用" 基础上,把手机端主聊天视图推到"好用 + 看着舒服":
- 顶部 header 瘦身(去掉 PC 向的 worktree 元素)
- 底部输入区 + 状态条重新布局(主操作占最大可点区域)
- 消息流组件(代码折叠块 / Thinking / 悬浮↓ / 气泡对比度)移动端可读
- 落地"窄屏再降级"策略(<360px 折叠次要元素)

**适配宽度范围:320px – 430px**(S5 的 `<768px` 单断点不够,需要 360px 以下再降级)。

## 痛点清单(A/C/D 组,来自 parent SoT)

### 主聊天视图(本次处理)

- `A1` **顶部 header 双层堆叠** —— 抽屉/通知 + worktree 路径 + attach worktree 按钮挤一行,worktree 元素挤掉功能按钮
- `A2` **worktree 选择器在手机端没场景**
- `D1` **窄屏下 header 变三层** —— `flex-wrap: wrap` 导致标题/路径/工具栏三层堆叠
- `D2` **worktree 路径窄屏截断无意义** —— `.../precauti` 只占行宽,信息价值 = 0
- `D3` **图标按钮无文字标签** —— `ⓘ / 🛡 / 📈 / 🔧` 只靠图标,不可辨识
- `D4` **`新对话` 标题被挤到行首贴边** —— 层级感全无
- `D5` **`attach worktree` 独占一行** —— 挤压主交互区

### 输入区 + 状态条(本次处理)

- `A6` **底部输入区 `Edit/wf` 标签 + 占位文字 + 发送按钮挤一行** —— 占位文案换行奇怪;Edit/wf 占触控空间
- `A7` **底部状态条 `LLM 47.5s 13.6K · 1% / 1M deepseek-v4-flash` 太技术**
- `D6` **窄屏下 `Edit / wf` 两个标签抢 1/3 横向** —— 主操作输入框被挤到右侧 1/3
- `D7` **底部状态条占一整行** —— header + 输入框 + 状态条三件工具栏吃掉纵向空屏一半

### 消息流组件(本次处理)

- `A3` **代码折叠块(input/output)信息密度低** —— 展开后几乎空内容,占满一屏
- `A4` **`Thought for 4.7s` 标识位置违和** —— 像系统日志混进对话正文
- `A5` **大段纯文本段落无视觉层次** —— 字号/行高在小屏太密,缺段落间距
- `C2` **悬浮 ↓ 按钮** —— 与滚动条区域重叠,易误触
- `C3` **气泡/背景对比度不足** —— 深色主题下气泡和背景几乎没区别

### 窄屏再降级(本次落地)

- `D8` **空状态本身 OK 但被挤压** —— `💬 开始对话` 可见区域只剩中间一小块
- `D9` **缺"窄屏降级"策略** —— 360px 以下无规则,需要明确哪些元素隐藏/保留

## 决策记录(已确认)

- `D10` **worktree 在手机端(<768px)彻底隐藏** —— cwd chip + attach worktree 按钮全部不渲染,不做抽屉入口(手机用户无 attach 场景,需回 PC 操作)
- **状态条拆半** —— 模型选择(`ModelSelect.vue`)保留并移入输入区;LLM 延迟 + token 用量在手机端隐藏或折叠成可点小圆点;Edit/wf 标签保留但缩小
- **断点策略** —— 沿用 S5 的 `@media (max-width: 767px)` 叠加约定;新增 `<360px` 再降级断点;断点不引入 CSS 变量(与 S5 一致)

## 验收标准

- [ ] 320px / 360px / 393px / 430px 下无横向滚动 / 无元素裁剪到不可用
- [ ] 手机端(<768px)ChatPanel header 不再出现 worktree 路径和 attach worktree
- [ ] 底部输入框占主宽度,Edit/wf 标签缩小不抢 1/3 横向;模型选择可见可点
- [ ] 状态条 LLM 延迟 / token 用量在手机端隐藏或折叠(不占整行)
- [ ] 消息流:代码折叠块/Thinking/消息气泡在 320px 可读,无横向撑破
- [ ] 360px 以下自动启用窄屏降级(worktree 已隐藏、状态条折叠)
- [ ] 主操作(发送/停止、header 4 图标、悬浮↓)移动端 44px 触摸目标;紧凑 chip(Edit/wf/群聊/git)保持紧凑可点(DEC-6)
- [ ] 桌面(≥768px)布局回归零变化

## Notes

- **强依赖 S5** 的断点基线 + 抽屉导航(已归档)
- **CSS 优先** —— 桌面样式是基线,移动端全部放 `@media (max-width: 767px)` 内,不改桌面块,结构保证桌面零回归(spec: `frontend/responsive-mobile.md`)
- 代码位置锚点:
  - ChatPanel header: `app/src/components/chat/ChatPanel.vue:604-747`(`flex-wrap` 在 `:1112-1119`;worktree chip `:638-645`;4 图标按钮 `:666-735`)
  - WorktreeChip: `app/src/components/chat/WorktreeChip.vue:167-255`(无任何移动端处理)
  - 输入区: `app/src/components/chat/ChatInput.vue:721-731`(布局行);Edit 标签 `ModeSelect.vue` / wf 标签 `PluginSelect.vue:228-240`
  - 状态条: `app/src/components/chat/ChatInputHintRow.vue:82-145`(LLM 延迟 / token 用量);模型选择 `ModelSelect.vue`
  - 折叠块: `app/src/components/chat/ToolOutputBody.vue:62-87` + `ToolInputBody.vue`
  - Thinking: `app/src/components/chat/ThinkingBlock.vue:60-84`,挂载 `MessageItem.vue:296-324`
  - 悬浮↓: `app/src/components/chat/MessageList.vue:268-277`(CSS `:374-402`)
- 测试: `cd app && pnpm test`(Vitest,`*.test.ts`);`pnpm build`(vue-tsc + vite)
- **不做的事**:触屏手势、pull to refresh、紧凑消息视图 V2、移动端专属导航(均 S5 推后,V2 范畴)
- 验收前手机宽度冒烟用 Chrome DevTools 移动模拟 + 真机,记录挤死/挡死/不可点的点,作为本任务检查清单
