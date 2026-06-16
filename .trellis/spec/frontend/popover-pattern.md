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

Filled (2026-06-09). PR2 follow-up (2026-06-13, A2 + B7 task
`06-12-a2-b7-permission-and-mode`) added a third production
instance: `ModeSelect.vue` in the ChatInput hint row, with the
same upward-opening popover geometry as `ModelSelect`. PR3 of
the same task added `PermissionModal.vue` (a CENTER modal,
not a popover — see "Modal vs Popover" callout below) but the
popover pattern itself was unchanged.

B3 /command (2026-06-16, task `06-16-b3-command-palette`) added
`<TriggerMenu>` — the fourth production instance and the first
whose trigger element is **external** to the popover's `root` (a
sibling `<textarea>`, not a child of `root`). See "Variation:
External Trigger Element" below.

Future dropdowns / popovers in this project SHOULD follow this
pattern unless the use case has a reason to deviate (e.g.
accessibility requirements that demand
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

## Variation: External Trigger Element (`triggerEl` prop)

> Added 2026-06-16 (B3 /command, task `06-16-b3-command-palette`).
> `<TriggerMenu>` is the first popover whose **trigger element lives
> outside the popover's `root`** — the trigger is `ChatInput`'s
> `<textarea>`, and the menu is a sibling panel anchored above it.
> B2 (@文件) and B4 (skill) will hit the same shape when they reuse
> `<TriggerMenu>`.

**Problem**: the standard `onDocumentClick` (§Pattern.3) only checks
`root.contains(target)`. When the trigger element is **outside**
`root` (a sibling, not a child), clicking the trigger to reposition
the caret mid-type registers as an "outside click" and **closes the
panel** — the user is typing `/he`, clicks to fix a typo, and the
autocomplete vanishes.

**Why ModeSelect / ModelSelect don't hit this**: their trigger button
sits **inside** their own `root` wrapper
(`<div ref="root">…<button/><menu/></div>`), so `root.contains(trigger)`
is always true. `<TriggerMenu>` can't wrap the textarea (the textarea
owns its own layout / v-model / autosize), so the menu mounts as a
sibling and the textarea is external.

**Solution**: add an optional `triggerEl` prop
(`HTMLElement | null`) and have `onDocumentClick` treat it as "inside":

```ts
// TriggerMenu.vue
const props = withDefaults(defineProps<{
  triggerEl?: HTMLElement | null;
}>(), { triggerEl: null });

function onDocumentClick(e: MouseEvent) {
  if (!open.value) return;
  const target = e.target as Node | null;
  if (!target) return;
  const insideRoot = root.value?.contains(target) ?? false;
  const insideTrigger = props.triggerEl?.contains(target) ?? false;
  if (!insideRoot && !insideTrigger) {
    open.value = false;
  }
}
```

The parent passes its textarea ref via template binding (Vue
auto-unwraps the parent's template ref):

```vue
<TriggerMenu :trigger-el="textareaEl" ... />
```

**Don't** pass a `{ readonly value: HTMLElement | null }` ref-like
object — `vue-tsc` rejects it and Vue's template binding already
unwraps refs. A plain `HTMLElement | null` is the reactive-enough
shape (the parent re-binds on every render).

**When to use**: any popover whose trigger is a sibling / external
element (can't be wrapped in the popover's `root`). For popovers
whose trigger is inside `root` (ModeSelect / ModelSelect / worktree
dropdown), the standard pattern suffices — no `triggerEl` needed.

**Reference**: `app/src/components/chat/TriggerMenu.vue` (B3 PR2,
commit `d57788a`). The `triggerEl` extension was caught + fixed by
the `trellis-check` pass on PR2.

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
- The worktree dropdown, `ModelSelect`, and `ModeSelect` are
  the existing visual + behavioural reference. A `reka-ui`
  `DropdownMenu` would render with a different default chrome
  (different border, padding, focus ring), creating visual
  drift.
- The hand-rolled pattern already covers the project's
  a11y minimum (`aria-haspopup`, `aria-expanded`,
  `role="menu"`, `role="menuitem"`). Reka-ui's additional
  `aria-controls` is nice-to-have, not a blocker.
- Reka-ui `DropdownMenu` v2.x uses `as Child` / `as`
  polymorphism that has shifted API between alpha and stable;
  adding it for one component pins the project to a
  specific reka-ui minor.

**ModeSelect.vue** (PR2, 2026-06-13) is the third production
instance of the hand-rolled popover pattern. It lives in the
ChatInput hint row next to `ModelSelect`, opens upward, and
follows the same code skeleton (state + onDocumentClick +
onKeydown) verbatim. The 4 entries (Edit / Plan /
Yolo) are listed in popover order matching `MODE_CYCLE` from
`stores/chat.ts`; clicking Yolo routes through
`chatStore.requestSetMode(sid, "yolo")` which gates the
`set_session_mode` IPC behind a Yolo confirm modal — so the
popover closes immediately and the Yolo modal opens on top.
The Shift+Tab cycle in `ChatInput.vue` (via `useKeyboard`)
routes through the same `requestSetMode` orchestrator, so the
keyboard and popover paths share exactly one confirm gate.

**PermissionModal.vue** (PR3, 2026-06-13) is NOT a popover
— it's a CENTER modal (teleported to `<body>` via Vue
`<Teleport>`). It uses a different shape (centered, with
backdrop + blur) because ⑨ 关 is a critical decision and
needs to fully block the user's input flow until they click
one of the 3 buttons. See `reka-ui-usage.md` §"Gotcha:
`<style scoped>` does NOT apply to portal children" — the
`<Teleport>` requires `:deep()` for all modal CSS rules.

### PermissionModal: path range row (re-grill 2026-06-13 PR2)

The re-grill task `06-13-a2-b7-regrill-path-based` (Q10
"保留 risk 字段作 UI 视觉,加 path 范围行") extended the
PermissionModal with a **path range row** between the
subtitle and the command preview block. Layout:

```
┌─ permission-modal__path-range ────────────────────────┐
│  📁  /repo/src/foo.ts                       [仓库内]  │
└───────────────────────────────────────────────────────┘
```

| Element | Class | Purpose |
|---|---|---|
| Container | `.permission-modal__path-range` | Same dark surface + border-radius-8px treatment as the existing `.permission-modal__preview` block below it (visual consistency) |
| Folder icon | `.permission-modal__path-range-icon` | 14px `Icon name="folder"` (already in the registry), `--color-text-muted` tint |
| Path text | `.permission-modal__path-range-text` | `<code>` element, monospace 12px, single-line ellipsis for long paths (`overflow: hidden; text-overflow: ellipsis`) |
| Badge | `.permission-modal__path-range-badge` | Pill-shaped (`border-radius: 999px`), 11px sans, 2px×8px padding, color/border-color set via inline `:style` binding |

**Badge text + color** (driven by `isPathInRoot(path, session.currentCwd)`,
the frontend mirror of the Rust `is_within_root`):

| Predicate | Badge text | Color token | Background |
|---|---|---|---|
| `isPathInRoot(path, cwd) === true` | `仓库内` | `var(--color-tool-write)` (emerald) | 12% mix of the color token |
| `isPathInRoot(path, cwd) === false` | `仓库外` | `var(--color-tool-shell)` (amber) | 12% mix of the color token |

**Why reuse `--color-tool-write` / `--color-tool-shell`**:
the re-grill brief mentioned `--color-tool-success` /
`--color-tool-warning` but those tokens do not exist in
`app/src/style.css` today (the existing tool-color tokens
are `--color-tool-read` / `-write` / `-shell` / `-error`
/ `-thinking`). Per design-tokens.md "Don't add a new
`--color-*` token for a one-off use", we reuse the
closest existing tool-color tokens — same Tailwind 400-500
palette, semantically right (in-repo writes use the
`write_file` color, out-of-repo uses the `shell` color
because the warning visual language is "extra caution").
A future token rename / new token should revisit this
choice.

**Conditional render** (`v-if="hasPath"` in the template):

```vue
<div v-if="hasPath" class="permission-modal__path-range">
  <span class="permission-modal__path-range-icon" aria-hidden="true">
    <Icon name="folder" :size="14" />
  </span>
  <code class="permission-modal__path-range-text">{{ pathText }}</code>
  <span
    class="permission-modal__path-range-badge"
    :style="{
      color: pathBadgeColor,
      borderColor: pathBadgeColor,
      background: `color-mix(in srgb, ${pathBadgeColor} 12%, transparent)`,
    }"
  >
    {{ pathBadgeText }}
  </span>
</div>
```

`hasPath` is `typeof ask.path === "string" && ask.path.length > 0`,
mirroring the backend's `#[serde(skip_serializing_if =
"Option::is_none")]` on `PermissionAskPayload.path`. When
the field is absent (shell / web_fetch), the entire row is
removed from the DOM — no empty placeholder, no layout
shift. `v-if` is the correct gate (not `v-show`); see the
"Don't: Forget the v-if Gate on the Popover Element" rule
above for the focus-order rationale.

**`:deep()` requirement**: like every other `.permission-modal__*`
rule, the path-range row's CSS lives in `:deep()` because
the modal portals to `<body>` via `<Teleport>` (Vue's
`<style scoped>` compiler doesn't apply the `data-v-xxx`
attribute to teleported elements; see reka-ui-usage.md
§"Gotcha: <style scoped> does NOT apply to portal
children"). This is no new gotcha — same convention as
the existing PermissionModal styles.

**Empty `currentCwd` defensive behavior**: if the chat
store's `currentCwd` is empty (very early in app boot,
before the chat store has resolved a session), `isInRepo`
returns `false` and the badge renders as out-of-repo
(amber, 仓库外). This matches the Tier 4 contract — better
to ask one extra time than to silently bypass the gate.
When the session later loads and `currentCwd` populates,
the badge updates reactively because `isInRepo` is a
`computed` over the chat store's ref.

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

## Animation

> Added 2026-06-09 in the UI polish PR. All modal / popover
> instances in the project now have enter/leave transitions.
> Conventions captured here so future dropdowns / modals
> follow the same pattern.

### Convention: 150ms / 100ms with `ease-out` / `ease-in`

| Trigger | Enter | Leave |
|---|---|---|
| Modal (centered overlay) | 150ms `ease-out` | 100ms `ease-in` |
| Popover (anchored) | 150ms `ease-out` | 100ms `ease-in` |
| Toast (AppShell, pre-existing) | 200ms | 200ms (stays the outlier) |

The 150ms / 100ms split is intentional — enter feels responsive,
leave feels snappy enough that the user doesn't wait on a closing
animation. The toast uses 200ms in both directions because it's
attention-grabbing by nature; not a precedent for other popups.

### Modal: fade + scale

Modal instances use **fade + scale 0.96 → 1**:

```css
@keyframes modal-enter {
  from { opacity: 0; transform: scale(0.96); }
  to   { opacity: 1; transform: scale(1); }
}

@keyframes modal-leave {
  from { opacity: 1; transform: scale(1); }
  to   { opacity: 0; transform: scale(0.96); }
}
```

**Trigger mechanism differs by component**:

- **reka-ui DialogContent** (SettingsModal): reka-ui auto-sets
  the `data-state="open|closed"` attribute on the content. Use
  that as the CSS selector:
  ```css
  [data-state="open"] { animation: modal-enter 150ms ease-out; }
  [data-state="closed"] { animation: modal-leave 100ms ease-in; }
  ```
  No Vue `<Transition>` wrapper needed — reka-ui handles the
  mount/unmount internally.

- **Hand-rolled modals** (DeleteWorktreeConfirm, diff modal
  in ChatPanel): wrap in Vue `<Transition name="confirm-modal">`
  and define `.confirm-modal-enter-active` / `-leave-active`
  scoped CSS. See DeleteWorktreeConfirm.vue for the
  reference implementation.

### Confirmation Dialog Pattern (added 2026-06-11, 体验优化 PR `0140502`)

> **Use `app/src/components/common/ConfirmDialog.vue` for all
> destructive / confirmable actions.** This component supersedes
> the older per-action `DeleteWorktreeConfirm` /
> `DeleteModelConfirm` copies. When adding a new "are you sure?"
> dialog in the app, always reach for `ConfirmDialog` first.

**Props**:

| Prop | Type | Default | Notes |
|---|---|---|---|
| `open` | `boolean` | — | v-model binding; `v-if` mounts/unmounts the dialog |
| `title` | `string` | — | Header title (renders the warn icon automatically when `variant === "danger"`) |
| `variant` | `"danger" \| "warning" \| "default"` | `"danger"` | Drives the confirm-button color (red / accent-muted / default) |
| `confirmText` | `string` | `"确认"` | Confirm button label |

**Slot**: `body` — arbitrary content (use `<p>` for short messages,
nested markup for richer warnings).

**Emits**: `cancel` (Escape, backdrop click, ✕ button, "取消" button)
and `confirm` (Enter or confirm button click).

**Built-in behavior**:

- **Esc closes** (emits `cancel`).
- **Enter confirms** (emits `confirm`).
- **Backdrop click cancels** (`@click.self` on `.confirm-backdrop`).
- **Focus** is auto-moved to the confirm button on `open` (via
  `setTimeout(..., 0)` after the v-if mount), so Enter works
  without a prior Tab.
- **Transition** uses `name="confirm-modal"` with the 150ms
  fade+scale convention from this file.

**Why the component exists**: see the "Don't" section below
about `window.confirm()` in Tauri webview. The whole reason
`ConfirmDialog` is hand-rolled is that the native dialog
silently no-ops in this environment.

**Example usage** (session delete with body content):

```vue
<script setup lang="ts">
import ConfirmDialog from "../common/ConfirmDialog.vue";
const showConfirm = ref(false);
const sessionIdToDelete = ref<string | null>(null);

function askDelete(id: string) {
  sessionIdToDelete.value = id;
  showConfirm.value = true;
}
async function onConfirm() {
  if (sessionIdToDelete.value) await doDelete(sessionIdToDelete.value);
  showConfirm.value = false;
}
</script>

<template>
  <ConfirmDialog
    :open="showConfirm"
    title="删除 session"
    variant="danger"
    confirm-text="删除"
    @cancel="showConfirm = false"
    @confirm="onConfirm"
  >
    <p>该 session 包含 <strong>{{ messageCount }}</strong> 条消息,删除后无法恢复。</p>
  </ConfirmDialog>
</template>
```

**Convention: skip the dialog for empty containers.** A
"delete this empty session" / "delete this fresh worktree" /
"remove this unused provider" should NOT pop a confirm — the
destructive cost is zero. Only show the dialog when there is
real content the user might regret losing. The current rule in
`SessionList.vue` is: a session is "empty" iff its message
count is 0; non-empty sessions always go through
`ConfirmDialog`.

**Migration path** (optional, not blocking): the older
`DeleteWorktreeConfirm` and `DeleteModelConfirm` components
can be replaced with `ConfirmDialog` calls. The PRD
(`.trellis/tasks/06-11-session-loading/prd.md`) marked this
as "可选，不改也行" — defer until a third call site appears
or visual drift is reported.

### Popover: fade + slide (direction matches position)

Popover instances use **fade + slide**, where the slide direction
**MUST match the popover's open position**:

| Popover open direction | Slide keyframe |
|---|---|
| Upward (e.g. `ModelSelect` — `bottom: calc(100% + 4px)`) | `translateY(4px → 0)` (slides up from below) |
| Downward (e.g. worktree dropdown — `top: calc(100% + 4px)`) | `translateY(-4px → 0)` (slides down from above) |

This makes the popover feel like it's "emerging from" the trigger
button. Sliding the wrong direction (e.g. upward popover slides
*upward* from `translateY(0 → -4px)`) reads as the popover
"running away" from the trigger.

**Implementation**: wrap the popover in Vue `<Transition>` and
define scoped CSS. Reference ModelSelect.vue (upward) and
ChatPanel.vue worktree popover (downward).

### Don't: Animate the popover's parent container

Animate the popover element itself, not its parent. If the
parent (e.g. `.chat-panel__worktree`) has `transition` set on
itself, the trigger button next to the popover may shimmer or
shift during the animation. This is subtle but breaks the
illusion of a "floating" popover.

### Don't: Use `transition-delay` on popover

The popover should appear in sync with the user's click. A
delay (even 50ms) feels sluggish. The 150ms enter is the full
duration, not "150ms after a 100ms delay".

### Don't: Animate `width` / `height` of the popover

Size animations look broken at small sizes (4-8px change is
invisible) and create reflow on the rest of the page. The
popover should snap to its final size and only animate
`opacity` + `transform`.

---

## Tauri Webview Gotcha: `window.confirm()` / `window.alert()` / `window.prompt()`

> **Never call `window.confirm()` / `window.alert()` /
> `window.prompt()` from this app's frontend code.** The Tauri
> webview does NOT reliably display native browser dialogs —
> the call often silently no-ops. Discovered during the 2026-06-11
> 体验优化 PR (commit `0140502`): clicking "delete" on a
> non-empty session would invoke `window.confirm()`, the dialog
> never appeared, and the click was lost. The fix was to
> replace it with the in-app `ConfirmDialog` component (see
> the "Confirmation Dialog Pattern" section above).

**Symptom**: the click handler runs, but the user sees nothing
happen. No dialog. No error. The action that should follow
the user's "OK" never executes.

**Why it happens**: Tauri uses a webview (WebKit on macOS,
WebView2 on Windows) for its frontend. These webviews
**block synchronous native dialogs** in their default config
to avoid pausing the main thread of the renderer. Tauri's
own dialog plugin (`@tauri-apps/plugin-dialog`) wraps the
native dialogs asynchronously — the synchronous
`window.confirm()` from a regular Vue event handler is not
wired up to it.

**Fix**: use the in-app `ConfirmDialog` component (or
`<Teleport>`-based modals for richer dialogs). The component
is just DOM rendered in the same webview — no native dialog
involved.

**Migration checklist for existing code**:

- [x] `SessionList.vue` `onDelete` + `contextDelete` — migrated
  to `ConfirmDialog` in 0140502.
- [ ] `DeleteWorktreeConfirm` — still uses its own hand-rolled
  modal, not migrated. Defer until a third call site exists.
- [ ] `DeleteModelConfirm` — same as above.
- [ ] (Search the codebase for any remaining
  `confirm(` / `alert(` / `prompt(` from Vue event handlers
  before shipping a release.)

**Note**: this gotcha is for the **synchronous** native dialog
APIs from the renderer JS context. The async Tauri dialog
plugin (`@tauri-apps/plugin-dialog`'s `ask()` / `message()`)
works correctly but is overkill for a Vue component — the
in-app `ConfirmDialog` is the right tool here.

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
