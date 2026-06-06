# CJK Font Bundling (frontend infra)

> 跨平台桌面应用打包中文字体的规范。经验沉淀自 task `06-06-font-cjk-harmosans-bundle` (2026-06)。

---

## Why this exists

WSL2 + Tauri 2 WebView2 渲染中文小字号时糊, 不是 CSS 杠杆能救的 — 根因在字体本身。`Microsoft YaHei` 是 2006 年给 Windows Vista 桌面 UI 设计的, Dark theme + 14-15px 场景下是糊的天花板。优化系统字体栈 (前置 `Microsoft YaHei UI` 等) 在多数 Win 10/11 镜像上**根本不会命中**, 浏览器一路回退到原栈, 改 CSS 看起来像没生效。

**判断 CSS 是否真的解决 CJK 字体问题**: 跑 DevTools 看 `getComputedStyle(document.body).fontFamily`, 第一项**真正命中**的字体才能解决。如果只是改了 font-size/line-height, 视觉变化在 Dark theme 14-15px 下肉眼几乎无感。

---

## Project Convention: Bundle HarmonyOS Sans SC Subset

### What

每个 Tauri 桌面 app 把 **HarmonyOS Sans SC Regular 子集**打包进 `app/src/assets/fonts/HarmonyOSSansSC-Regular.subset.woff2`, 通过 `@font-face` 接入全局 `--font-sans` 栈首位。

### Why

- HarmonyOS Sans SC 是华为 2021 年为 HarmonyOS 设计的现代 UI 黑体, Dark theme + 14-16px 场景下笔画清晰、字面舒服
- 免费商用 (HarmonyOS Sans Fonts License Agreement), 详见 [THIRD_PARTY_LICENSES.md](../../../THIRD_PARTY_LICENSES.md)
- 跨平台渲染一致: 解决 WSL / Windows / 未来 macOS 字体回退不可控问题
- 子集化到 3500 常用字 + ASCII + 标点 = **472 KB** woff2, 比全量 8 MB 缩 17x, 罕见字回退到系统栈第二位视觉无感

### How

```css
/* app/src/style.css — 文件顶部, 在 @import "tailwindcss" 之前 */
@font-face {
  font-family: "HarmonyOS Sans SC";
  src: url("./assets/fonts/HarmonyOSSansSC-Regular.subset.woff2")
    format("woff2");
  font-weight: 400;
  font-style: normal;
  font-display: swap;
}

@theme {
  --font-sans: "HarmonyOS Sans SC", "Microsoft YaHei UI", "Microsoft YaHei",
    "PingFang SC", "Noto Sans CJK SC", "Source Han Sans SC", "Noto Sans CJK",
    "WenQuanYi Zen Hei", system-ui, -apple-system, "Segoe UI", Roboto,
    sans-serif;
  /* ... */
}
```

字体栈第二位起保留系统字体 (Microsoft YaHei UI / YaHei / PingFang SC 等) 作为 woff2 加载失败时的兜底, 以及子集未覆盖罕见字的按字回退。

---

## Pattern: Subset Sizing (3500 常用字)

### Numbers (实测, 2026-06)

| 阶段 | 大小 |
|---|---|
| 原始 TTF (全量字) | 8.09 MB |
| 3500 常用字 + ASCII + 标点 (3639 chars) subset 后 TTF | 0.84 MB |
| 最终 woff2 (brotli 压缩) | **472 KB** |

### Coverage

- 3500 常用字覆盖中文 UI 99.9% 场景: 菜单、按钮、状态、聊天消息、错误提示、文件路径
- 罕见字 (人名、地名、古文) 触发浏览器按字 fallback 到 `--font-sans` 第二位的 `Microsoft YaHei UI` 等
- UX 几乎无损: 用户极难在同一屏同时看到常用字和罕见字; 偶尔出现 fallback 字也不会破版

### When to re-subset

- 字体上游发新版本 (HarmonyOS Sans 改版)
- 报告某种常用字缺失 (LLM 输出含子集外的高频字)
- 字表规则升级 (比如扩到 7000 通用字)

子集化脚本: [`app/scripts/subset-font.mjs`](../../../app/scripts/subset-font.mjs) — 接受 `TTF_PATH` / `CHARS` / `OUT_PATH` 三个环境变量覆盖, 默认值指向项目内路径, 任何 cwd 都能跑。

---

## Pattern: Toolchain (Node.js subset-font + wawoff2)

### Why Node, not Python fonttools

WSL Ubuntu 22.04 默认无 `pip` / `python3-venv`, 装 fonttools 要 sudo apt install。Node 24 + npm 11 + pnpm 项目里都有, 走 Node 工具链零额外系统依赖。

### Dependencies (project devDependencies)

```json
{
  "devDependencies": {
    "subset-font": "^2.0.0",  // HarfBuzz WASM subsetter
    "wawoff2": "^2.0.1"       // woff2 编/解码 (Node binding)
  }
}
```

**放在 devDependencies** 而不是 dependencies: 这两个包仅子集化脚本用, 不进 Vite 构建产物, 装到用户机器上没意义。

### Idempotency

`node app/scripts/subset-font.mjs` 重复跑产出的 woff2 字节级一致 (HarfBuzz WASM 是确定性子集算法, 输入相同输出相同)。可以安全加入 CI 检查。

### Error handling pattern

`subset-font.mjs` 缺依赖时打印清晰中文错误并 `exit 1`, 引导用户 `cd app && pnpm install`。**不要在错误时静默 fallback** — 用户得知道为什么脚本不工作。

---

## Pattern: Vite + Tauri 2 @font-face URL Handling

### Verified

- `@font-face { src: url("./assets/fonts/...woff2") }` 用**相对路径**在 dev (`pnpm dev` / `pnpm tauri dev`) 和 prod (`pnpm build`) 都正确解析
- Vite 产出会自动加 content hash: `HarmonyOSSansSC-Regular.subset-Cxk3ItG8.woff2`
- Tauri 2 走 `frontendDist: "../dist"`, 整个 dist/ 跟随 app 打包, woff2 自动 ship
- CSS bundle (`dist/assets/index-*.css`) 中 src 会被 Vite 改写为带哈希的绝对路径

### Don't

```css
/* ❌ 绝对路径, dev/prod 不一致 */
src: url("/src/assets/fonts/HarmonyOSSansSC-Regular.subset.woff2");

/* ❌ 引用上游 CDN, 依赖网络 */
src: url("https://fonts.googleapis.com/...");

/* ❌ font-display: block, 会触发 FOIT */
font-display: block;
```

### Do

```css
/* ✅ 相对 CSS 文件位置的相对路径, Vite 处理 */
src: url("./assets/fonts/HarmonyOSSansSC-Regular.subset.woff2")
  format("woff2");
font-display: swap;  /* 避免 FOIT, 接受 FOUT 闪一下 */
```

---

## Convention: Third-Party Font License Compliance

### Three-place notice pattern

打包的第三方字体 (HarmonyOS Sans / 任何其他 OFL / 商用字体) 必须**三处声明**:

1. **`THIRD_PARTY_LICENSES.md`** (项目根) — 列出名称、版权、license 路径、限制条件摘要
2. **`app/src/assets/fonts/LICENSE.txt`** (字体资产同目录) — 完整 license 文本, 不可修改
3. **`app/src/style.css` 顶部注释** — `@font-face` 上方明确指出版权 + license 文件位置

### HarmonyOS Sans 限制 (3 条)

打包可, 需:
1. **Prominent notice** — 软件里显著声明使用了 HarmonyOS Sans
2. **No modification** — 不可修改字体文件 (不可再切子集? 不, 子集化是 bundle 行为不是修改字体本身, 仍合规; 改 glyph 才是)
3. **No standalone redistribution** — 不能以 HarmonyOS Sans 为唯一内容独立再分发 (作为软件一部分 OK)

### Check before bundling any third-party font

- [ ] License 允许 bundle + 商用
- [ ] 保留原始 license 文本在字体目录
- [ ] 项目根 `THIRD_PARTY_LICENSES.md` 加条目
- [ ] CSS / 代码里有 prominent notice 注释
- [ ] 不修改 glyph / name table

---

## Anti-pattern: Don't Try These for CJK in Tauri WebView2

### 1. 只改 CSS 杠杆就以为能解决

```css
/* ❌ 期望仅靠 CSS 优化让雅黑在 Dark theme 小字号下变清晰 */
font-size: 14px → 15px;
line-height: 1.55 → 1.7;
letter-spacing: 0.01em;
text-rendering: optimizeLegibility;
```

**根因是字体本身, 改 CSS 只是让糊法稍微不同。** 14→15px 在 Dark theme 灰字场景下肉眼几乎无感。

### 2. 用 CDN 字体服务 (Google Fonts / fontcdn)

```html
<!-- ❌ 依赖网络, 离线启动会 FOIT -->
<link href="https://fonts.googleapis.com/css2?family=Noto+Sans+SC" rel="stylesheet">
```

桌面应用期望开箱即用, 网络字体违反这一预期。

### 3. 打包全量字体 (8 MB TTF)

```bash
# ❌ 全量 HarmonyOS Sans SC Regular ≈ 8 MB
cp HarmonyOSSansSC-Regular.ttf app/src/assets/fonts/
```

8 MB 在桌面 app 是一笔不小的成本, 实际 UI 只会用到几百字。**必须子集化**。

### 4. 在多个组件里硬编码 font-family

```vue
<!-- ❌ 绕过全局 token, 维护噩梦 -->
<style scoped>
.chat-bubble { font-family: "Microsoft YaHei", sans-serif; }
</style>
```

应该统一用 `var(--font-sans)`, 改全局一次生效。

---

## Wrong vs Correct

### Wrong: 仅调 CSS 期望改善 CJK 渲染

```css
/* 改了 5 行, 热更新了, 看着没变化 */
:root {
  font-size: 15px;
  line-height: 1.7;
  letter-spacing: 0.01em;
}
/* 字体栈优化, 但 Microsoft YaHei UI 在 Win 10/11 上不存在, 回退到 Microsoft YaHei, 视觉无感 */
--font-sans: "HarmonyOS Sans SC", "Microsoft YaHei UI", "Microsoft YaHei", ...;
```

### Correct: 打包 web font 子集

```css
/* 一次性打包 HarmonyOS Sans SC 子集 (472 KB), @font-face 接入, 视觉明显改善 */
@font-face {
  font-family: "HarmonyOS Sans SC";
  src: url("./assets/fonts/HarmonyOSSansSC-Regular.subset.woff2") format("woff2");
  font-weight: 400;
  font-display: swap;
}

@theme {
  --font-sans: "HarmonyOS Sans SC", /* woff2 已声明, 浏览器真命中 */
    "Microsoft YaHei UI", "Microsoft YaHei", /* 兜底 */
    /* ... */;
}
```

配: `app/src/assets/fonts/HarmonyOSSansSC-Regular.subset.woff2` (472 KB)

---

## Reference

- Task: `.trellis/tasks/06-06-font-cjk-harmosans-bundle/` (planning doc, ADR-lite, 技术细节)
- Task: `.trellis/tasks/06-06-font-cjk-stack-polish/` (前置任务, CSS 杠杆尝试, 失败但有 typography 排版记录)
- Script: [`app/scripts/subset-font.mjs`](../../../app/scripts/subset-font.mjs)
- License: [THIRD_PARTY_LICENSES.md](../../../THIRD_PARTY_LICENSES.md)
- 字体源: https://github.com/SunsetMkt/HarmonyOS_Sans_SC_Webfont_Splitted
- 3500 常用字源: https://github.com/jinghu-moon/Simplified-Chinese-Characters
