# Frontend Development Guidelines

> Best practices for frontend development in this project.

---

## Overview

This directory contains guidelines for frontend development. Fill in each file with your project's specific conventions.

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [Directory Structure](./directory-structure.md) | Module organization and file layout | To fill |
| [Component Guidelines](./component-guidelines.md) | Component patterns, props, composition | To fill |
| [Hook Guidelines](./hook-guidelines.md) | Custom hooks, data fetching patterns | To fill |
| [State Management](./state-management.md) | Local state, global state, server state | To fill |
| [Quality Guidelines](./quality-guidelines.md) | Code standards, forbidden patterns | To fill |
| [Type Safety](./type-safety.md) | Type patterns, validation | To fill |
| [CJK Font Bundling](./cjk-fonts.md) | 跨平台中文字体打包规范 (HarmonyOS Sans SC 子集, 工具链, license 合规) | Filled (2026-06) |
| [Popover Pattern](./popover-pattern.md) | Hand-rolled popover pattern (onDocumentClick + Esc close), position direction rule (top/bottom of viewport), worktree + ModelSelect references, anti reka-ui DropdownMenu rationale, animation (modal scale / popover slide 150ms) | Filled (2026-06-09, PR5 follow-up + UI polish) |
| [Reka-UI Usage](./reka-ui-usage.md) | reka-ui 2.9.9 version pin, primitives used in project, `TextFieldRoot` not-in-2.9.9 gotcha + native `<input>` substitute, **`<style scoped>` + portal = `:deep()` gotcha** (SelectContent / DialogContent / PopoverContent et al. portal to body and escape scoped selectors — wrap rules in `:deep()` to match), **`--reka-select-trigger-width` tip** (reka-ui 2.9.9 exposes this CSS var for sizing SelectContent to its trigger; prefix is `--reka-` not `--radix-`), Label wrapper convention, data-state theming, anti reka-ui Popover rationale | Filled (2026-06-09, UI polish + SettingsModal Select fix) |
| [Design Tokens](./design-tokens.md) | Color tokens (bg/text/accent/tool), `--color-text-muted` 加亮决策 (R5), font/sans/mono, spacing + radius ladder, 禁止硬编码 hex | Filled (2026-06-09, UI polish) |

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
