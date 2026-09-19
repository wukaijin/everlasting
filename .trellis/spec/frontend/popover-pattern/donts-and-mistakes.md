<!-- Moved from popover-pattern.md 2026-09-19 (doc-split) -->

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

