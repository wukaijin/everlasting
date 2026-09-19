# Reka-UI Usage Notes

> Project-specific conventions and gotchas for using
> [reka-ui](https://reka-ui.com) (the Vue port of Radix UI)
> in this codebase. Captures the version-pinned primitives
> we use, the wrapper classes we apply, and the known
> constraints of the pinned version.

---

## Status

Filled (2026-06-09). Pinned reka-ui version: **2.9.9** (per
`app/package.json`). New code MUST use reka-ui primitives
from this version; do not assume a primitive exists without
checking.

---

## Overview

Reka-ui is the design-system primitive layer for all
modal / popover / form-control UI in this project. It provides
unstyled, accessible, headless components — we supply all
visual styling via project CSS classes and CSS variables.

The project uses reka-ui for:

- `DialogRoot` / `DialogContent` / `DialogOverlay` /
  `DialogTitle` / `DialogClose` (SettingsModal overlay)
- `SelectRoot` / `SelectTrigger` / `SelectContent` /
  `SelectItem` / `SelectValue` (Settings forms — protocol,
  provider, thinking effort)
- `CheckboxRoot` / `CheckboxIndicator` (Settings forms —
  supportsThinking)
- `RadioGroupRoot` / `RadioGroupItem` / `RadioGroupIndicator`
  (DefaultTab — default model)
- `Label` (wrapping form fields for accessibility)
- `ToastRoot` / `ToastProvider` / `ToastViewport` /
  `ToastPortal` / `ToastTitle` / `ToastDescription` /
  `ToastClose` (全局错误 toast 路由 — AppShell 挂载
  `ToastProvider`,`useToast` composable 驱动;2026-07-17
  A5 scope B 落地,见 `state-management.md` §useToast composable
  + `backend/error-handling.md` RULE-A-018)

Reka-ui is **not** used for the project popovers (ModelSelect,
worktree dropdown). Those are hand-rolled per
`.trellis/spec/frontend/popover-pattern.md` (PR5 decision).

---

## Version Pin: 2.9.9

The project is pinned to `reka-ui@2.9.9`. This matters because:

- reka-ui **3.x** introduced new primitives (e.g. `TextFieldRoot`)
  and renamed some APIs.
- reka-ui **2.9.x** ships a smaller primitive set; some things
  the docs show as "the modern way" don't exist here yet.

When using a reka-ui primitive, **verify it exists in 2.9.9**
before writing code. The two failure modes are:

1. Importing a non-existent primitive → build / type error
2. Importing a primitive that exists in 3.x but not 2.9.x
   (e.g. `TextFieldRoot`) → silent runtime error or empty render

---
> **分篇**(2026-09-19):本文保留 Status / Overview / Version Pin 与 Related;Gotcha/Tip、Convention/Don't 与组件专题(Tooltip / DropdownMenu / Date/Time / roving tabindex)已按 tool-contract 模式拆至 `reka-ui-usage/` 子目录(一主题一文件,原锚点以 stub 保留)。

## Gotcha: `TextFieldRoot` does NOT exist in 2.9.9

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/gotchas.md`](./reka-ui-usage/gotchas.md) — 同文件还有 `Sheet` 不存在、`<style scoped>` 不穿透 portal、`--reka-select-trigger-width` Tip

## Gotcha: `Sheet` does NOT exist in 2.9.9

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/gotchas.md`](./reka-ui-usage/gotchas.md)

## Gotcha: `<style scoped>` does NOT apply to portal children

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/gotchas.md`](./reka-ui-usage/gotchas.md)

## Tip: Use `--reka-select-trigger-width` to size SelectContent to its trigger

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/gotchas.md`](./reka-ui-usage/gotchas.md)

## Pattern: Tooltip for hover affordances (added 2026-06-10, A4 token-usage)

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/component-patterns.md`](./reka-ui-usage/component-patterns.md)

## Convention: Wrap reka-ui primitives in project-scoped CSS classes

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/conventions-and-donts.md`](./reka-ui-usage/conventions-and-donts.md) — 同文件还有 Theming via `data-state` / `Label` wrapper / Don't×2(Popover primitive、inline style)/ forward `data-*` Common Mistake

## Convention: Theming via `data-state` and `data-highlighted` attributes

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/conventions-and-donts.md`](./reka-ui-usage/conventions-and-donts.md)

## Convention: Form fields use `Label` wrapper for accessibility

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/conventions-and-donts.md`](./reka-ui-usage/conventions-and-donts.md)

## Don't: Use reka-ui's `Popover` primitive for project popovers

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/conventions-and-donts.md`](./reka-ui-usage/conventions-and-donts.md)

## Don't: Re-style reka-ui primitives with inline `style=""`

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/conventions-and-donts.md`](./reka-ui-usage/conventions-and-donts.md)

## Common Mistake: Forgetting to forward `data-*` attributes

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/conventions-and-donts.md`](./reka-ui-usage/conventions-and-donts.md)

## D3 PR2 (2026-06-17): `DropdownMenu` for per-message actions

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/component-patterns.md`](./reka-ui-usage/component-patterns.md)

## Date/Time primitives (2026-08-29, task `08-29-sched-datetime-pickers`)

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/component-patterns.md`](./reka-ui-usage/component-patterns.md)

## Pattern: roving tabindex keyboard nav inside a Dialog(2026-09-03,task `09-03-dirbrowser-desktop-unify`)

> **已拆出**(2026-09-19 doc-split):完整内容见 [`reka-ui-usage/component-patterns.md`](./reka-ui-usage/component-patterns.md)

---

## Related

- `.trellis/spec/frontend/popover-pattern.md` — the hand-rolled
  popover pattern for `ModelSelect` / worktree dropdown (the
  reason reka-ui `Popover` is not used in this project).
- `.trellis/spec/frontend/design-tokens.md` — the CSS variable
  system (`--color-bg-elevated`, `--color-accent`, etc.) that
  reka-ui primitives are themed against.
- `app/package.json` — reka-ui version pin.
- PR5 follow-up PR (`b919d9e`) — established the
  hand-rolled popover pattern; UI polish PR (this one)
  established the reka-ui form-control pattern.

---

