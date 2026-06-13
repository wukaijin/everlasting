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
- `TabsRoot` / `TabsList` / `TabsTrigger` / `TabsContent`
  (SettingsModal inner tabs)
- `SelectRoot` / `SelectTrigger` / `SelectContent` /
  `SelectItem` / `SelectValue` (Settings forms — protocol,
  provider, thinking effort)
- `CheckboxRoot` / `CheckboxIndicator` (Settings forms —
  supportsThinking)
- `RadioGroupRoot` / `RadioGroupItem` / `RadioGroupIndicator`
  (DefaultTab — default model)
- `Label` (wrapping form fields for accessibility)

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

## Gotcha: `TextFieldRoot` does NOT exist in 2.9.9

**Symptom**: `import { TextFieldRoot } from "reka-ui"` compiles
(type-only import may pass) but the component renders as
`<undefined>` at runtime, and Vue logs a warning about a missing
component.

**Cause**: `TextFieldRoot` was added in reka-ui 3.x. The 2.9.9
API does not include any "TextField" / "Input" primitive — text
inputs are expected to use the platform's native `<input>`.

**Fix**: use native `<input>` wrapped in reka-ui `Label`,
themed via the project's existing `.xxx__input` class (e.g.
`.providers-tab__input`, `.models-tab__input`). The visual
result is identical to a reka-ui `SelectRoot` trigger because
both share the same padding, background, border, and
focus-color tokens.

**Example** (the project's working pattern):

```vue
<Label class="providers-tab__field">
  <span class="providers-tab__label">Display name</span>
  <input
    v-model="form.displayName"
    class="providers-tab__input"
    type="text"
    placeholder="My provider"
  />
</Label>
```

The `.providers-tab__input` class applies the same tokens as
the `SelectRoot` trigger:

```css
.providers-tab__input {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  color: var(--color-text-primary);
  padding: 6px 10px;
  font-size: 13px;
  font-family: inherit;
  transition: border-color 0.15s, box-shadow 0.15s;
  outline: none;
}
.providers-tab__input:focus {
  border-color: var(--color-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent);
}
```

**When to revisit**: if / when the project upgrades to
reka-ui 3.x, swap `<input>` for `<TextFieldRoot>` in a
follow-up PR. The v-model contract is identical, so the
swap is a mechanical wrapper change with no behavioral
impact.

---

## Gotcha: `<style scoped>` does NOT apply to portal children

**Symptom**: a `SelectContent` (or any other reka-ui primitive
that portals to body — `DialogContent` inside another
`DialogContent`, `PopoverContent`, `TooltipContent`,
`DropdownMenuContent`, etc.) renders with **no styling at all**:
transparent background, no border, no padding, no width,
no z-index. The CSS rule block "exists" in the file but
visually has zero effect. Items appear as naked text in
the document flow, often below or behind the dialog.

**Cause**: Vue 3 `<style scoped>` compiles each selector with
a `data-v-xxx` attribute suffix (e.g. `.models-tab__content`
becomes `.models-tab__content[data-v-models-tab-xxx]`).
The compiled selector therefore only matches elements
**inside the component's own template**. Elements rendered
through `<Teleport to="body">` — which is what every
reka-ui `*Portal` primitive uses internally — do not
receive the component's `data-v-xxx` attribute (they were
not in the component's template at compile time). The
selector silently fails to match, and the rule is dead.

**Why this bites reka-ui users specifically**: reka-ui's
architecture *requires* a portal for any overlay primitive
(`SelectContent`, `DialogContent`, `PopoverContent`,
`TooltipContent`, `DropdownMenuContent`, `HoverCardContent`,
`ContextMenuContent`, `MenubarContent`, `Toast`,
`AlertDialogContent`, etc.). Almost every interactive
reka-ui component will hit this. The same is true of
Radix UI, Headless UI, Ark UI, and any other Floating-UI-
based library.

**Fix**: use `:deep()` to escape the scoped boundary.
Wrap the class name (and any data-attribute selectors) in
`:deep(...)`:

```css
/* In SettingsModal/ProvidersTab.vue <style scoped> */
/* WRONG — dead rule, content is rendered to <body> via
   <SelectPortal>, so the compiled selector never matches */
.models-tab__content { ... }

/* CORRECT — :deep() strips the data-v-xxx suffix from
   the inner selector, so it matches portal children */
:deep(.models-tab__content) { ... }
```

**Rule of thumb** — which rules need `:deep()`:

| Element | Where rendered | Needs `:deep()`? |
|---|---|---|
| `SelectTrigger` / `DialogContent` (when this is the OUTER dialog) | inside the component's own template | **No** — keep scoped |
| `SelectContent` / `SelectViewport` / `SelectItem` | rendered to `<body>` via `<SelectPortal>` | **Yes** — wrap in `:deep()` |
| `DialogContent` (when nested inside another dialog) | rendered to `<body>` via `<DialogPortal>` | **Yes** — wrap in `:deep()` |
| `DialogOverlay` (sibling of `DialogContent` inside `DialogPortal`) | rendered to `<body>` | **Yes** — wrap in `:deep()` |
| `<Teleport to="body">` content (Vue's built-in Teleport, not reka-ui) | rendered to `<body>` | **Yes** — wrap in `:deep()` |
| Trigger icon / label / form field wrapper | inside the component's own template | **No** — keep scoped |

The last row was added when the PR3 `PermissionModal.vue` (2026-06-13)
chose Vue's built-in `<Teleport to="body">` over reka-ui's
`DialogPortal` (the modal isn't a reka-ui `Dialog` — it's hand-rolled
markup with the same visual / behavioral contract as the
`DialogContent`-based modals). The Teleport still portals the
modal's DOM to `<body>`, so the `<style scoped>` compiler's
`data-v-xxx` attribute is never applied to the teleported elements
and every CSS rule that targets them must be wrapped in `:deep()`.
This is a *Vue* `Teleport` constraint, not a reka-ui one — but
reka-ui's `*Portal` primitives use the same `<Teleport>` under
the hood, so the same `:deep()` rule applies.

**Example** (the project's working pattern in
`app/src/components/settings/ProvidersTab.vue`, 2026-06-09):

```css
/* Trigger — stays scoped (in-component) */
.providers-tab__trigger { ... }
.providers-tab__trigger:hover { ... }
.providers-tab__trigger[data-state="open"] { ... }

/* Content / viewport / option — :deep() (rendered via SelectPortal) */
:deep(.providers-tab__content) {
  position: fixed;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  z-index: 3000 !important; /* see also: width strategy below */
  min-width: var(--reka-select-trigger-width, 240px);
  width: var(--reka-select-trigger-width);
  overflow: hidden;
}
:deep(.providers-tab__viewport) { padding: 4px; }
:deep(.providers-tab__option) { ... }
:deep(.providers-tab__option[data-highlighted]) { ... }
:deep(.providers-tab__option[data-state="checked"]) { ... }
```

**Diagnosis tip — how to confirm this is the bug you're
hitting, not a z-index / specificity issue**:

1. Open DevTools, find the `SelectContent` element in the
   Elements panel. It's a direct child of `<body>`, not of
   `#app` or your component.
2. Check the **Attributes** panel. If the element does
   **not** have a `data-v-xxx` attribute, you are hitting
   this gotcha.
3. Check the **Styles** panel for the class you wrote.
   If the rule is **not listed at all** (or only listed
   as "not matching"), the compiled scoped selector
   silently dropped it. Switch to `:deep()` and the
   rule will appear.

**Don't** try to fix this with:
- `!important` on the z-index — the rule isn't being
  applied at all, specificity is moot.
- Higher-specificity selectors (`body .xxx__content`) —
  works in some cases but fights the rest of the design
  system and is brittle.
- Inline `style=""` — spec forbids it; bypasses the
  design system tokens.
- Removing `<SelectPortal>` — changes reka-ui behavior
  in ways that break positioning.

**Cross-reference**: same gotcha applies to
`.trellis/spec/frontend/popover-pattern.md` hand-rolled
popovers (ModelSelect, worktree dropdown) — but those
don't portal, so they don't hit it. The lesson is
specific to portal-based primitives.

**When to revisit**: if the project ever migrates to a
CSS-in-JS solution (e.g. CSS Modules, Vanilla Extract,
Pinceau) that doesn't use Vue's `data-v-xxx` scope
attribute, this gotcha goes away. Until then, every new
reka-ui portal primitive needs the `:deep()` check.

---

## Tip: Use `--reka-select-trigger-width` to size SelectContent to its trigger

reka-ui 2.9.9's `SelectContent` does not size itself to
the trigger button by default — it uses content-based
natural width. Hardcoding `min-width: 240px` (or any
fixed value) in the class means a wider trigger (typical
for a form field) renders a narrower dropdown that looks
detached.

**Fix**: use the `--reka-select-trigger-width` CSS
variable that reka-ui sets on `SelectContent` to match
the trigger's measured width:

```css
:deep(.providers-tab__content) {
  min-width: var(--reka-select-trigger-width, 240px);
  width: var(--reka-select-trigger-width);
}
```

- The `240px` fallback in `min-width` covers edge cases
  where the variable is undefined (e.g. the popper is
  mounted before the trigger has measured).
- The `width` line intentionally has **no** fallback — if
  the variable is missing, the popover falls back to
  content-based natural width, which is more graceful
  than locking to 240px.
- **Note**: the prefix is `--reka-` (reka-ui 2.9.9),
  **not** `--radix-`. Older Reka / Radix docs may use
  `--radix-`; that's wrong for this project.

**When to use**: any `SelectContent` (or other popper-
based reka-ui primitive that supports a similar variable)
that should visually align with its trigger — typical
for form controls in a `Dialog` (SettingsModal) or any
constrained-width container. For chip-attached popovers
(hand-rolled `ModelSelect` / worktree dropdown), this is
moot — those don't use reka-ui `Select` per
`popover-pattern.md`.

---

## Pattern: Tooltip for hover affordances (added 2026-06-10, A4 token-usage)

Use reka-ui's `Tooltip` primitive for **hover-only static
information** — e.g. breaking down a single number into
its components, or providing a one-line hint that doesn't
need its own click target.

**Production instance** (2026-06-10, A4):
`app/src/components/chat/ChatInput.vue` — the
`chat-input__token-usage` chip (e.g. "14.2K · 7% / 200K")
hovers out a 4-line breakdown (`input / cache_read /
cache_creation / output`) via reka-ui `Tooltip`. The
trigger is the chip itself; the tooltip content is
the breakdown list.

**Six-piece structure** (always required, in this order):

```vue
<TooltipProvider>
  <TooltipRoot :delay-duration="150">
    <TooltipTrigger as-child>
      <span class="my-chip">14.2K</span>
    </TooltipTrigger>
    <TooltipPortal>
      <TooltipContent class="my-chip__tooltip" :side-offset="4">
        <TooltipArrow class="my-chip__tooltip-arrow" />
        <!-- tooltip body -->
      </TooltipContent>
    </TooltipPortal>
  </TooltipRoot>
</TooltipProvider>
```

**Why all six pieces**:

- **`TooltipProvider`** — top-level context provider. reka-ui 2.9.9's
  `TooltipRoot` is **not** self-contained: it relies on a
  `TooltipProviderContext` (Vue's Symbol-based `provide`/`inject`)
  that the Provider `provide`s. Rendering `TooltipRoot` without a
  `TooltipProvider` ancestor throws at runtime with
  `Injection Symbol(TooltipProviderContext) not found` and the
  entire Vue tree (here: `ChatWindow`) goes blank. TypeScript
  / `pnpm build` does NOT catch this because the inject is
  runtime-only. **Always wrap TooltipRoot in TooltipProvider.**
  Add it as a local wrapper at the consumer site (one provider
  per Tooltip instance, NOT app-root) — lifting to app root is
  YAGNI for a single consumer.
- **`TooltipRoot`** — context provider; `delay-duration` (ms) defers the open so quick mouse-passes don't trigger.
- **`TooltipTrigger as-child`** — merges trigger props onto the existing child (`<span>` / `<button>`) so the chip's own class + click handler are preserved. **Without `as-child` the trigger renders as a `<button>` that wraps the chip — you lose styling and get a nested clickable.**
- **`TooltipPortal`** — portals to `<body>` to escape overflow containers. **Required for the portal-child styling to work** (see gotcha above).
- **`TooltipContent`** — receives `data-state="delayed-open|closed"` for animation; `side-offset` is the gap between trigger and tooltip (4px is the project default, matches `popover-pattern.md`).
- **`TooltipArrow`** — the little triangle pointing at the trigger. Optional but conventional; users expect it.

**Don't**: wrap `TooltipContent` in `v-if`. reka-ui
manages the open/close lifecycle itself; `v-if` will fight
it and the tooltip will flicker or fail to open.

**Don't**: set `delay-duration="0"`. Even 100-150ms
defer prevents "tooltip pops up on every mouse pass" —
annoying for dense chip rows. The project's default
delay-duration is 150ms.

**Styling**: see `<style scoped>` gotcha above —
`TooltipContent` portals to body, so the rule MUST be
wrapped in `:deep()` to match. Trigger styles stay scoped.

**Example CSS** (from ChatInput.vue A4):

```css
/* trigger — stays scoped, in-component */
.chat-input__token-usage { ... }

/* tooltip content — :deep() required */
:deep(.chat-input__token-usage-tooltip) {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  padding: 8px 12px;
  font-size: 12px;
  z-index: 1000;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
}

@keyframes tooltip-enter {
  from { opacity: 0; transform: translateY(2px); }
  to   { opacity: 1; transform: translateY(0); }
}

:deep(.chat-input__token-usage-tooltip[data-state="delayed-open"]) {
  animation: tooltip-enter 150ms ease-out;
}
```

**Don't use reka-ui Tooltip** for click-triggered
dropdowns / menus. Those are the hand-rolled popover
pattern (see `popover-pattern.md`).

---

## Convention: Wrap reka-ui primitives in project-scoped CSS classes

Reka-ui primitives are unstyled by default. The project styles
them via the **same BEM-style `.component-name__element` classes
** that wrap the rest of the UI. Do not write reka-ui-specific
class names like `.reka-select-trigger`.

**Why**: keeping a single naming system makes grep-ability
easier and avoids a parallel "reka-ui CSS" subsystem.

**Example**:

```vue
<!-- SettingsModal.vue -->
<DialogContent class="settings-modal__content">
  <DialogTitle class="settings-modal__title">Settings</DialogTitle>
  <!-- ... -->
</DialogContent>
```

```css
/* SettingsModal.vue <style scoped> */
.settings-modal__content {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 8px;
  /* ... */
}
```

---

## Convention: Theming via `data-state` and `data-highlighted` attributes

Reka-ui sets `data-*` attributes on its primitives to reflect
state. Use these as CSS selectors instead of binding state
to Vue refs and toggling classes.

**Common attributes**:

- `data-state="open|closed|indeterminate"` (Dialog, Popover,
  Checkbox, RadioGroup)
- `data-highlighted="true|false"` (SelectItem hover/focus)
- `data-disabled="true|false"` (all primitives)
- `data-placeholder="true"` (SelectValue when no value chosen)

**Example** (Select trigger, like the ones in ProvidersTab):

```css
.select-trigger {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  padding: 6px 10px;
  font-size: 13px;
  color: var(--color-text-primary);
}
.select-trigger[data-state="open"] {
  border-color: var(--color-accent);
}
.select-trigger[data-disabled] {
  opacity: 0.5;
  cursor: not-allowed;
}
```

This is cleaner than `:class="{ 'is-open': isOpen }"` on a
ref-driven boolean.

---

## Convention: Form fields use `Label` wrapper for accessibility

Every form field in a reka-ui form MUST be wrapped in a reka-ui
`Label` for screen-reader association, even if you also write a
visible `<label>` text. The `Label` primitive generates the
correct `for` / `aria-labelledby` relationship that assistive
tech expects.

**Wrong**:

```vue
<label class="providers-tab__label">Display name</label>
<input v-model="form.displayName" class="providers-tab__input" />
<!-- ❌ The label and input are not programmatically linked -->
```

**Correct**:

```vue
<Label class="providers-tab__field">
  <span class="providers-tab__label">Display name</span>
  <input v-model="form.displayName" class="providers-tab__input" />
</Label>
<!-- ✅ Reka-ui Label auto-links the inner input -->
```

The `Label` wrapper sets `for` on the inner input by walking
its slot children. If the slot contains a `SelectRoot`, the
Label links to the Select's hidden input. If it contains a
native `<input>`, the Label links directly to it.

---

## Don't: Use reka-ui's `Popover` primitive for project popovers

The project has two popovers (worktree dropdown, ModelSelect)
and **both** are hand-rolled per `.trellis/spec/frontend/popover-pattern.md`.
Do not switch them to reka-ui `Popover` — the visual + behavioral
contract (CSS variables, `onDocumentClick` close, Esc close,
`min-width: 200px` / `220px`, etc.) is already set in the
existing popovers and reka-ui `Popover` would render with
different defaults.

**Exception**: if a future popover needs keyboard-first
navigation (↑/↓/Home/End/Enter), virtual scrolling for >100
items, or `aria-controls` referencing an out-of-tree element,
reka-ui `Popover` may be appropriate. Document the deviation.

---

## Don't: Re-style reka-ui primitives with inline `style=""`

Use the project's BEM class system. Inline `style=""` on
reka-ui components bypasses the design system and makes
future theme changes (e.g. `--color-accent` swap) require
touching every consumer.

---

## Common Mistake: Forgetting to forward `data-*` attributes

When wrapping a reka-ui primitive in a custom Vue component,
the `data-*` attributes set by reka-ui may not bubble through
automatically. Use `v-bind="$attrs"` (with `inheritAttrs: false`
on the wrapper) to forward them.

**Symptom**: A wrapped `SelectRoot` trigger's `data-state="open"`
attribute is missing on the rendered element, so your CSS
`[data-state="open"]` selector never matches.

**Fix**: in the wrapper component:

```vue
<script setup>
defineOptions({ inheritAttrs: false });
</script>
<template>
  <SelectTrigger v-bind="$attrs" :class="triggerClass">
    <slot />
  </SelectTrigger>
</template>
```

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
