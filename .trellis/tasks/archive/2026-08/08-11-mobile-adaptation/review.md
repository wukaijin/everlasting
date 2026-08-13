# Review — S5 移动端中限适配(抽屉式单栏 + Dialog 全屏化 + ChatInput/内容区移动端)

> 评审日期:2026-08-12。评审对象:`prd.md` + `design.md` + `implement.md`(status=planning,实施前评审)。
> 方法:对 design 引用的代码库事实逐条核验(AppShell/Sidebar/AppHeader/BrowserHeader/ProjectTabs 的行号与 CSS、style.css 断点现状、7 个 Dialog 宽度设定、ChatInput CM6 与按钮、代码块横滚现状、S4 router/views 边界);对 Dialog 清单做**全仓 reka-ui Dialog 穷举比对**;对三文档一致性(prd ↔ design ↔ implement)与验收映射做交叉核对。
> 关联评审:[S4 review](../08-11-pairing-and-pwa/review.md)(P1-1 remote-served 判定等,S5 直接消费的 S4 产物已核验落地)。

## 总体评价

design 质量高——**代码事实核验全部属实**(AppShell 三段骨架/`height:100vh`、Sidebar 260px、AppHeader `isTauriWebview` 分支、ProjectTabs 横滚、style.css 全仓唯一媒体查询 @191 且零断点、7 个 Dialog 宽度与行号逐条精确),决策链完整(断点方案、抽屉式 vs tab bar 的论证、"CSS 优先 + desktop-first 叠加"不变量),PRD 偏离(顶部 tab bar → 抽屉式)论证充分且已获用户确认,implement Step 0-9 与 PRD 验收/design 冒烟策略逐条对应,"零新增依赖 + 零后端改动"现实可行。

**结论:可批准进入实施,但 P1-1 必须先修**——design §1.3 / implement Step 5 的 Dialog 清单**漏了 2 个同型硬溢出 bug 组件(MemoryModal、AuditLogModal,均 `min-width:640px`),且统一覆盖块的 5 个选择器里有 2 个类名写错(照抄必静默失效)**。按文档原样落地,5 个硬 bug 只修得掉 1 个。另有 3 个 P2(不存在的 CSS token、iOS 软键盘机制描述偏差 + sticky 实为 no-op、safe-area 缺口)建议实施前修订。

## ✅ 核验通过(证据确凿)

| 声明(design) | 核验结果 |
|---|---|
| 顶部 40px AppHeader + 左右两栏(`AppShell.vue:55-94`) | **属实**(template 55-94;`.app-shell` `height:100vh` @100;`Sidebar v-if="showSidebar"` @60;`.app-shell__main` flex:1 @112-118) |
| `Sidebar.vue:187-202` width:260px 固定 | **属实**(`Sidebar.vue:188` `width: 260px; flex-shrink: 0`) |
| `AppHeader.vue:47` isTauriWebview 分支 | **属实**(@47 `const shell = isTauriWebview() ? TitleBar : BrowserHeader`;移动 PWA 必走 BrowserHeader,其结构 = logo + slot + spacer,汉堡可放 slot 内容首位,方案可行) |
| `ProjectTabs.vue:186-192` `.tabs__scroll` 横滚 | **属实**(@190 `overflow-x: auto`) |
| style.css 全文唯一媒体查询 + 零断点 | **属实**(`style.css`:唯一 `@media` 为 prefers-reduced-motion @191;`@theme` @22 无 `--breakpoint-*`;S5 首次引入断点属实) |
| 7 个 Dialog 宽度设定(§1.3 表) | **全部属实,行号精确**:`PermissionGrantsModal.vue:159-161` 80vw/min560;**`MarkdownDetailModal.vue:212-214` 80vw/min640**;`RuntimeMemoryModal.vue:518-520` 80vw/min640;`SettingsModal.vue:95-96` 640/max(100vw-40);`YoloConfirmModal.vue:194-195` 100%/max480;`ConfirmDialog.vue:133-134` 100%/max460;`GroupChatConfigModal.vue:509` min(640,92vw) |
| `ChatInput.vue` CM6 host 28/200px + 按钮 32px | **属实**(@743 min-height:28px、@754 line-height:1.5、@755 max-height:200px、@828 height:32px;`utils/chatInputCodeMirror` @99 引入) |
| 断点值 768px 单一分界、desktop-first 叠加 | **合理且可执行**;`--ease-out` token 存在(`style.css:156`);移动端覆盖不改桌面样式块的不变量与"桌面零回归"验收一致 |
| 抽屉状态放 AppShell(共同父) | **合理**(AppShell 是 AppHeader + Sidebar 唯一共同父;`v-if="showSidebar"` 语义下抽屉仅 /chat 路由内存在) |
| 代码块横滚"确认已有,没有则补"(§3.6) | **实为"已有,纯验证"**:`.msg__markdown :deep(pre)` 已有 `overflow-x:auto`(`MessageItem.vue:1073`);`CodeBlockPrimitive .ui-prim__code` `overflow:auto`(@98);`MarkdownDetailModal` pre `overflow-x:auto`(@390)。Step 7 退化为逐处确认 |
| `ToolCallCard` 卡片 max-width:100%(§3.6) | **属实**(`ToolCallCard.vue:723`;L769 `overflow-y:auto`) |
| ChatInput 移动端 sticky bottom 方案 | **布局事实属实**(ChatInput 根 `<footer class="chat-input">` @568,是 ChatPanel flex 列的底部常驻成员)——但 sticky 在该布局是 no-op,见 P2-2 |
| S4 视图边界(抽屉只影响 /chat) | **属实**(`App.vue` = 裸 `<router-view/>`;AppShell 只在 `views/ChatView.vue` 内;`/pairing` `/nodes` 为独立页,max-width 360/560px 已移动优先;无冲突) |
| viewport meta 已就位 | **属实且无需改**(`index.html` 已有 `width=device-width, initial-scale=1.0` + apple-mobile-web-app-capable)——但缺 `viewport-fit=cover`,见 P2-3 |
| reka-ui Dialog body scroll lock 默认行为(§4.3) | **属实**(reka-ui `DialogRoot` modal 默认 lock 滚动;8 个组件用 reka-ui Dialog,全屏化后自带滚动) |
| implement/check jsonl 已 curated | **属实**(两文件均已填 spec 条目)——但 reka-ui 条目写"7 个 Dialog",修订清单后需同步,见复评建议 |
| 抽屉式偏离 PRD"顶部 tab bar" | **已被用户确认**(design §2.2 注明 2026-08-12 design review 选"抽屉式(推荐)");退路不采用,与 §7 验收映射一致 |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — Dialog 清单漏 2 个同型硬 bug 组件,且统一覆盖块 2/5 选择器类名错误(照抄必静默失效)

**位置**:design §1.3 表、§3.4 统一覆盖块;implement Step 5 同源复制。

**问题 A — 清单遗漏(全仓 reka-ui Dialog 穷举比对)**:

| 组件 | 实际宽度 | 375px 屏行为 |
|---|---|---|
| `memory/MemoryModal.vue:100-102` | `width:80vw; min-width:640px` | **溢出**(640>375) |
| `audit/AuditLogModal.vue:329-331` | `width:80vw; min-width:640px` | **溢出** |

- **MemoryModal 是重度可达组件**:引用点含 `ChatPanel`(chat 主界面有入口)、`TracePanel`、`RuntimeMemoryModal`、`PermissionGrantsModal`、`MemoryPreview`、`GroupChatConfigModal`、`MarkdownDetailModal` —— 移动端主流程必达,不是边角。
- AuditLogModal 经 `AuditLogItem` 从审计入口可达。
- 两组件与 §1.3 已列的 3 个硬 bug **同一病根**(min-width 硬设超视口),但文档没列 → implement Step 5 的 review gate("design §1.3 清单 7 个 + 确认无其他")以残缺清单为基准,也查不出来。**硬 bug 实为 5 个,不是 3 个。**

**问题 B — 选择器类名错误(照抄必静默失效)**:

- §3.4 / Step 5 统一块里的 `.grant-modal__content` **不存在**(`PermissionGrantsModal` 实际根类 `.grant-modal`,min-width 就在它上面 @159-161;内容类是 `.grant-modal__body`)。
- `.md-detail-modal` **不存在**(实际 `.markdown-detail-modal`,min-width @212-214)。
- 按文档原样落地:5 个硬 bug 只有 `RuntimeMemoryModal`(.runtime-memory-modal,类名对)能修掉,**PermissionGrantsModal / MarkdownDetailModal 的选择器全部落空** —— 这两个恰好就是 §1.3 表里点名"必须修"的组件。

**建议**:§1.3 表扩为 9 行(加 MemoryModal / AuditLogModal,行号如上);统一覆盖块改为真实类名并全量核对:

```css
.grant-modal, .markdown-detail-modal, .runtime-memory-modal,
.memory-modal, .audit-modal, .settings-modal, .gcfg-content
```

(implement Step 5 的验证标准同步改"5 个溢出 bug 消失")。

### 🟡 P2-1 — implement Step 3 用了不存在的 CSS token `--duration-normal`

**位置**:implement Step 3 抽屉 transition 示例(`transition: transform var(--duration-normal) var(--ease-out)`)。

**问题**:全仓无 `--duration-normal`(grep 零命中)。`style.css:145-149` 的时长 token 是 `--duration-instant: 80ms / --duration-fast: 100ms / --duration-base: 150ms / --duration-slow: 240ms / --duration-pulse: 1800ms`。无效 var() 使整个 transition 声明在 computed-value 阶段失效 → **抽屉滑入动画静默消失**(transform 仍生效,只是无过渡)。

**建议**:改 `var(--duration-base)`(150ms,与仓库 drawer 类组件一致;TracePanel 滑出用 `--duration-slow` 240ms 可参考)。

### 🟡 P2-2 — iOS 软键盘机制描述有偏差,sticky 在当前布局是 no-op;visualViewport 应从"必要时"升为"必做"

**位置**:design §4.1、§3.5;implement Step 6。

**问题**:
1. "软键盘弹起时浏览器会 shrink `dvh`,输入框自然贴底" —— **iOS 上不成立**:iOS Safari 软键盘是 overlay,不改变 layout viewport(`dvh` 只对 URL bar 收起敏感);layout viewport resize 是 Android Chrome 的行为。iOS 的可用机制是浏览器自动 scroll-into-view + **Visual Viewport API**。
2. `position: sticky; bottom: 0` 在 ChatPanel 布局是 **no-op**:`.chat-panel` 是 flex 列(header + main 滚动 + ChatInput 常驻底部),ChatInput 本就在滚动容器之外贴底,sticky 没有任何可粘对象。implement 时若按文档期望"sticky 生效"会卡壳。
3. design §4.1 把 Visual Viewport API 列为"仅在前两者不够时加"——前两者(§4.1 的 dvh 方案)在 iOS 软键盘场景恰恰是不够的。

**建议**:§4.1/§3.5 改述:移动端输入区贴底 = flex 常驻 + `--app-height`(dvh 处理 URL bar);**Step 6 直接把 `visualViewport.resize` 监听(调整 `.chat-input` 的 translateY/或给 main 加 padding)列为必做**,Step 0 真机基线专门记录软键盘表现。这是 PRD 验收第 3 条(软键盘不挡输入框)的核心机制,不应留到"实测不够再说"。

### 🟡 P2-3 — safe-area 缺口(全仓零 `env(safe-area-inset-*)`,且无 `viewport-fit=cover`)

**位置**:全仓(design 未覆盖)。

**问题**:iPhone 刘海屏 + Home Indicator 下,底部输入框会被 Home Indicator 压住、抽屉顶部内容顶进刘海。`index.html` viewport meta 缺 `viewport-fit=cover`(没有它 `env(safe-area-inset-*)` 恒为 0)。

**建议**:Step 1 顺手做:`viewport` meta 加 `viewport-fit=cover`;移动端 `.chat-input` 加 `padding-bottom: env(safe-area-inset-bottom)`(或放在 `--app-height` 同一 token 块),抽屉加 `padding-top: env(safe-area-inset-top)`。一行级改动,真机收益明显。

### 🟢 P3(可不动,建议显式声明)

- **P3-1 — TracePanel / SubagentDrawer 未进清单**:两者是 fixed 右抽屉(`TracePanel.vue:221` min(420px,90vw)、`SubagentDrawer.vue:892` min(640px,90vw)),移动端 90vw 覆盖可点可用但未评估。建议 §6 不做项或 §3 补一句"抽屉类组件移动端维持现状(90vw 覆盖),实测不可用再收窄"——否则 implement 时会被"要不要改"悬空。
- **P3-2 — z-index 同级**:design §3.1 抽屉 `z-index:100` 与 `TracePanel` z-index:100(`TracePanel.vue:227`)相同,同时打开时由 DOM 顺序定层级。建议抽屉用 90-99 或明确"不与 TracePanel 同开",避免偶发叠层。
- **P3-3 — 无项目时汉堡行为**:`AppShell.vue:60` `Sidebar v-if="showSidebar"`,无项目时抽屉不存在,汉堡点击无反馈。建议与桌面对称:无项目时隐藏汉堡(空状态本身有"+ 添加项目"入口)。
- **P3-4 — `-webkit-overflow-scrolling: touch` 已过时**:iOS 13+ 默认 momentum 滚动,该属性已废弃(被忽略)。§3.6 的补丁项可整条删掉——代码块横滚已存在(见核验表),此属性加了也无效。
- **P3-5 — AskUserQuestionCard 不是弹窗**:PRD 把它列进弹窗清单,但实际是 inline 消息卡(`AskUserQuestionCard.vue` 注释明确"inline card,NEVER modal"),design §7 验收映射写"ask_user_question 弹窗居中全屏"有误导。建议映射改"inline 卡随消息流适配(§3.6 卡片 max-width:100%)",并把该组件从"Dialog 清单"语义里摘出(它确实不需要全屏化)。

## 🟦 其他备注

- **"CSS 优先 + desktop-first 叠加"不变量执行路径清晰**:移动端覆盖全部收在 `@media (max-width:767px)`,桌面样式块零改动;spec 的 design-tokens/reka-ui-usage/popover-pattern/transport-and-pwa-modes 已进 implement.jsonl,方向正确。
- **Step 0 基线 gate 设计得好**:"若发现 design 未预见的问题回 §1/§3 补条目再继续"——P1-1 的两处遗漏正属于此类,基线冒烟时会被真机截图暴露,但文档先行修掉更省一轮。
- **两处渲染 ProjectTabs 的显隐分工方案可行**:`ProjectTabs.vue` 无 useId/teleport/全局副作用(核验无),双实例挂载只共享 Pinia store,无冲突;桌面显隐由媒体查询控制,不引入 JS isMobile,正确。
- **不跑 cargo test 的边界成立**:本任务纯前端,ChatPanel/Dialog 相关有现成单测(`GroupChatConfigModal.test.ts`/`YoloConfirmModal.test.ts`/`MarkdownDetailModal.test.ts`/`RuntimeMemoryModal.test.ts`),Step 5 后跑 `pnpm test` 有实际覆盖,不是空转。

## 复评建议

1. **修 P1-1**(§1.3 清单扩到 9 个组件 + §3.4/Step 5 覆盖块改真实类名,同步 implement 验证标准"3 个溢出 bug"→"5 个")→ 修完即可 `task.py start`
2. **修 P2-1**(Step 3 token 改 `--duration-base`)+ **P2-2**(§4.1 机制改述,visualViewport 升 Step 6 必做)+ **P2-3**(viewport-fit=cover + safe-area padding,并入 Step 1/6)
3. P3-1/P3-2/P3-3/P3-4/P3-5 合并修订(单 commit doc 修);P3-2/P3-3 也可留到 implement 阶段实测定夺
4. 修订后同步更新 `implement.jsonl` reka-ui 条目("7 个 Dialog"→ 实际清单)与 `check.jsonl`(若保留 Dialog 核验条目,注明覆盖 9 个)
