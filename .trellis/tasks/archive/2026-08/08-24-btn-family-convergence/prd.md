# Button CSS 家族收敛:167 个 `<button>` 统一 .btn 共享原语

## Goal

把散落在 69 个 .vue 文件里的 167 个 `<button>` 各自手写的 scoped 样式,收敛为
`app/src/style.css` 里的共享 `.btn` CSS 家族(variant × size × shape 修饰类),
消灭「同样是主按钮,radius/底色/hover 各写各的」的漂移;顺带清掉盘点中发现的
死 token 引用与裸值债。沿用 08-23 spinner/skeleton + z-index 两单确立的收敛模式:
**共享 CSS 家族放 style.css,不引入 Vue 组件**(design.md D3)。

## 出处与关联

- 08-24-session-item-keyboard 归档 evidence "Follow-up 输入" 第三条
  (`.trellis/tasks/archive/2026-08/08-24-session-item-keyboard/research/evidence-20260824.md:49`),
  UI 清单最后一项,三项候选(spinner / z-index / button)里最大的一单。
- 08-22 focus-visible 基线已给全库按钮兜焦点环(`:where()` 零特异性 +
  `--shadow-ring`),本任务收敛 padding/底色/变体/圆角;**家族不写任何
  focus 规则**,两者互不侵占。
- 规划期决议:单任务全量收敛、分 6 WP 批次(design.md D1);视觉归一取
  保守档——接受小收敛(奇数 padding 落整档、disabled 五档归一、hover 写法
  统一),尺寸阶梯贴合现值不重排(design.md D2)。

## 现状清点(2026-08-24 全量盘点,子代理 grep/read 实证)

基础数据:167 个 `<button>` / 69 文件,约 135 条按钮样式规则。token 地基现成
(style.css `@theme` 已有 radius/spacing/text/motion/shadow 完整阶梯)。
**`.btn` 类名在 src 下零占用,可安全引入**;现有按钮类全是 BEM 前缀式
(`gcfg-btn`/`confirm-modal__btn`/`ui-prim__btn`),与裸 `.btn` 不撞。
注意:`components/chat/primitives/ButtonPrimitive.vue` 是 B9+ 生成式 UI 的
LLM 按钮渲染器,与本任务同名不同物(见 Out of Scope)。

### A. 变体家族(规则级统计,~135 条;映射表见 design.md)

| 家族 | 规则数 | 占比 | 形态 | 代表 |
|---|---|---|---|---|
| ghost | ~44 | 33% | 透明底,hover 才有底 | 全部 modal close ×10、Sidebar/TitleBar/ProjectTabs 导航、settings 表单基底 |
| muted | ~32 | 24% | elevated/surface 弱底,hover 多变 accent-muted | ChatPanel 四兄弟(memory/audit/grants/trace)、cancel ×4、refresh 类 ×8 |
| accent-tint | ~19 | 14% | accent-muted 本体或 hover | EditFileCard/WorktreeChip/ToolCallCard、ChatPanel 四按钮 hover |
| primary | 13 | 10% | accent 实底 + accent-hover | settings 各 --primary、AskUser/PermissionAsk --always、PairingView、ChatInput send(圆形) |
| danger-soft | 9 | 7% | ghost 本体,hover 红 tint | RuntimeMemoryModal 等 ×6 + 裸 rgba ×3 |
| danger | 8 | 6% | tool-error 实底 | YoloConfirm/DeleteWorktree/ConfirmDialog --danger、ChatInput stop(圆形) |
| outline | 3-4 | 3% | 描边主导、透明底 | NodeListView retry、DiffPrimitive/CodeBlockPrimitive ui-prim |

前 5 类 ≈ 92% 规则可直接映射。另 3 处 ghost 本体 hover 变红实底
(TitleBar close / SessionList delete / ProjectTabs tab close)走
ghost + 本地 hover 覆写(3 处不达立变体门槛)。

### B. 尺寸与字号

- padding 48 种写法;三档(sm 4px 8px+text-xs / 默认 md 6px 12px+text-sm /
  lg 10px 16px+text-base)+ `--icon` 方形修饰可覆盖 ~85%,映射表见 design.md。
  奇数 padding(5/9/11px)12 处按保守档落整档。
- font-size:text-xs 38 + text-sm 37 两档占 82%;裸值漂移 `11px` ×2
  (DiffPrimitive:283、CodeBlockPrimitive:89)、`10px`(ChatInput:926)、
  `13px`(ProjectTabs:220)。

### C. 形状(按钮作用域 ~103 处 radius)

- `var(--radius-sm)` 73 处(~71%)→ 家族默认;`50%` 圆形真按钮 5 个
  → `--circle`;`999px` 药丸真按钮 4 个 → `--pill`(`--radius-pill` 已有)。
- 裸 radius 漂移:`3px` ×3(ProjectTabs:302、ReviewMatrix:191、Sidebar:361)、
  `4px` ×1(ProjectTabs:386)。
- **修正上轮认知:ChatInputTokenUsage.vue:825 的 `border-radius: 1px` 在
  token 进度条分段上,不是按钮**,不属本任务范围。

### D. 状态与动效惯例(顺带收敛的债)

- disabled:98 个样式块,`cursor: not-allowed` 29 处、opacity 34 处且 **5 种值**
  (0.5 ×20 / 0.6 ×10 / 0.4 ×2 / 0.7 / 0.55)→ 家族统一 0.5 + not-allowed。
- transition 69 处,~85% 已 token 化;裸值漂移仅 ProjectTabs ×4(`0.1s`)、
  SearchHistoryCard:302(`0.15s ease`)。主流组合
  `background, color, border-color` + fast + ease-out → 家族默认。
- **未定义 token(失效声明)**:`var(--color-bg-overlay)` ×2(PluginSelect.vue:473/559,
  `@theme` 无此变量)→ WP3 迁移时按视觉意图修。
- 裸 rgba 红 `rgba(239, 68, 68, 0.15)` ×3(ProvidersTab:682、ModelRow:261、
  DeleteModelConfirm:113)→ 归并 danger-soft 统一 color-mix 写法。
- fallback 噪音 `var(--radius-sm, 4px)` ×2(DiffPrimitive:280、CodeBlockPrimitive:86)。

### E. 热点文件 TOP10(65/167 = 39%)

ProvidersTab(8)、SearchModal(8)、RuntimeMemoryModal(7)、WorktreeChip(7)、
PermissionAskBody(6)、ChatPanel(6)、Sidebar(5)、AuditLogModal(5)、SessionList(5)、
ReviewMatrix/GroupChatConfigModal/ProjectTabs(4)。

### F. 特例(不进家族,保留原样 + 行内注释)

node-card 卡片按钮(NodeListView:271)、palette-dot 色板圆点(SessionList:1036)、
toggle-pill 开关(PluginSelect:485)、ui-prim 生成式 UI 家族(ButtonPrimitive/
DiffPrimitive/CodeBlockPrimitive —— LLM 渲染语义),共 6-8 条规则;
另 EmptyProjectState 的 `--ease-spring` :active press、FAB 阴影特例
(MessageList/SubagentDrawer)按 design-tokens.md 既有特例表保留。

## Requirements

- **R1 家族落地(style.css,WP1)**:`.btn` 基类 + 7 variant(`--primary`/
  `--danger`/`--danger-soft`/`--ghost`/`--muted`/`--tint`/`--outline`)+
  size(`--sm`/默认 md/`--lg`)+ shape(`--pill`/`--circle`/`--icon`);
  家族自带统一 disabled(0.5 + not-allowed)与 transition;默认 radius-sm +
  text-sm;**不写任何 focus 规则**(基线负责);注释块照 spinner 家族体例。
- **R2 全量迁移(WP2-5,按 modal/settings/chat/布局导航四簇分批)**:
  可映射规则全部改挂 `.btn` 修饰类,模板**只增类不删原 BEM 类**(测试/探针
  锚点不破坏),scoped 删家族拥有的声明、保留定位性声明;奇数 padding 落整档,
  裸 radius/font/transition/rgba 全部落 token。
- **R3 债清理(随 WP3/WP6)**:`--color-bg-overlay` 失效引用 ×2 修复;
  disabled opacity 五档归一;D 节裸值漂移清零。
- **R4 spec 回写(WP1)**:design-tokens.md 新增「Button family」节
  (照 z-index ladder 体例),含映射表、特例说明与 grep 审计命令。
- **R5 验证(每批 + 终盘)**:每 WP `pnpm test` + `vue-tsc`;WP2/4/5 批后
  `ui-review.sh --screenshots-only` 前后对照;WP6 全量 `ui-review.sh`(VLM)
  + 终盘 grep 审计 + 键盘 Tab 抽查 focus 环。

## Acceptance Criteria

- **AC1 全量收敛**:终盘审计 grep 证明按钮作用域内家族拥有的属性
  (background/padding/radius/font-size/transition/disabled 样式)仅剩 F 节
  特例清单 + design.md 记录的本地覆写(×4),每处有注释;`.btn` 消费点
  ≥ 120 处规则级。
- **AC2 债清零**:D 节所列漂移(未定义 token ×2、裸 rgba ×3、裸 radius ×4、
  裸 font ×4、裸 transition ×5、fallback ×2、disabled 五档)复跑盘点 grep
  全部清零,输出存 research/。
- **AC3 视觉无回归**:ui-review.sh 7 界面无新增缺陷;家族归一带来的有意
  收敛(±1-2px padding 归档、disabled/hover 归一)前后对照图存 research/。
- **AC4 测试与可达性**:pnpm test 全绿 + vue-tsc 0 错;迁移后按钮键盘 Tab
  focus-visible 环仍在(抽查含 modal close、ChatInput send、侧栏)。

## Out of Scope

- 生成式 UI `ui-prim__btn` 家族(ButtonPrimitive/DiffPrimitive/CodeBlockPrimitive)
  —— LLM 渲染语义特殊;是否未来消费 `.btn` 完工时写回 BACKLOG 附录 A 候选。
- 新增任何 `--color-*` token(design-tokens.md「一次性使用不立 token」红线)。
- `border-radius: 1px`(ChatInputTokenUsage:825,进度条分段非按钮)。
- Vue 组件封装(D3 明确走 CSS 家族路线)。
- role="button" 嵌套瑕疵与 Shift+F10 右键菜单(08-24 键盘任务另两条 follow-up)。
- AppShell legacy toast 与 ToastProvider 合并(design-tokens.md 既有 follow-up)。
