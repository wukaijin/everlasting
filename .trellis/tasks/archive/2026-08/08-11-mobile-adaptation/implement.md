# S5 移动端中限适配 — 执行计划

> 设计依据见 [design.md](./design.md),需求见 [prd.md](./prd.md)。
> 本文档是有序执行 checklist + validation + review gate + rollback 点。

## 改造原则(每步都要守住)

1. **桌面零回归**——所有改动包在 `@media (max-width: 767px)`,桌面样式块一字不动。
2. **CSS 优先**——能用媒体查询解决的不动 template/state;只在抽屉状态机(Step 2-4)必须时才加 JS 状态。
3. **`!important` 最小化**——优先提升选择器特异性,只有覆盖 reka-ui 内联/高特异性样式时才用,集中在 Step 5。
4. **每步独立可提交**——按 commit 粒度组织,失败可单独 revert。

---

## Step 0 — 真机冒烟基线(强制前置 gate)

> PRD §验收:"实施前必须先做手机宽度冒烟记录问题清单"。
> design §5.2 已论证:headless 测不了 iOS-specific 行为,必须真机/模拟器。

- [ ] `cd app && pnpm dev` 起 vite
- [ ] Chrome DevTools → Device Toolbar → iPhone 12 Pro(390×844)
- [ ] 逐项截图存证,对照 design §1.2 / §1.3 表格验证挤死点 + Dialog 溢出
- [ ] **真机 iOS Safari**(主力 iPhone)走通:配对码 → 节点列表 → 进会话 → 发消息 → 流式 → permission 弹窗 → 设置弹窗
- [ ] 记录问题清单到 `research/mobile-smoke-baseline.md`(每条:`组件:行号 → 现象 → 修法对应 design 哪节`)
- [ ] **重点记录 design §4 三个风险项的实测表现**(软键盘/CodeMirror 触摸/viewport),作为 Step 6/7 的调整依据

**gate**:问题清单产出后才开始 Step 1。若发现 design 未预见的问题(如新溢出点),回 design §1/§3 补条目再继续。

---

## Step 1 — 断点基线(style.css + index.html)

- [ ] `app/src/style.css` 加动态视口高度 token:
  ```css
  :root {
    --app-height: 100dvh;  /* iOS 15.4+ 动态视口,随 URL bar 收缩(不处理软键盘,见 design §4.1) */
  }
  @supports not (height: 100dvh) {
    :root { --app-height: 100vh; }
  }
  ```
- [ ] **safe-area**(review P2-3):`app/index.html` viewport meta 加 `viewport-fit=cover`(没有它 `env(safe-area-inset-*)` 恒 0):
  ```html
  <meta name="viewport" content="width=device-width, initial-scale=1.0, viewport-fit=cover" />
  ```
  style.css 移动端块加 safe-area token(供 Step 3/6 消费):
  ```css
  @media (max-width: 767px) {
    :root {
      --safe-area-top: env(safe-area-inset-top);
      --safe-area-bottom: env(safe-area-inset-bottom);
    }
  }
  ```
- [ ] 不在此步加全局移动端重置——逐组件在各自 scoped style 里处理,避免全局副作用。

**验证**:`pnpm build`(确认 token 不破坏构建),桌面视觉无变化;移动 viewport 刘海屏真机 `getComputedStyle(document.documentElement).getPropertyValue('--safe-area-bottom')` 非 0。

---

## Step 2 — AppShell 抽屉宿主(状态 + 遮罩)

> 抽屉状态放 AppShell(AppHeader 汉堡 + Sidebar 都要读写)。用 composable 还是 provide/inject implement 时定,design §2.2 倾向 provide。

- [ ] `AppShell.vue`:
  - 加移动端导航 open 状态(`mobileNavOpen`)
  - Sidebar 外包一层遮罩 `<div class="sidebar-overlay" v-if="mobileNavOpen" @click="close">`(仅移动端显示)
  - provide 状态 + toggle/close 方法给子组件
- [ ] `.app-shell` 移动端:`height: var(--app-height)`(替换 `100vh`)
- [ ] `.app-shell__body` 移动端:不改 flex 方向,但 Sidebar 改 fixed 定位(Step 3 处理样式),main 占满

**验证**:DevTools 移动 viewport,汉堡未加(AppHeader Step 4)所以手动在 console 触发 `mobileNavOpen=true` 看遮罩出现;桌面 `mobileNavOpen` 状态不影响布局(Sidebar 仍常驻)。

**review gate**:状态机成型,自查 provide/inject 链路通畅。

---

## Step 3 — Sidebar 抽屉化

- [ ] `Sidebar.vue` scoped style 加移动端:
  ```css
  @media (max-width: 767px) {
    .sidebar {
      position: fixed;
      inset: 0;
      z-index: 110;           /* 高于 TracePanel(100),见 design §3.1 */
      width: 100vw;
      padding-top: var(--safe-area-top);  /* 避开刘海,review P2-3 */
      transform: translateX(-100%);
      transition: transform var(--duration-base) var(--ease-out);  /* --duration-normal 不存在,改 --duration-base,review P2-1 */
    }
    .sidebar--open { transform: none; }
  }
  ```
- [ ] 接收 AppShell provide 的 open 状态,绑定 `.sidebar--open` class
- [ ] 选 session 自动关抽屉:`watch(() => chat.currentSessionId)` 变化时调 close;或 SessionList 点击 emit `navigate`
- [ ] 顶部补 ProjectTabs 移动端渲染——见 Step 4 的显隐分工

**验证**:移动 viewport 下点遮罩/选 session 抽屉关闭;桌面 Sidebar 仍 260px 常驻。

---

## Step 4 — AppHeader 移动端(汉堡 + 面包屑)

- [ ] `BrowserHeader.vue` + `AppHeader.vue`:
  - 移动端左侧加汉堡 `<button class="app-header__menu-toggle">☰</button>`(`@media (max-width:767px){display:inline-flex}` 桌面 `display:none`),@click toggle `mobileNavOpen`
  - 中间面包屑:当前会话名或项目名(从 chatStore / projectsStore 取)
- [ ] ProjectTabs 显隐分工:
  - AppHeader 里的 `<ProjectTabs>` 移动端 `display:none`(包在媒体查询里)
  - Sidebar 抽屉顶部渲染一份移动端 `<ProjectTabs>`(桌面 `display:none`)
  - 两处都渲染、各自媒体查询控制显隐——避免 JS `isMobile` 检测(不可靠)
- [ ] HiddenProjectsMenu / PendingBadge 评估挪进抽屉还是保留顶栏(implement 时定,倾向挪进抽屉)
- [ ] 汉堡按钮触摸目标 ≥44×44px

**验证**:移动端汉堡点击抽屉滑出;ProjectTabs 在抽屉里可切换;桌面 AppHeader 仍显示 ProjectTabs。

**review gate**:Step 2-4 一起提交(抽屉骨架完整),自查整体交互闭环。

---

## Step 5 — Dialog 全屏化 + 修 min-width 溢出

> **review P1-1 修订**:5 个现成硬 bug(原版只列 3 个,漏 MemoryModal/AuditLogModal),且统一块原 2/5 选择器类名错(`.grant-modal__content` / `.md-detail-modal` 不存在)会静默失效。

- [ ] 集中写在 `style.css` 移动端块(避免散落 scoped style 被 specificity 压住),**选择器用 DialogContent 真实根类**(已核验):
  ```css
  @media (max-width: 767px) {
    .settings-modal,
    .grant-modal,              /* 原 design 误写 .grant-modal__content */
    .markdown-detail-modal,    /* 原 design 误写 .md-detail-modal */
    .runtime-memory-modal,
    .memory-modal,             /* review 补 */
    .audit-modal,              /* review 补 */
    .gcfg-content {
      width: 100vw !important;
      min-width: 0 !important;   /* 覆盖 5 个硬 bug 的 min-width */
      max-width: 100vw !important;
      height: var(--app-height);
      max-height: var(--app-height);
      border-radius: 0;
      margin: 0;
    }
  }
  ```
- [ ] 手搓 modal(`YoloConfirmModal` `.yolo-confirm-modal` / `ConfirmDialog` `.confirm-modal`):非 reka-ui 不走统一块,本就 `width:100%` 收缩;仅按钮 `min-height:44px; min-width:44px`
- [ ] 所有 modal(含手搓)内 button 移动端触摸目标放大(Apple HIG 44px)
- [ ] 确认 reka-ui DialogOverlay 移动端覆盖全屏(本就 `position:fixed; inset:0`)

**验证**:移动 viewport 逐一打开 **7 个 reka-ui Dialog + 2 个手搓 modal**,无横向滚动条、内容自适应、按钮可点;**Step 0 记录的 5 个溢出 bug 全部消失**(PermissionGrants 560 / MarkdownDetail 640 / RuntimeMemory 640 / Memory 640 / Audit 640)。

**验证**:`pnpm test`(`GroupChatConfigModal.test.ts` / `YoloConfirmModal.test.ts` / `MarkdownDetailModal.test.ts` / `RuntimeMemoryModal.test.ts` 确认不破)。

**review gate**:Dialog 全部改完,自查清单完整(design §1.3 现列 9 个 = 7 reka-ui + 2 手搓,确认无其他 reka-ui Dialog 遗漏)。

---

## Step 6 — ChatInput(CodeMirror + 软键盘)

> **review P2-2 修订**:`position: sticky; bottom:0` 删除(ChatInput 是 ChatPanel flex 列底部常驻成员 `ChatPanel.vue:856`,sticky 是 no-op)。visualViewport 从"实测不够再加"升为**必做**——iOS 软键盘 overlay 不改 layout viewport,dvh 对它无感。

- [ ] `ChatInput.vue` scoped style 移动端:
  - CodeMirror host `font-size: 16px`(**iOS 防自动缩放**,Step 0 实测确认)
  - 工具栏按钮 `min-height:44px; min-width:44px`(桌面 32px 不动)
  - 发送按钮触摸目标放大
  - `.chat-input` 加 `padding-bottom: var(--safe-area-bottom)`(Home Indicator,配合 Step 1 的 viewport-fit=cover)
- [ ] **`useMobileKeyboard()` composable(必做)**:`window.visualViewport` 监听 `resize`/`scroll`,软键盘弹起时调整 `.chat-input` 位置(`transform: translateY()` 或 main 滚动容器加 `padding-bottom`)。挂 ChatInput `onMounted`/`onUnmounted` 加/移除监听。
- [ ] 若 Step 0 发现 CodeMirror iOS 选词异常,调 CM6 `drawSelection` extension(见 design §4.2)——不预设改,按实测

**验证**:真机 iOS Safari 聚焦输入框,软键盘弹起输入框可见不被挡(visualViewport 生效);输入中文 IME 正常;发送按钮可点;Home Indicator 不压输入框。

---

## Step 7 — chat 内容区可读性

> **review P3-4 修订**:`-webkit-overflow-scrolling: touch` 删除(iOS 13+ 默认 momentum 滚动,该属性已废弃)。代码块横滚 review 核验已存在,本步退化为确认 + 补卡片溢出。

- [ ] **代码块横滚已存在,逐处确认**(review 核验):`MessageItem.vue:1073` `.msg__markdown :deep(pre)` `overflow-x:auto` ✓、`CodeBlockPrimitive .ui-prim__code` `overflow:auto` ✓、`MarkdownDetailModal:390` ✓。**不补** `-webkit-overflow-scrolling`(已废弃)
- [ ] `ToolCallCard` / tool_result 卡片:移动端确认 `max-width:100%`(已是 `ToolCallCard.vue:723`),补内部长字段(路径/命令)`overflow-x:auto; word-break:break-all`
- [ ] chat 消息区字号/行距不动(已用 token,design §3.6 确认无需调)

**验证**:移动 viewport 发消息触发 tool_use,卡片不撑破、代码块可横滚。

---

## Step 8 — 桌面回归验证(强制 gate)

- [ ] `pnpm dev` 桌面宽度(≥768px,如 1280×800)逐视图截图:
  - 空状态 / 有项目 / 会话列表 / chat / 每个 Dialog
- [ ] 与 `main` 分支同视图对比,**零视觉差异**(所有改动在 `@media max-width:767px` 内,桌面样式块未动)
- [ ] 重点查:AppHeader ProjectTabs 常驻、Sidebar 260px、Dialog 桌面宽度、ChatInput 桌面布局

**gate**:桌面任何像素级变化 = 必须修(要么是 specificity 误伤,要么误改了桌面样式块)。

---

## Step 9 — 真机验收冒烟(对照 PRD 7 条)

- [ ] iPhone Safari 真机走一遍 PRD §验收 7 条:
  - [ ] 三栏不并排,抽屉 + main 全屏可用
  - [ ] 切 session → 抽屉关闭,main 显示新会话
  - [ ] chat 输入框软键盘不挡,可输入
  - [ ] 发消息 → 流式,代码块横滚
  - [ ] permission:ask / ask_user_question 弹窗全屏居中,按钮可点
  - [ ] tool_use / tool_result 卡片可读
  - [ ] 桌面零变化(Step 8 已验)
- [ ] Android 国产浏览器(WebKit/Blink)二次走一遍,CSS 兼容性确认
- [ ] 把 Step 0 基线问题清单的每条标记"已修/残留",残留项评估是否推 V2

---

## Validation 命令汇总

```bash
# 类型检查 + 构建(每 Step 后跑)
cd app && pnpm build

# 单元测试回归(Step 5 Dialog 改动后 + 最终)
cd app && pnpm test

# 桌面视觉回归(Step 8)
cd app && pnpm dev   # 桌面宽度截图对比 main

# 真机冒烟(Step 0 + Step 9)
cd app && pnpm dev   # 手机连 dev server IP,或 Chrome DevTools 模拟
```

> 注:本任务是纯前端 CSS + 少量组件结构,**不跑 cargo test**(不改后端)。

---

## Review Gates 汇总

| Gate | 时机 | 通过条件 |
|---|---|---|
| G1 | Step 0 后 | 真机问题清单产出,design §1 与实测吻合(或补差异) |
| G2 | Step 2-4 后 | 抽屉骨架交互闭环(开/关/选 session 自动关) |
| G3 | Step 5 后 | 7 个 Dialog 全改完,无遗漏 |
| G4 | Step 8 后 | **桌面零回归**(强制) |
| G5 | Step 9 后 | PRD 7 条验收全过(或残留项明确推 V2) |

---

## Rollback 点

- 每个 Step 独立 commit;失败 `git revert <sha>` 即回退该步。
- **Step 2-4(抽屉骨架)是一组**,状态机 + 三个组件联动,失败整组 revert 回到 Step 1(纯 CSS,无副作用)。
- **Step 5 Dialog** 改动相互独立,可逐组件 revert。
- 最坏情况:全部 revert 回 `main`,S5 重做——不影响 S1-S4 已合分支。

---

## 实施顺序总览

```
Step 0 (真机基线) → Step 1 (style.css token)
                   → Step 2-4 (抽屉骨架: AppShell/Sidebar/AppHeader) [一组提交]
                   → Step 5 (7 个 Dialog 全屏化)
                   → Step 6 (ChatInput 软键盘)
                   → Step 7 (chat 内容区)
                   → Step 8 (桌面回归 gate)
                   → Step 9 (真机验收 gate)
```

预估涉及文件 12-15 个 `.vue` + `style.css`,零新增依赖,零后端改动。
