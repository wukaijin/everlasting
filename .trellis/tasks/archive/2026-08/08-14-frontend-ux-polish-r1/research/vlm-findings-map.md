# VLM 视觉评审发现 → 代码定位(2026-08-14)

## 评审方法

`scripts/ui-review.sh`(2026-08-14 固化,首次运行 `out/ui-review/20260814-215224/`):
daemon serve dist → headless Chromium 截 7 界面 → 每张过 mmx vision(MiniMax VLM)
结构化评审。完整逐图报告见同目录 `vlm-report-20260814.md`;截图本身在
`out/ui-review/20260814-215224/`(gitignored,不入库)。

本文件把 VLM 发现逐条映射到代码位置,并标注**已验证 / 部分成立 / 不成立**,
供 PRD 与实现直接引用。VLM 会产生幻觉,凡未标注"已验证"的条目实现时需先复核。

## 发现清单(按工作包分组)

### A. 移动端(评审 06-chat-mobile / 07-pairing-mobile)

| # | VLM 发现 | 验证状态 | 代码定位 |
|---|---------|---------|---------|
| A1 | 聊天顶栏 4 个功能图标(记忆/审计/trace/授权)触控目标 ~32px,低于 44px 规范 | **已验证** | `app/src/components/chat/ChatPanel.vue:1557`(32×32)。spec `responsive-mobile.md` §6 已有 44px 机制但只覆盖 modal 内部按钮,顶栏不在其列 |
| A2 | 消息操作按钮(msg-actions__trigger)同样偏小 | 待复核 | `app/src/components/chat/MessageActionsMenu.vue` |
| A3 | 移动端是"桌面压缩版":顶栏塞 logo/面包屑/截断标题/群聊 chip/4 图标 | 定性问题 | `ChatPanel.vue` 顶栏区;spec §1.2 的 desktop-first overlay 模式决定了这是 CSS 收纳问题,不是重构问题 |
| A4 | 输入框内 Edit/Wf chips 挤压输入区 | 定性问题 | `ChatInput.vue`(PluginSelect / ModeSelect) |
| A5 | 配对页占位符与已输入状态难区分;"配对码"与"设备名"输入框风格差异大;配对按钮无 disabled 态 | 部分成立 | `app/src/views/PairingView.vue:77-97`。注意 :12-14 注释:单输入框是**有意的 MVP 决策**,分段 OTP 输入不在本轮范围,只做视觉层(占位符对比度 / 输入框风格统一 / 按钮 disabled) |
| A6 | 引导文案 "Settings → Remote" 中英混排 | 成立 | `PairingView.vue` 文案 |

### B. 长文排版(评审 01-chat-desktop)

| # | VLM 发现 | 验证状态 | 代码定位 |
|---|---------|---------|---------|
| B1 | "行高偏紧" | **部分不成立** | 气泡正文已 `--leading-relaxed`(1.6),`MessageItem.vue:910/1141`;:715 的 1.4 是 speaker chip(11px pill),合理。真正的"密集感"更可能来自 **markdown 段落/条目垂直节奏**(p/li/blockquote 的 margin)与中英混排无字距节奏,实现时从 markdown 渲染样式入手排查,别盲目调 line-height |
| B2 | 条目间缺段间距,重点词只有加粗一种强调 | 成立 | `MessageItem.vue` markdown 区 / `DiscussionSummaryCard.vue`(该卡 :86 已 1.6) |
| B3 | 微字号低对比:latency chip / 时间戳(10px + muted) | **已验证**(此前已调亮一档仍被判弱) | `--text-2xs`(10px)+ `--color-text-muted`,见 `style.css` token 注释(2026-08-05 调亮记录);`ChatInputLatencyPopover.vue` 等 |
| B4 | 中英文之间无间距("盘古之白") | 成立(浏览器无原生方案,评估成本后决定做否) | 全局 markdown 区;可选 CSS `text-spacing-trim`/`text-autospace`(Chromium 支持度有限)或接受现状 |

### C. 层级与指示器(评审 01 / 02 / 05)

| # | VLM 发现 | 验证状态 | 代码定位 |
|---|---------|---------|---------|
| C1 | 侧栏会话选中态辨识度弱(仅 12% accent 底色) | **已验证** | `style.css` `--color-bg-selected: accent 12%`;建议加左侧 2px accent bar |
| C2 | 桌面顶栏图标区缺分组(记忆/审计/trace/授权连排) | 成立 | `ChatPanel.vue` 顶栏;分隔线分组即可 |
| C3 | 设置弹窗 Tab 选中态仅靠文字亮度,缺下划线/背景块 | 待复核 | `app/src/components/settings/` Tab 组件 |
| C4 | "已加密保存"逐行常驻是噪音;部分小图标无 tooltip/aria-label | 成立 | settings Providers 列表行 |

### D. 动效缺口(代码盘点,非 VLM——静态截图看不出)

现有动效体系完整(modal zoom / toast / skeleton / pulse / TransitionGroup 消息进入,
motion token 阶梯 + prefers-reduced-motion 全在 `style.css`)。缺口:

| # | 缺口 | 代码定位 |
|---|------|---------|
| D1 | 路由切换零过渡 | `app/src/App.vue:39` 裸 `<router-view />`;`v-slot` + fade/slide,token 现成(`--duration-base` 150ms / `--ease-decelerate`) |
| D2 | 会话切换时聊天内容硬切 | `ChatPanel` / `MessageList` 挂载处 cross-fade |
| D3 | 侧栏 project tab 切换过渡 | `ProjectTabs.vue`(有 4 处 transition,复核覆盖面) |

## 明确不做(本轮)

- **trace ↔ 消息联动**(Turn 徽章嵌入气泡):功能开发非样式,单独立项
- **OTP 分段输入框**:PairingView 注释明确为 MVP 决策,维持
- bundle 体积 / 首屏加载(另案,已有分析)
- 图标库合并(@heroicons + @lucide 双库):设计一致性另议
