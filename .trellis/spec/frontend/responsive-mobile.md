# Responsive & Mobile Adaptation

> How this project adapts the desktop-first SPA for mobile (PWA) viewports.
> Captures the patterns introduced by S5 (`08-11-mobile-adaptation`), the
> project's first responsive work.

---

## Status

Filled (2026-08-12, S5 `08-11-mobile-adaptation`). Applies to the PWA
remote-access flow; desktop/Tauri layout is unchanged (regression-free
by structure — see §1.2).

---

## Overview

The app is **desktop-first** — every component is styled for ≥768px and
uses `<style scoped>` + CSS variables (`var(--name)`), with **zero
Tailwind utility classes** in templates. Mobile adaptation is therefore
done as **desktop-first overlay**: all mobile rules live inside
`@media (max-width: 767px)` and the desktop style blocks are never
edited. This makes "desktop zero-regression" a structural guarantee,
not something verified by eyeballing.

Single breakpoint: **768px**. No sm/md/lg ladder (that's a V2 concern).

---

## 1. Breakpoint strategy

### 1.1 Native `@media`, not Tailwind utilities

```css
/* ✅ correct — matches project convention */
@media (max-width: 767px) {
  .my-component { /* mobile override */ }
}

/* ❌ forbidden — project doesn't use utility classes in templates */
<div class="md:hidden" />
```

**Why**: 40+ existing components use semantic class names + scoped CSS.
Introducing `md:`/`lg:` utilities in one task creates two coexisting
style paradigms. Media queries fit the "overlay on desktop baseline"
model; utilities fit "build layout from scratch". Desktop-first overlay
wins here.

### 1.2 Desktop-first overlay (not mobile-first)

Desktop styles are the baseline (already shipped, must not change).
Mobile is the override. So `@media (max-width: 767px)` (desktop-first),
**not** `@media (min-width: 768px)` (mobile-first). This violates pure
mobile-first dogma but satisfies the "desktop zero-regression" invariant
without touching desktop blocks.

**Cost**: override specificity. When a mobile rule must beat a desktop
rule of equal specificity, prefer escalating selector specificity over
`!important`. `!important` is acceptable only when overriding
third-party (reka-ui) inline styles, and should be centralized (see §5).

---

## 2. Drawer navigation pattern (Sidebar)

Desktop: `Sidebar` is a 260px left column, always visible. Mobile: it
becomes a full-screen overlay drawer, toggled by a hamburger in
`AppHeader`.

### 2.1 State: module-level singleton composable

```ts
// composables/useMobileNav.ts — mirrors useToast / useErrorBus pattern
const mobileNavOpen = ref(false);  // module-level = shared singleton

export function useMobileNav() {
  return {
    mobileNavOpen: mobileNavOpen as Readonly<Ref<boolean>>,
    open, close, toggle,
  };
}
```

`AppHeader` (hamburger toggle), `Sidebar` (read open + auto-close on
session switch), and `AppShell` (overlay backdrop) all call
`useMobileNav()` and share the same ref. SPA-only (no SSR) so a
module-level singleton is safe.

**Why not provide/inject**: the project's other global singletons
(useToast, useErrorBus, useKeyboard) all use module-level refs —
`useMobileNav` follows the same shape.

### 2.2 CSS: fixed overlay + slide

```css
@media (max-width: 767px) {
  .sidebar {
    position: fixed;
    inset: 0;
    z-index: 110;               /* > overlay(105) > TracePanel(100) */
    width: 100vw;
    padding-top: var(--safe-area-top);
    transform: translateX(-100%);
    transition: transform var(--duration-base) var(--ease-out);
  }
  .sidebar--open { transform: none; }
}
```

Desktop `.sidebar` (width:260px, flex-shrink:0) is untouched. The
mobile `position: fixed` takes the sidebar out of flex flow; `main`
naturally fills the body.

### 2.3 ProjectTabs double-mount (no JS `isMobile`)

`ProjectTabs` appears in two places: `AppHeader` (desktop top bar) and
`Sidebar` drawer top (mobile). Both render always; CSS controls
visibility:

```css
.app-header__project-tabs { display: flex; }      /* desktop default */
@media (max-width: 767px) {
  .app-header__project-tabs { display: none; }    /* hide from header */
  .sidebar__project-tabs { display: flex; }       /* show in drawer */
}
.sidebar__project-tabs { display: none; }          /* desktop default */
```

**Why not `v-if="isMobile"`**: JS viewport detection is unreliable
(resize race, SSR). Double-mount + CSS is robust. Verified safe:
`ProjectTabs` has no `useId`/teleport/global side effects — both
instances share the same Pinia store, no conflict.

### 2.4 Auto-close on session switch

```ts
// Sidebar.vue
watch(() => chat.currentSessionId, () => closeMobileNav());
```

Tapping a session in the drawer closes it and the user lands on the
chat (main is always visible underneath). Desktop: `close()` is a
no-op (state is ignored by CSS).

### 2.5 z-index ladder (mobile)

| Layer | z-index | Notes |
|---|---|---|
| TracePanel | 100 | pre-existing |
| sidebar overlay backdrop | 105 | covers TracePanel |
| Sidebar drawer | 110 | covers backdrop |
| toast | 9999 | always on top |

Avoid `z-index: 100` for new mobile layers — it collides with TracePanel.

---

## 3. iOS software keyboard (Visual Viewport API)

**The trap**: iOS Safari's software keyboard is an **overlay** — it
does NOT resize the layout viewport. So `100vh`/`100dvh` are unaffected
by the keyboard (dvh only tracks the URL bar). An input pinned to the
bottom gets hidden behind the keyboard.

Android Chrome **does** resize the layout viewport, so the same code is
a no-op there (harmless).

### 3.1 Mechanism: composable + CSS variable

```ts
// composables/useMobileKeyboard.ts
function updateViewportHeight() {
  document.documentElement.style.setProperty(
    "--visual-viewport-height",
    `${window.visualViewport!.height}px`,
  );
}
export function useMobileKeyboard() {
  onMounted(() => {
    window.visualViewport?.addEventListener("resize", updateViewportHeight);
    updateViewportHeight();
  });
  onUnmounted(() => /* remove listeners */);
}
```

```css
/* AppShell.vue — mobile height consumes the variable */
@media (max-width: 767px) {
  .app-shell {
    height: var(--visual-viewport-height, var(--app-height));
  }
}
```

Keyboard opens → `visualViewport.height` shrinks → `--visual-viewport-
height` updates → AppShell shrinks to the area above the keyboard →
ChatInput (bottom of flex column) stays visible.

### 3.2 When to use

Mount `useMobileKeyboard()` in the component that owns the input
(`ChatInput`). It's a no-op on desktop (visualViewport exists but
height is stable; AppShell desktop uses `100vh`, not the variable).

---

## 4. Safe-area insets (notch / home indicator)

### 4.1 Prerequisite: `viewport-fit=cover`

```html
<!-- index.html — without viewport-fit=cover, env() is always 0 -->
<meta name="viewport" content="width=device-width, initial-scale=1.0, viewport-fit=cover" />
```

### 4.2 Tokens (global, harmless on desktop)

```css
/* style.css — env() is 0 on desktop / non-notch devices */
:root {
  --safe-area-top: env(safe-area-inset-top);
  --safe-area-bottom: env(safe-area-inset-bottom);
}
```

Consume in mobile blocks:
```css
@media (max-width: 767px) {
  .sidebar { padding-top: var(--safe-area-top); }      /* avoid notch */
  .chat-input { padding-bottom: calc(16px + var(--safe-area-bottom)); } /* home indicator */
}
```

---

## 5. Dialog full-screen on mobile (reka-ui)

reka-ui `DialogPortal` teleports content to `<body>`, so **scoped
styles can't target it** — full-screen rules must live in global
`style.css`, not the component's `<style scoped>`.

### 5.1 The min-width overflow bug pattern

Several modals set `min-width: 560-640px` for desktop readability. On a
375px viewport these overflow horizontally (scrollbar / clipped
content). Mobile override must reset `min-width: 0`.

### 5.2 Override block (global style.css)

```css
@media (max-width: 767px) {
  /* real DialogContent root classes — NOT invented names */
  .settings-modal,
  .grant-modal,
  .markdown-detail-modal,
  .runtime-memory-modal,
  .memory-modal,
  .audit-modal,
  .gcfg-content {
    width: 100vw !important;
    min-width: 0 !important;
    max-width: 100vw !important;
    height: var(--app-height);
    max-height: var(--app-height);
    inset: 0 !important;              /* override top:50%;left:50% */
    transform: none !important;       /* override translate(-50%,-50%) */
    border-radius: 0 !important;
    margin: 0 !important;
  }
}
```

**Critical**: must override both `inset: 0` AND `transform: none`.
Desktop centers via `top:50%; left:50%; transform:translate(-50%,-50%)`.
If you only set `width:100vw` without `transform:none`, the 100vw
element gets pushed off-screen left by the translate.

**Selector names must match `DialogContent`'s `class` attribute exactly**
(e.g. `.grant-modal`, not `.grant-modal__content`). Verify by reading
the component's `<DialogContent class="...">` line before writing the
selector — a wrong name silently fails (no error, no effect).

### 5.3 Hand-rolled modals (non-reka)

`YoloConfirmModal` / `ConfirmDialog` are hand-rolled (`<div
class="...-backdrop"><div class="...-modal">`), not reka-ui, and don't
teleport. They don't enter the global block; they already use
`width:100%` and shrink fine. Only their buttons need touch-target
uplift.

---

## 6. Touch targets (Apple HIG 44px)

```css
@media (max-width: 767px) {
  /* scope to modal interiors to avoid hitting unrelated buttons */
  .settings-modal button, /* ...other modal roots... */ .confirm-modal button {
    min-height: 44px;
    min-width: 44px;
  }
}
```

ChatInput's send/stop buttons go from 32×32 (desktop) to 44×44 (mobile)
in the component's own scoped mobile block.

---

## 7. Viewport height tokens (recap)

| Token | Value | Use |
|---|---|---|
| `--app-height` | `100dvh` (fallback `100vh`) | URL bar dynamic height |
| `--visual-viewport-height` | set by `useMobileKeyboard` | software keyboard (iOS) |
| `--safe-area-top/bottom` | `env(safe-area-inset-*)` | notch / home indicator |

Desktop AppShell uses `100vh` and consumes none of these — they're
mobile-only in practice.

---

## Forbidden patterns

- ❌ Tailwind utility classes (`md:`, `lg:`) in templates — project is
  scoped-CSS + variables. Mobile rules go in `@media` blocks.
- ❌ `100vh` as the mobile viewport height — use `--app-height` (dvh);
  and `100vh`/`dvh` do NOT handle the iOS keyboard, use
  `--visual-viewport-height`.
- ❌ `position: sticky; bottom: 0` to pin an input that's already a
  flex-column bottom member — it's a no-op (sticky needs a scroll
  ancestor; the input isn't in one). Use the flex layout + visual
  viewport variable.
- ❌ `-webkit-overflow-scrolling: touch` — deprecated since iOS 13
  (momentum scrolling is the default). Remove on sight.
- ❌ Inventing CSS token names — verify against `style.css` `@theme`
  before use (durations are `--duration-{instant,fast,base,slow,...}`,
  not `--duration-normal`).
- ❌ Guessing reka-ui DialogContent class names — read the component's
  `<DialogContent class="...">` line. A wrong selector silently fails.
- ❌ JS `isMobile` detection for conditional render — use double-mount
  + CSS `display` control (robust across resize/SSR).
