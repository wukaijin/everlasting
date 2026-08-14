# 执行计划 — 前端样式与体验优化第一轮

> 依据:`prd.md`(需求/验收)+ `research/vlm-findings-map.md`(发现→代码定位,含真伪验证)。
> 顺序 = WP1 → WP2 → WP3 → WP4(优先级排序,各 WP 独立可验证,可分批提交)。

## 前置

- [ ] `./scripts/daemon.sh restart --no-build` serve 最新 dist,跑 `./scripts/ui-review.sh --screenshots-only` 留基线(若 dist 未重建则先 `cd app && pnpm build`)
- [ ] 读 spec:`design-tokens.md`、`responsive-mobile.md`(§1.2 overlay 模式、§6 触控)、`chat.md`

## WP1 移动端触控与顶栏收纳(P0)

- [x] 1.1 `ChatPanel.vue`:顶栏四个 `__btn` 移动端扩 hit area 至 ≥44px——视觉保持 32×32,`::after { inset: -6px }` 透明外扩(32+6×2=44)+ `position: relative`;`title-actions` gap 0→12px 防相邻外扩区互相覆盖(08-14 ux-polish-r1)
- [x] 1.2 `MessageActionsMenu.vue`:`msg-actions__trigger` 视觉 22×22 不变,`::after { inset: -11px }` 外扩至 44px;顺带把 dropdown 菜单项 `min-height: 44px`(与 style.css modal 按钮 44px 档一致)
- [x] 1.3 `ChatPanel.vue` 顶栏:cwd/git/worktree chips S6a 已隐藏;本轮补 `mobile-hide-group-chat`(群聊/编辑参与者入口,低频,移动端隐藏,编辑回桌面端);4 图标保留
- [x] 1.4 `ChatInput.vue`:移动端 row 切 grid(`auto 1fr auto`),ModeSelect/PluginSelect 移到编辑框上方一行;chips 用 margin-bottom 间距(而非 row-gap,避免无 session 时垫出幽灵行距);TriggerMenu abspos 不受影响。**check 修正(08-14)**:原实现把 `gap` 整体归零、只给 chips 补 margin,漏了横向——编辑框(r2 跨 c1-c2)与发送/停止按钮(r2 c3)0px 贴死(桌面/S6a 是 8/6px)。改为 `row-gap: 0` + `column-gap: var(--space-2)`(纵向仍走 chips margin-bottom 规避空 grid 行 1 的幽灵行距——field 显式落在行 2 时空行 1 依然存在,任何 row-gap 都会泄漏;横向 column-gap 不受影响),plugin-select 的 margin-left 移除(由 column-gap 提供,避免 16px 双倍间距)
- [x] 1.5 `PairingView.vue`:两输入框 `::placeholder` 统一 muted(占位/已输入可辨)、padding 统一 10px + focus 焦点环 `--shadow-ring`、`#fff` → `--color-text-on-accent`。**注**:配对按钮 `:disabled="!canSubmit"`(code 长度=6)原本已存在,VLM 评审 A5 该子项在基线 dist 中即已实现,本轮未重复改动
- 验证:390px 视口 devtools 量触控目标;`01/06` 截图对照;桌面 1440px 无变化(主会话统一跑视觉评审)

## WP2 长文排版节奏与微字号(P1)

- [x] 2.1 markdown 垂直节奏:`MessageItem.vue` p 8→12(--space-3)、li 2→4、ul/ol 6→4/12(尾元素清零)、h* 12/6→16/4、blockquote 8→8/12,pre/table/hr 原值仅 token 化;行高 1.6 未动(research B1)。**根因发现**:`DiscussionSummaryCard.vue` 复用 `.msg__markdown` 类名但 MessageItem 的 :deep 规则不跨作用域生效,preflight 把 p/li margin 清零 → 总结正文原本零间距,已补齐镜像节奏;`MarkdownDetailModal.vue` / `SubagentDrawer.vue` reply-markdown(自述 mirror MessageItem)同步改版
- [x] 2.2 长文容器 line-height 统一:`MarkdownDetailModal` 1.55 / `SubagentDrawer` reply 1.55 / `DiscussionSummaryCard` 1.6 / `PairingView` subtitle 1.6 → `var(--leading-relaxed)`;`ToolCallCard` subagent-summary 1.5、`ReviewFindingDetail` 1.5 → `var(--leading-relaxed)`(多行 prose);`SubagentDrawerErrorCard` 1.5 → `var(--leading-normal)`(mono 错误文本,仅 token 化)。**保留**:代码块 `pre` 1.45(代码节奏非密集感来源)、chip/角标 1.4、mono 数据块(ToolInput/OutputBody)1.4
- [x] 2.3 `--text-2xs` 逐点审查(40+ 消费点):升 `--text-xs` 的常驻信息型文字——`SessionGroupHeader` 标签+计数、`SessionList` compact meta(时间戳)、`MemoryLayerItem` 路径、`RuntimeMemoryModal` stat-label、`MemoryPreview` runtime-memory 时间戳、`ChatPanel` recall-item。**保留 2xs**:pill/badge 类(PendingBadge/WorkerBranchBadge/DiffView status/ModelRow tag/PermissionAskBody badge/AskUserQuestionCard chip 等)、菜单内过渡性文字(TriggerMenu/ModelSelect/ModeSelect captions——design-tokens.md 已记录为 2xs 预期消费方)、`(edited)` 标记(带 title)、latency popover 头(非常驻)
- [x] 2.4 盘古之白:**已做**——`text-autospace: ideograph-alpha` 加到 `.msg__markdown`(style.css 全局,MessageItem + DiscussionSummaryCard 共用)+ `MarkdownDetailModal` + `SubagentDrawer` reply-markdown。支持度(2026-08 核实):Chromium ≥140 默认支持(Android Chrome PWA / Windows WebView2 / ui-review headless Chromium);WebKit(iOS Safari/WKWebView/WebKitGTK)与 Firefox 不支持 → 声明被忽略,零风险渐进增强。`text-spacing-trim`(全角标点压缩)改变标点宽度、排版影响大,本轮不启用;JS 注入方案(改 markdown 管线)成本不划算,不做
- 验证:`01-chat-desktop` 截图对照密集感(主会话统一跑);`pnpm test`(chat 相关快照)

## WP3 选中态与指示器(P2)

- [x] 3.1 `SessionList.vue` 会话项左侧 accent 指示:**复核结论 = 已存在,零改动**。`.session-item` 基线即有 `border-left: 2px solid transparent`(常宽,零布局抖动,等价于推荐的 inset box-shadow 方案),`--active` 态设 `border-left-color: var(--color-accent)` + `background: var(--color-bg-selected)`(12% tint)+ dot 变 accent,2026-06-06(4628049)起就在。C1 误判原因:基线 `05-sidebar.png` 里非"今天"分组默认折叠(今天 expanded、其余 collapsed,localStorage 持久化),评审环境无当日会话 → 截图零会话行,VLM 只看到 token 值
- [x] 3.2 Settings Tab 选中指示:**复核结论 = 桌面下划线已存在,零改动**。`.settings-modal__tab` 基线 `border-bottom: 2px solid transparent`,`[data-state="active"]` 设 `border-bottom-color: var(--color-accent)`(2026-06-09 cb00812 起);移动端 S6b 已改 pill 高亮。选择理由:维持现状即"下划线(桌面)+ pill(移动)",与 Linear 风格及既有组件语言一致。C3 误判:用 VLM 复析同一张基线 `02-settings.png` 可明确看到 Providers tab 蓝色下划线
- [x] 3.3 `ChatPanel.vue`:memory(项目级)与 audit/trace/grants(会话级)之间加 1px × 16px `--color-bg-border` 竖分隔线(`chat-panel__action-divider`,`v-if` 两端都渲染才显示;移动端保留,断口 25px > 组内 12px,分组语义可读,与 WP1 收纳无冲突)。`ProvidersTab.vue`:"已加密保存"降级为 title tooltip + `cursor: help`("未设置"是可行动警告,桌面保留可见文字;移动端维持 S6b 只留图标);补 aria-label/title:编辑/删除 icon 按钮(原裸图标)、API key 显隐按钮补 aria-label(原有 title)。ChatPanel 4 按钮、SettingsModal close 等本轮触到的组件逐一核对无其它缺口
- 验证:`02-settings`、`05-sidebar` 截图对照(主会话统一跑视觉评审)

## WP4 路由与切换过渡(P2)

- [x] 4.1 `App.vue`:`v-slot="{ Component }"` + `<Transition name="route-fade" mode="out-in">`(三个 view 均单根节点,Transition 可用);`.route-fade-*` 定义进全局 style.css(视图根节点无组件级挂点),`opacity` 单属性过渡,`--duration-base`(150ms)+ `--ease-decelerate`,仅动合成器属性。reduced-motion 已验证:style.css 顶层 `@media (prefers-reduced-motion: reduce)` 的 `*` + `transition-duration: 0.01ms !important` 选择器覆盖任何携带 `.route-fade-*-active` 的元素 → 路由切换退化为瞬时
- [x] 4.2 会话切换 cross-fade:**修 bug 而非加动画**。排查发现 5b1fc81(2026-07-30 交错思考 run 分组)把 TransitionGroup 直接子节点换成 run-group `<li>` 后,enter 类落在 run-group 上,旧选择器 `.msg--user/.msg--assistant.msg-enter-from`(方向类 + enter 类需同元素)从此匹配不上 —— **enter/appear 动画静默失效两周+**(新消息无划入、切会话重挂载无动画),这才是 D2 "硬切"的根因。修复:from 态重定向到 run-group li(`.msg-enter-from { opacity: 0; translateX(24px) }`,整 run 从用户侧划入 —— run 由用户消息开启,沿用原方向词汇;流式追加的 assistant turn 归入已有 run 不重复触发,符合分组语义);`appear` 原本就在,切会话(skeleton→重挂载)与首挂载全列表划入即 D2 要的 cross-fade。**不加容器级 key fade**:会与 run 级 enter 在同一元素链上双重动画,且 out-in 模式把重挂载推迟一个 duration,与 `stickToBottomUntilStable` 的挂载期滚动锚定时序耦合,得不偿失(按清单"别硬凑"放弃)。顺带:原 `!important` 移除(它压的是 MessageItem `.msg` transition,当时争同一元素;现在动画目标是 run-group li,无竞争);三处失实注释(run-group "不接管动画"等)一并改正
- 验证:dev 起服手动切 /chat↔/nodes、切会话;`emulate prefers-reduced-motion` 下无动画(主会话统一跑)

## 收尾(每 WP 完成后 + 最终)

- [ ] 每 WP:跑 `cd app && pnpm test`;样式无快照覆盖的以截图对照
- [ ] 最终:全量 `pnpm test` + `pnpm build`(vue-tsc)
- [ ] 重建 dist → `./scripts/ui-review.sh` 重跑,报告存 `research/vlm-report-after.md`,对照 AC2 类目收敛
- [ ] grep 复核 AC6:diff 内无新增 hex/裸 px 字号(`git diff -U0 | grep -E '#[0-9a-fA-F]{3,8}|font-size: *\d'`)
- [ ] `trellis-check`(Agent)全量复查 → `trellis-update-spec`(若沉淀出新约定:如触控规范扩展、markdown 节奏 token)→ Phase 3.4 commit

## 回滚点

- 各 WP 独立 commit,样式层回滚 = revert 单个 commit,无数据/接口风险。
