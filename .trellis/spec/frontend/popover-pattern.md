# Popover Pattern — Hand-Rolled `onDocumentClick` + `Esc` Close

> Reusable pattern for click-triggered dropdowns / popovers in
> this project. Two production instances today: worktree dropdown
> in `app/src/components/chat/ChatPanel.vue` and model dropdown in
> `app/src/components/chat/ModelSelect.vue`. PR5 follow-up
> (2026-06-09, task
> `06-09-06-09-06-08-multi-model-pr5-ux-followup-settingsbar-chatinput-model-popover`)
> adopted this pattern for the new `ModelSelect` to keep
> behaviour and visual style consistent.

---

## Status

Filled (2026-06-09). Future dropdowns / popovers in this project
SHOULD follow this pattern unless the use case has a reason to
deviate (e.g. accessibility requirements that demand
`reka-ui`'s built-in `aria-haspopup` / `aria-controls`).

---

## Overview

The project uses a hand-rolled popover pattern instead of
`reka-ui`'s `DropdownMenu` / `Popover` / `Menu` primitives for
two existing dropdowns. The reasons (PR5 D3 decision):

- **Visual consistency** — the existing worktree dropdown is the
  visual reference for any "small menu attached to a chip in a
  dense bar" UI. Matching its look + behaviour across the app
  is preferred over re-using the design system primitive that
  may style menus differently.
- **No new dependency path** — `reka-ui`'s `DropdownMenu` works,
  but its API surface and a11y conventions are different from
  what the worktree dropdown exposes. Introducing it for one
  component would fork the codebase's popover implementations.
- **Simplicity** — the hand-rolled pattern is ~20 lines of TS +
  ~20 lines of CSS. Reka-ui's `DropdownMenu` would be ~50
  lines of TSX-style markup + provider wrappers.

The trade-off (acknowledged in PR5 D3) is that we maintain
two near-identical popover implementations and the
`usePopover` composable extraction is left as future work
(OOS).

---

## Pattern

The hand-rolled popover is a Vue 3 `<script setup>` component
with three pieces:

1. **Trigger button** — the chip / icon that toggles the menu.
2. **Popover container** — a sibling `div` of the trigger,
   absolutely positioned, `v-if="open"`-gated.
3. **Outside-click + Esc close handler** — document-level event
   listeners mounted on setup, torn down on unmount.

### 1. State

```ts
const open = ref(false);
const root = ref<HTMLElement | null>(null);
```

The `root` ref wraps both the trigger and the popover. The
outside-click handler closes when the click target is **not**
inside `root`.

### 2. Toggle

```ts
function toggle() { open.value = !open.value; }
function close()  { open.value = false; }
```

### 3. Outside-click close

```ts
function onDocumentClick(e: MouseEvent) {
  if (!open.value) return; // no-op when closed (perf)
  const target = e.target as Node | null;
  if (root.value && target && !root.value.contains(target)) {
    open.value = false;
  }
}

if (typeof document !== "undefined") {
  document.addEventListener("click", onDocumentClick);
  onUnmounted(() => document.removeEventListener("click", onDocumentClick));
}
```

The `if (typeof document !== "undefined")` guard is important
for SSR-safety, even though this project is Tauri-only. It
keeps the pattern portable if a future web variant is added.

### 4. Esc close

```ts
function onKeydown(e: KeyboardEvent) {
  if (open.value && e.key === "Escape") {
    open.value = false;
  }
}

onMounted(() => document.addEventListener("keydown", onKeydown));
onUnmounted(() => document.removeEventListener("keydown", onKeydown));
```

### 5. Template

```vue
<div ref="root" class="my-popover-root">
  <button
    type="button"
    class="my-popover-trigger"
    :aria-haspopup="'menu'"
    :aria-expanded="open"
    @click="toggle"
  >
    Trigger label
  </button>

  <div
    v-if="open"
    class="my-popover-menu"
    role="menu"
  >
    <button
      v-for="item in items"
      :key="item.id"
      type="button"
      class="my-popover-menu-item"
      role="menuitem"
      @click="onPick(item)"
    >
      {{ item.label }}
    </button>
  </div>
</div>
```

The `role="menu"` + `role="menuitem"` is the minimum a11y
hint. Full keyboard nav (↑ / ↓ / Enter) is **not** implemented
in either production instance; if a future dropdown needs
keyboard-first navigation, switch to `reka-ui` `DropdownMenu`.

---

## Position Direction Rule

> **Trigger at the top of the viewport → popover opens downward.
> Trigger at the bottom of the viewport → popover opens upward.**

This is the single most-forgotten part of the pattern. A
downward-opening popover attached to a bottom-of-viewport
trigger (e.g. the chat input bar) would be clipped by the
viewport edge. Always check the trigger's vertical position
relative to the parent scroll container and pick the
direction.

| Trigger location | Popover CSS | Why |
|---|---|---|
| Top of viewport (e.g. `AppHeader`, `ChatPanel` worktree chip) | `top: calc(100% + 4px);` | Popover hangs below the trigger |
| Bottom of viewport (e.g. `ChatInput` model button) | `bottom: calc(100% + 4px); top: auto;` | Popover floats above the trigger |

The `4px` gap is a project-wide convention (matches the
worktree dropdown's spacing). Other values may be used per
context but should be consistent within a single trigger.

### Reference: worktree dropdown (downward)

```css
.chat-panel__menu {
  position: absolute;
  top: calc(100% + 4px);
  right: 0;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  min-width: 200px;
  z-index: 100;
  padding: 4px;
  display: flex;
  flex-direction: column;
}
```

### Reference: model dropdown (upward)

```css
.model-select__menu {
  position: absolute;
  bottom: calc(100% + 4px);
  top: auto;
  right: 0;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  min-width: 220px;
  z-index: 100;
  padding: 4px;
  display: flex;
  flex-direction: column;
}
```

The differences are exactly two: `top: calc(100% + 4px)` →
`bottom: calc(100% + 4px); top: auto;`, and a slight
`min-width` adjustment for the model list (220px vs 200px
because model names are typically longer than worktree
operations).

---

## Code Skeleton (Copy-paste Starting Point)

```vue
<script setup lang="ts">
// MyNewPopover — hand-rolled popover following the project pattern.
// Trigger at the top of its container → opens downward. To open
// upward (bottom-of-container trigger), swap the CSS in <style>.

import { ref, onMounted, onUnmounted } from "vue";

const open = ref(false);
const root = ref<HTMLElement | null>(null);

function toggle() { open.value = !open.value; }
function close()  { open.value = false; }

function onDocumentClick(e: MouseEvent) {
  if (!open.value) return;
  const target = e.target as Node | null;
  if (root.value && target && !root.value.contains(target)) {
    open.value = false;
  }
}

function onKeydown(e: KeyboardEvent) {
  if (open.value && e.key === "Escape") {
    open.value = false;
  }
}

onMounted(() => {
  document.addEventListener("click", onDocumentClick);
  document.addEventListener("keydown", onKeydown);
});
onUnmounted(() => {
  document.removeEventListener("click", onDocumentClick);
  document.removeEventListener("keydown", onKeydown);
});
</script>

<template>
  <div ref="root" class="mnp">
    <button
      type="button"
      class="mnp__trigger"
      :aria-haspopup="'menu'"
      :aria-expanded="open"
      @click="toggle"
    >
      <slot name="trigger" />
    </button>

    <div
      v-if="open"
      class="mnp__menu"
      role="menu"
    >
      <slot />
    </div>
  </div>
</template>

<style scoped>
.mnp {
  position: relative;
  display: inline-block;
}

.mnp__trigger {
  background: transparent;
  border: 0;
  cursor: pointer;
  font: inherit;
  color: inherit;
  padding: 0;
}

.mnp__menu {
  position: absolute;
  top: calc(100% + 4px);
  right: 0;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  min-width: 200px;
  z-index: 100;
  padding: 4px;
  display: flex;
  flex-direction: column;
}
</style>
```

Use `slot name="trigger"` for the trigger content and the
default slot for the menu body. This lets each instance
customize both without forking the popover component.

---

## Don't: Use `reka-ui` `DropdownMenu` for New Dropdowns in This Project

**Why**:
- The worktree dropdown and `ModelSelect` are the existing
  visual + behavioural reference. A `reka-ui` `DropdownMenu`
  would render with a different default chrome (different
  border, padding, focus ring), creating visual drift.
- The hand-rolled pattern already covers the project's
  a11y minimum (`aria-haspopup`, `aria-expanded`,
  `role="menu"`, `role="menuitem"`). Reka-ui's additional
  `aria-controls` is nice-to-have, not a blocker.
- Reka-ui `DropdownMenu` v2.x uses `as Child` / `as`
  polymorphism that has shifted API between alpha and stable;
  adding it for one component pins the project to a
  specific reka-ui minor.

**Exception**: switch to reka-ui if the dropdown needs
keyboard-first navigation (↑/↓/Home/End/Enter), virtual
scrolling for >100 items, or `aria-controls` referencing
an out-of-tree element. None of the existing dropdowns need
this.

**If a future dropdown genuinely needs reka-ui**, document
the deviation in a new section of this file. Don't silently
mix the two patterns.

---

## Don't: Re-Implement Close Logic Per-Component

**Why**: it's exactly the same 20 lines in every dropdown.
Future work (OOS) is to extract a `usePopover` composable:

```ts
// Sketch (NOT YET IMPLEMENTED)
function usePopover() {
  const open = ref(false);
  const root = ref<HTMLElement | null>(null);
  // ... onDocumentClick, onKeydown handlers ...
  return { open, root, toggle, close };
}
```

Once a third dropdown is added, this extraction becomes
worth doing. Until then, the duplication is acceptable.

---

## Don't: Forget the `v-if` Gate on the Popover Element

The popover container must be `v-if="open"`, not
`v-show="open"`. With `v-show`, the element stays in the DOM
and the `root.contains(target)` check would still work, but
focus stays trapped in a hidden element (tab order breaks
on Chrome). The `v-if` removes the element entirely so the
focus order is correct when the popover is closed.

---

## Common Mistake: Popover Clipped by Parent `overflow: hidden`

**Symptom**: popover appears in the wrong place or is
invisible when the trigger is near the edge of a
container with `overflow: hidden` (e.g. the sidebar's
session list, the chat panel's input row).

**Cause**: the popover uses `position: absolute` relative
to its `root` container; if the root or any ancestor has
`overflow: hidden` (or `auto` / `scroll` with a fixed
height), the popover gets clipped at the overflow boundary.

**Fix options**:
1. Move the popover out of the clipping container
   (last-resort; loses the "anchored to the trigger" UX).
2. Use Vue `<Teleport to="body">` to render the popover
   at the document root, with `position: fixed` and
   computed coordinates. This is the most-robust answer
   for popovers near viewport edges.
3. Adjust the parent container's overflow (often
   undesirable — the overflow is there for a reason).

**Status**: the worktree dropdown and `ModelSelect` are
NOT yet near the clipping boundary today. If a future
dropdown runs into this, the fix is `<Teleport>` + fixed
position. Don't try to "fix" the existing dropdowns'
positioning until a real clipping bug is reported.

---

## Related

- `app/src/components/chat/ChatPanel.vue:127-149, 401-471` —
  worktree dropdown (downward) + the original
  `worktreeMenuOpen` / `worktreeMenuRoot` /
  `onDocumentClick` pattern reference implementation.
- `app/src/components/chat/ModelSelect.vue` — model
  dropdown (upward) added 2026-06-09, copies the worktree
  pattern verbatim with two CSS changes (position +
  min-width).
- `.trellis/spec/frontend/component-guidelines.md` —
  general Vue 3 component conventions (this file is
  popover-specific and should be referenced from there
  if a "Popovers" section is added in the future).
- PR5 follow-up task
  `.trellis/tasks/06-09-06-09-06-08-multi-model-pr5-ux-followup-settingsbar-chatinput-model-popover/prd.md`
  — D3 (抄 worktree popover) is the decision that
  established this pattern as the project convention.
