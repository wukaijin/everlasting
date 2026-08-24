# Design — Button CSS 家族收敛

## 决议(2026-08-24,planning 定稿)

- **D1 单任务全量收敛**:一个任务分 6 个 WP 批次实施,不留半迁移状态。
  依据:半收敛 = 同屏两套按钮观感并存;迁移是机械操作(挂类 + 删 scoped
  声明),风险集中在验证,批间截图对照控制。
- **D2 视觉归一保守档**:家族归一接受**小视觉收敛**(奇数 padding 落整档、
  disabled 五档归一、hover 写法统一),但尺寸阶梯贴合现值不重排(不整体
  放大/缩小任何按钮档位)。改动前后 ui-review 截图对照留档。
- **D3 不引入 Vue 组件**:纯 CSS 家族,spinner 单先例。

## 架构与边界

`app/src/style.css` 新增 `.btn` 家族区(照 spinner 家族区注释体例,
放在 focus-visible 基线与 spinner 区之间)。

**家族拥有的属性**(迁移后组件不得重复声明,豁免需注释):
background、color、border(宽度/颜色/样式)、padding、border-radius、
font-size、cursor、transition、`:hover` / `:disabled` 状态。
**不拥有**(实现期修正):font-weight 与 line-height —— 继承为默认,
偏差站点保留单行本地声明;gap 基类给 `--space-1` 默认,组件可自由覆写。

**组件保留的属性**:定位(margin / position / flex / align-self)、显式尺寸
特例(fixed width/height)、gap(内容排布)、z-index、阴影(特例表内的
FAB 类)、动画(spinner 呼吸等非按钮状态动画)。

**焦点样式零声明**:家族不写任何 :focus 规则 —— 08-22 全局
`:where()` 基线负责焦点环。家族类若声明 box-shadow 会以 (0,1,0) 特异性
压过零特异性基线,所以禁写。

## API(类契约)

```
.btn                 基类:inline-flex 居中、text-sm、radius-sm、cursor:pointer、
                     transition(background,color,border-color + --duration-fast + --ease-out)、
                     :disabled(opacity 0.5 + cursor:not-allowed)
.btn--primary        accent 实底 + on-accent 文字 + accent-hover
.btn--danger         tool-error 实底 + on-accent 文字 + hover 深化
.btn--danger-soft    ghost 本体 + hover 红 tint + error-text 文字
.btn--ghost          透明底 + hover --color-bg-hover(10% primary wash)
.btn--muted          elevated 底 + hover accent-muted + primary 文字
.btn--tint           accent-muted 底 + hover 加深(实现期对截图定)
.btn--outline        bg-border 描边 + 透明底 + hover elevated
.btn--sm / --lg      尺寸修饰,默认 md
.btn--pill           --radius-pill 药丸
.btn--circle         50% 圆(需配合固定尺寸,常与 --icon 组合)
.btn--icon           icon-only 方形(aspect-ratio 1,padding 收紧),与 size 正交
```

### 变体映射表(盘点 A 节 → 类)

| 现状家族(规则数) | 目标类 | 备注 |
|---|---|---|
| primary(13) | `.btn .btn--primary` | ChatInput send 另加 `--circle` |
| danger 实底(8) | `.btn--danger` | ChatInput stop 另加 `--circle`(呼吸动画保留本地) |
| danger-soft(9) | `.btn--danger-soft` | 裸 rgba ×3 与 token ×6 统一为 color-mix(tool-error 12%) |
| ghost(44) | `.btn--ghost` | hover 目标归一 `--color-bg-hover`;hover 变 accent-muted 的 15 处实际是 muted/tint 语义,按本体归类 |
| muted(32) | `.btn--muted` | hover 统一 accent-muted(现状主流) |
| accent-tint(19) | `.btn--tint` | 本体 accent-muted 的 4 处直接映射;hover 变 accent-muted 的 15 处归 `--muted`(见上) |
| outline(3-4) | `.btn--outline` | NodeListView retry 的 tool-error 边框特例:outline + 本地 border-color 覆写 |
| ghost+hover 红实底(3) | `.btn--ghost` + 本地 hover 覆写 | TitleBar close / SessionList delete / ProjectTabs tab close;不立 `--close` 变体(3 处不达家族门槛) |

### 尺寸映射表

| 档 | padding | font-size | 吸收现状值 |
|---|---|---|---|
| `--sm` | 4px 8px | text-xs | 4px 8px / 2px 8px / 3px 8px / 2px 10px / 1px 6px(孤值) |
| md(默认) | 6px 12px | text-sm | 6px 12px / 6px 10px / 5px 12px(→6) / 4px 12px / 6px 8px / 5px 10px(→6 10?逐处对照,原则上 5→6) |
| `--lg` | 10px 16px | text-base | 10px 16px / 12px 16px / 11px 22px(→10 16)/ 9px 16px(→10 16) |
| `--icon` | 4px 方形 | 继承 | padding: 0 ×22 / 4px ×14 的 icon-only;配 Icon 组件偶数尺寸规则 |

注:6px 是 --space-1(4)/--space-2(8) 半步 —— design-tokens.md 明文不立
半档 token,家族区在 style.css(token 定义文件)内用 raw 值 + 注释,合规。
奇数 padding(5/9/11)12 处按上表落整档(D2 保守归并,±1px 内)。

## 状态与动效语义

- disabled:统一 `opacity: 0.5` + `cursor: not-allowed`(五档 0.4/0.55/0.5/0.6/0.7
  归一到主导值 0.5 ×20)。
- transition:家族统一 `background, color, border-color` + fast + ease-out
  (主流组合 ×15 已如此);ProjectTabs ×4(0.1s)、SearchHistoryCard(0.15s)
  裸值随迁移消失。
- `:active` press(--ease-spring)不入家族:仅 EmptyProjectState 一处,保留本地特例。

## 迁移机制(每处三步)

1. **模板只增不删**:原 BEM 类保留(测试 / `:deep()` / 探针可能锚定),
   追加 `btn btn--{variant}[--{size}][--{shape}]`。
2. **scoped 删家族拥有的声明**,保留定位性声明;删不干净会以 `[data-v]`
   特异性压过家族,终盘 grep 审计兜底。
3. **特例注释**:不进家族的(F 节清单)或本地覆写的(hover 红实底 ×3、
   outline 红边 ×1),行内注释说明原因。

## 批次与验证策略

| WP | 范围 | 验证 |
|---|---|---|
| WP1 | style.css 家族落地 + design-tokens.md 回写「Button family」节(照 z-index ladder 体例) | pnpm test + vue-tsc(纯增量,零迁移) |
| WP2 | modal/confirm 簇:ConfirmDialog、YoloConfirm、DeleteWorktree、DeleteModelConfirm、RuntimeMemoryModal、GroupChatConfigModal、MemoryModal、DiffModal、AuditLogModal、MarkdownDetailModal、SettingsModal、PermissionGrantsModal | test + vue-tsc + `ui-review.sh --screenshots-only` 对照 |
| WP3 | settings 簇:ModelForm、ProvidersTab、ModelsTab、RemoteTab、ModelRow、PluginSelect(含 `--color-bg-overlay` 失效修复) | 同上 |
| WP4 | chat 簇:ChatPanel、ChatInput、AskUserQuestionCard、PermissionAskBody、RequestModeChangeCard、RequestTaskStateTransitionCard、MessageItemEdit、MessageItemFooter、ToolCallCard、EditFileCard、WorktreeChip、SubagentDrawer、MessageList、ChecklistCard、SearchHistoryCard、WorkerAskBanner、PendingBadge | 同上 |
| WP5 | 布局导航簇:Sidebar、TitleBar、ProjectTabs、SessionList、ModeSelect、ModelSelect、TriggerMenu、HiddenProjectsMenu、SearchModal、ReviewMatrix、TracePanel、DrawerSection、DrawerPromptCard、NodeListView、PairingView、EmptyProjectState、ChatPanel 残余 | 同上 |
| WP6 | 长尾清余 + 债清扫(disabled 五档、裸 rgba/radius/font/transition、fallback 噪音)+ 终盘 grep 审计(复跑盘点命令)+ 全量 `ui-review.sh`(VLM) | 全量验证四件套 |

- 每 WP 独立 commit,单 WP 可独立 revert(回滚点)。
- VLM quota 控制:批间 `--screenshots-only` 人工/像素对照,仅 WP6 后跑全量。

## 风险与对策

| 风险 | 对策 |
|---|---|
| 视觉回归面大(69 文件) | 批间截图对照 + WP6 全量 VLM;AC3 留前后对照图 |
| 测试锚定类名/样式快照 | 只增类不删类;失败逐处归因,不为过测试改契约 |
| hover 归一改变部分按钮观感(modal close elevated→bg-hover 等) | 属任务目标内的有意收敛(D2),截图对照确认可接受 |
| scoped 残留压过家族 | WP6 终盘 grep:按钮作用域内 background/padding/radius/font-size/transition 的残留清点 |
| ChatInput send/stop 动画与形状耦合(呼吸 + 圆形) | 形状进 `--circle`,动画保留本地;迁移时单独截图验证 |
