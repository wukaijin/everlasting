# S5 移动端中限适配 — 技术设计

> 需求见 [PRD](./prd.md),架构决策见 [parent PRD](../08-11-remote-control-epic/prd.md)。
> 本文档只细化技术方案:现状分析(代码证据) + 改造决策 + 逐组件改造点 + 风险 + 冒烟策略。

## 0. 范围与不变量

**做什么**:让手机宽度(<768px)下 PWA 可用——三区(项目/会话/对话)从并排改成轮流全屏,关键交互(输入框、弹窗)触摸友好,代码块/卡片不撑破布局。

**不做什么**(对齐 PRD §Notes,Q11 推后项):触屏手势、pull to refresh、紧凑消息视图、底部 tab bar 动效、移动端专属导航重设计、原生体验。桌面(≥768px)布局零变化(回归不变量)。

**关键不变量**:
1. **桌面零回归**——所有改造在 `@media (max-width: 767px)` 内,桌面样式块一字不动。
2. **CSS 优先**——能用媒体查询 + 现有 CSS 变量解决的不改组件结构;只在交互模式必须变时(如 Sidebar 抽屉化)才动 template/state。
3. **不引入 Tailwind utility class**——全仓组件统一 `<style scoped>` + CSS 变量风格(见 §2.1 决策)。

---

## 1. 现状分析(代码证据)

### 1.1 布局结构现状

实际不是 PRD 字面说的"三栏并排",而是 **顶部 tab + 左右两栏**:

```
┌──────────────────────────────────────────────┐
│ AppHeader (40px)                             │  <- flex-shrink:0
│  BrowserHeader/TitleBar                      │     isTauriWebview()?Title:Browser
│   └ ProjectTabs (横向 overflow-x:auto)       │     移动端 PWA 必走 BrowserHeader
│      + HiddenProjectsMenu + PendingBadge     │     (pwa-remote 模式 isTauri=false)
├──────────────┬───────────────────────────────┤
│ Sidebar      │ main (slot)                   │  <- .app-shell__body { display:flex }
│ width:260px  │  flex:1                       │
│ flex-shrink:0│  ChatView/ChatWindow          │
│              │                               │
│ SESSIONS 头  │                               │
│ SessionList  │                               │
│ 设置 footer  │                               │
└──────────────┴───────────────────────────────┘
```

代码位置:
- `app/src/components/layout/AppShell.vue:55-94` — 三段式骨架
- `app/src/components/layout/Sidebar.vue:187-202` — `width: 260px; flex-shrink: 0`
- `app/src/components/layout/AppHeader.vue:47` — `isTauriWebview() ? TitleBar : BrowserHeader`
- `app/src/components/ProjectTabs.vue:186-192` — `.tabs__scroll { overflow-x: auto }`

### 1.2 挤死点量化(375×812 iPhone 尺寸)

| 区域 | 桌面宽度 | 375px 屏实际 | 问题 |
|---|---|---|---|
| Sidebar | 260px 固定 | 260px(占 69%) | 主区被挤剩 115px |
| main | flex:1(剩 ~全部) | ~115px | chat 内容不可读 |
| ProjectTabs 单 tab | min 100 / max 240 | 100px | 375px 只容 3 个 tab,溢出靠横滚 |
| AppHeader 高度 | 40px | 40px | OK |

**结论**:核心病灶是 Sidebar 260px 在窄屏不退让。修它 = 解决大半。

### 1.3 Dialog 横向溢出 bug 清单(现成 bug,S5 须修)

> **review P1-1 修订(2026-08-12)**:全仓 reka-ui Dialog 穷举 + 手搓 modal 比对后,清单从 7 扩到 9,硬 bug 从 3 改 5。原版漏了 `MemoryModal` / `AuditLogModal`。

手机宽度下 `min-width` 超过视口的 Dialog 会横向溢出(横向滚动条 / 内容被裁)。按实现分两类:

**(A) reka-ui Dialog(7 个,DialogContent 根类 = §3.4 全屏化覆盖对象)**

| 组件(DialogContent 根类) | 现有宽度设定 | 375px 屏行为 |
|---|---|---|
| `permissions/PermissionGrantsModal.vue:159-161` (`.grant-modal`) | `80vw; min-width:560px` | **溢出**(560>375) |
| `common/MarkdownDetailModal.vue:212-214` (`.markdown-detail-modal`) | `80vw; min-width:640px` | **溢出** |
| `memory/RuntimeMemoryModal.vue:518-520` (`.runtime-memory-modal`) | `80vw; min-width:640px` | **溢出** |
| `memory/MemoryModal.vue:100-102` (`.memory-modal`) | `80vw; min-width:640px` | **溢出** |
| `audit/AuditLogModal.vue:329-331` (`.audit-modal`) | `80vw; min-width:640px` | **溢出** |
| `settings/SettingsModal.vue:95-96` (`.settings-modal`) | `640px; max-width:calc(100vw-40px)` | 收缩到 335px,挤但可用 |
| `chat/GroupChatConfigModal.vue:509` (`.gcfg-content`) | `min(640px,92vw)` | 自适应,OK |

**(B) 手搓 modal(2 个,非 reka-ui,backdrop + modal div,仅做按钮触摸适配)**

| 组件(根类) | 现有宽度设定 | 375px 屏行为 |
|---|---|---|
| `chat/YoloConfirmModal.vue:194` (`.yolo-confirm-modal`) | `100%; max-width:480px` | 收缩,OK |
| `common/ConfirmDialog.vue:133` (`.confirm-modal`) | `100%; max-width:460px` | 收缩,OK |

**5 个 `min-width` 硬设超视口的 = 硬 bug**(PermissionGrants 560 / MarkdownDetail 640 / RuntimeMemory 640 / Memory 640 / Audit 640),必须在媒体查询里覆盖。原 design 漏 MemoryModal + AuditLogModal —— MemoryModal 是重度可达组件(ChatPanel/TracePanel/RuntimeMemoryModal/PermissionGrantsModal/MemoryPreview/GroupChatConfig/MarkdownDetail 七处引用),移动主流程必达,非边角。

### 1.4 断点现状

`app/src/style.css` 全文只有一个媒体查询(`prefers-reduced-motion`,L191)。**全项目零响应式断点**,S5 是首次引入。Tailwind v4 `@theme` 块已用于颜色 token,但**未定义 `--breakpoint-*`**,且组件 template 里**几乎不用 Tailwind utility class**——都是语义化 class + scoped CSS。

### 1.5 ChatInput(CodeMirror 6)现状

`app/src/components/chat/ChatInput.vue`:
- CodeMirror 6 host:`min-height:28px; max-height:200px; line-height:1.5`(L743-755),auto-grow
- 发送按钮 + 工具栏按钮 `height:32px`(L828)——**低于 Apple HIG 44px 触摸目标**
- 已用 `useChatInputCodeMirror` composable 处理中文 IME(`view.composing`)

---

## 2. 改造决策

### 2.1 断点方案:原生 CSS `@media`,不引入 Tailwind utility

**决策**:用 `@media (max-width: 767px)` 媒体查询,写在各组件 `<style scoped>` 里。**不**引入 Tailwind `md:`/`lg:` utility class,也**不**在 `@theme` 里加 `--breakpoint-*`。

**理由**:
- 现有 40+ 组件统一 `<style scoped>` + CSS 变量风格,template 零 utility class。引入 utility 会在一个任务里制造两套并存的样式范式,增加认知负担。
- S5 的改造是**窄屏覆盖**(在现有桌面样式上叠加移动端样式),媒体查询天然适合"覆盖"语义;utility class 适合从零写布局。
- 不引入新依赖,零 build 风险。

**断点值**:**768px** 单一分界(mobile-first 取 Tailwind/Bootstrap 主流值)。不引入 sm/md/lg 多档——PRD 明确"中限",多档断点是 V2。

**mobile-first 还是 desktop-first**:本项目桌面样式是基线(已存在且不能动),所以用 **desktop-first 叠加**:`@media (max-width: 767px) { /* 覆盖 */ }`。这违反纯 mobile-first 教条,但符合"桌面零回归"不变量——桌面样式块完全不动,只在窄屏叠加覆盖。代价:覆盖时要小心 specificity。

### 2.2 三区 → 单屏轮流:抽屉式(Sidebar overlay)+ main 常驻

PRD §三栏→移动端单栏 tab 写了"顶部加 tab bar(项目/会话/对话),同一时间只显示一栏全屏",并列了 Slack/Discord/GitHub 三个参考。但这三者的实际移动模式不同:

- **Slack/Discord** = 左侧抽屉滑出(默认隐藏,点汉堡展开全屏 overlay,选完收起)
- **GitHub mobile web** = 底部 tab bar(常驻,切换全屏 view)

**决策:采用抽屉式(Slack/Discord 模式),不做顶部/底部 tab bar。** ✅ 已确认(用户 design review 2026-08-12,选"抽屉式(推荐)")

**理由**:
1. **贴合现有结构**:Sidebar 本就是左侧栏,改成"移动端默认隐藏 + 全屏滑出"改动最小;tab bar 方案要新造一个 view-switching 组件 + 三区 visibility 状态机,改动大。
2. **项目 tabs 归位**:桌面把 ProjectTabs 放在 AppHeader 顶部常驻;移动端把它**收进抽屉顶部**(跟 SessionList 一起),不占宝贵的顶部常驻空间。AppHeader 移动端只保留一个汉堡按钮 + 当前项目名/会话名(面包屑)。
3. **"切 session 自动跳对话"自然实现**:抽屉里点 session → 触发 `chatStore.switchSession` → 抽屉关闭 → main 自然显示新会话。无需独立 tab 状态。
4. PRD 点名的三个参考里,Slack/Discord 抽屉占两个,GitHub tab bar 占一个——多数倾向也是抽屉。

**移动端交互**:
```
┌────────────────────────┐
│ ☰  当前会话名      ⋯   │  <- AppHeader(移动端)
├────────────────────────┤
│                        │
│   main(对话/ChatView) │  <- 默认全屏
│   常驻显示              │
│                        │
├────────────────────────┤
│ [CodeMirror 输入框] ➤  │  <- 固定底部
└────────────────────────┘

点 ☰ → Sidebar 从左滑出全屏 overlay:
┌────────────────────────┐
│ 项目 tabs(横向滚动)    │  <- 原本在 AppHeader,移动端挪到这里
│ SESSIONS               │
│  ├ session A (active)  │
│  ├ session B           │
│ 设置                    │
│                        │
│  [点 session 自动收起]  │
└────────────────────────┘
   半透明遮罩(点遮罩也收起)
```

**抽屉状态管理**:Sidebar 已是独立组件,加一个移动端 open 状态。两种放法:
- (a) 状态在 Sidebar 内部 `ref`,AppHeader 通过 store/event 触发
- (b) 状态提到 AppShell(store 或 provide/inject)

倾向 **(b) 提到 AppShell**,因为 AppHeader 的汉堡按钮和 Sidebar 都要读写这个状态,AppShell 是它俩的共同父。用一个新的小 composable `useMobileNav()` 或直接在 AppShell 里 `provide`。具体实现细节留给 implement。

### 2.3 分界策略与桌面回归保护

- 所有移动端样式包在 `@media (max-width: 767px) { }` 内,**绝不**修改桌面样式块。
- 抽屉的 open/close 状态本身不影响桌面——桌面下 Sidebar 始终 `flex-shrink:0; width:260px` 常驻,open 状态被忽略(或 `@media (min-width:768px)` 里强制 `display:flex`)。
- 验收第 7 条"桌面零变化"用 `pnpm dev` 桌面宽度截图对比。

---

## 3. 逐组件改造点

### 3.1 AppShell.vue — 移动端骨架 + 抽屉宿主

- `.app-shell__body` 移动端:`flex-direction` 不变(row),但 Sidebar 变成绝对/固定定位的全屏 overlay。
- Sidebar 移动端:`position: fixed; inset: 0; z-index: 110; transform: translateX(-100%)`(默认隐藏),`.sidebar--open` 时 `transform: none` + 遮罩出现。**z-index 110 高于 TracePanel(100),移动导航抽屉需盖住右侧 Trace 面板**(review P3-2)。
- 加遮罩 `<div class="sidebar-overlay" v-if="mobileNavOpen">`,点击关闭。
- main 移动端:`width: 100%`,占满。
- 抽屉状态(host 在 AppShell,provide 给 AppHeader + Sidebar)。

### 3.2 AppHeader.vue — 移动端汉堡 + 面包屑

- 移动端隐藏 `ProjectTabs`(挪进 Sidebar 抽屉),`HiddenProjectsMenu` / `PendingBadge` 评估是否保留(可能也挪进抽屉)。
- 左侧加汉堡按钮 `☰`(仅 `@media (max-width:767px)` 显示,`display:none` 桌面),点击 toggle `mobileNavOpen`。
- 中间显示当前会话名或项目名(面包屑),让用户在 main 常驻时知道自己在哪。
- BrowserHeader 已是无 Tauri 依赖的壳,移动端只改它的 slot 布局。
- **无项目时隐藏汉堡**(review P3-3):`Sidebar v-if="showSidebar"`(`AppShell.vue:60`,无项目时抽屉根本不存在),汉堡的 `v-if` 联动 `projectsStore.currentProjectId !== null`。无项目时点击汉堡没东西可弹,空状态本身已有"+ 添加项目"入口,与桌面 `/chat` 空状态对称。

### 3.3 Sidebar.vue — 抽屉化

- 移动端:`position: fixed`,默认隐藏,`.sidebar--open` 滑入。
- 顶部补一个 `ProjectTabs`(移动端专属渲染),或在 AppHeader 里用 `v-if="!isMobile"` 隐藏 + Sidebar 里 `v-if="isMobile"` 显示——但 `isMobile` 不能靠 JS 检测(不可靠),**用 CSS 控制显隐**:两处都渲染,各自由媒体查询 `display:none/block`。
- 选 session 后自动关抽屉:`watch(chatStore.currentSessionId)` 或 SessionList 的点击回调里 `emit('navigate')` → AppShell 关抽屉。

### 3.4 Dialog 移动端全屏化 + 修 min-width 溢出

> **review P1-1 修订(2026-08-12)**:原版统一块 2/5 选择器类名写错(`.grant-modal__content` / `.md-detail-modal` 均不存在),照抄会静默失效——只剩 `.runtime-memory-modal` 一个能修掉。已改为 DialogContent 真实根类(核验:`grant-modal` / `markdown-detail-modal` / `memory-modal` / `audit-modal`),并补齐 MemoryModal / AuditLogModal。

通用模式(集中写在 `style.css` 的移动端块,避免散落各组件 scoped style 里被 specificity 压住):
```css
@media (max-width: 767px) {
  .settings-modal,
  .grant-modal,                 /* 原 design 误写 .grant-modal__content */
  .markdown-detail-modal,       /* 原 design 误写 .md-detail-modal */
  .runtime-memory-modal,
  .memory-modal,                /* review 补 */
  .audit-modal,                 /* review 补 */
  .gcfg-content {
    width: 100vw !important;
    min-width: 0 !important;    /* 覆盖 5 个硬 bug 的 min-width */
    max-width: 100vw !important;
    height: var(--app-height);  /* dvh,见 §4.1;非 100vh */
    max-height: var(--app-height);
    border-radius: 0;
    margin: 0;
  }
}
```

逐组件(reka-ui Dialog 7 个全部全屏化):
- `PermissionGrantsModal` — min 560 → 0
- `MarkdownDetailModal` — min 640 → 0(代码块仍横向滚动,见 §3.6)
- `RuntimeMemoryModal` — min 640 → 0
- `MemoryModal` — min 640 → 0(review 补,重度可达组件)
- `AuditLogModal` — min 640 → 0(review 补)
- `SettingsModal` — 640 → 100vw(tab 列表 + 表单竖排)
- `GroupChatConfigModal` — `min(640,92vw)` → 100vw

手搓 modal(YoloConfirmModal `.yolo-confirm-modal` / ConfirmDialog `.confirm-modal`):本就 `width:100%` 收缩,不全屏化(非 reka-ui,不走统一块),仅按钮触摸目标放大。

按钮触摸友好:所有 modal(含手搓)内 `button` 移动端 `min-height: 44px; min-width: 44px`(Apple HIG)。

### 3.5 ChatInput.vue — 软键盘 + 触摸

> **review P2-2 修订(2026-08-12)**:原版"`position: sticky; bottom:0` 贴底"在当前布局是 no-op——ChatInput 是 ChatPanel flex 列的底部常驻成员(`ChatPanel.vue:856`,不在滚动容器内),sticky 无可粘对象。改为 flex 常驻 + visualViewport 监听。

- CodeMirror host 移动端 `font-size: 16px`(**关键**:iOS Safari 字号 <16px 触发整页自动缩放)。line-height 保持 1.5。
- 工具栏按钮 `height:32px` → 移动端 `min-height:44px`。
- **输入区贴底 = flex 常驻 + visualViewport**:`.chat-input` 本就在 ChatPanel 底部常驻(`dvh` 处理 URL bar);iOS 软键盘弹起时(overlay,不改 layout viewport)靠 `window.visualViewport.resize` 监听调整位置。见 §4.1 机制 + implement Step 6(必做,非可选)。
- 发送按钮触摸目标放大。

### 3.6 chat 内容区可读性

- `MessageItem` / markdown 渲染:移动端 `font-size` 不动(已用 token),行距不动。
- **代码块 `<pre>` 横滚已存在,Step 7 退化为逐处确认**(review 核验):`.msg__markdown :deep(pre)` `overflow-x:auto`(`MessageItem.vue:1073`)、`CodeBlockPrimitive .ui-prim__code` `overflow:auto`、`MarkdownDetailModal` pre `overflow-x:auto`(@390)。~~原 design 写"补 `-webkit-overflow-scrolling: touch`"~~ —— **删除**(review P3-4:iOS 13+ 默认 momentum 滚动,该属性已废弃,加了无效)。
- `ToolCallCard` / `tool_result` 卡片:移动端 `max-width:100%`(`ToolCallCard.vue:723` 已是),内部长字段(路径/命令)补 `overflow-x:auto; word-break:break-all`。
- 不做"紧凑视图"(PRD 推后)。

**TracePanel / SubagentDrawer**(review P3-1):两者是 fixed 右抽屉(`TracePanel.vue:221` min(420,90vw) / `SubagentDrawer.vue:892` min(640,90vw)),移动端 90vw 覆盖本就可点可用,**维持现状,实测不可用再收窄**(不纳入 §3.4 全屏化,它们是侧抽屉非常规 Dialog)。

---

## 4. 风险与对策

### 4.1 iOS Safari `100vh` + URL bar + 软键盘(高风险)

> **review P2-2 修订(2026-08-12)**:原版把"dvh 处理软键盘"作为机制——不成立。iOS Safari 软键盘是 overlay,**不改 layout viewport**,dvh 对它无感(dvh 只对 URL bar 收起敏感)。软键盘贴底必须靠 Visual Viewport API。原版把 visualViewport 列"兜底"是错位的,升为必做。

**两个独立问题,两个独立机制**:

1. **URL bar 伸缩 + `100vh` 恒定**(layout viewport 问题):
   - iOS Safari `100vh` = large viewport(地址栏收起态),地址栏出现时底部被遮。
   - 对策:`100dvh`(dynamic viewport height,iOS 15.4+)随 URL bar 自动收缩。`--app-height: 100dvh`(`@supports not (height:100dvh)` 回退 `100vh`),AppShell `.app-shell { height: var(--app-height) }` 移动端,桌面仍 `100vh`。

2. **软键盘弹起遮输入框**(visual viewport 问题,**必做,非可选**):
   - iOS 软键盘 overlay 在 layout viewport 之上,**不触发 resize、不改 dvh**。Android Chrome 才 resize layout viewport。
   - 对策:`window.visualViewport` 监听 `resize` + `scroll`,读 `height`/`pageTop`,调整 `.chat-input` 的 `transform: translateY()` 或给 main 滚动容器加 `padding-bottom`。
   - 封装 composable `useMobileKeyboard()`,挂 ChatInput/AppShell,implement Step 6 必做。Step 0 真机基线记录软键盘实测表现。

### 4.2 CodeMirror 6 移动端行为(中风险)

**已知关注点**:
- iOS 长按选词 vs CodeMirror 自定义 selection 模型可能冲突
- 软键盘弹起时 EditorView 是否正确 scroll into view(CM6 有 `scrollToSelection`)

**对策**:`useChatInputCodeMirror` 已处理 IME,移动端验证项——implement 冒烟时真机测。若选词异常,CM6 有 `drawSelection` extension 可调;不预设改,留给验证发现。

### 4.3 reka-ui Dialog body scroll lock(低风险)

reka-ui `DialogRoot` 默认 lock body 滚动。移动端全屏 Dialog 下这是对的(全屏自己滚)。关注点:多个 Dialog 嵌套(Settings 里开 Memory Modal?)时 lock 计数。implement 时验证。

### 4.4 specificity 与 `!important`(中风险)

desktop-first 叠加覆盖时,移动端样式要赢过桌面样式。策略:
- 优先**提升选择器特异性**(加父类 / 双类),避免 `!important`。
- 实在不行(如覆盖第三方 reka-ui 内联样式)才用 `!important`,且集中在 §3.4 那个统一媒体查询块里。
- Dialog 宽度覆盖大概率要 `!important`(覆盖 `style` 属性或高特异性),可接受。

---

## 5. 冒烟策略

PRD 要求"实施前先做手机宽度冒烟记录问题清单"。本 design 的冒烟分两段:

### 5.1 Design 阶段(已完成)= 代码静态分析

上文的 §1 现状分析就是冒烟问题清单的代码侧等价物:布局挤死点已量化(§1.2)、Dialog 溢出 bug 已定位到行(§1.3)、断点缺失已确认(§1.4)。代码分析比 headless 截图更精确(能拿到确切像素值和 CSS 规则)。

### 5.2 Implement 阶段(强制 validation gate)= 真机 + 模拟器

**为什么不在 design 阶段跑 headless 浏览器冒烟**:PRD 点名的三个高风险项(CodeMirror iOS Safari 触摸、软键盘弹起、iOS viewport unit bug)都是 **iOS-specific 运行时行为,headless Chromium 测不了**。headless 只能复现 §1 已用代码确定的布局挤死,增量价值低。

**implement 第一步冒烟清单**(写进 implement.md):
1. `pnpm dev` 起 vite,Chrome DevTools 切 iPhone 12 Pro(390×844)模拟器,逐项截图对照 §1 表格
2. 真机 iOS Safari(主力 iPhone)走一遍:配对 → 节点 → 进会话 → 发消息 → 看流式 → 触发 permission:ask 弹窗 → 打开设置
3. 重点验证 §4 三个风险项:软键盘是否挡输入框、CodeMirror 选词/IME、Dialog 全屏 + body lock
4. Android 国产浏览器(WebKit/Blink)二次验证,CSS 兼容性差异预期小

冒烟问题清单格式:每个发现记 `组件:行号 → 现象 → 修法`,直接进 implement 的 checklist。

---

## 6. 不做项(对齐 PRD §Notes + parent Q11)

- ❌ 触屏手势(swipe 切 session / swipe 关抽屉)—— V2
- ❌ pull to refresh —— V2
- ❌ 紧凑消息视图 / 字号档位 —— V2
- ❌ 移动端专属导航重设计 —— V2
- ❌ 底部 tab bar 动效 —— V2
- ❌ 多档断点(sm/md/lg)—— 单一 768px 分界,V2 再加档
- ❌ 原生体验追求 —— 永久(本 epic 定位"可用"非"好用")
- ❌ 远程读写权限分层 —— V2(parent Q11)
- ❌ 家里电脑浏览器(大屏)适配 —— 不需要,零改动

---

## 7. 验收映射

| PRD 验收标准 | 本 design 改造点 |
|---|---|
| 三栏不并排,改单栏 + tab 切换 | §2.2 抽屉式(Sidebar overlay) + §3.3 |
| 切 session → 自动跳"对话" | §2.2 抽屉关闭 + main 常驻 |
| chat 输入框可点可输入,软键盘不挡 | §3.5 + §4.1(dvh 处理 URL bar + visualViewport 处理软键盘;sticky 已判 no-op) |
| 发消息 → 流式输出,代码块横向滚动 | §3.6(代码块 overflow-x:auto) |
| permission:ask 弹窗 / ask_user_question | permission 弹窗走 §3.4 全屏化;**ask_user_question 是 inline 消息卡非 modal**(review P3-5,`AskUserQuestionCard.vue` 注释明示 NEVER modal),随消息流按 §3.6 卡片 `max-width:100%` 适配 |
| tool_use / tool_result 卡片可读 | §3.6(卡片 max-width:100%) |
| 桌面(≥768px)零变化 | §2.3(desktop-first 叠加,桌面样式不动) |

PRD 验收第 1 条原文"单栏 + 顶部 tab 切换" → 本 design 改为抽屉式(§2.2 已论证理由)。**用户 design review(2026-08-12)已确认接受偏离**,退路(view-switching + tab bar)不采用。

---

## 8. 实施前置依赖

- 强依赖 S4(PWA 壳 + router + transport 已就绪)—— ✅ 已完成
- 本任务**不改 agent core / 不改后端**——纯前端 CSS + 少量组件结构
- 涉及文件预估:AppShell / AppHeader / BrowserHeader / Sidebar / ProjectTabs / 7 个 Dialog / ChatInput / style.css,约 12-15 个 `.vue` + 1 个 `.css`
- 新增依赖:**零**(不引入 Tailwind utility、不引手势库)
