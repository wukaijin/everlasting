<!-- Moved from design-tokens.md 2026-09-19 (doc-split) -->

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
| `--color-text-secondary` | `#9aa3b8` | Less important text (captions, labels) — bumped 2026-08-05 from `#8b95a7` (slate-350, +6% luminance) |
| `--color-text-muted` | `#8a93a8` | Subtitles, status bar, sidebar headers, hint text — bumped 2026-08-05 from `#7c8aa0` (slate-400) in step with `--color-text-secondary` to preserve the 7% luminance gap |
| `--color-text-on-accent` | `#ffffff` | Pure white for text on saturated accent / tool-error backgrounds (buttons, toasts, counts) where `--color-text-primary` reads dirty. Added 2026-06-27 PR2 (14 ad-hoc `#ffffff` swept). |
| `--color-accent-text` | `#7c9aff` | Accent as **ink** (text / icons) on dark backgrounds — 6.71:1 on surface, AA. `--color-accent` (#3b5bdb) as text is only 3.14:1 (graphics-only); see "填充 500 / 文字 400" principle below. Added 2026-08-15 (contrast-color-r1). |

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
| `--color-tool-error` | `#ef4444` | Errors — red. **Graphics only** (left bars, fills, icon backgrounds; 3:1 ok) |
| `--color-tool-error-text` | `#f87171` | Error **copy** (error message text, error badge text) — 5.87:1 on elevated vs `#ef4444`'s 4.32 FAIL. Added 2026-08-15 (contrast-color-r1) |
| `--color-tool-thinking` | `#a78bfa` | Extended thinking blocks — violet |

**Principle: 填充 500 档 / 文字 400 档** (2026-08-15). Tailwind-500-range
colors work as fills / left bars / rings on the dark backgrounds (graphics
need only 3:1) but underserve as text (AA needs 4.5:1). When a saturated
token is needed as ink, add / reach for a 400-range sibling token
(`--color-accent-text`, `--color-tool-error-text`), never reuse the fill
token for text.

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

### Status colors (added 2026-06-27 PR2)

| Token | Value | Use |
|---|---|---|
| `--color-status-success` | `#4ade80` | Success / positive feedback (green-400) — MemoryPreview/MemoryLayerItem loaded, ChatInputHintRow ok, FileInjectionsHint success |
| `--color-status-warn` | `#fbbf24` | Warning / caution feedback (amber-400) — MemoryPreview/MemoryLayerItem error, ChatInputHintRow warn |

Distinct from `--color-tool-write` (emerald) / `--color-tool-shell`
(amber): tool colors are **tool-category** semantics (which LLM
tool ran), status colors are **outcome** semantics (success/warn
feedback). Pre-PR2 the green/amber hex was hardcoded across 4
components; `FileInjectionsHint` already referenced
`--color-status-success` via a CSS fallback, so PR2 defined what
the project already expected. The re-grill note above (reusing
`--color-tool-write`/`--color-tool-shell` for PermissionModal
path-range badges) is unaffected — those badges describe
path-range risk, not success/warn outcome.

---

## State Tints (added 2026-06-27, PR-1)

| Token | Value | Use |
|---|---|---|
| `--color-bg-hover` | `color-mix(in srgb, var(--color-text-primary) 10%, transparent)` | List item / nav / chip hover — 10% primary wash (was 6% until 2026-08-15; the 6% wash was specimen-verified as indistinguishable from default, see the 2026-08-15 Decision below) |
| `--color-bg-active` | `color-mix(in srgb, var(--color-text-primary) 14%, transparent)` | `:active` press feedback — slightly stronger than hover (14% vs 10%) to confirm the click registered |
| `--color-bg-selected` | `color-mix(in srgb, var(--color-accent) 12%, transparent)` | Selected list item / active nav state — 12% accent tint, distinct from hover (which is primary wash) so the two states don't blur together |

**Convention**: the wash concentration (10% → 14% → 12% →
16%) gives a clean 4-state read: `default → hover → pressed
→ selected`. The 16% selected+hover wash is composed inline
as `color-mix(in srgb, var(--color-accent) 16%, transparent)`
(see `SessionList.vue` `.session-item--active:hover`); it's
deliberately not a new token because it's only used in one
place today (the active session item hover).

These tokens are used in 3+ unrelated components
(Sidebar session items, ToolCallCard hover, EmptyProjectState
hidden projects, reka-ui SelectItem hover) so they pass
the "Don't add a new `--color-*` token for a one-off use"
threshold below.

