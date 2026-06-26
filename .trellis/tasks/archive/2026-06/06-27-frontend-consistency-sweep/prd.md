# 前端一致性收口：token 漂移 / 列表动效 / 布局复核 / 配色取舍 / 主题结构预留

## Goal

前端经过 8 个 PR 的 polish + design-tokens 体系（color/spacing/radius/type/motion/shadow 六大家族）后已相当成熟。本任务做一次"收口"：清理残留 token 漂移（违反自身 design-tokens.md 的硬编码）、补齐动效最后一环（列表 enter/leave）、复核布局细节，并对两个前瞻取舍（配色区分度、light theme 结构）做出明确决策（做 or 不做）。所有改动遵循现有 token 体系，不引入新设计语言。

## What I already know（已侦察，file:line 实锤）

### ① Token 漂移（违反 design-tokens.md "禁止硬编码"）

- **硬编码 box-shadow（绕过 `--shadow-*`）共 12 处**，最值得收口的是 modal 那一档 `0 16px 48px rgba(0,0,0,0.5)` 重复 3 次：`DeleteWorktreeConfirm.vue:137` / `YoloConfirmModal.vue:199` / `MarkdownDetailModal.vue:230` → 暗示 shadow scale 缺一档 `--shadow-xl`（现有 `--shadow-lg` = `0 8px 24px`，modal 用不够大才各自写更大）
  - 其余：`AppShell.vue:83`(toast, 应为 `--shadow-md`)、`ModeSelect.vue:350` / `WorktreeChip.vue:308`(应为 `--shadow-sm`)、`MemoryLayerItem.vue:240/249`(color-mix 状态环)、`SessionList.vue:568`、`HiddenProjectsMenu.vue:178`、`EmptyProjectState.vue:187`、`MessageActionsMenu.vue:350`
- **硬编码 `font-size: 10px` 共 6 处**（比 type scale 最小值 `--text-xs: 11px` 还小，是没纳入 scale 的"地下层"）：`MemoryPreview.vue:354` / `MemoryLayerItem.vue:374` / `DiffView.vue:316` / `MessageItem.vue:715`((edited) 标签) / `MessageActionsMenu.vue:427`
- **AppShell.vue 硬编码**：`:97/:102 color: #ffffff`、`:76 bottom: 24px`、`:78 padding: 10px 18px`
- **markdown 代码块白底魔数**：`MessageItem.vue:793 rgba(255,255,255,0.08)` / `:800 rgba(255,255,255,0.06)`（dark-only，light theme 必坏）
- **MemoryLayerItem 硬编码色**：`:240 #4ade80` / `:249 #fbbf24`（green/amber 状态环，绕过 tool 色族）
- ✅ 甄别：`MessageItem.vue` 的 `1.4em / 0.9em / 0.95em` 是**相对单位**（相对气泡字体），合理，**不算漂移**，不动

### ② 列表动效（最大体验空白）

- 全项目仅 `AppShell.vue` 用 `<transition>`（toast），**无任何 `transition-group`**
- 后果：新消息（用户发送 / assistant 首字）突然冒出、工具卡片展开收起硬切、session 增删硬切
- motion token（`--duration-base/slow` + `--ease-out/decelerate`）配齐，但只在 hover/active 消费，enter/leave 几乎未消费

### ③ 布局细节

- `ChatPanel.vue:805 padding: 20px 4px 0px 20px` —— 右 4px、下 0px 不对称（右侧贴边、底部 input 顶死），疑为给滚动条留位。可用 `scrollbar-gutter: stable` 替代

### ④ 配色取舍（preference，需决策）

- user 气泡（`accent-muted` 深蓝）与 assistant 气泡（`bg-elevated` 深灰）都是深色块，区分度偏弱
- 缺通用"成功/正面"语义色（`--color-tool-write` emerald 被 design-tokens.md 明确记录为 success 复用）

### ⑤ Light theme 结构（前瞻，需决策）

- grep 确认：**无任何 `[data-theme]` 结构**，所有色写死在 `@theme`
- design-tokens.md 写"dark only, reserve light extension"，但实际零预留
- markdown 白底魔数 + MemoryLayerItem 硬编码色 = 一旦做 light theme 大面积坏

## Decision (ADR-lite)

**D1 — `--text-2xs: 10px`（新增）**：承认 6 处 10px 现状，纳入 type scale，语义"仅 caption / 角标元数据"，与 `--text-xs: 11px` 形成阶梯。design-tokens.md Type Scale 表补一行。
**Context**: 6 处 `font-size: 10px` 是 PR-2 sed sweep 漏网的"地下层"，比 scale 最小值还小。
**Consequences**: type scale 多一档（xs 之上多 2xs），但消除游离魔数；后续 caption 类一律 `--text-2xs`。

**D2 — `--shadow-xl: 0 16px 48px rgba(0,0,0,0.5)`（新增）**：收口 modal 3 处重复硬编码，shadow ladder 完整（xs/sm/md/lg/xl）。popover-pattern.md 同步"modal = xl"。
**Context**: 现有 `--shadow-lg: 0 8px 24px` 对 modal 不够大，3 个 modal 各自写了更大的 `0 16px 48px`。
**Consequences**: 它已被 3 处复用，过"不为一次性新增 token"门槛，新增合理。

**D3 — ④⑤ 一并实施**：本任务实施 ①②③④⑤ 全部。PR5 拆为 ④(配色) + ⑤(light 结构) 两子 PR。

**D4 — 引入 `--color-status-success` / `--color-status-warn`（PR2 修订，推翻原"暂不新增"）**：PR2 侦察发现状态色 `#4ade80`(green-400) / `#fbbf24`(amber-400) 已在 4 组件硬编码（MemoryPreview / MemoryLayerItem / ChatInputHintRow / FileInjectionsHint），且 `FileInjectionsHint.vue:214` 已用 `var(--color-status-success, #4ade80)` fallback——说明项目预期该 token 存在只是未定义。过"3+ 组件"门槛，引入两个 status token 收口。
**PR2 实际规模**（侦察修正）：font-size:10px **21 处**（非 6）、`#ffffff` **14 处**（全部 on-accent/on-error 白字，归 `--color-text-on-accent`）、rgba 白底 2 处（→ color-mix text-primary）。原 D4"暂不新增"判断作废（证据变化）。

**D5 — ⑤ light theme 结构预留（PR5b 改用方案 X）**：原方案"挪 @theme 到 `:root[data-theme='dark']`"经评估有风险——`@theme` 是 Tailwind v4 注册 token 的指令，挪空可能影响 Tailwind 内部；且 grep 确认项目纯 `var()` 无 utility 引用，挪动零收益。**改为**：`@theme` 保持为 dark 默认（不动），style.css 末尾加 `:root[data-theme="light"]` 占位作扩展点。未来加 light theme：填该块 + 设 `data-theme`，覆盖靠特异性赢（`[data-theme]` > `:root`），零 per-component 改动。零视觉变化，Tailwind 不受影响。

**D6 — ④ 气泡区分度（方案 A）**：user 气泡（`MessageItem.vue` `.msg--user .msg__bubble`）加 3px accent 左边条 `border-left: 3px solid var(--color-accent)`，复用 tool card 左色条视觉语义。不动 PR-3a 的 muted 底色（两角色仍等权重）。assistant 气泡不变。

## Open Questions

_全部收敛。见 Decision D1–D6。_

## Requirements

- ① 清理全部 token 漂移，组件 CSS 零硬编码（shadow / font-size / 色 / px），符合 design-tokens.md
- ② MessageList 新消息 enter 动效（fade + translateY），reduced-motion 下静默
- ③ ChatPanel padding 对称化（`scrollbar-gutter: stable` 替代右贴边）
- ④（方案待定 A/B/C）加强 user/assistant 气泡区分度
- ⑤ light theme 结构预留（D5 方案）+ markdown 白底魔数改 `color-mix` + MemoryLayerItem 硬编码色归位

## Acceptance Criteria (evolving)

- [ ] `grep "box-shadow: 0\|font-size: [0-9]\|rgba(255, 255, 255\|#ffffff" app/src/components/` → 仅剩 spec 允许的例外
- [ ] DEBT.md 记录收口项，完成后闭合删除
- [ ] design-tokens.md 同步新增 token（若有）+ popover-pattern.md 同步 modal shadow
- [ ] vitest 绿 + vue-tsc 绿
- [ ] reduced-motion 下动效静默

## Definition of Done

- 单测 / 类型检查绿
- design-tokens.md / popover-pattern.md / DEBT.md 同步
- 分批 PR 提交（见 Technical Approach）

## Technical Approach（分批 PR）

- **PR1（token 漂移 · shadow）**：`style.css` 加 `--shadow-xl: 0 16px 48px rgba(0,0,0,0.5)`；modal 3 处 + AppShell/ModeSelect/WorktreeChip/SessionList/HiddenProjectsMenu 等替换为 token；popover-pattern.md 同步 modal=xl
- **PR2（token 漂移 · text/色/px）**：10px 决策落地（`--text-2xs` 或上抬）；AppShell `#ffffff`/`24px`；markdown 白底 → `color-mix`；MemoryLayerItem 硬编码色 → token/复用
- **PR3（动效）**：MessageList `TransitionGroup` enter —— user 从右(+24px)、assistant 从左(-24px) 方向化 translateX + opacity fade；`appear` 开（首条/切换也动画）；`:deep()` + `transition !important`（绕开 scoped 子组件根匹配 + `.msg` 高特异性 transition 覆盖两个坑）；`.messages` `overflow-x: hidden`（避免 translateX 出界冒水平滚动条）。详见 design-tokens.md "List enter (TransitionGroup)"
- **PR4（布局）**：ChatPanel padding 对称化
- **PR5（决策类 ④⑤，视 MVP 边界）**：配色取舍 / light theme 结构预留

## Out of Scope

- 推翻现有设计语言（Prussian blue 冷色系不变）
- 引入新组件库 / 动画库
- （待确认）light theme 实际实现 —— 本任务最多做结构预留，不做 light 配色

## Technical Notes

- design-tokens.md：`.trellis/spec/frontend/design-tokens.md`
- popover-pattern.md：`.trellis/spec/frontend/popover-pattern.md`
- 全局样式：`app/src/style.css`（`@theme` block）
- DEBT.md：`.trellis/reviews/DEBT.md`（当前未记录这些项，PR1 同步补）
- reduced-motion 已在 style.css 顶层 `@media` 兜底（全局限 0.01ms）
