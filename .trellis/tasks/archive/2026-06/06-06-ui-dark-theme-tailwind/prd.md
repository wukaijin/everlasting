# 前端 UI 重构：dark theme + Tailwind + 自定义顶栏 + 组件化

## Goal

把当前 light theme + 手写 CSS 的前端重构为：现代普鲁士蓝 dark theme + Tailwind CSS v4 + Tauri 自定义顶栏（替代原生） + ChatWindow 拆为 8-10 个中粒度组件。视觉锚点是 `docs/spikes/003-ui-reference-prompts.md` 留档的 3 张 Midjourney Figma 风格参考图（ui-A.png / ui-B-1.png / ui-B-2.png）。

## Decisions (9 个偏好都拍板)

| # | 决策 | 决定 | 理由 |
|---|---|---|---|
| 1 | MVP 范围 | **A. 严格重构**（不实现新 Dialog） | 一次重构太多事 PR 难 review；新 Dialog 留 3b-2 / step 5 |
| 2 | Tailwind 版本 | **v4.3.x + `@tailwindcss/vite`** | CSS-first `@theme` 写 token 比 v3 config 短；5× 快 |
| 3 | 自定义顶栏跨平台策略 | **B. macOS Overlay + Win/Linux 自绘混合** | macOS 保留红绿灯（可访问性 + 用户预期）；其他平台自绘 |
| 4 | 顶栏与 Tab 位置 | **B. 单行融合**（TitleBar 28px + ProjectTabs 16-20px 紧凑在一行） | 紧凑、视觉冲击；总高度 ~48px 而非 ~70px |
| 5 | 暗色策略 | **A. Dark only**，预留 `.light` 扩展点 | 个人 dev 工具，长时间盯代码；扩展点不难留 |
| 6 | 组件拆分粒度 | **B. 中粒度（8-10 个）** | 对应参考图视觉区域，1:1；不 prop drilling |
| 7 | 验证策略 | **B. AI 视觉 diff**（截图给 Claude，对比参考图） | 利用 Claude 视觉能力；不强上 Playwright |
| 8 | WSLg 拖拽风险 | **B. spike-004 先验证** | WSLg 是主开发环境；30-60 分钟验证值 |
| 9 | 任务结构 | **A. 1 个 task，5 个 deliverable 阶段** | D0-D4 顺序依赖紧，1 task 线性推进最自然 |

## Deliverables（5 个阶段，1 个 task 顺序推进）

### D0 — spike-004 验证 WSLg 拖拽（前置）

**目的**：在 30-60 分钟内用最小 Tauri 测试 app 验证 `data-tauri-drag-region` + `decorations: false` 在 WSLg（Weston 合成器）下的行为。

**文件**：
- `docs/spikes/004-wslg-drag-verification.md` —— 留档
- `app/src-tauri/tauri.conf.json` —— 加 `titleBarStyle: "Transparent"` + `decorations: false`（临时改 spike 用）

**最小测试 app**：1 个 .vue（30 行）含 1 个 `data-tauri-drag-region` div + min/max/close 按钮。

**AC**：
- [ ] WSLg 下拖拽正常（≥5 次）
- [ ] WSLg 下 min/max/close 三按钮工作
- [ ] macOS / Windows 端红绿灯位置正确（如果本机有 macOS 顺手测）
- [ ] 留档"WSLg 行为结论"到 spike-004

**失败 fallback**：
- 如果 WSLg 拖不动 → spike 写明，回退到「WSL 走原生 + 其他平台自绘」feature flag
- 如果 WebKitGTK 焦点问题 → 用 `-webkit-app-region: drag` fallback

### D1 — Tauri 配置（窗口尺寸 + 自定义顶栏启用）

**文件**：
- `app/src-tauri/tauri.conf.json` —— 改 `width: 1440` / `height: 900` / `minWidth: 1280` / `minHeight: 800` + `titleBarStyle: "Transparent"` + `decorations: false`（Win/Linux）+ `trafficLightPosition: {x: 14, y: 14}`（macOS）
- `app/src-tauri/capabilities/default.json` —— 加 `core:window:allow-start-dragging` 权限

**AC**：
- [ ] `pnpm tauri dev` 启动窗口 = 1440×900
- [ ] 拖窗口到边缘最小化尺寸 ≥ 1280×800
- [ ] 顶栏不再有系统原生红绿灯/标题栏
- [ ] macOS 红绿灯位置 (14, 14)
- [ ] `cargo check` 通过

### D2 — Tailwind v4 接入 + dark theme tokens

**文件**：
- `app/package.json` —— 加 `tailwindcss@^4.3.0` + `@tailwindcss/vite@^4.3.0`
- `app/vite.config.ts` —— 加 `tailwindcss()` plugin
- `app/src/style.css` —— `@import "tailwindcss"` + `@theme { --color-*: ... }` 定义 14 个 token
- `app/src/tokens.css`（新）—— 备份或为将来 light 模式预留（不强制拆，看 D3 决定）

**色板 token 命名**（v4 风格）：
- `--color-bg-app` / `--color-bg-surface` / `--color-bg-elevated` / `--color-bg-border`
- `--color-text-primary` / `--color-text-secondary` / `--color-text-muted`
- `--color-accent` / `--color-accent-hover` / `--color-accent-muted`
- `--color-tool-read` / `--color-tool-write` / `--color-tool-shell` / `--color-tool-error` / `--color-thinking`

**字体**：
- `--font-sans`: `Noto Sans CJK SC, ...`
- `--font-mono`: `JetBrains Mono, ...`

**AC**：
- [ ] DevTools 取色器能拿到 `--color-accent` = `#3B5BDB`
- [ ] 取色器能拿到 5 个 tool 色
- [ ] 字体 fallback 链含 Noto Sans CJK SC + JetBrains Mono
- [ ] `pnpm build` 成功
- [ ] `<style scoped>` 里能 `var(--color-accent)` 引用

### D3 — 组件拆分 + dark theme 应用

**文件**（新 8 个子组件 + 重写 3 个现有）：
- 新：`app/src/components/layout/AppShell.vue`（顶层布局）
- 新：`app/src/components/layout/AppHeader.vue`（TitleBar + ProjectTabs 单行融合）
- 新：`app/src/components/layout/Sidebar.vue`
- 新：`app/src/components/chat/ChatPanel.vue`
- 新：`app/src/components/chat/MessageList.vue`
- 新：`app/src/components/chat/MessageItem.vue`
- 新：`app/src/components/chat/ThinkingBlock.vue`
- 新：`app/src/components/chat/ToolCallCard.vue`（含 read/write/shell/error 4 变体）
- 新：`app/src/components/chat/ChatInput.vue`
- 新：`app/src/components/layout/StatusBar.vue`
- 重写：`app/src/components/SessionList.vue`（应用 dark + restyle）
- 重写：`app/src/components/ProjectTabs.vue`（应用 dark + restyle）
- 简化：`app/src/App.vue`（只 `<AppShell />`）

**AC**：
- [ ] ChatWindow.vue < 200 行（拆分后变成 ChatPanel + 引用子组件）
- [ ] 每个新组件 50-150 行
- [ ] 5 个场景截图：空状态 / 单条对话 / 流式中 / 含 tool call / 多 session 切换
- [ ] tool call 卡片左边色条按工具类型正确变色（read 青 / write 绿 / shell 琥珀 / error 朱砂）
- [ ] 选中态背景用 `--color-accent-muted`（`#1E2A5E`）
- [ ] thinking 块用 `--color-thinking` 紫罗兰配色
- [ ] `vue-tsc --noEmit` 无 type 错误
- [ ] `pnpm build` 成功

### D4 — 自定义 TitleBar 组件 + 跨平台按钮

**文件**：
- 新：`app/src/components/layout/TitleBar.vue`（24-32px 高，跨平台拖拽 + 按钮）
- 修改：`app/src/components/layout/AppHeader.vue`（集成 TitleBar + ProjectTabs 单行融合）

**关键实现细节**（来自研究）：
- 拖拽区用 `data-tauri-drag-region` HTML 属性
- 子按钮（min/max/close）用 `data-tauri-drag-region="false"` 反向标记
- 按钮调 `getCurrentWindow().minimize() / toggleMaximize() / close()`
- macOS 红绿灯位置 14,14（已在 D1 配置），自绘内容从 x ≈ 80px 开始（避开红绿灯）
- Windows/Linux 自绘 min/max/close 在右侧

**AC**：
- [ ] macOS 拖拽 TitleBar 区域可拖动（≥5 次）
- [ ] Windows/Linux min/max/close 三按钮工作
- [ ] ProjectTabs 与 TitleBar 在同一行（48px 总高）—— 单行融合方案
- [ ] macOS 红绿灯在 TitleBar 左侧 (14, 14)，自绘内容在其右侧
- [ ] WSLg 拖拽 ≥5 次稳定（spike-004 验证后）
- [ ] `vue-tsc --noEmit` 无 type 错误
- [ ] `pnpm build` 成功
- [ ] `cargo check` 通过

## Verification Protocol（AI 视觉 diff 工作流）

每完成一个 D，把截图存到 `docs/spikes/refactor-d{N}.png`，告诉 Claude 路径：

| 阶段 | 你截什么 | Claude 验证什么 |
|---|---|---|
| D0 | WSLg 拖拽测试视频/截图 | 不需要 AI 验证——是手测 + 文档留档 |
| D1 | `pnpm tauri dev` 启动截图（含尺寸） | 1440×900 确认、顶栏原生控件消失确认 |
| D2 | 已应用 dark 的主界面截图 | 取色对比 token hex + 视觉对 ui-A.png |
| D3 | 5 张：空状态 / 单条对话 / 流式中 / 含 tool call / 多 session 切换 | 逐张对照参考图，视觉 90% 匹配 |
| D4 | 带新 TitleBar + Tabs 的截图 | macOS 红绿灯位置 / Win-Linux 按钮布局 |

## Acceptance Criteria（总）

- [ ] `pnpm tauri dev` 启动后窗口默认 1440×900，最小 1280×800
- [ ] 顶栏由前端绘制，原生 OS 控件不再显示
- [ ] `pnpm build` 成功（`vue-tsc --noEmit && vite build`）
- [ ] `cargo check` 通过
- [ ] Tailwind utility class 能在组件里正常生效
- [ ] ChatWindow.vue < 200 行（拆分后）
- [ ] 视觉对照参考图：色板 / 间距 / 字体 90% 匹配（5 个场景）
- [ ] tool call 卡片左边色条按工具类型正确变色
- [ ] WSLg 拖拽稳定（spike-004 验证 + D4 实测 ≥5 次）
- [ ] 跨平台拖拽 / 按钮 / 红绿灯位置（macOS）正确

## Definition of Done

- 5 个 commit 落地：D0 spike 留档 + D1 tauri 配置 + D2 tailwind + D3 组件拆分 + D4 TitleBar
- `vue-tsc --noEmit` 无 type 错误
- `pnpm build` 成功
- `cargo check` 无 warning（已存在的 warning 忽略）
- 5 个场景截图通过 Claude 视觉验证
- WSLg 拖拽 + min/max/close ≥5 次手测通过
- 必要时更新 `docs/IMPLEMENTATION.md` 加 step 3c 条目

## Out of Scope (explicit)

- 新增 ProjectPickerDialog 组件（属于 3b-2 范围）
- 新增 PermissionDialog 组件（属于 step 5 范围）
- 引入 light mode（本次 dark only，预留扩展点）
- 引入 E2E 测试（项目尚未配 vitest/playwright）
- 服务端改动（Rust 端不动）
- 替换 reka-ui 为其他 UI 库（本次不引入 reka-ui——不需要 Dialog/Overlay）
- 国际化（i18n）—— 仍用 hard-coded 中文 + 英文混排
- 动画 / 过渡（除了已经存在的 toast 过渡）

## Research References

- [`research/tailwind-v3-vs-v4.md`](research/tailwind-v3-vs-v4.md) — 推荐 Tailwind **v4.3.x + `@tailwindcss/vite`** 插件（CSS-first `@theme` 写 token、5× 完整构建快、Vue 3 scoped `<style>` 共存无障碍）
- [`research/tauri-titlebar-patterns.md`](research/tauri-titlebar-patterns.md) — 推荐 **macOS Overlay（保留红绿灯） + Windows/Linux `decorations: false` 自绘** 混合方案；拖拽用 `data-tauri-drag-region`（**不要用** `-webkit-app-region`，WebKitGTK 不支持）

## Technical Notes

**关键文件清单**（实施时要改/创建）：
- `app/src-tauri/tauri.conf.json` — D1
- `app/src-tauri/capabilities/default.json` — D1
- `app/package.json` — D2
- `app/vite.config.ts` — D2
- `app/src/style.css` — D2
- `app/src/App.vue` — D3
- `app/src/components/ChatWindow.vue` — D3 拆
- `app/src/components/SessionList.vue` — D3
- `app/src/components/ProjectTabs.vue` — D3
- `app/src/components/layout/AppShell.vue` — D3 新
- `app/src/components/layout/AppHeader.vue` — D3 + D4
- `app/src/components/layout/Sidebar.vue` — D3 新
- `app/src/components/layout/StatusBar.vue` — D3 新
- `app/src/components/layout/TitleBar.vue` — D4 新
- `app/src/components/chat/ChatPanel.vue` — D3 新
- `app/src/components/chat/MessageList.vue` — D3 新
- `app/src/components/chat/MessageItem.vue` — D3 新
- `app/src/components/chat/ThinkingBlock.vue` — D3 新
- `app/src/components/chat/ToolCallCard.vue` — D3 新
- `app/src/components/chat/ChatInput.vue` — D3 新
- `docs/spikes/004-wslg-drag-verification.md` — D0 新

**关键约束**（来自 spec / 已落地的设计决策）：
- ChatMessage 形状在 `spec/frontend/state-management.md` 锁定——拆分组件不能改 store 接口
- reka-ui 1.0.0-alpha.10 已装但未用——本次不引入
- WSL 2 + Ubuntu 22.04（来自 `docs/HACKING-wsl.md`）—— Tailwind v4 WebKitGTK 2.40+ 满足
- Tauri 2 + Vue 3.5 + Vite 6 技术栈已定

**风险**：
- WSLg 拖拽：spike-004 验证后才知道（D0 解决）
- CJK + JetBrains Mono 字体回退：D2 阶段 DevTools 检查 fallback
- 跨平台 WebView 兼容性：研究已确认 Tauri 2 三大平台 webview 都满足 Tailwind v4
- ProjectTabs 的拖拽性能：100+ tab 时横向滚动（已有 4 个 tab，未到瓶颈）
