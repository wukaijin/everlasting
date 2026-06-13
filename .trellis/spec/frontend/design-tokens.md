# Frontend Design Tokens

> CSS variable system for the frontend visual language.
> Captures the color, spacing, radius, and font tokens that
> every component should reference — never hardcode hex
> values or px magic numbers in component CSS.

---

## Status

Filled (2026-06-09). Token definitions live in
`app/src/style.css` (single global stylesheet imported by
`main.ts`). Components reference tokens via `var(--name)`.

---

## Color Tokens

All colors are defined in `app/src/style.css` under
`:root { ... }`. They cascade to every component.

### Backgrounds (dark theme base)

| Token | Value | Use |
|---|---|---|
| `--color-bg-app` | `#0a0e14` | App body background (deepest) |
| `--color-bg-surface` | `#131822` | Cards, modals, popover content |
| `--color-bg-elevated` | `#1a2030` | Form inputs, hover states, popover triggers |
| `--color-bg-border` | `#1e2530` | Default 1px borders |
| `--color-bg-border-strong` | `#3b475a` | Stronger borders where `--color-bg-border` reads invisible (it is only 4 luminance units brighter than `--color-bg-elevated`) |

**Convention**: backgrounds progress from
`bg-app` (deepest) → `bg-surface` (mid) → `bg-elevated`
(raised). A child element should never be on the same
background as its parent — pick a step that creates
contrast.

### Text

| Token | Value | Use |
|---|---|---|
| `--color-text-primary` | `#cbd5e1` | Main body text, form input text |
| `--color-text-secondary` | `#8b95a7` | Less important text (captions, labels) |
| `--color-text-muted` | `#7c8aa0` | Subtitles, status bar, sidebar headers, hint text |

**Token gap rules**:

- `--color-text-muted` and `--color-text-secondary` must
  always be distinguishable. Current gap is **7% luminance**
  (slate-450 vs slate-400-equivalent). Don't bump one
  without checking the other.
- `--color-text-primary` is the default. Don't use
  `--color-text-secondary` for body text (too dim).

### Accent

| Token | Value | Use |
|---|---|---|
| `--color-accent` | `#3b5bdb` | Primary actions (Save, Add), focus rings, selected state |
| `--color-accent-hover` | `#4263eb` | Hover state for accent backgrounds |
| `--color-accent-muted` | `#1e2a5e` | Accent at low alpha (e.g. selected row background) |

**Convention**: focus rings use
`box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent)`
(not a hardcoded alpha). The 20% mix is a project convention
— keep it consistent across components.

### Tool colors (LLM agent tool categories)

| Token | Value | Use |
|---|---|---|
| `--color-tool-read` | `#06b6d4` | `read_file` — cyan |
| `--color-tool-write` | `#10b981` | `write_file` — emerald |
| `--color-tool-shell` | `#f59e0b` | `shell` — amber |
| `--color-tool-error` | `#ef4444` | Errors — red |
| `--color-tool-thinking` | `#a78bfa` | Extended thinking blocks — violet |

These map 1:1 with the LLM tool categories the agent
executes. New tool categories should pick a new color from
the same family (Tailwind 400-500 range for readability on
dark background).

**Note (re-grill 2026-06-13 PR2)**: the re-grill brief
referred to `--color-tool-success` and `--color-tool-warning`
tokens for the PermissionModal path-range row's in-repo /
out-of-repo badge. These tokens **do not exist** in
`app/src/style.css` — the project uses the 5 tokens listed
above. To stay within the "Don't add a new `--color-*`
token for a one-off use" rule below, PR2 reuses the
existing `--color-tool-write` (emerald) and `--color-tool-shell`
(amber) tokens for the in-repo / out-of-repo badges. The
visual semantics are tight (in-repo writes already use the
`write_file` color; the `shell` color carries the
"extra caution" connotation that fits "out of repo"). If a
future refactor renames these tokens or introduces
`--color-tool-success` / `--color-tool-warning`, the
PermissionModal path-range row should be updated to follow.

---

## Decision: `--color-text-muted` bumped to `#7c8aa0` (2026-06-09)

**Context**: the original `--color-text-muted: #64748b`
(slate-500) was too dim for 11px mono text. Sidebar headers,
status bar, chat input hint, and form labels were all nearly
invisible against the dark surface.

**Decision**: bumped to `#7c8aa0` (slate-450, ~6% luminance
lift) as part of the UI polish PR.

**Rationale**:

- Lifts 11px mono gray text to readable weight without
  crossing into "primary text" territory.
- Maintains a 7% luminance gap from `--color-text-secondary: #8b95a7`
  so the two are still distinguishable.
- Matches the 11px mono text in AppShell toast, sidebar
  header (`SESSIONS`), and chat input hint (now more
  readable).

**When to revisit**: if the contrast between `--color-text-muted`
and `--color-text-secondary` ever feels too small (or too
large), bump both in step, keeping the relative gap.

---

## Typography Tokens

| Token | Value | Use |
|---|---|---|
| `--font-sans` | `"HarmonyOS Sans SC", -apple-system, BlinkMacSystemFont, "Microsoft YaHei UI", sans-serif` | Default body font |
| `--font-mono` | `ui-monospace, "SF Mono", "Cascadia Code", "Source Code Pro", Menlo, Consolas, monospace` | Monospace (chip labels, hint text, status bar, code blocks) |

The sans stack starts with `HarmonyOS Sans SC` (bundled,
472KB woff2 subset) — see `.trellis/spec/frontend/cjk-fonts.md`
for why. Mono falls back to system fonts only; we don't
bundle a CJK mono font.

**Convention**: 11px mono for "metadata" (hints, labels,
chips, status); 13-14px sans for "content" (form input
text, message bodies, dropdown options).

---

## Spacing Tokens

The project does **not** use a fixed spacing scale (no
`--space-1` / `--space-2` etc.). Components use `gap`,
`padding`, and `margin` values directly, picked per
context. This is intentional — the design is dense and
visual rhythm comes from alignment, not from a strict
spacing scale.

**When to introduce a spacing scale**: if the same value
(e.g. `8px`) appears in 10+ unrelated components, extract
it to a token. Until then, leave as-is.

---

## Radius Tokens

| Token | Value | Use |
|---|---|---|
| (none) | `4px` | Small chips, buttons |
| (none) | `6px` | Popovers, dropdowns, form inputs |
| (none) | `8px` | Modals, large cards |
| (none) | `12px` | Chat input row (large rounded rect) |

Like spacing, the project uses direct values rather than
named tokens for radius. The 4 / 6 / 8 / 12 ladder is the
project's de-facto scale.

**When to extract**: if a 5th step appears or if a
component needs a non-standard radius, the project should
decide whether the new value is a one-off or a new scale
step. Don't add `--radius-md` unless it's reused 3+ times.

---

## Border Tokens

| Token | Value | Use |
|---|---|---|
| `--color-bg-border` | `#1e2530` | Default 1px borders |
| `--color-bg-border-strong` | `#3b475a` | Stronger borders (use when `--color-bg-border` is invisible against `--color-bg-elevated`) |

Border **width** is always `1px` — no thicker borders in
this design. Border **style** is always `solid`.

**Exception (2026-06-13)**: the `PermissionModal --critical`
variant and the `YoloConfirmModal` content card use a 3px red
left border (`var(--color-tool-error)`). This is a deliberate
one-off — the 3px width is reserved for "extreme risk" modals
in the app, so the two highest-stakes confirmation surfaces
(Yolo entry, Tier 2 hard-deny) look like visual cousins. See
`yolo-safety-design.md §7` and `permission-modal-ux.md §"视觉规范"`.
No new token is needed; the existing
`--color-tool-error` is the only color used.

---

## Don't: Hardcode color / spacing / radius values in component CSS

Component CSS MUST reference the tokens. Hardcoded
`#131822` in a component file will silently drift if the
token is ever updated.

**Wrong**:

```css
.my-card {
  background: #131822; /* ❌ hardcoded, will not follow --color-bg-surface updates */
}
```

**Correct**:

```css
.my-card {
  background: var(--color-bg-surface); /* ✅ tracks the token */
}
```

**Exception**: the `app/src/style.css` file itself, where
the token values are defined.

---

## Modal Tokens (added 2026-06-13, PR3 of A2 + B7)

The `PermissionModal` (and the pre-existing `YoloConfirmModal`
+ `SettingsModal` + `MemoryModal`) all share a modal-pattern
set of values. These are NOT new CSS variables — they're
convention values that the modals reference via the existing
tokens. Captured here so a future modal knows what to reach for
without re-deriving the numbers.

| Concern | Value | Token / source |
|---|---|---|
| Backdrop z-index | `9998` | Convention (overlay below content) |
| Content z-index | `9999` | Convention (above overlay) |
| Toast z-index | `10000` | Per PR1 audit §3.2 (toast above all modals) |
| Backdrop alpha | `70%` mix of `--color-bg-app` + 4px blur | `color-mix(in srgb, var(--color-bg-app) 70%, transparent)` |
| Modal width | `min(560px, 90vw)` | PermissionModal (smaller than SettingsModal's `720px`) |
| Modal max-height | `80vh` | PermissionModal body scrolls above this |
| Modal padding | `16px` | `YoloConfirmModal` + `ConfirmDialog` precedent |
| Border radius | `8px` | Matches `--color-bg-surface` cards |
| Box shadow | `0 8px 24px rgba(0,0,0,0.5)` | Standard modal shadow |
| Animation | `150ms` enter / `100ms` leave (fade + scale 0.96→1) | `popover-pattern.md` "Modal: fade + scale" |
| Critical border-left | `3px solid var(--color-tool-error)` | See "Border Tokens" exception above |

**Risk-level visual** (PermissionModal header icon container +
risk label dot):

| Risk level | Icon | Tint color | Container bg (12% mix) |
|---|---|---|---|
| `low` | `info` (lucide) | `var(--color-text-muted)` | gray tint |
| `medium` | `circle-dot` (lucide) | `var(--color-tool-write)` | emerald tint |
| `high` | `shield-check` (lucide) | `var(--color-tool-shell)` | amber tint |
| `critical` | `shield-x` (lucide) | `var(--color-tool-error)` | red tint |

The risk-label Chinese text (`低` / `中` / `高` / `极高`) lives
in `app/src/stores/permissions.ts` as the `RISK_META` constant;
the `Risk.label_cn()` method on the backend
(`agent/permissions::Risk`) is the source of truth (mirrored on
the frontend to avoid an IPC round-trip for a static label).

---

## Don't: Add a new `--color-*` token for a one-off use

The token system is intentionally small. Before adding a
new color token, ask:

1. Will this color appear in 3+ unrelated components?
2. Is it a "primary" use case (action, surface, text) or
   a one-off accent?

If the answer to (1) is "no" or (2) is "one-off", put the
hex value in the component CSS with a comment explaining
why it can't be a token.

---

## Related

- `app/src/style.css` — token definitions.
- `.trellis/spec/frontend/cjk-fonts.md` — `--font-sans`
  bundling (where HarmonyOS Sans SC comes from).
- `.trellis/spec/frontend/popover-pattern.md` — popover
  styling conventions (use these tokens).
- `.trellis/spec/frontend/reka-ui-usage.md` — reka-ui
  primitives are themed against these tokens.
