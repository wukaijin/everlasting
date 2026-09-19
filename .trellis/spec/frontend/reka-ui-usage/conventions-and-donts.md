<!-- Moved from reka-ui-usage.md 2026-09-19 (doc-split) -->

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
  <DialogTitle class="settings-modal__title">设置</DialogTitle>
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

