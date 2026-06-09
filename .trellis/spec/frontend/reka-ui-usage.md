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
