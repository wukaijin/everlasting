# Step 0 移动端冒烟基线

> 按 [design §5](../design.md) 冒烟策略:代码静态分析(design §1,已完成 + review 核验)+ 真机 iOS 验证(implement 阶段 Step 9 + 用户配合)。
> **headless 浏览器冒烟跳过** —— design §5.2 已论证增量价值低,review 认可。本环境另有限制:无 daemon 后端,`/chat` 的 Sidebar(核心挤死场景,`v-if="showSidebar"` 依赖 `currentProjectId`)不渲染。

## 代码分析问题清单(= design §1 摘要,全部经 review 逐条核验属实)

### 1. 布局挤死(核心病灶)
- `Sidebar.vue:188` `width:260px; flex-shrink:0` —— 375px 屏占 69%,main 仅剩 115px
- `ProjectTabs.vue:212` 单 tab `min-width:100px` —— 375px 屏只容 ~3 个 tab,靠 `overflow-x:auto` 横滚
- AppHeader 40px 高 + Sidebar 260px 是桌面常量,窄屏不退让

### 2. Dialog 横向溢出(5 个硬 bug,min-width > 视口)
| 组件 (DialogContent 根类) | min-width 行 | 值 |
|---|---|---|
| PermissionGrantsModal (`.grant-modal`) | :159 | 560px |
| MarkdownDetailModal (`.markdown-detail-modal`) | :212 | 640px |
| RuntimeMemoryModal (`.runtime-memory-modal`) | :518 | 640px |
| MemoryModal (`.memory-modal`) | :100 | 640px ← review 补,原 design 漏 |
| AuditLogModal (`.audit-modal`) | :329 | 640px ← review 补,原 design 漏 |

(另 2 个手搓 modal YoloConfirmModal / ConfirmDialog 本就 `width:100%`,不溢出)

### 3. 断点缺失
- `style.css` 全文唯一媒体查询 = `prefers-reduced-motion`(@191),零响应式断点
- `@theme`(@22)无 `--breakpoint-*`;组件 template 零 Tailwind utility class
- S5 首次引入断点,768px 单分界,原生 `@media` + desktop-first 叠加

### 4. ChatInput(CodeMirror 6)
- host `min-height:28px / max-height:200px / line-height:1.5`,**未设 `font-size:16px`**(iOS 会自动缩放)
- 工具栏按钮 `height:32px`(< Apple HIG 44px)
- `ChatPanel.vue:856` ChatInput 是 flex 列底部常驻成员 —— `position:sticky` 是 no-op(review P2-2)

### 5. 其他
- `index.html` viewport meta 缺 `viewport-fit=cover` → `env(safe-area-inset-*)` 恒 0(review P2-3)
- 全仓零 `env(safe-area-inset-*)` 使用
- `--duration-normal` token 不存在(style.css 实际:instant/fast/base/slow/pulse/blink/modal-in/out)—— 原 design 编造,review P2-1
- TracePanel `z-index:100`(`TracePanel.vue:227`)—— 抽屉要避开(review P3-2)

## 真机 iOS 验证待办(Step 9 验收 + 改造中段配合)

改造完成后,真机 iPhone Safari 连 dev server 走一遍:
- [ ] 配对码 → 节点列表 → 进会话 → 发消息 → 流式输出可见
- [ ] 软键盘弹起:输入框不被挡(`useMobileKeyboard` visualViewport 生效)
- [ ] CodeMirror:中文 IME 正常 + 长按选词不异常
- [ ] viewport:URL bar 收起/展开,`dvh` 随之伸缩
- [ ] safe-area:Home Indicator 不压输入框,刘海不顶抽屉顶部
- [ ] 7 个 reka-ui Dialog 移动端全屏化,无横向滚动条
- [ ] 抽屉:汉堡开 / 遮罩关 / 选 session 自动关 / 无项目时汉堡隐藏
- [ ] 桌面(≥768px)零回归(Step 8 截图对比)

## headless 冒烟跳过的理由(design §5.2)

- PRD 点名 3 个高风险项(CodeMirror iOS 触摸 / 软键盘 / viewport dvh bug)都是 **iOS-specific**,headless Chromium 测不了
- 布局挤死代码分析已精确到像素值 + 行号,headless 截图不会更精确
- 本环境无 daemon 后端,`/chat` 的 Sidebar(核心挤死场景)不渲染,headless 看不到
- 真机是唯一能验证 iOS 行为的手段,留给 Step 9 + 用户 iPhone 配合
- Step 1-7 是纯 CSS / 少量组件结构改造,依据是代码分析(已扎实),不依赖运行时冒烟结果
