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

2026-08-21 (quota panel relocation, follow-up to
`08-20-turn-usage-event-quota-view`) added
`app/src/components/chat/ChatInputTokenUsage.vue` — the hint-row
token chip's click popover (context bar + last-turn breakdown +
cache-hit rate + rolling-window aggregates + settings). It
absorbed the short-lived AppHeader `QuotaChip.vue` (08-20, since
deleted) and replaced the chip's reka-ui hover Tooltip. Geometry
follows `ChatInputLatencyPopover` (opens up from the hint row)
with one variation: the popover is wide (420px) and **centered on
the chip** via `left: 50%; transform: translateX(-50%)` +
`max-width: calc(100vw - 32px)` instead of `left: 0` anchoring —
remember to compose `translateX(-50%)` into any Transition
`transform` keyframes when copying.

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
  box-shadow: var(--shadow-md);
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
  box-shadow: var(--shadow-md);
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

> **分篇**(2026-09-19):本文保留核心 Pattern(含 External Trigger 变体与 Position Direction Rule);Code Skeleton、Don't/Common Mistake、Animation 与 Tauri Webview Gotcha 已按 tool-contract 模式拆至 `popover-pattern/` 子目录(原锚点以 stub 保留)。

## Code Skeleton (Copy-paste Starting Point)

> **已拆出**(2026-09-19 doc-split):完整骨架见 [`popover-pattern/code-skeleton.md`](./popover-pattern/code-skeleton.md)。

## Don't: Use `reka-ui` `DropdownMenu` for New Dropdowns in This Project

> **已拆出**(2026-09-19 doc-split):完整反例见 [`popover-pattern/donts-and-mistakes.md`](./popover-pattern/donts-and-mistakes.md)(含 Re-Implement Close Logic / `v-if` Gate / `overflow: hidden` Clipped)。

## Don't: Re-Implement Close Logic Per-Component

> **已拆出**(2026-09-19 doc-split):见 [`popover-pattern/donts-and-mistakes.md`](./popover-pattern/donts-and-mistakes.md)。

## Don't: Forget the `v-if` Gate on the Popover Element

> **已拆出**(2026-09-19 doc-split):见 [`popover-pattern/donts-and-mistakes.md`](./popover-pattern/donts-and-mistakes.md)。

## Common Mistake: Popover Clipped by Parent `overflow: hidden`

> **已拆出**(2026-09-19 doc-split):见 [`popover-pattern/donts-and-mistakes.md`](./popover-pattern/donts-and-mistakes.md)。

## Animation

> **已拆出**(2026-09-19 doc-split):完整动效契约见 [`popover-pattern/animation.md`](./popover-pattern/animation.md)。

## Tauri Webview Gotcha: `window.confirm()` / `window.alert()` / `window.prompt()`

> **已拆出**(2026-09-19 doc-split):见 [`popover-pattern/tauri-webview-gotcha.md`](./popover-pattern/tauri-webview-gotcha.md)。

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
