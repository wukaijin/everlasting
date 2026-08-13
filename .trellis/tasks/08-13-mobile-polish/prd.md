# S6 移动端体验打磨:主聊天视图 + 消息流 + Settings 面板(承接 S5 推后项)

## Goal

在 S5 "可用" 基础上,把手机 PWA 推到"好用 + 看着舒服"。覆盖 S5 推后的 V2 项 + 实测截图发现的 30 个具体痛点(见「痛点清单 SoT」)。

**适配宽度范围:320px – 430px**(iPhone SE 320 / 标准 Android 360 / iPhone 14/15 Pro 393 / Pixel 7 412 / iPhone Pro Max 430)。S5 的 `<768px` 单断点不够:等比缩放塞进 360px 以下会崩,需要"窄屏再降级"策略(360px 以下折叠次要元素,主操作占最大可点区域)。

## 决策记录(已确认)

| # | 决策 | 影响 |
|---|------|------|
| `DEC-1` | **任务拆 2 个独立可验收子任务**:S6a 主聊天视图+消息流 / S6b Settings 面板 | 见「任务拆分」 |
| `DEC-2` | **worktree 手机端(<768px)彻底隐藏** —— cwd chip + attach worktree 按钮全部不渲染,不做抽屉入口(手机无 attach 场景,需回 PC) | S6a,解 A1/A2/D1/D2/D4/D5 |
| `DEC-3` | **底部状态条拆半** —— 模型选择保留并移入输入区;LLM 延迟 + token 用量手机端隐藏或折叠成可点小圆点;Edit/wf 标签保留但缩小 | S6a,解 A6/A7/D6/D7 |
| `DEC-4` | **Settings tab 横向可滚动 + 背景 pill 高亮** —— `overflow-x:auto` + 隐藏滚动条 + 边缘渐变提示;当前 tab 从下划线改背景 pill | S6b,解 B1/B2/B3 |
| `DEC-5` | **断点策略** —— 沿用 S5 `@media (max-width: 767px)` 叠加约定,新增 `<360px` 再降级断点,不引入 CSS 变量/多档 ladder | 全部 |
| `DEC-6` | **44px 只给主操作,chip 保持紧凑** —— 触摸目标 44px 仅限主操作(发送/停止、header 4 图标按钮、悬浮↓、modal 按钮);紧凑 chip(ModeSelect/PluginSelect/`--chip` 家族)保持 ~22-25px 不拉高,靠缩小 padding 省空间。理由:chip 是低点击频率的状态标签,拉高 44px 会把输入行/标题行重新挤回 A6/D6。验收解读:"Edit/wf 可点" = chip 可见可点,不要求 44px | S6a + S6b 共 |

## 任务拆分

```
08-13-mobile-polish/                    (S6 parent,本文件 = 痛点 SoT + 决策 SoT)
  ├── 08-13-mobile-chat-view/   S6a: A1-A7, C2-C3, D1-D9(落地窄屏降级)
  └── 08-13-mobile-settings/    S6b: B1-B11
```

- S6a 与 S6b **无依赖,可并行**(改动面互不相交:ChatPanel/ChatInput/消息流 vs SettingsModal + 6 tab)
- parent 只做范围裁决与跨子任务验收,不直接实现
- 横切约束:两子任务对齐同一断点方案(DEC-5),跨子任务验收见下

## 痛点清单 SoT

> 每条 = 截图证据 + 代码锚点(详见对应子任务 prd 的 Notes)。A/B/C/D 编号稳定,子任务按组认领。

### A 组 · 主聊天视图布局/适配

- `A1` 顶部 header 双层堆叠 —— 抽屉/通知 + worktree 路径 + attach worktree 挤一行
- `A2` worktree 选择器手机端没场景
- `A3` 代码折叠块信息密度低 —— 展开后几乎空内容占满一屏
- `A4` `Thought for 4.7s` 位置违和 —— 像系统日志混进对话正文
- `A5` 大段纯文本无视觉层次 —— 字号/行高小屏太密,缺段落间距
- `A6` 底部输入区 `Edit/wf` + 占位文字 + 发送按钮挤一行,占位文案换行奇怪
- `A7` 底部状态条 `LLM 47.5s 13.6K · 1% / 1M deepseek-v4-flash` 太技术,像报错

### B 组 · Settings 面板(手机宽度下)

- `B1` 顶部 tab 全平铺挤死 —— 6 tab 字压成一团
- `B2` 缺水平滚动提示 —— 用户不知 Subagents/Remote 在屏外
- `B3` 当前 tab 仅下划线高亮 —— 视觉反馈太弱
- `B4` 关闭 × 在右上角 —— 全屏弹窗应语义化(`Done` / `← 返回`)
- `B5` 整体未做移动端适配 —— 只有容器全屏化兜底
- `B6` Provider 卡片标题断行丑 —— `Carlos-Api-OpenAI` 拆三行
- `B7` 卡片内信息密度过高 —— 徽章+URL+状态+操作图标挤一行,URL 截断
- `B8` `已加密保存` 系统状态文字占空间
- `B9` 模型条目字号层级混乱 —— name(大)+ id(小)+ thinking 徽章紧挨
- `B10` 分组标题 `CARLOS-API-XXX` 对比度低
- `B11` 选中状态视觉反馈不足 —— 圆点+边框变化对比度不够

### C 组 · 宏观设计

- `C1` ~~单栏后缺 tab bar~~ —— **已验证不需要**:S5 design 明确决策改抽屉式放弃 tab bar(`archive/2026-08/08-11-mobile-adaptation/design.md:119-121`),全仓无 tab bar 组件
- `C2` 悬浮 ↓ 按钮 —— 与滚动条区域重叠,易误触
- `C3` 气泡/背景对比度不足 —— 深色主题下气泡和背景几乎没区别

### D 组 · 窄屏再降级(320px 附近极端情况)

- `D1` 窄屏下 header 变三层 —— `flex-wrap:wrap` 导致标题/路径/工具栏三层堆叠
- `D2` worktree 路径窄屏截断无意义 —— `.../precauti` 信息价值 = 0
- `D3` 图标按钮无文字标签 —— `ⓘ / 🛡 / 📈 / 🔧` 只靠图标不可辨识
- `D4` `新对话` 标题被挤到行首贴边 —— 层级感全无
- `D5` `attach worktree` 独占一行 —— 挤压主交互区
- `D6` 窄屏下 `Edit/wf` 两个标签抢 1/3 横向 —— 主操作输入框被挤到右侧 1/3
- `D7` 底部状态条占一整行 —— 三件工具栏吃掉纵向空屏一半
- `D8` 空状态本身 OK 但被挤压 —— `💬 开始对话` 可见区域只剩中间一小块
- `D9` 缺"窄屏降级"策略 —— 360px 以下无规则
- `D10` ~~窄屏降级范围待明确~~ —— **已由 DEC-2/DEC-3/DEC-5 解决**:worktree 隐藏、状态条拆半、Edit/wf 缩小、断点对齐

## Acceptance Criteria

**跨子任务(全部通过才算 S6 完成):**

- [ ] 所有 UI 在 320px / 360px / 393px / 430px 四个宽度下无横向滚动 / 无元素裁剪到不可用
- [ ] 360px 以下自动启用"窄屏降级"策略(次要元素折叠)
- [ ] 桌面(≥768px)布局回归零变化(由 S5 的 desktop-first overlay 结构保证,子任务各验收再验证)

**子任务各自验收**见 S6a / S6b prd。

## Notes

- 起点是 S5 验收后的实测手机宽度截图问题清单,不是凭空发散
- 断点对齐 S5 约定:`@media (max-width: 767px)` desktop-first overlay(spec: `frontend/responsive-mobile.md`)
- 组件命名/样式约定:scoped CSS + CSS 变量、BEM 风格、44px 触摸目标、reka-ui portal 陷阱(`!important` 只用于覆盖第三方且集中 style.css)
