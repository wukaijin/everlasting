# 调前端中文字体栈与排版（方案 1）

> 关联 task: `.trellis/tasks/06-06-font-cjk-stack-polish/`
> 优先级: P2
> 范围: 1 个文件 (`app/src/style.css`)
> 不满意再升级到方案 2（打包 HarmonyOS Sans SC 子集到 Tauri resources）

## Goal

在零打包成本前提下，让 Dark theme 下的中文字符在 WSL2 + WebView2 环境下渲染更清晰易读。

## What I already know

- `app/src/style.css:30-36` 现有 `--font-sans` 字体栈：`Noto Sans CJK SC → Noto Sans CJK → PingFang SC → 微软雅黑 → 文泉驿 → system-ui`
- 当前基础字号 `14px`、行高 `1.55`、`letter-spacing` 未设置
- 平台: WSL2 + Tauri 2 (Windows WebView2)，可见字体主要是 Windows 系统的 CJK 字体
- 现有 `@theme` 用 Tailwind v4 CSS-first 配置（`@import "tailwindcss"` + `@theme {}`）
- 已用 token: `--color-text-primary` (#E5E7EB) / `--color-text-secondary` (#8B95A7) / `--color-text-muted` (#64748B)

## Assumptions (resolved)

- [A1] ✅ **方案 1 优先**：仅改 CSS，不动打包资产；如不达预期再升级到方案 2 打包 web font
- [A2] ✅ **不动 `index.html`**：不引入 `<link rel="stylesheet">` 或 Google Fonts CDN，避免运行时网络依赖
- [A3] ✅ **不动组件内联样式**：只在 `style.css` 顶层 `:root` 改全局默认，让所有未显式 `font-family` 的元素自动继承
- [A4] ✅ **优先用 WebView2 真能看见的字体**：HarmonyOS Sans SC（Win 上不一定有）放第一位作为理想目标，落到微软雅黑 UI 兜底

## Requirements

### R1 — 字体栈优化

- `--font-sans` 调整顺序，把"WebView2 实际可能命中"的字体前置：
  - 新顺序：`"HarmonyOS Sans SC"`, `"Microsoft YaHei UI"`, `"Microsoft YaHei"`, `"PingFang SC"`, `"Noto Sans CJK SC"`, `"Source Han Sans SC"`, system-ui, sans-serif
- 备注：`Microsoft YaHei UI` 是 Windows 7+ 自带的 UI 优化版雅黑，比经典雅黑渲染更清晰，作为 WebView2 命中后的首选

### R2 — 排版基线调整

- 基础字号: `14px` → `15px`（中文 UI 14px 偏小，15-16px 是舒适阅读区间）
- 行高: `1.55` → `1.7`（中文需要比英文更松的行高避免笔画粘连）
- `letter-spacing`: `0` → `0.01em`（Dark theme 灰字场景下微微撑开字间距，可读性明显提升）
- 新增 `text-rendering: optimizeLegibility`
- 保留原有 `-webkit-font-smoothing: antialiased` + `-moz-osx-font-smoothing: grayscale`

### R3 — 不动其他 token / 组件

- 不动 `--color-*` token
- 不动 `--font-mono`
- 不动其他组件的 `font-size` / `line-height`（让全局继承生效即可）

## Acceptance Criteria

- [ ] `app/src/style.css` 的 `--font-sans` 包含 `"HarmonyOS Sans SC"` 作为第一项
- [ ] `app/src/style.css` 的 `:root` `font-size` = `15px`
- [ ] `app/src/style.css` 的 `:root` `line-height` = `1.7`
- [ ] `app/src/style.css` 的 `:root` 新增 `letter-spacing: 0.01em` 和 `text-rendering: optimizeLegibility`
- [ ] 原有 `-webkit-font-smoothing` / `-moz-osx-font-smoothing` 保留
- [ ] `cd app && pnpm build` 通过（vue-tsc --noEmit + vite build）
- [ ] 跑一次 `pnpm tauri dev` 视觉确认中文字符可读性改善

## Out of Scope

- 方案 2（打包 web font 进 Tauri resources）—— 留作 follow-up
- 任何组件级样式调整 —— 让全局继承生效
- Light theme 支持 —— 项目目前只支持 dark
- 字体子集化、unicode-range 拆分

## Technical Notes

- 改动文件: `app/src/style.css`（仅此一个）
- 不需要改 `tailwind.config` / `vite.config` / `package.json`
- 不需要重启 dev server（HMR 会自动 reload CSS）
- 关联 spec: `.trellis/spec/frontend/quality-guidelines.md`（如有 typography 相关规范需参考）

## Rollback

如效果不达预期，单 commit revert 即可，无需 schema migration、无需清缓存。
