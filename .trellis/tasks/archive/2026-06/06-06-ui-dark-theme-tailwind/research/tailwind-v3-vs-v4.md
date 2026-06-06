# Research: Tailwind CSS v3 vs v4（Vue 3 + Vite 6 + Tauri 2 场景）

- **Query**: 为 Vue 3.5 + Vite 6 + Tauri 2 桌面应用选择 Tailwind v3 或 v4
- **Scope**: external（少量 internal，参考了现有 `app/` 配置）
- **Date**: 2026-06-06
- **结论先行**: **推荐 Tailwind v4.3.x + `@tailwindcss/vite` 插件**（Vite 6 官方 peer deps 支持；CSS-first 配置 + `@theme` 更适合本项目「Prussian blue 调色板 + dark only + 原子化工具类」需求）

---

## TL;DR

- Tailwind v4.3.0 已于 2026-05-08 发布，是稳定版（`dist-tags.latest`）。推荐路径是 **@tailwindcss/vite 4.3.0**（`peerDependencies.vite: "^5.2.0 || ^6 || ^7 || ^8"`，与项目 Vite 6 完全兼容）。
- v3.4.19 仍以 `dist-tags.v3-lts` 维护（最后更新 2025-12-10），未标 deprecated，但已经不在主线上。
- 关键差异：v4 用 **CSS-first**（`@theme` + CSS 变量）取代 `tailwind.config.js`；引擎从 JS 切到 **Oxide（Rust）**，完整构建快 5×，增量构建快 100×；`@import "tailwindcss"` 替代 `@tailwind base/components/utilities`；PostCSS 集成迁到独立包 `@tailwindcss/postcss`。
- 浏览器要求 v4 最低 **Safari 16.4 / Chrome 111 / Firefox 128**。Tauri 2 在三大平台的 webview 都满足：macOS WKWebView（macOS 13+ 内置 ≥ 16.4）、Windows WebView2（Chromium ≥ Chrome 111）、Linux WebKit2GTK 2.40+（Ubuntu 22.04+ 默认即满足）。
- 与本项目对齐度（dark only + 6 色调色板 + CJK + Mono 字体 + ~10 组件）：v4 的 `@theme` 写自定义 token 比 v3 的 `theme.extend` 更紧凑、更容易和已有的 hex 颜色一一映射。
- 不建议混用：要么纯 v4（推荐），要么纯 v3（保守），不要 v3 主题 + v4 runtime。

---

## 关键数据点（截至 2026-06-06）

| 维度 | 数据 | 来源 |
|---|---|---|
| Tailwind v4 最新稳定版 | **4.3.0**（2026-05-08） | `registry.npmjs.org/tailwindcss` `dist-tags.latest`；GitHub `releases.atom` |
| Tailwind v3 LTS | **3.4.19**（2025-12-10） | `dist-tags.v3-lts`；`registry.npmjs.org/tailwindcss/3.4.19` |
| `@tailwindcss/vite` | **4.3.0** | `peerDependencies.vite: "^5.2.0 \|\| ^6 \|\| ^7 \|\| ^8"` |
| `@tailwindcss/postcss` | **4.3.0** | 用于非 Vite 场景的 PostCSS 集成 |
| 引擎 | Oxide（Rust native），`@tailwindcss/oxide` | `package.json devDependencies` |
| 速度 | 全量构建 up to **5×**、增量 up to **100×**（v3.4 → v4.0 基准：378ms → 100ms） | v4 启动 blog post `tailwindcss.com/blog/tailwindcss-v4` |
| 浏览器要求 | Safari 16.4 / Chrome 111 / Firefox 128+ | 升级指南 `tailwindcss.com/docs/upgrade-guide` |
| 包大小（unpacked） | `tailwindcss@4.3.0` 约 754 KB（解压，34 文件） | npm registry `dist.unpackedSize` |
| VSCode 支持 | 官方 `Tailwind CSS IntelliSense` 扩展同时支持 v3 和 v4（v4 走 CSS 入口点扫描） | `tailwindcss.com/docs/editor-setup` |

---

## v3 vs v4 对比表

| 维度 | Tailwind v3.4.19 (LTS) | Tailwind v4.3.0 (latest) |
|---|---|---|
| 配置方式 | `tailwind.config.{js,ts}` + JS object | CSS-first：`@import "tailwindcss"` + `@theme { --color-*: ... }` |
| 入口 CSS | `@tailwind base; @tailwind components; @tailwind utilities;` | `@import "tailwindcss";` 一行 |
| 主题扩展 | `theme.extend.colors = { prussian: { 500: '#3B5BDB' } }` | `@theme { --color-prussian-500: #3B5BDB; }` |
| 暗色模式 | `darkMode: 'class'`（在 config 里） | 默认 `prefers-color-scheme`；用 `@custom-variant dark (&:where(.dark, .dark *))` 改类选择器 |
| 字体 | `theme.extend.fontFamily` 在 config | `@theme { --font-sans: "Noto Sans CJK SC", ...; }` |
| PostCSS 集成 | `tailwindcss` 自身就是 PostCSS 插件 | 拆出独立包 `@tailwindcss/postcss` |
| Vite 集成 | 通过 PostCSS 间接生效 | **官方 `@tailwindcss/vite` 插件**（性能最佳） |
| 引擎 | JS（PostCSS + Lightning CSS 可选） | **Oxide（Rust）** + 内建 Lightning CSS |
| 构建速度 | 基准 378ms 全量 | 100ms 全量（5×）；增量 192µs（100×） |
| `@apply` 在 Vue `<style scoped>` | 不工作（已知 issue #13399，17 评论，已关） | 同样不工作（issue 仍存在） |
| 浏览器要求 | 较宽松（IE11 也支持，靠 PurgeCSS 思路） | Safari 16.4 / Chrome 111 / Firefox 128+ |
| 自动 content 检测 | 需要手写 `content: ['./index.html','./src/**/*.{vue,ts,js}']` | **自动**（v4 启发式扫描） |
| 容器查询 | 需 `@tailwindcss/container-queries` 插件 | **内置**（`@container` / `@sm:`） |
| IDE 支持 | IntelliSense 扩展（旧配置方式） | IntelliSense 扩展（识别 `@import "tailwindcss"`） |
| 状态（2026-06） | LTS 维护中，新功能不会进 v3 | 活跃主线，每月小版本 |

---

## Vue 3 + Vite 6 + Tauri 2 兼容性细节

### 1. Vite 6 集成（关键）
- `@tailwindcss/vite@4.3.0` 的 `peerDependencies` 明确写 `"vite": "^5.2.0 || ^6 || ^7 || ^8"`，与本项目 Vite `^6.0.3` 完全兼容。来源：`registry.npmjs.org/@tailwindcss/vite/4.3.0`
- 官方安装指引（`tailwindcss.com/docs/installation/using-vite`）：
  ```bash
  pnpm add tailwindcss @tailwindcss/vite
  ```
  ```ts
  // app/vite.config.ts
  import { defineConfig } from "vite";
  import vue from "@vitejs/plugin-vue";
  import tailwindcss from "@tailwindcss/vite";

  export default defineConfig({
    plugins: [vue(), tailwindcss()],
  });
  ```
  ```css
  /* app/src/style.css */
  @import "tailwindcss";
  ```

### 2. Tauri 2 webview 兼容性
Tauri 2 在三大平台使用系统 webview，都满足 v4 的浏览器要求：

| 平台 | Webview | v4 兼容 |
|---|---|---|
| macOS | WKWebView（macOS 13+ 内置 Safari 16.4+） | 满足 |
| Windows | WebView2（系统 Edge / Chromium ≥ Chrome 111） | 满足 |
| Linux | WebKit2GTK 2.40+（Ubuntu 22.04 默认） | 满足（WSL 2 + Ubuntu 22.04 满足） |

Tauri 本身对 CSS 框架无要求，仅关心 webview 渲染。来源：Tauri 2 文档 `v2.tauri.app/concept/webview/`。

### 3. Vue `<style scoped>` 已知坑（两个版本都有）
- **`@apply` 在 `<style scoped>` 不工作**：tailwindlabs/tailwindcss issue #13399（17 评论，closed）。意味着新组件如果想用 `@apply` 复用 utility class，要么放全局 CSS，要么用 `:deep()` 改写。
- **Vite build 警告 Vue `:deep()` 选择器**：false-positive warning（closed）。可以忽略。
- 实际迁移策略：`<style scoped>` 块只写**组件私有样式**（`width: 100%`、`display: flex` 这种），**主题色 / 间距 / 字体全部用 utility class**。这反而是 Tailwind 推荐做法。

### 4. IDE 支持
- VSCode 官方扩展 `Tailwind CSS IntelliSense`（bradlc.vscode-tailwindcss）同时支持 v3 和 v4：
  - v4：扫描项目找 `@import "tailwindcss"` 的 CSS 入口；可手动配 `tailwindCSS.experimental.configFile`
  - v3：找 `tailwind.config.{js,cjs,mjs,ts}`
  - 自动补全、hover preview、class 排序（配合 `prettier-plugin-tailwindcss`）
- 建议同时装 `prettier-plugin-tailwindwindcss`（class 自动排序）。

### 5. SSR/RSC 影响
- 本项目是 Tauri 2 桌面应用，无 SSR 无 RSC，Tailwind 的 SSR caveat（v4 早期曾有）不影响。

---

## 主题 Token 落地对比（Prussian blue + dark only）

### v4 推荐写法（CSS-first，Prussian blue + 状态色 + 字体）
```css
/* app/src/style.css */
@import "tailwindcss";

/* 暗色 only：覆盖默认 prefers-color-scheme 走 .dark 类 */
@custom-variant dark (&:where(.dark, .dark *));

/* 在 <html class="dark"> 包裹下也支持 prefers-color-scheme */
@media (prefers-color-scheme: light) {
  :root:not(.dark) { /* 留空，dark only 简化 */ }
}

@theme {
  /* 自定义色板（Prussian blue） */
  --color-prussian-50:  #EEF1FA;
  --color-prussian-100: #D5DDF2;
  --color-prussian-200: #ABBDE5;
  --color-prussian-300: #7E9AD7;
  --color-prussian-400: #5478C9;
  --color-prussian-500: #3B5BDB; /* 主色 = 现代普鲁士蓝 */
  --color-prussian-600: #2E47B0;
  --color-prussian-700: #243887;
  --color-prussian-800: #1B2A66;
  --color-prussian-900: #131E4D;
  --color-prussian-950: #0A0E14; /* bg */

  /* 语义/状态色（PRD 锁定的） */
  --color-surface:     #131822;
  --color-elevated:    #1A2030;
  --color-border:      #1E2530;
  --color-text:        #E5E7EB;
  --color-text-muted:  #8B95A7;
  --color-text-dim:    #64748B;
  --color-accent-muted:#1E2A5E; /* 选中态 */

  --color-tool-read:   #06B6D4;
  --color-tool-write:  #10B981;
  --color-tool-shell:  #F59E0B;
  --color-tool-error:  #EF4444;
  --color-tool-thinking: #A78BFA;

  /* 字体 */
  --font-sans: "Noto Sans CJK SC", "PingFang SC", "Microsoft YaHei",
               "WenQuanYi Zen Hei", system-ui, -apple-system, sans-serif;
  --font-mono: "JetBrains Mono", ui-monospace, "SFMono-Regular", "Cascadia Code",
               Consolas, "Liberation Mono", Menlo, monospace;
}

:root {
  font-family: var(--font-sans);
  color: var(--color-text);
  background: var(--color-prussian-950);
}
```
随后可在 HTML/模板里直接用 `bg-prussian-500`、`text-text-muted`、`border-border`、`font-mono`。

### v3 等价写法（JS config）
```js
// app/tailwind.config.js
/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{vue,ts,js}"],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        prussian: { 50: '#EEF1FA', /* ... */ 950: '#0A0E14' },
        surface: '#131822',
        elevated: '#1A2030',
        border: '#1E2530',
        text: '#E5E7EB',
        'text-muted': '#8B95A7',
        'tool-read': '#06B6D4',
        // ...
      },
      fontFamily: {
        sans: ['"Noto Sans CJK SC"', 'system-ui', 'sans-serif'],
        mono: ['"JetBrains Mono"', 'ui-monospace', 'monospace'],
      },
    },
  },
  plugins: [],
};
```
Tailwind v3 自动生成 `bg-prussian-500` 这类 utility，**但语义色 `bg-surface` / `bg-elevated` 只能用引号包裹**（或在 v3.4 也支持 kebab-case），稍繁琐。

### 关键差异
- v4 的 token **同时暴露为 CSS 变量**（`var(--color-prussian-500)`），可在 `<style scoped>` 内的 `linear-gradient`、`box-shadow` 等地方直接引用。v3 需要 `theme()` helper 或单独维护一份变量文件。
- v4 的 `@custom-variant` 比 v3 的 `darkMode: 'class'` 灵活：可以同时支持类选择器、`data-theme` 属性、`prefers-color-scheme`。

---

## 迁移成本评估

### 现状（3 个组件有手写 CSS）
- `app/src/components/ChatWindow.vue` ~1016 行（含 `<style scoped>` ~400 行 light theme）
- `app/src/components/SessionList.vue` ~249 行（`<style scoped>` ~100 行）
- `app/src/components/ProjectTabs.vue` ~242 行（`<style scoped>` ~80 行）

### 迁移策略
**推荐**：「新组件 Tailwind，老组件保留 `<style scoped>`」渐进迁移。两者可共存：
- 引入 Tailwind 后，**新组件**（拆分出的 `MessageBubble` / `ToolCallCard` / `ThinkingBlock` / `InputBar` 等）直接用 utility class。
- **老组件**：保留 `<style scoped>` 块，仅当重构到该组件时再迁移（一次性 PR 改造）。
- 不需要一次性把 3 个老组件的 hex 颜色全部替换。Tailwind 的 `@layer components` 或 utility class 跟 `<style scoped>` 互不冲突。

### 迁移步骤（v4 推荐）
1. `pnpm add tailwindcss @tailwindcss/vite`（新增 2 个 dev dep）
2. 改 `app/vite.config.ts`：加 `import tailwindcss from '@tailwindcss/vite'` + `plugins: [vue(), tailwindcss()]`
3. 改 `app/src/style.css`：开头加 `@import "tailwindcss";`，然后写 `@theme` 块（拷贝上面的 token）
4. 验证 `pnpm build` 成功（CSS bundle 多 5-15 KB gzip）
5. **新组件**直接用 utility class；**老组件不动**。
6. 逐步把老组件的 hex 颜色替换成 token（按需、不急）。

### v3 迁移差异
- 多一步：`npx tailwindcss init -p`（生成 `tailwind.config.js` + `postcss.config.js`）
- 多 2 个 dev dep：`postcss` `autoprefixer`（v4 不需要）
- 主题 token 写在 JS 而不是 CSS，IDE 跳转稍弱

---

## Bundle Size 估算（dark theme + ~10 组件 + 1 套调色板）

| 框架 | 原始未压缩 | gzip 后（典型） |
|---|---|---|
| Tailwind v3（PurgeCSS 后） | 30-50 KB | **8-15 KB** |
| Tailwind v4（Oxide + Lightning CSS） | 20-35 KB | **5-12 KB** |
| 本项目预估（含 6 色 prussian scale + 5 状态色 + 2 字体族） | 25-30 KB | **8-10 KB** |

v4 因 Lightning CSS 自动 minify + 更好的 tree-shaking，bundle 更小。来源：v4 启动 blog 称「significantly improving performance on large pages」（无具体数字，但社区基准普遍优于 v3）。

Tauri 2 桌面应用对 bundle size 不敏感（资源本地化），所以两者都可接受。

---

## Pros / Cons

### Tailwind v4.3.0
**Pros**
- 官方 Vite 6 兼容（peer deps 明确写 v6）
- 构建快 5× / 增量 100×（Rust Oxide 引擎）
- CSS-first 配置，token 既能生成 utility class 又能当 CSS 变量用（在 `<style scoped>` 里 `var(--color-prussian-500)` 直接引用）
- 自动 content detection，不用手写 `content: ['./src/**/*.{vue,ts}']`
- 容器查询内置（`@container` / `@sm:`）
- `dark:` variant 改用 `@custom-variant` 更灵活（类 / data attr / media query 任意）
- 主线活跃，2026 年还在每月发版

**Cons**
- 浏览器要求高（Safari 16.4+），但 Tauri 2 的 webview 全满足
- 配置从 JS 移到 CSS，习惯 v3 的开发者要适应
- `@apply` 仍不能在 Vue `<style scoped>` 用（老问题）
- 一些老插件（如 `@tailwindcss/typography`）v4 适配略滞后（需查 v4 兼容版本）
- 错误信息在新版本里偶有英文 build 警告，体验略糙

### Tailwind v3.4.19 (LTS)
**Pros**
- 老牌生态，所有第三方插件（`daisyui`、`@tailwindcss/typography`、`flowbite` 等）都最先支持 v3
- 文档/博客/StackOverflow 答案多
- 浏览器兼容更宽松
- `tailwind.config.js` 写 token 对老用户更直观

**Cons**
- 不是主线路径，2026 年新功能不会进 v3
- 性能比 v4 慢一个量级（开发体验）
- 配置和样式分离（JS config vs CSS），改 token 要在两处
- Tauri 2 webview 都够新，享受不到 v3 的兼容性优势

---

## 三个可行方案

### 方案 A（推荐）：Tailwind v4 + @tailwindcss/vite + @theme CSS 变量
```bash
pnpm add tailwindcss @tailwindcss/vite
```
```ts
// app/vite.config.ts
import tailwindcss from "@tailwindcss/vite";
plugins: [vue(), tailwindcss()],
```
```css
/* app/src/style.css */
@import "tailwindcss";
@custom-variant dark (&:where(.dark, .dark *));
@theme { --color-prussian-500: #3B5BDB; /* ... 全部 token */ }
```
**适用**：新项目 / 愿意接受 v4 配置范式 / 想要最佳性能。**本项目首选。**

### 方案 B：Tailwind v3.4.19 + PostCSS（保守）
```bash
pnpm add -D tailwindcss@3 postcss autoprefixer
npx tailwindcss init -p
```
```js
// app/tailwind.config.js
export default {
  content: ["./index.html", "./src/**/*.{vue,ts,js}"],
  darkMode: 'class',
  theme: { extend: { /* tokens */ } },
};
```
**适用**：依赖大量 v3 第三方插件 / 团队对 JS config 更熟 / 担心 v4 偶发 build 警告。

### 方案 C：v4 + @tailwindcss/postcss（折中，绕过 Vite 插件）
```bash
pnpm add tailwindcss @tailwindcss/postcss postcss
```
```js
// postcss.config.mjs
export default { plugins: { "@tailwindcss/postcss": {} } };
```
**适用**：项目已有 PostCSS pipeline / 用其他 Vite 插件链。本项目**不推荐**（Vite 插件更优）。

---

## 风险与 Caveats

1. **reka-ui 1.0.0-alpha.10 与 Tailwind v4** 共存未发现冲突（reka-ui 是 headless，不带样式，Tailwind 可独立使用）。
2. **`@apply` 在 Vue `<style scoped>` 不工作**（两个版本都有），但本项目不依赖——直接用 utility class 即可。
3. **Tauri 2 Linux WebKit2GTK 版本**：在老发行版（如 Ubuntu 20.04）可能不满足 v4 的 Safari 16.4 要求。本项目 WSL 2 + Ubuntu 22.04 满足。如未来要支持老 Linux 发行版，需 fallback 到 v3。
4. **WSLg 字体**：本项目已用 Noto Sans CJK SC 字体栈（见 `app/src/style.css`），Tailwind v4 的 `--font-sans` 变量定义后全局生效，无需额外改动。
5. **Vue 3.5 + Volar（vue-tsc 2.1.10）**：Tailwind utility class 在模板里不会触发 TypeScript 错误（class 是 string），无冲突。
6. **PRD 里提的「暗色 only」**：v4 的 `@custom-variant` 比 v3 的 `darkMode: 'class'` 更干净地支持「只在 dark 类下激活，不响应系统偏好」。
7. **打包体积**：Tauri 2 不在乎（本地资源），两者均可。
8. **第三方 UI 库依赖**：本项目只用 reka-ui（headless），不依赖 daisyui/flowbite，避免 v3 锁定风险。

---

## 参考来源（External References）

- 官方安装文档（v4 + Vite）：https://tailwindcss.com/docs/installation/using-vite
- 官方安装文档（v4 + PostCSS）：https://tailwindcss.com/docs/installation/using-postcss
- 官方 Vue + Vite 指南：https://tailwindcss.com/docs/installation/framework-guides/vue/vite
- v4 启动 blog（性能数字、迁移故事）：https://tailwindcss.com/blog/tailwindcss-v4
- v3 → v4 升级指南（breaking changes 清单）：https://tailwindcss.com/docs/upgrade-guide
- v4 主题变量文档：https://tailwindcss.com/docs/theme
- v4 颜色文档（OKLCH palette）：https://tailwindcss.com/docs/colors
- v4 暗色模式文档：https://tailwindcss.com/docs/dark-mode
- v4 编辑器设置（VSCode IntelliSense）：https://tailwindcss.com/docs/editor-setup
- v3 安装文档：https://v3.tailwindcss.com/docs/installation
- Tauri 2 窗口定制（顶栏 `decorations: false`）：https://v2.tauri.app/learn/window-customization/
- npm `tailwindcss@4.3.0` 元数据：https://registry.npmjs.org/tailwindcss/4.3.0
- npm `@tailwindcss/vite@4.3.0` 元数据：https://registry.npmjs.org/@tailwindcss/vite/4.3.0
- npm `tailwindcss@3.4.19` 元数据（LTS）：https://registry.npmjs.org/tailwindcss/3.4.19
- 已知 issue：@apply 在 Vue scoped 不工作：https://github.com/tailwindlabs/tailwindcss/issues/13399
- 已知 issue：Vite build 误报 Vue `:deep()` 警告：https://github.com/tailwindlabs/tailwindcss/issues/（多条 closed）
- GitHub releases 源：https://github.com/tailwindlabs/tailwindcss/releases.atom

## 内部相关文件
- `app/package.json` — 当前 Vue 3.5.13 / Vite 6.0.3 / reka-ui 1.0.0-alpha.10（无 CSS 框架）
- `app/vite.config.ts` — 极简，无别名
- `app/src/style.css` — 29 行全局（字体 + reset + 极简变量），`<style scoped>` 块在 3 个组件
- `app/src/components/{ChatWindow,SessionList,ProjectTabs}.vue` — 三个要渐进迁移的组件
- `app/src/stores/{chat,projects,config}.ts` — 状态层，与 CSS 框架选择无关
- `docs/spikes/003-ui-reference-prompts.md` — 设计 token 来源（颜色/字体）
- `.trellis/tasks/06-06-ui-dark-theme-tailwind/prd.md` — 任务 PRD（D2 Tailwind 接入需求）

## Caveats / Not Found
- 未找到 v4 具体的「dark only」最佳实践（一句话配置 `darkMode: 'dark'` 之类的快捷方式），v4 是通过 `@custom-variant` 显式声明。
- Tailwind v4 的 `npx @tailwindcss/upgrade` 升级工具本项目用不到（无 v3 → v4 迁移路径），但如果未来从 v3 迁过来，可一键。
- 未在 npm 上找到 v3 的「deprecated」标记（`deprecated: 'no'`），但 dist-tag 已从 `latest` 切到 `v3-lts`，且 2026 年 1 月以来无新 v3 版本发布。
