# 窗口加 6px 圆角 + 边框（无背景模糊）

## Goal

为 Tauri 桌面窗口加 macOS Sonoma 风格的圆角 + 1px 边框 + 微阴影,不开背景模糊。
保持暗色主题、frameless、自绘 tab bar 等已有约束。

## What I already know

* tauri.conf.json 当前 window: `decorations: false` + `titleBarStyle: "Overlay"`
  + trafficLightPosition `{x:14, y:14}` → frameless + 自绘标题栏。
* AppShell.vue: `.app-shell` 100vh + `background: var(--color-bg-app)` (#0a0e14)。
* 顶部 40px AppHeader 是 macOS-style 自绘 tab bar,traffic lights 仍由 OS 绘。
* 现有 token: `--color-bg-border: #1e2530`(设计文档里备注"几乎不可见")、
  `--color-bg-border-strong: #3b475a`(markdown table 用)。
* Toast 自带 1px border + 8px radius + box-shadow,独立 fixed 位置,不受影响。
* 项目无 vitest/jest,类型安全靠 `vue-tsc --noEmit`。

## Requirements

* `tauri.conf.json` 给唯一 window 加 `"transparent": true`。
* `style.css` 在 `html, body, #app` 套同一组 frame 样式:
  * `border-radius: 6px`
  * `border: 1px solid var(--color-bg-border-strong)`(复用现有 token,不引入新色)
  * `box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3)`(subtle 浮起)
  * `overflow: hidden`(裁掉子元素越界到 6px 圆角外的部分)
  * `background: var(--color-bg-app)`(深色基底,防止 transparent 窗口透桌面)
  * **不**加 margin(用户选 0px 贴边)
* **不**开背景模糊:不写 `backdrop-filter`、不写 `vibrancy` / `effects` 字段。

## Acceptance Criteria

* [ ] tauri.conf.json 加上 `"transparent": true`,`pnpm tauri dev` 启动后窗口四角呈 6px 圆角
* [ ] body 1px 边框 = `#3b475a`(来自 `--color-bg-border-strong`)
* [ ] body 有 subtle 阴影,目视不抢戏
* [ ] **不**出现 `backdrop-filter` / `vibrancy` / `effects`(grep 校验)
* [ ] 1440×900 窗口四周都有 1px 边 + 圆角
* [ ] AppHeader / Sidebar / ChatWindow 内部布局不动
* [ ] Toast 仍正常显示在 fixed bottom-center
* [ ] `pnpm build`(vue-tsc --noEmit + vite build)通过
* [ ] `cd app/src-tauri && cargo check` 通过

## Definition of Done

* `pnpm tauri dev` 启动后目视确认三项(圆角 / 边框 / 阴影)都生效
* `pnpm build` + `cargo check` 干净
* dev 期间手测:窗口拖拽、最小化/恢复、resize 行为不退化
* 如发现 spec 文档需要更新(窗口主题 token / Tauri config 约束),落 `.trellis/spec/`

## Technical Approach

**Tauri 2 配置层**:在 `app.windows[0]` 加 `"transparent": true`。
macOS / Windows / Linux(WSLg)均支持。WSLg 走 X11 路径,Linux 也能渲染圆角
(合成器偶尔吃掉 1px 抗锯齿,肉眼基本不可见)。

**CSS 层**:`html, body, #app` 三个选择器套同一组样式。
- 透明窗口下 OS 桌面背景会从透明区露出来,body 自带 `background: var(--color-bg-app)`
  + 6px 圆角 + 边框 + 阴影正好把它"框"住,看不到桌面穿透。
- `overflow: hidden` 在 #app 上裁掉子元素超出 6px 圆角外的部分
  (AppHeader 顶部 40px 顶到窗口顶时,4 个角会被裁出圆角弧)。
- `html, body { margin: 0; padding: 0; }` 保留现有 reset(已是这样)。
- `box-sizing: border-box` 让 border 不撑大 100vh 高度。

**与现有 drag region 的关系**:Tauri 的 traffic lights 位置是 (14, 14),
不受 transparent 影响;AppHeader 仍位于窗口顶部,无需调整。

## Decision (ADR-lite)

**Context**: 用户希望窗口有 6px 圆角 + 边框,不要背景模糊。已锁定的"暗色主题 /
自绘标题栏 / frameless"约束下,唯一需要新增的就是 Tauri 的 transparent + CSS 的
圆角/边框/阴影。

**Decision**:
- tauri.conf.json 唯一窗口加 `"transparent": true`(Tauri 2 跨平台字段)
- CSS 复用现有 token `--color-bg-border-strong`,不引入新色值
- 0px inset、1px 边框、subtle 阴影 `0 4px 16px rgba(0, 0, 0, 0.3)`
- 不开 `backdrop-filter` / `vibrancy` / `effects`

**Consequences**:
- Windows 失去 Mica / Acrylic 模糊能力 — 这是 `transparent: true` 模式的固有 trade-off
  (设计文档 spike-003 讨论过)。如果以后想要 backdrop-blur,需要在 OS 模糊和 CSS 圆角之间二选一。
- macOS 仍能拿到 OS-level 圆角抗锯齿(Sonoma 风格)。
- WSLg 下圆角边缘可能有 1px 像素级抖动,肉眼基本不可见。
- Toast / 内部组件的 border / radius 各自独立,不会被窗口外框影响。

## Out of Scope

* 窗口阴影强度调节(不加 settings 面板,CSS 写死)
* 圆角尺寸随 DPI / 显示器缩放(写死 6px)
* 自定义窗口拖拽热区 / 边缘 resize 手势(保持现状)
* macOS vibrancy / Windows Mica / Linux blur(用户明确不要)
* 启动 / 关闭动画(保持现状)
* 移动端(项目本身只面向桌面)

## Technical Notes

* tauri.conf.json 当前 window 配置:
  `decorations: false`, `titleBarStyle: Overlay`, `trafficLightPosition: {x:14, y:14}`
* AppShell.vue 的 .app-shell 是 100vh 高 + `--color-bg-app` 背景
* 现有 token:
  - `--color-bg-border` #1e2530(备注"几乎不可见")
  - `--color-bg-border-strong` #3b475a(markdown table 用)
* 风险点:
  - `transparent: true` 在 Windows 上 drag region 行为可能微变,dev 期间需手动验证
  - Tauri 2 的 `transparent` 字段是 window-level,不是 app-level
* 相关 spec:
  - `.trellis/spec/frontend/quality-guidelines.md`(整体质量基线)
  - `.trellis/spec/frontend/index.md`(前端总览)
