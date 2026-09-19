<!-- Moved from reka-ui-usage.md 2026-09-19 (doc-split) -->


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

## Gotcha: `Sheet` does NOT exist in 2.9.9

**Symptom**: `import { Sheet } from "reka-ui"` (or any
`Sheet*` primitive — `SheetRoot` / `SheetContent` /
`SheetOverlay` / `SheetTrigger` / `SheetClose`) fails
the build / type-check, or imports as `undefined` and
renders empty.

**Cause**: reka-ui's `Sheet` primitive (the side-panel
drawer — Radix's "Dialog rendered as a side panel"
variant) is not in the 2.9.9 API; it was added in a
later version. Same version-gap class as `TextFieldRoot`
above: the Radix / reka-ui docs show `Sheet`, but 2.9.9
doesn't ship it.

**Fix**: compose a side-panel drawer from the existing
`Dialog*` primitives (`DialogRoot` / `DialogPortal` /
`DialogOverlay` / `DialogContent` / `DialogTitle` /
`DialogClose`) + sidebar CSS
(`position: fixed; inset-block: 0; right: 0; transform:
translateX(...)` slide-in). The `Dialog*` set already
provides focus trap, Esc-to-close, click-overlay-to-close
(via `DialogOverlay`), and `data-state` for enter/exit
animation — functionally equivalent to `Sheet` for our
right-anchored side-panel use case.

**Production instance** (2026-06-20, B6 PR3):
`app/src/components/chat/SubagentDrawer.vue` — right-side
drawer showing a worker subagent's live transcript.
Composed from `Dialog*` + `.subagent-drawer__*` classes;
open state bound to the `subagentRuns` store's `openRunId`.
**Render `<DialogOverlay>`** — the overlay CSS is dead
weight without it and click-outside-to-close won't work
(this was a real bug caught in review: the CSS class was
defined but the element wasn't mounted; overlay was
invisible and clicks fell through).

**Why not upgrade reka-ui**: the version pin is deliberate
(see Version Pin above); upgrading risks API renames
touching every consumer. The `Dialog*` composition satisfies
every drawer requirement (right side panel, Esc /
click-outside / X close, focus trap).

**When to revisit**: if / when the project upgrades to a
reka-ui version that ships `Sheet`, migrate
`SubagentDrawer.vue` to native `Sheet*` in a follow-up.
The accessibility + behavior contract is identical, so the
swap is mechanical.

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

**Update (2026-06-14) — Vue 3.5 empirical behavior**: The
"never applied / must `:deep()`" model above is the
*theoretical* Vue 3 description. **Empirically in Vue 3.5**,
scoped CSS propagates `data-v-xxx` to `<Teleport>` / reka-ui
`*Portal` children, so plain `<style scoped>` reaches the
teleported DOM **without** `:deep()`. Proof: `SettingsModal.vue`
+ `MemoryModal.vue` style their reka-ui `DialogOverlay` /
`DialogContent` in plain `<style scoped>` (no `:deep()`) and
render correctly — overlay background / padding / shadow all
apply. So the table's **"Yes" = defensive recommendation, not
a hard requirement** on current Vue. `:deep()` is a strict
superset: harmless when scoped already matches, and a safety
net if a future Vue upgrade reverts the propagation.
`PermissionModal.vue` wraps every rule in `:deep()` as the safe
default (41 occurrences); new code may omit it for portal
children, but wrapping stays the preferred default. Both
coexisting styles (plain scoped vs `:deep()`-wrapped) are
correct under Vue 3.5.

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

**Re-grill 2026-06-13 PR2 (path range row)**: the new
`.permission-modal__path-range*` classes added to the
PermissionModal follow the same convention — every rule
is wrapped in `:deep(...)`. No new gotcha is introduced;
the path range row is rendered inside the same `<Teleport
to="body">` boundary as the rest of the modal, so the
existing `:deep()` rule applies verbatim. See the
`PermissionModal: path range row` case study in
`popover-pattern.md` for the layout + color-token details.

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

> **Warning (2026-08-29): do NOT add `position: fixed` to the
> SelectContent itself.** The example above carries it historically
> (ProvidersTab / SubagentsTab), and it *looks* harmless there — pinned
> `width: trigger-width` + default `align="start"` make the placement
> error invisible. But reka's popper **wrapper** (which already has
> `position: fixed` and carries the floating-ui transform) is what
> positions the panel; if the content element takes itself out of the
> wrapper's flow, the wrapper collapses to 0×0, floating-ui computes
> alignment/collision for a zero-width box, and:
> - `align="end"` silently does nothing (panel's left edge lands on the
>   trigger's right edge and overflows the viewport right — measured
>   2026-08-29 in the Settings project picker), and
> - viewport collision shift never kicks in.
>
> The content element must stay `position: static` (default) inside the
> wrapper. The unpinned-width picker in `SettingsModal.vue`
> (`.settings-modal__picker-content`, `align="end"`) is the reference
> implementation.

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

