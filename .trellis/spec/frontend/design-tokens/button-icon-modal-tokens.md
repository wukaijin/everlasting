<!-- Moved from design-tokens.md 2026-09-19 (doc-split) -->

## Button Family (added 2026-08-24, 08-24-btn-family-convergence)

`.btn` CSS class family in `app/src/style.css` — the single source of truth
for what a button looks like (previously 167 `<button>` across 69 files each
carried hand-written scoped styles; variants/sizes/radii drifted). Same
pattern as `.app-spinner`: shared CSS classes, **no Vue component**.

### API

```
.btn                        base: inline-flex centered, text-sm, 6px 12px (md),
                            radius-sm, transparent bg, fast bg/color/border
                            transition, :disabled = opacity .5 + not-allowed
.btn--primary               accent fill + on-accent text; hover accent-hover
.btn--danger                tool-error fill + on-accent text; hover brightness(1.1)
.btn--danger-soft           ghost body; hover red 12% tint + error-text ink
.btn--ghost                 transparent; hover --color-bg-hover + primary ink
.btn--muted                 elevated fill + 1px bg-border; hover accent-muted fill
                            + accent border + accent-text ink
.btn--tint                  accent-muted fill; hover brightness(1.15)
.btn--outline               1px bg-border on transparent; hover elevated fill
.btn--sm / --lg             size modifiers (default md); sm = 4px 8px + text-xs,
                            lg = 10px 16px + text-base
.btn--icon                  icon-only square: padding --space-1 + aspect-ratio 1;
                            fixed-size icon buttons declare local w/h + padding 0
                            (explicit geometry stays component-owned)
.btn--pill / --circle       shape modifiers (default radius-sm); circle needs
                            explicit component width/height
```

Keep the original BEM class alongside (`class="confirm-modal__btn btn
btn--danger"`) — tests / `:deep()` / probes anchor on BEM names.

### Ownership rules

The family owns `background / color / border / padding / border-radius /
font-size / cursor / transition / :hover / :disabled`. Components keep only
positioning and explicit geometry (margin / flex / width / height / gap /
z-index). A local override of a family-owned property requires an inline
comment stating why. **font-weight and line-height are NOT owned** — they
inherit; sites deviating keep one local declaration.

The family declares **no focus styles**: the global `:where()` focus-visible
baseline (08-22) owns keyboard rings; a `.btn*` box-shadow would out-rank the
zero-specificity baseline.

`:active` is not in the family (single-site `--ease-spring` press in
EmptyProjectState stays local). The `6px` vertical md padding is a documented
half-step (between `--space-1`/`--space-2`) written raw in `style.css`,
allowed per the spacing half-step rule.

### Audit

```bash
cd app/src && grep -rn "background: var(--color-accent)" --include="*.vue" . | grep -v "\.btn"
# expect: only documented exceptions (node-card / palette-dot / toggle-pill /
# ui-prim generative-UI family / hover-red-solid ×3 with local overrides)
```

---

## Icon Sizing

All icons go through the `Icon.vue` wrapper (the only component
that imports `@lucide/vue`); components render
`<Icon name="..." :size="N" />` and never touch the underlying SVG
libraries directly. The wrapper pins the glyph in a `<span>` with
`width` / `height` + `flex-shrink: 0` so a flex container can't
squeeze it.

**Rule: `:size` MUST be an even pixel value** — `6` / `10` / `12` /
`14` / `16` / `18` / `20` / `24`. Odd values (`11` / `13` / `15` …)
are forbidden.

**Why**: an odd CSS pixel size lands the 1px stroke on a half-device-pixel boundary on fractional-DPR / 1.5× WSLg screens. Subpixel rasterization then shimmers between reflows and the glyph visibly "shifts" / distorts frame to frame. Even sizes snap the stroke to a whole device pixel so the icon stays put. The pinned `<span>` wrapper prevents flex squeeze on top of this rule.

**Audit (2026-06-26)**: swept the tree and normalized 20 odd sizes
to even — `11→12` across 13 files + `13→14` in
`EmptyProjectState.vue`. `6` and `10` were already even and kept.

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
| Backdrop z-index | `var(--z-modal-overlay)` = 2000 | Z-index ladder (08-23-zindex-ladder-tokens). **Pre-2026-08-23 this row claimed 9998 — stale, reality was always 2000.** |
| Content z-index | `var(--z-modal)` = 2001 | Ladder. **Pre-2026-08-23 this row claimed 9999 — stale.** |
| In-modal Select portal | `var(--z-over-modal)` = 3000 (+`!important`) | reka-ui Select portals mount under `body`; must beat 2001 content. 6 modal-form sites. |
| Toast z-index | `var(--z-toast)` = 5500 | `ToastProvider`. **Pre-2026-08-23 this row claimed 10000 — stale.** Known smell: AppShell keeps a legacy `projectsStore.toast` at `--z-top` (9999) alongside ToastProvider; merging them is a behavior change, tracked as follow-up input. |
| Backdrop alpha | `70%` mix of `--color-bg-app` + 4px blur | `color-mix(in srgb, var(--color-bg-app) 70%, transparent)` |
| Modal width | `min(560px, 90vw)` | PermissionModal (smaller than SettingsModal's `720px`) |
| Modal max-height | `80vh` | PermissionModal body scrolls above this |
| Modal padding | `var(--space-4)` 16px | `YoloConfirmModal` + `ConfirmDialog` precedent |
| Border radius | `var(--radius-lg)` 8px | Matches the 8px card ladder |
| Box shadow | `var(--shadow-lg)` | `0 8px 24px rgba(0, 0, 0, 0.5)` — the dark-bg elevation tier |
| Animation | `var(--duration-base)` enter / `var(--duration-fast)` leave (fade + scale 0.96→1, `var(--ease-out)` / `ease-in`) | See "Motion Vocabulary" above; `popover-pattern.md` "Modal: fade + scale" |
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
