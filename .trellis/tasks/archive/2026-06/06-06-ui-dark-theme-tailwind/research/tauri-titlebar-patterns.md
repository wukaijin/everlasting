# Research: Tauri 2 自定义顶栏模式（cross-platform titlebar customization）

- **Query**: Tauri 2.x 替换原生标题栏为自绘顶栏，跨平台兼容 macOS / Windows / Linux + WSL2
- **Scope**: external（Tauri 官方文档 + 社区库 + 真实问题单 + 参考应用）
- **Date**: 2026-06-06
- **目标项目状态**: Tauri 2 + Vue 3.5 + Vite 6，当前 `tauri.conf.json` 走系统原生顶栏（无 `titleBarStyle` / `decorations`）；窗口 960×720，目标 1440×900

---

## TL;DR

**推荐方案：Approach B（"macOS Overlay + Windows/Linux decorations:false"）**。

3 个关键决策：

1. **macOS 用 `titleBarStyle: "Overlay"` + `hiddenTitle: true` + `trafficLightPosition`**：保留原生红绿灯（功能、外观、可访问性全有），把自绘内容布在它们右边。Linear / Cursor / GitHub Desktop 走这条路。Tauri 2.4 起 `trafficLightPosition` 配置项 stable。
2. **Windows + Linux + WSL2 用 `decorations: false`**：自己画 min/max/close，按钮全靠 `getCurrentWindow().minimize() / toggleMaximize() / close()`。Windows 11 想要圆角再加 `transparent: true` + `border-radius`。
3. **拖拽用 `data-tauri-drag-region` HTML 属性**——不要用 `-webkit-app-region: drag`（**Linux/WebKitGTK 完全不支持**，确认见 `liminal-hq/emoji-nook#5`）。子元素如按钮加 `data-tauri-drag-region="false"` 反向标记。

**WSL2 风险**：WebKitGTK 的 `data-tauri-drag-region` 在某些 Wayland 合成器下会"偷焦点"导致弹窗式窗口消失（参见 issue 详情），但**对常驻主窗口不致命**——WSLg 是 Weston 合成器，与 KDE/Wayland 行为不一定一致；建议 WSL 跑通后做 ≥5 次拖拽/最大化/关闭手测。

**可选加速器**：用社区库 [`@tauri-controls/vue`](https://www.npmjs.com/package/@tauri-controls/vue)（951 stars，agmmnn/tauri-controls）抄一份组件代码，比从零写省半天；或者用 Rust 端 [`tauri-plugin-decorum`](https://github.com/clearlysid/tauri-plugin-decorum)（313 stars）把跨平台细节封装在 Rust 一边。两者都不是必选——直接用官方 API 也 ok。

---

## 1. Tauri 2 顶栏相关 config 全景

### 1.1 `tauri.conf.json` 窗口字段（仅列与顶栏相关的）

来源：`tauri-2.11.2` Rust crate + `v2.tauri.app/reference/config/`：

| 字段 | 类型 | 平台 | 默认 | 说明 |
|---|---|---|---|---|
| `decorations` | `bool` | 全平台 | `true` | `false` 删掉原生标题栏 + 边框 + 系统按钮。**Linux 上还会去掉 GTK CSD 的 resize 把手** |
| `titleBarStyle` | `"Visible" \| "Transparent" \| "Overlay"` | **仅 macOS**（Windows/Linux 静默忽略） | `"Visible"` | 见下表 |
| `trafficLightPosition` | `LogicalPosition` | **仅 macOS** | `null` | 红绿灯坐标偏移；**要求 `titleBarStyle: "Overlay"` + `decorations: true`**（since Tauri 2.4） |
| `hiddenTitle` | `bool` | **仅 macOS** | `false` | 隐藏标题文字，但保留红绿灯 |
| `transparent` | `bool` | 全平台（macOS 需 `macos-private-api` feature） | `false` | 窗口本身透明；macOS 启用会被 App Store 拒绝 |
| `shadow` | `bool` | 全平台 | `true` | OS 阴影 |
| `resizable` | `bool` | 全平台 | `true` | 配合 `decorations: false` 时 Linux/Windows 必须自己实现 resize handle |
| `maximizable` / `minimizable` / `closable` | `bool` | macOS / Windows | `true` | 控制原生按钮启用状态（对自绘无影响） |
| `windowEffects` | `WindowEffectsConfig` | macOS / Windows | `null` | Vibrancy / Acrylic / Mica 等模糊效果 |

### 1.2 `TitleBarStyle` 三个变种的实际行为（**仅 macOS**）

来源：`docs.rs/tauri/2.11.2/tauri/enum.TitleBarStyle.html`：

| 变种 | 行为 | 适用场景 |
|---|---|---|
| `Visible`（default） | 标准原生标题栏，红绿灯 + 标题文字全在 28px 高的灰条上 | 不需要自绘顶栏 |
| `Transparent` | 标题栏透明，**窗口背景色透过来**。"useful if you don't need to have actual HTML under the title bar" | 想画一条纯色横条但保留红绿灯 + 标题。**Tauri 官方推荐的"温和定制"**——不丢系统功能（red:move、aligning） |
| `Overlay` | 标题栏作为 **transparent overlay 浮在 webview 内容之上**。红绿灯仍在屏幕坐标 (10, 10) 附近。**这是 Linear / Cursor / VS Code 用的模式** | 需要 HTML 内容延伸到顶栏区域（如左侧 sidebar 顶到屏幕顶，红绿灯浮在其上） |

`Overlay` 模式的 3 个 caveat（来自官方 enum 文档）：

- 不同 macOS 版本顶栏高度不同 → 自绘内容布局要按 dynamic height 写
- **必须自己加 drag region** —— "you can't drag the window when it's not in focus" 是已知限制（[tauri#4316](https://github.com/tauri-apps/tauri/issues/4316)）
- 标题文字颜色跟系统主题（亮/暗），如果你 `hiddenTitle: true` 就无所谓

### 1.3 与 Windows/Linux 的对应

**`titleBarStyle` 字段在 Windows/Linux 上完全不起作用**（不是 deprecated，是 macOS-only no-op）。Windows / Linux 想做自定义顶栏只能 `decorations: false`。

VS Code / Electron 有 Windows 专属的 [Window Controls Overlay (WCO)](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/overlay-window-controls)，让系统画 min/max/close 浮在 webview 上。但 **Tauri 2 / wry 还没有实现**（[wry#1650](https://github.com/tauri-apps/wry/issues/1650)，2026 年仍是 open feature request）。所以 Windows 上**只能"全自绘"或"完全用系统的"**，没有中间档。

---

## 2. 推荐方案（Approach B）的完整 config diff

### 2.1 `app/src-tauri/tauri.conf.json`

```jsonc
{
  "app": {
    "windows": [
      {
        "title": "Everlasting",
        "width": 1440,           // ← 改：原 960
        "height": 900,           // ← 改：原 720
        "minWidth": 1280,        // ← 改：原 640
        "minHeight": 800,        // ← 改：原 480
        "decorations": false,    // ← 新：删原生顶栏
        "titleBarStyle": "Overlay",     // ← 新：macOS 红绿灯浮在内容上（其他平台忽略）
        "hiddenTitle": true,            // ← 新：macOS 隐藏标题文字（其他平台忽略）
        "trafficLightPosition": {       // ← 新：红绿灯位置（since Tauri 2.4）
          "x": 14,
          "y": 14
        },
        "transparent": false,    // 暗色 only 项目，背景已 #0A0E14，不需要 vibrancy
        "shadow": true,
        "resizable": true,
        "center": true           // 可选：首次启动居中
      }
    ]
  }
}
```

**注意**：官方文档 `v2.tauri.app/learn/window-customization/` 写的是 `decorations: false` 走"全平台 fully custom"路线。这里"macOS overlay"是**与官方稍偏离的混合路线**——好处是 macOS 保留红绿灯，缺点是**`decorations: false` 与 `titleBarStyle: Overlay` 在 macOS 上的相互作用没有官方明确文档**。

社区共识（来自 `tauri-plugin-decorum` README）：macOS 上**只用 `titleBarStyle: "Overlay"` + `hiddenTitle: true`，不设 `decorations: false`**；Windows/Linux 才用 `decorations: false`。Tauri 配置允许这种"宽配置 + 平台静默忽略"，所以单文件就能搞定（参见 §2.3 verify）。

### 2.2 `app/src-tauri/capabilities/default.json`

**这是关键且容易漏的一步**——Tauri 2 默认权限不允许 webview 调 `minimize/close/startDragging`，必须显式声明：

```jsonc
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Capability for the main window",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "core:window:allow-start-dragging",
    "core:window:allow-minimize",
    "core:window:allow-unminimize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-maximize",
    "core:window:allow-unmaximize",
    "core:window:allow-is-maximized",
    "core:window:allow-close",
    // 可选：
    "core:window:allow-set-size",
    "core:window:allow-set-focus",
    "core:window:allow-start-resize-dragging"
  ]
}
```

`core:window:default` 包含大部分但**不包含 `start-dragging`**（来自官方文档 "By default, all plugin commands are blocked"）。如果漏了 `allow-start-dragging`，所有拖拽点击都会静默无反应，DevTools 也没明显报错。

### 2.3 macOS 上 `decorations: false` + `titleBarStyle: Overlay` 同时存在的相互作用

社区经验（tauri-plugin-decorum、Linear 反编译、cc-switch、openless）共识：

- **配两个都行**：macOS 优先按 `titleBarStyle` 来；`decorations: false` 在 macOS 上视作"边框移除但保留 system chrome 入口"。但**红绿灯位置 (`trafficLightPosition`) 要求 `decorations: true`**（来自官方文档警告）。
- **更保险**的是 setup hook 里**按 `cfg!(target_os = "macos")` 分支调 builder API**：

```rust
// src-tauri/src/lib.rs setup hook
let win = app.get_webview_window("main").unwrap();
#[cfg(target_os = "macos")]
{
    use tauri::TitleBarStyle;
    // 移除已设置的 decorations: false（让 Tauri 走 Overlay 路径）
    win.set_decorations(true).ok();
    win.set_title_bar_style(TitleBarStyle::Overlay).ok();
    // 红绿灯位置可用 set_traffic_lights_position（Tauri 2.4+）
}
```

但**最简单可行的做法是**：

- `tauri.conf.json` 里只配 `titleBarStyle: "Overlay"` + `hiddenTitle: true`，**不配 `decorations: false`**
- 然后用 `#[cfg(not(target_os = "macos"))]` 的 builder 调 `decorations(false)` 给 Windows/Linux

参考实现：[Open-Less/openless#531](https://github.com/Open-Less/openless/pull/531) PR 用 `tauri.linux.conf.json` 平台 override 文件做这件事——Tauri 2 支持 `tauri.${platform}.conf.json` 合并。

---

## 3. 前端 Vue 3 组件（TitleBar.vue 草图）

完整可用版本（无外部库，参考 tauri-controls/Vue + 官方 demo + Linear 截图风格）：

```vue
<!-- app/src/components/TitleBar.vue -->
<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue"
import { getCurrentWindow } from "@tauri-apps/api/window"
import { platform } from "@tauri-apps/plugin-os"

const win = getCurrentWindow()
const isMaximized = ref(false)
const os = ref<"macos" | "windows" | "linux" | "unknown">("unknown")

let unlistenResize: (() => void) | undefined

onMounted(async () => {
  // platform() returns "macos" | "windows" | "linux" | "ios" | "android"
  const p = await platform()
  os.value = p === "macos" || p === "windows" || p === "linux" ? p : "unknown"
  isMaximized.value = await win.isMaximized()
  unlistenResize = await win.onResized(async () => {
    isMaximized.value = await win.isMaximized()
  })
})

onUnmounted(() => unlistenResize?.())

async function onMin() { await win.minimize() }
async function onMax() { await win.toggleMaximize() }
async function onClose() { await win.close() }
</script>

<template>
  <!-- 整条顶栏可拖；按钮 / 项目 Tab 等子元素禁拖 -->
  <header
    class="titlebar"
    :class="`titlebar--${os}`"
    data-tauri-drag-region
  >
    <!-- macOS：红绿灯由系统画在左上 (14, 14)，这里留 70px 占位 -->
    <div v-if="os === 'macos'" class="titlebar__traffic-spacer" />

    <!-- 应用名 / 项目 Tab（拖拽区，但子按钮不可拖） -->
    <div class="titlebar__brand">Everlasting</div>
    <slot />

    <!-- Windows / Linux / WSL：自绘右上 min/max/close -->
    <div
      v-if="os !== 'macos'"
      class="titlebar__controls"
      data-tauri-drag-region="false"
    >
      <button class="titlebar__btn" title="最小化" @click="onMin">
        <svg viewBox="0 0 12 12" width="12" height="12"><path d="M0 5h12v2H0z" fill="currentColor"/></svg>
      </button>
      <button class="titlebar__btn" title="最大化" @click="onMax">
        <svg viewBox="0 0 12 12" width="12" height="12">
          <rect v-if="!isMaximized" x="0.5" y="0.5" width="11" height="11" fill="none" stroke="currentColor"/>
          <g v-else fill="none" stroke="currentColor">
            <rect x="2.5" y="0.5" width="9" height="9"/>
            <rect x="0.5" y="2.5" width="9" height="9" fill="var(--bg)"/>
          </g>
        </svg>
      </button>
      <button class="titlebar__btn titlebar__btn--close" title="关闭" @click="onClose">
        <svg viewBox="0 0 12 12" width="12" height="12"><path d="M1 1l10 10M11 1L1 11" stroke="currentColor" stroke-width="1.2"/></svg>
      </button>
    </div>
  </header>
</template>

<style scoped>
.titlebar {
  display: flex;
  align-items: center;
  height: 36px;            /* Linear ~38px / Cursor ~36px */
  background: #0A0E14;     /* 与项目主背景一致 */
  color: #E5E7EB;
  user-select: none;
  -webkit-user-select: none;
  flex-shrink: 0;
}
.titlebar__traffic-spacer { width: 78px; }   /* 红绿灯 3 × 12 + 间距 14 + 14 */
.titlebar__brand {
  font-size: 12px;
  font-weight: 500;
  letter-spacing: 0.02em;
  padding: 0 12px;
}
.titlebar__controls {
  margin-left: auto;
  display: flex;
  height: 100%;
}
.titlebar__btn {
  width: 46px;            /* Windows 11 标准 */
  height: 100%;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: none;
  color: #8B95A7;
  cursor: default;
  transition: background 80ms;
}
.titlebar__btn:hover { background: rgba(255,255,255,0.06); color: #E5E7EB; }
.titlebar__btn:active { background: rgba(255,255,255,0.10); }
.titlebar__btn--close:hover { background: #c42b1c; color: #fff; }

/* Linux/GNOME 习惯：按钮稍微小一点更贴近系统 */
.titlebar--linux .titlebar__btn { width: 38px; }
</style>
```

### 子元素禁拖的 3 种写法（按可移植性排序）

1. **`data-tauri-drag-region="false"`**（最优，官方支持）：直接在按钮容器上加这个属性
2. **HTML 交互元素**：按钮 `<button>`、输入 `<input>` Tauri 默认不响应 drag region（参见官方文档 "interactive elements like buttons and inputs can function properly"）—— **但只有"裸"interactive 元素**，包了一层 div 还是会被拖
3. **CSS `-webkit-app-region: no-drag`**：**不要用**。`-webkit-app-region` 在 WebKitGTK 上完全不支持（确认见 §5.4）

---

## 4. Cross-platform 行为矩阵

| 场景 | macOS（Overlay） | Windows 10/11 | Linux X11 | Linux Wayland | **WSL2 (WSLg)** |
|---|---|---|---|---|---|
| 原生 min/max/close | ✅ 红绿灯保留 | ❌ 全删 | ❌ 全删 | ❌ 全删 | ❌ 全删 |
| 自绘按钮调 `getCurrentWindow().minimize()` 等 | ✅（按钮放右边） | ✅ | ✅ | ✅ | ✅（预期） |
| `data-tauri-drag-region` 拖窗口 | ⚠️ 失焦窗口拖不动（[#4316](https://github.com/tauri-apps/tauri/issues/4316)） | ✅ | ✅ | ⚠️ 部分合成器（KDE）"偷焦点"，弹窗/popover 会被关闭；常驻主窗口 OK | ⚠️ Weston 合成器未广泛测过，预期接近 KDE Wayland |
| 双击顶栏切换最大化 | ✅ 默认（Overlay 模式） | ❌ 默认 → 自己加 listener，调 `toggleMaximize()` | ❌ 同上 | ❌ 同上 | ❌ 同上 |
| 拖窗口到屏幕边缘 snap | ✅ 系统接管 | ❌ 自绘后**丢 Aero Snap**——除非用 `tauri-plugin-decorum::show_snap_overlay` | N/A（X11 无 snap） | 取决于 compositor | 取决于 WSLg |
| 边框 resize handle | ✅ 系统接管 | ❌ `decorations: false` 后**没有 8px resize 边**——必须自己在边缘加 `startResizeDragging(direction)` 区域 | ❌ 同上 | ❌ 同上 | ❌ 同上 |
| 窗口圆角 | ✅ 系统 | ⚠️ Win11 需 `transparent: true` + `border-radius` 在 body；Win10 不支持 | ⚠️ 同 Win10 | ⚠️ compositor 决定 | ⚠️ Weston 默认无 |
| 窗口阴影 | ✅ `shadow: true` | ⚠️ 自绘+rounded 后阴影会被裁，需要 `windowEffects` 或 `tauri-plugin-decorum` | ⚠️ 同 Win | ⚠️ 同 Win | ⚠️ 同 Win |
| 字体 / 中文渲染 | ✅ 系统 | ✅ Segoe UI / 微软雅黑 | ✅ Fontconfig | ✅ | ⚠️ **WenQuanYi 默认会有 baseline 不齐**——必须装 `fonts-noto-cjk` + 写 `/etc/fonts/local.conf`（**已记录在 spike-001**） |

**WSLg 特殊注意**（来自 spike-001 + WebKitGTK 行为推断）：

- WSLg 是 Weston 合成器（基于 Wayland），跑在 WSL 内的 X11 与 Wayland 客户端都行
- spike-001 已确认窗口本身工作正常，但**没测过 `decorations: false`** —— 本次重构是 first time，必须实测
- 字体配置已通过 spike-001 修好（Noto Sans CJK SC），改 dark theme 不会回退这条
- WSLg 下 `data-tauri-drag-region` 行为**未知**——预期接近 Linux/Wayland，但 Weston 比 KDE Wayland/Sway 行为更简单（更少自定义 input handler），所以有可能**反而更稳定**

---

## 5. 常见 gotcha 清单

### 5.1 macOS-only：setTitle 重置红绿灯位置（[tauri#13044](https://github.com/tauri-apps/tauri/issues/13044)）

每次 `getCurrentWebviewWindow().setTitle(newTitle)` 后红绿灯会跳回默认位置 (7, 7)。workaround：set title 后再调一次 `set_traffic_lights_position`。如果项目里**不会动态改窗口标题**（Everlasting 项目固定 `"Everlasting"`，应该不踩），可忽略。

### 5.2 macOS-only：拖不焦点窗口（[tauri#4316](https://github.com/tauri-apps/tauri/issues/4316)）

`data-tauri-drag-region` 在 macOS 上窗口失焦时不响应。Electron 行为不同（能拖）。workaround：监听 `mousedown`，手动调 `startDragging()`。对单窗口应用影响小，多窗口/弹窗场景才明显。

### 5.3 Linux/Wayland：`decorations: false` + visible(false) 后 chrome 无响应（[tauri#11856](https://github.com/tauri-apps/tauri/issues/11856)）

GNOME on Ubuntu/Fedora，如果 `WebviewWindowBuilder` 设 `visible(false)` 后再 `show()`，window decoration 无响应。Workaround：先 show 后才 builder 完，或者初次 show 后做一次 ±1px resize "nudge"（参见 [openless#531](https://github.com/Open-Less/openless/pull/531)）。

我们项目目前在 `tauri.conf.json` 声明窗口（非 builder），**不踩**。

### 5.4 全平台但 Linux 致命：`-webkit-app-region: drag` 在 WebKitGTK 不支持（[liminal-hq/emoji-nook#5](https://github.com/liminal-hq/emoji-nook/issues/5)）

> "Property is completely unsupported on WebKitGTK. Confirmed via Web Inspector which reports 'Unsupported property name'. This approach only works on Chromium/Electron."

**结论**：在 Tauri 项目里**永远不要用 `-webkit-app-region`**——一律用 `data-tauri-drag-region` HTML 属性。这是 Electron 文档抄过来的常见误区。

### 5.5 Windows-only：`decorations: false` 后丢 Aero Snap（拖到屏幕边自动 1/2 / 1/4 屏）

Tauri 没有官方实现。`tauri-plugin-decorum` 提供 `show_snap_overlay` 命令（基于 [WindowChromeHook](https://github.com/tauri-apps/tauri/pull/12366)）作 workaround。如果项目用户主要在 macOS / WSL 工作，可以接受不修。

### 5.6 Windows 11：圆角丢失（[tauri#9287](https://github.com/tauri-apps/tauri/issues/9287)）

`decorations: false` 后想要 Win11 圆角必须：
```jsonc
"transparent": true,
```
+ CSS：
```css
:root, body { border-radius: 12px; overflow: hidden; }
```
但 `transparent: true` 在 macOS 上需要 `macos-private-api` feature（拒 App Store）。所以分平台 conf 文件 `tauri.windows.conf.json` 单独开 `transparent`。

### 5.7 macOS Overlay 模式下 sidebar 顶到屏幕顶的位置冲突

如果你想 sidebar 从 `top: 0` 开始（VS Code/Cursor 风格），红绿灯会浮在 sidebar 上。**解法 2 选 1**：
- 把 sidebar 从 `top: 36px` 开始（让出红绿灯位）—— 简单但 sidebar 上方有空区
- sidebar 顶到 0，把 sidebar 里**左上 80×36 区域留空**作为 drag region —— 视觉上 sidebar 与红绿灯无缝；这是 Linear / Cursor 用的方式

### 5.8 `core:window:allow-start-dragging` 漏配权限 → 拖拽静默失败

默认 `core:window:default` **不含** `start-dragging`（参见 §2.2）。漏了会"拖了没反应、控制台没报错"，非常容易误以为是 CSS / 事件冒泡问题。

### 5.9 resize handle 自绘（`decorations: false` 必做）

`decorations: false` 后窗口边框 8px 的 resize 区域消失。3 种实现：

1. **官方 JS API**：8 个角/边各放一个透明 `<div>`，`mousedown` 时调 `getCurrentWindow().startResizeDragging("North-West"|"East"|...)`
2. **Rust 端 wry workaround**：调 `WindowChromeHook`（Windows 专有，from tauri-plugin-decorum）
3. **不实现**：用户必须拖按钮才能 resize —— 单人开发工具可接受

参见请求 [tauri#7900](https://github.com/tauri-apps/tauri/issues/7900) "Add data-tauri-drag-resize-region"（**已关闭，未实现**）。

---

## 6. 3 个 feasible 方案对比

### Approach A：纯 `decorations: false` + 全自绘（官方文档推荐）

```jsonc
{ "decorations": false }
```
+ 前端画一切（包括 macOS 的"假红绿灯"）。

**优点**：跨平台一致；config 简单；前端控制力最大。
**缺点**：macOS 失去原生红绿灯（功能 + 可访问性 + 用户预期）；视觉再像也是模仿；macOS 用户会觉得"廉价"。
**适合**：跨平台游戏/工具、想要完全统一品牌的应用、不在乎 macOS 用户感受的产品。
**例子**：很多 Electron 应用、Discord 早期。

### Approach B：macOS Overlay + 其他 decorations:false（**推荐**）

```jsonc
{
  "decorations": false,                  // Windows + Linux 生效
  "titleBarStyle": "Overlay",            // macOS 生效
  "hiddenTitle": true,                   // macOS 生效
  "trafficLightPosition": { "x": 14, "y": 14 }
}
```

**优点**：macOS 用原生红绿灯（功能、外观、可访问性、深色模式自动适配全有）；Windows/Linux 自绘统一风格；Linear / Cursor / VS Code Insiders 都走这条。
**缺点**：3 个平台的"顶栏 layout 计算"不一样（macOS 左 78px 占位、Windows/Linux 右 138px 按钮位）；需要 `platform()` runtime 判断；调试要在 3 个平台都跑。
**适合**：本项目。dark theme 一致、用户主要在 macOS + Windows + WSL、个人工具但要"像 Linear 那种 polish"。
**例子**：Linear、Cursor、GitHub Desktop、Raycast。

### Approach C：用 `tauri-plugin-decorum` 抽象层

```rust
// src-tauri/src/lib.rs
.plugin(tauri_plugin_decorum::init())
.setup(|app| {
    let main = app.get_webview_window("main").unwrap();
    main.create_overlay_titlebar()?;
    #[cfg(target_os = "macos")]
    main.set_traffic_lights_inset(14.0, 14.0)?;
    Ok(())
})
```

**优点**：3 平台逻辑封装在 Rust 一边；提供 Win11 Snap Layout 弹窗 helper；提供 macOS transparent without privateApi。
**缺点**：313 stars、维护状态"mostly maintenance mode"（README 原话），未来 Tauri 官方实现这些后会废弃；多一个依赖；多一层 CSS class（`decorum-tb-btn` 等）；CSS 控制更绕。
**适合**：不想自己写跨平台分支的小项目、需要 Snap Overlay 的 Windows-first 项目。
**不推荐本项目**：Everlasting 已有 Vue 3 组件体系，再引一个 Rust plugin 会增加学习成本；Approach B 的代码量不大（一个 TitleBar.vue + 5 行 conf）；自己实现也是学习 harness 工程的一部分。

---

## 7. 参考资料

### 7.1 官方文档（v2.tauri.app / docs.rs）

- [Window Customization](https://v2.tauri.app/learn/window-customization/) — 官方教程，包含 HTML/CSS/JS 完整 demo（`decorations: false` 路线）
- [Config schema](https://v2.tauri.app/reference/config/) — 所有窗口字段权威定义
- [`TitleBarStyle` Rust enum](https://docs.rs/tauri/2.11.2/tauri/enum.TitleBarStyle.html) — 三个变种的精确语义
- [`Window` JS API](https://v2.tauri.app/reference/javascript/api/namespacewindow/) — `getCurrentWindow().minimize()` 等方法签名
- [Capability permissions](https://v2.tauri.app/security/capabilities/) — `core:window:*` 权限列表

### 7.2 社区库

- **[`@tauri-controls/vue`](https://www.npmjs.com/package/@tauri-controls/vue)** (951 ⭐, agmmnn/tauri-controls master) — Vue 3 + Tailwind 写的跨平台 WindowControls / WindowTitlebar，可直接抄源码。关键文件：
  - `WindowControls.vue` — 平台分发（macOS/Windows/Gnome）
  - `WindowTitlebar.vue` — 拖拽容器 + slot
  - `controls/MacOs.vue` / `Windows.vue` / `linux/Gnome.vue` — 平台特定渲染
  - `utils/window.ts` — `getCurrent()` + `minimize()` / `toggleMaximize()` / `close()` 包装
  - `utils/os.ts` — `@tauri-apps/plugin-os` 检测
- **[`tauri-plugin-decorum`](https://github.com/clearlysid/tauri-plugin-decorum)** (313 ⭐) — Rust 端封装，含 macOS traffic lights inset + Win11 Snap Layout + 透明窗口（无 privateApi）

### 7.3 关键 issue / PR

- [tauri#4316](https://github.com/tauri-apps/tauri/issues/4316) — macOS unfocused drag region 不响应（4 年未修）
- [tauri#11856](https://github.com/tauri-apps/tauri/issues/11856) — Linux GNOME `visible(false)` decoration 卡死
- [tauri#13044](https://github.com/tauri-apps/tauri/issues/13044) — macOS setTitle 重置红绿灯位置
- [tauri#9287](https://github.com/tauri-apps/tauri/issues/9287) — Windows 圆角 + 阴影问题完整复盘
- [tauri#7900](https://github.com/tauri-apps/tauri/issues/7900) — `data-tauri-drag-resize-region` 请求（已 closed not implemented）
- [tao#1046](https://github.com/tauri-apps/tao/issues/1046) — Linux Wayland client decoration 回归（与 KDE 兼容争论）
- [wry#1650](https://github.com/tauri-apps/wry/issues/1650) — Windows Controls Overlay API 请求（vs Electron）
- [liminal-hq/emoji-nook#5](https://github.com/liminal-hq/emoji-nook/issues/5) — **`-webkit-app-region: drag` 在 WebKitGTK 完全不支持的确认**
- [Open-Less/openless#531](https://github.com/Open-Less/openless/pull/531) — 中文 PR，KDE/Wayland 自定义 titlebar workaround 完整方案（含 WebKit env vars + nudge resize）

### 7.4 参考应用反推

| 应用 | 顶栏方案 | 备注 |
|---|---|---|
| Linear（Electron） | macOS `titleBarStyle: "hiddenInset"`（Electron 等价 Overlay） | sidebar 顶到屏幕 0，红绿灯浮其上 |
| Cursor (Electron) | 同上 | "Command Bar" 居中（与红绿灯同行） |
| VS Code (Electron) | macOS `titleBarStyle: "hiddenInset"`；Windows/Linux `frame: false` + WCO | 单一 titlebar 容纳菜单 + 标题 + 按钮 |
| GitHub Desktop (Electron) | Linear 同款 | |
| Raycast (Native macOS) | 系统原生 | 无 Windows 版本，简单 |

VS Code 是最复杂的——支持原生 / 自定义 / WCO 三种模式切换（用户可在 settings 选）。如果项目只单一模式（dark only），不需要这么复杂。

---

## Caveats / Not Found

- **WSLg + `decorations: false` 没有现成报告**：spike-001 只验证了"窗口能开 + 中文渲染"；具体 `decorations: false` 后拖拽/最大化/关闭按钮在 Weston 合成器下的表现**没人发过 issue**（搜过 `tauri+wsl+wslg+decorations` 无结果）。**必须实测**。建议在重构落地后做一个 `spike-004-wsl-custom-titlebar.md` 记录实测结果。
- **`titleBarStyle: "Overlay"` 与 `decorations: false` 同时设的官方推荐**：Tauri 官方文档没有明确说"macOS 单独用 titleBarStyle，Windows/Linux 单独用 decorations false"——这是社区共识，从 `tauri-plugin-decorum` README + `openless` PR 推断。本研究未做反向实证（没在 macOS 真机测过两个都设 + `trafficLightPosition` 的相互作用）。
- **`trafficLightPosition` 在 `decorations: false` 时是否失效**：官方文档明确说 "Requires `titleBarStyle: 'overlay'` and `decorations: true`"——但社区库 `tauri-plugin-decorum` 在 `decorations: false` 时也调 `set_traffic_lights_inset` 并 work，可能是 Rust 端走的是 NSWindow 直接 API 绕过 Tauri 的检查。本研究未实证。
- **macOS Sequoia (15.x) 全屏行为差异**：tauri-controls/Vue 源码里有 `fullscreen` vs `maximize` 区分（Alt 键切换），原因是 macOS 默认 + 是全屏不是最大化。本研究未深入 macOS 全屏与 maximize 的语义差。
- **`@tauri-apps/plugin-os` 是 Tauri 2 一等公民**但当前项目 `Cargo.toml` / `package.json` **未引用**——如果用 `platform()` 函数检测 OS，需要先 `pnpm add @tauri-apps/plugin-os` + `cargo add tauri-plugin-os`。或者改用 `userAgent` 字符串解析（不需要插件，但不可靠）。
