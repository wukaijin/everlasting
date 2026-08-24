# AC 证据归档(2026-08-24 收官)

## AC1 全量收敛 ✓

- 家族消费点:**141 处钮级** `class="… btn btn--*"`(grep -roh 统计),
  覆盖 primary 17 / muted 45 / ghost 64 / danger·danger-soft·tint·outline·
  pill·circle·icon·sm·lg 其余。
- 仍未挂家族的含 `<button>` 文件恰为 6 个特例(各有注释):
  - `chat/MessageImages.vue` — 图片缩略图瓦片(媒体平铺语义)
  - `settings/DefaultTab.vue` — reka RadioGroupItem 表单控件
  - `chat/ModeSelect.vue` / `ModelSelect.vue` — chat-input chip 三件套
    (与 PluginSelect 一致性,WP3 判定)
  - `chat/primitives/CodeBlockPrimitive.vue` / `DiffPrimitive.vue` —
    生成式 UI ui-prim 家族(Out of Scope 立项备案)
- 文档化本地覆写(行内注释):TitleBar/ProjectTabs/SessionList hover 红实底 ×3、
  RequestModeChangeCard plan(tool-read 实底)、WorkerMergeControls 绿变体、
  PermissionAskBody deny 红字、NodeListView retry 红边、TracePanel clear 红 hover、
  ChatInput staged-remove 删除语义 hover、MemoryLayerItem missing 防双重变暗、
  HiddenProjectsMenu trigger 计数 hover、ReviewMatrix tab 透明边防跳位、
  gcfg-add dashed、PendingBadge/WorktreeChip 分体药丸几何等。

## AC2 债清零 ✓(复跑盘点 grep)

- `var(--color-bg-overlay)` 失效引用:0(原 ×2,PluginSelect → bg-hover wash)
- `var(--color-text)` 失效引用:0(原 ×1,ChatPanel,WP4b 顺手清)
- 裸 `rgba(239,68,68,…)`:0(按钮 hover ×3 随 WP2/3 迁移消失;diff 行底 ×3
  WP6 token 化为 color-mix tool-error 12%)
- `var(--radius-sm, 4px)` fallback:0(原 ×2 ui-prim)
- 按钮裸 transition 秒数(0.1s×4/0.15s×1):0(ProjectTabs/SearchHistoryCard)
- 按钮裸 radius(3px×3+4px×1):0(ReviewMatrix/ProjectTabs/Sidebar)
- 按钮裸 font-size(11px×2/10px/13px):0(→ --text-2xs/token/删除)
- disabled opacity 分档:家族统一 0.5;残留非按钮(input 表单控件)除外

## AC3 视觉无回归 ✓

- 全量 ui-review(7 界面 VLM,`research/ui-review-final-report.md` +
  `out/ui-review/0824-btn-final/`):**零渲染坏掉级缺陷**(无背景丢失/文字
  缺失/错位/重叠);仅存例行观感建议(热区/间距/层级),其中两条
  (删除钮悬停警示色、图标热区)属静态截图看不见 hover/触控的已知 VLM 局限
  (AGENTS.md 方法论注明),代码级复核:尺寸未变(22×22 等),删除钮
  hover 实为 danger-soft 红 tint。
- 中间检查点 ×2:WP2+3 后 settings/memory VLM 抽查零异常;WP4 后
  chat 界面 VLM 判 send 圆钮/chips/重试全部正常。
- 8/17 基线无文本报告可 diff(仅截图);对比以「缺陷类别」为准。

## AC4 测试与可达性 ✓

- `pnpm test`:**80 文件 / 1175 全绿**(每 WP 批后复跑,共 6 轮)
- `vue-tsc --noEmit` 0 错;`pnpm build`(vite)通过
- 焦点环:`git diff a30cf30..HEAD` 中按钮作用域 **:focus 规则增删均为 0**
  —— 家族零焦点声明,08-22 `:where()` 基线完整承接;MessageActionsMenu
  自有焦点增强保留(注释)。
- 测试锚点零破坏:BEM 类全部保留(WorkerMergeControls/YoloConfirm/
  ConfirmDialog 等测试锚定的类名实证未动)。

## 过程缺陷(留痕)

- WP5 代理撞 5h 限额中断(7/14 文件),主代理接手补完。
- 脚本 replace 两处静默失败(TurnTimeline/ReviewFindingDetail 模板类缩进
  不匹配,样式已删类未挂)——终盘「无家族类文件清单」审计兜住并修复;
  该审计命令已沉淀在本文件 AC1 节。
