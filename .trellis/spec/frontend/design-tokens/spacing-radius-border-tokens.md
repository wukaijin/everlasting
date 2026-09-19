<!-- Moved from design-tokens.md 2026-09-19 (doc-split) -->

## Spacing Tokens (added 2026-06-27, PR-1)

The 4-based scale below supersedes the pre-PR-1 ad-hoc
"no spacing scale" convention. Components reference
`var(--space-N)` instead of writing raw px values; the
sed sweep in `PR-2` normalized the 219 spacing-related
declarations in component CSS.

| Token | Value | Use |
|---|---|---|
| `--space-0` | `0` | Reset (rare) |
| `--space-1` | `4px` | Micro spacing: chip internal padding, tag inner padding, icon margin |
| `--space-2` | `8px` | Component internal gap, adjacent element spacing, default section padding |
| `--space-3` | `12px` | Chip padding, small card padding, hint row gap |
| `--space-4` | `16px` | Standard section padding, card padding, large spacing |
| `--space-5` | `20px` | Large block section padding, header padding, empty-state outer padding |
| `--space-6` | `24px` | Panel internal padding, modal body padding, large gap |
| `--space-7` | `32px` | Section empty-state padding, hero spacing |
| `--space-8` | `48px` | Reserved (not used today — landing page hero tier) |

**Don't add `--space-1-5` / `--space-2-5` half-step tokens.**
If a component needs 6px or 10px (the two ad-hoc values
that exist pre-PR-1: 6px in `ChatPanel.vue` header padding,
10px in `Sidebar.vue` header padding), add the raw value
with a comment explaining why it doesn't fit the scale
(typically: a half-step between two larger tokens where
neither works).

---

**When to introduce a spacing scale**: if the same value
(e.g. `8px`) appears in 10+ unrelated components, extract
it to a token. Until then, leave as-is.

---

## Radius Tokens (formalized 2026-06-27, PR-1)

The 4 / 6 / 8 / 12 / 999 ladder is the project's radius
scale. Pre-PR-1 these were used as raw values (`4px`,
`6px`, etc.); PR-1 formalized them as tokens and added
`--radius-pill` for circular buttons (send / stop) and
pill chips.

| Token | Value | Use |
|---|---|---|
| `--radius-sm` | `4px` | Small chips, tags, compact buttons |
| `--radius-md` | `6px` | Popovers, dropdowns, form inputs, tool cards |
| `--radius-lg` | `8px` | Modals, large cards, surfaces, message bubbles (was `6px` raw pre-PR-3a — bumped to 8px when the asymmetric `border-bottom-*-radius: 2px` "tail" decoration was removed in `PR-3a`) |
| `--radius-xl` | `12px` | Chat input row (the single 12px corner radius in the app's primary input surface) |
| `--radius-pill` | `999px` | Circular buttons (send / stop), pill chips (e.g. `--color-bg-selected` row) |

**Don't** add a half-step token (e.g. `--radius-2` = 2px or
`--radius-3` = 3px). The 3 non-standard radii in the
codebase today (`2px` in `ChatInput.vue` stop-button
glyph, `3px` in `DiffView.vue` and a few `delete confirm`
modals) are inner-element decorations; if reused 3+ times
they get a token, otherwise they stay raw with a comment.

---

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

