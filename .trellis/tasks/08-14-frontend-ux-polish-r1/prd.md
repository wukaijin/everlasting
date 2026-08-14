# 前端样式与体验优化第一轮(移动端触控/排版/选中态/路由过渡)

## Goal

基于 2026-08-14 的 VLM 视觉评审(`research/vlm-findings-map.md`),做一轮**纯样式/模板层**的体验打磨:移动端触控达标、长文阅读节奏、选中态与指示器辨识度、路由切换过渡。目标是评审缺陷清单可度量地收敛(用 `scripts/ui-review.sh` 前后对照),且**桌面端零回归**。

## Background

- 评审基线:`out/ui-review/20260814-215224/`(dist @ 2026-08-14 21:30),逐图 VLM 报告见 `research/vlm-report-20260814.md`,发现→代码定位见 `research/vlm-findings-map.md`(含真伪验证)。
- 本任务**不含**功能开发:trace↔消息联动、OTP 分段输入、bundle 优化均明确 out of scope(见 research 文件"明确不做")。

## Requirements

### WP1 移动端触控与顶栏收纳(P0)

- R1.1 聊天顶栏功能图标(记忆/审计/trace/授权)及消息操作按钮,移动端触控目标 ≥44px——视觉尺寸可不变,扩大 hit area(transparent padding / min-height)。沿用 `responsive-mobile.md` §6 既有模式,把覆盖面从 modal 内部扩展到聊天高频触点。
- R1.2 移动端顶栏信息收纳:logo/面包屑/截断标题/chips/图标的堆叠按 spec 的 CSS overlay 模式收纳(进 overflow 菜单或隐藏低频项),不做组件重构。
- R1.3 ChatInput 内 Edit/Wf chips 移动端不再挤压输入区(移到输入框上方一行或收纳)。
- R1.4 配对页视觉层:占位符对比度、"配对码"与"设备名"输入框风格统一、配对按钮按输入长度 disabled。分段 OTP 输入**不做**(MVP 决策)。

### WP2 长文排版节奏与微字号(P1)

- R2.1 排查 markdown 渲染的段落/条目垂直节奏(p/li/blockquote/h* 的 margin),消除"密集感";**不是**调气泡正文 line-height(已 1.6,验证结论见 research B1)。
- R2.2 长文容器统一走 `--leading-relaxed`;个别仍用 1.4/1.5 的长文选择器(实现时 grep 复核)对齐 token。
- R2.3 微字号治理:常驻 UI 文字最小 11px(`--text-xs`);10px(`--text-2xs`)只允许用于非关键装饰性角标,latency/耗时类信息若保持小字则需 hover/tap 可展开。
- R2.4 中英混排间距(盘古之白):评估 `text-autospace` 支持度,成本低就加,不支持则记录不做。

### WP3 选中态与指示器(P2)

- R3.1 侧栏会话选中项加左侧 2px accent bar(保留现有底色 tint)。
- R3.2 设置弹窗 Tab 选中态加明确指示器(下划线或背景块,二选一,与现有组件风格一致)。
- R3.3 桌面顶栏图标区按功能分组加细分隔线;"已加密保存"类常驻提示降级为 tooltip;补缺失的图标 aria-label/tooltip。

### WP4 路由与切换过渡(P2)

- R4.1 `App.vue` 路由切换加 fade 过渡(`v-slot` + `<Transition>`,用现有 `--duration-base`/`--ease-decelerate` token;遵守 prefers-reduced-motion)。
- R4.2 会话切换时聊天内容 cross-fade(复用 MessageList TransitionGroup 既有机制,避免与消息进入动画打架)。

## Constraints

- 所有颜色/字号/间距/时长**必须走 design token**(`design-tokens.md`),不引入新 hex/px 字面量;确需新 token 先进 `style.css` 再消费。
- 移动端改动沿用 `responsive-mobile.md` 的 desktop-first overlay 模式(`max-width` media query),桌面端样式零改动、零回归。
- 不新增依赖;不改 store/transport 层;模板改动限于 class/结构微调。
- 动效遵守既有 motion 词汇表与 `prefers-reduced-motion` 全局规则。

## Acceptance Criteria

- [ ] AC1 移动端(390px)聊天顶栏与消息操作的触控目标 ≥44px(实现时用 ui-review.sh 截图 + DOM 断言或手动 devtools 验证)。
- [ ] AC2 `scripts/ui-review.sh` 重跑:本轮覆盖的缺陷类目(触控目标 / 顶栏堆叠 / 微字号对比 / 选中态 / Tab 指示器)在 VLM 报告中不再出现或明确收敛;报告存入本任务 `research/`。
- [ ] AC3 桌面端 1440px 视觉零回归:ui-review.sh 桌面截图与基线对比无样式性差异(内容自然变化除外)。
- [ ] AC4 路由(/ ↔ /chat ↔ /nodes)切换有 fade 过渡,且系统开启 reduce-motion 时无动画。
- [ ] AC5 `cd app && pnpm test` 全绿;`pnpm build`(含 vue-tsc)通过。
- [ ] AC6 改动无硬编码色值/字号(新增样式全部引用 token;check 阶段 grep 复核)。

## Notes

- 评审发现的"行高偏紧"已被验证为**部分不成立**(正文 1.6),实现 WP2 时以 research B1 的结论为准,避免被 VLM 误导。
- 本任务为中等复杂度:PRD + implement.md,无独立 design.md(技术决策已内联到 implement.md 并引用 spec)。
