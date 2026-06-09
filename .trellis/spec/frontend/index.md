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
| [Popover Pattern](./popover-pattern.md) | Hand-rolled popover pattern (onDocumentClick + Esc close), position direction rule (top/bottom of viewport), worktree + ModelSelect references, anti reka-ui DropdownMenu rationale, animation (modal scale / popover slide 150ms) | Filled (2026-06-09, PR5 follow-up + UI polish) |
| [Reka-UI Usage](./reka-ui-usage.md) | reka-ui 2.9.9 version pin, primitives used in project, `TextFieldRoot` not-in-2.9.9 gotcha + native `<input>` substitute, **`<style scoped>` + portal = `:deep()` gotcha** (SelectContent / DialogContent / PopoverContent et al. portal to body and escape scoped selectors — wrap rules in `:deep()` to match), **`--reka-select-trigger-width` tip** (reka-ui 2.9.9 exposes this CSS var for sizing SelectContent to its trigger; prefix is `--reka-` not `--radix-`), Label wrapper convention, data-state theming, anti reka-ui Popover rationale | Filled (2026-06-09, UI polish + SettingsModal Select fix) |
| [Design Tokens](./design-tokens.md) | Color tokens (bg/text/accent/tool), `--color-text-muted` 加亮决策 (R5), font/sans/mono, spacing + radius ladder, 禁止硬编码 hex | Filled (2026-06-09, UI polish) |

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
