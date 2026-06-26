# Frontend Development Guidelines

> Best practices for frontend development in this project.

---

## Overview

This directory contains guidelines for frontend development. Fill in each file with your project's specific conventions.

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [State Management](./state-management.md) | Pinia store patterns, chat facade + streamController split, per-session message buffer LRU 20, SSE listener routing | Filled (2026-06, 6 UI/状态 bug 修复 PR + 8-PR3) |
| [CJK Font Bundling](./cjk-fonts.md) | 跨平台中文字体打包规范 (HarmonyOS Sans SC 子集, 工具链, license 合规) | Filled (2026-06) |
| [Popover Pattern](./popover-pattern.md) | Hand-rolled popover pattern (onDocumentClick + Esc close), position direction rule (top/bottom of viewport), worktree + ModelSelect references, anti reka-ui DropdownMenu rationale, animation (modal scale / popover slide **150/100ms token 化**, 2026-06-27 PR-3d), **Reduced Motion 段** (2026-06-27 PR-1), **ConfirmDialog component** (2026-06-11 体验优化), **Tauri webview `window.confirm()`/`alert()`/`prompt()` gotcha** | Filled (2026-06-09, PR5 follow-up + UI polish; 2026-06-11 体验优化 added ConfirmDialog + Tauri gotcha; 2026-06-27 PR-3d animation 表更新为 token 引用 + 新增 reduced motion 段) |
| [Reka-UI Usage](./reka-ui-usage.md) | reka-ui 2.9.9 version pin, primitives used in project, `TextFieldRoot` not-in-2.9.9 gotcha + native `<input>` substitute, **`<style scoped>` + portal = `:deep()` gotcha** (SelectContent / DialogContent / PopoverContent et al. portal to body and escape scoped selectors — wrap rules in `:deep()` to match), **`--reka-select-trigger-width` tip** (reka-ui 2.9.9 exposes this CSS var for sizing SelectContent to its trigger; prefix is `--reka-` not `--radix-`), Label wrapper convention, data-state theming, anti reka-ui Popover rationale | Filled (2026-06-09, UI polish + SettingsModal Select fix) |
| [Design Tokens](./design-tokens.md) | Color tokens (bg/text/accent/tool), **State Tints** (hover/active/selected, 2026-06-27 PR-1), `--color-text-muted` 加亮决策 (R5), font/sans/mono, **Type Scale** + **Line Heights** + **Font Weights** (2026-06-27 PR-1), **Spacing scale** (4-based, 2026-06-27 PR-1 — 替换了旧 "no scale" 措辞), **Radius ladder** (formalized 2026-06-27 PR-1), border (default 1px + 3px critical exception), **Motion Vocabulary** (durations 80/100/150/240/1800/1000ms + easings 2026-06-27 PR-1), **Shadow Scale** (4 elevation + focus ring, 2026-06-27 PR-1), **Reduced Motion 兜底** (2026-06-27 PR-1), **Modal Tokens** (added 2026-06-13 PR3 of A2 + B7), 禁止硬编码 hex, **icon `:size` 必须偶数 px** (防亚像素抖动, 2026-06-26) | Filled (2026-06-09, UI polish; 2026-06-26 icon sizing 规则; 2026-06-27 PR-1 token 体系补全 + PR-2 节奏字体 token 替换 + PR-3d 动效统一) |
| [Chat Components](./chat.md) | 主 chat panel + subagent drawer 组件规范; **SubagentDrawer** (重构 PR1-6) 5 段分组布局 + accumulator `liveSections` 数据流 + 视觉原语复用边界(不 wrap ToolCallCard) + `pairSections` snake→camel + 3 边界态(R25 error 4级fallback / R23 cancelled 降级 wall-clock / R24 permission_ask 降级 historical) + common mistakes | Filled (2026-06-21, subagent-drawer 重构 PR1-6) |

> ℹ️ 8-PR4 cleanup (2026-06-10): 移除 5 个空骨架文件 (`component-guidelines.md` / `directory-structure.md` / `quality-guidelines.md` / `hook-guidelines.md` / `type-safety.md`) — 项目无对应填充需求,直接删除更清晰。

---

## How to Fill These Guidelines

For each guideline file:

1. Document your project's **actual conventions** (not ideals)
2. Include **code examples** from your codebase
3. List **forbidden patterns** and why
4. Add **common mistakes** your team has made

The goal is to help AI assistants and new team members understand how YOUR project works.

---

**Language**: All documentation should be written in **English**.
