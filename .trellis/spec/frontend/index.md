# Frontend Development Guidelines

> Best practices for frontend development in this project.

---

## Overview

This directory contains guidelines for frontend development. Fill in each file with your project's specific conventions.

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [Transport Modes & PWA Navigation](./transport-and-pwa-modes.md) | How the SPA distinguishes daemon/remote/tauri contexts; token vs health-probe signals; 401 handling; wire casing | ✅ Filled (S4) |
| [Responsive & Mobile](./responsive-mobile.md) | Breakpoints (native @media, desktop-first overlay), drawer nav, iOS keyboard (visualViewport), safe-area, Dialog full-screen, touch targets | ✅ Filled (S5) |
| [Design Tokens](./design-tokens.md) | CSS variable system (color/spacing/radius/motion tokens); never hardcode hex/px | ✅ Filled |
| [Reka-UI Usage](./reka-ui-usage.md) | reka-ui 2.9.9 primitives, wrapper classes, version-pinned constraints | ✅ Filled |
| [Popover Pattern](./popover-pattern.md) | Hand-rolled onDocumentClick + Esc close dropdowns/popovers | ✅ Filled |
| [Chat Components](./chat.md) | Chat panel, message rendering, tool cards | ✅ Filled |
| [Memory UI](./memory-ui.md) | Memory modal / preview components | ✅ Filled |
| [Directory Structure](./directory-structure.md) | Module organization and file layout | To fill |
| [Component Guidelines](./component-guidelines.md) | Component patterns, props, composition | To fill |
| [Hook Guidelines](./hook-guidelines.md) | Custom hooks, data fetching patterns | To fill |
| [State Management](./state-management.md) | Local state, global state, server state | To fill |
| [Quality Guidelines](./quality-guidelines.md) | Code standards, forbidden patterns | To fill |
| [Type Safety](./type-safety.md) | Type patterns, validation | To fill |

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
