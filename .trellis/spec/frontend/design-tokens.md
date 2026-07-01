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
| `--color-text-on-accent` | `#ffffff` | Pure white for text on saturated accent / tool-error backgrounds (buttons, toasts, counts) where `--color-text-primary` reads dirty. Added 2026-06-27 PR2 (14 ad-hoc `#ffffff` swept). |

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
| `--color-bg-hover` | `color-mix(in srgb, var(--color-text-primary) 6%, transparent)` | List item / nav / chip hover — 6% primary wash, reads as "interactive but not pressed" |
| `--color-bg-active` | `color-mix(in srgb, var(--color-text-primary) 10%, transparent)` | `:active` press feedback — slightly stronger than hover (10% vs 6%) to confirm the click registered |
| `--color-bg-selected` | `color-mix(in srgb, var(--color-accent) 12%, transparent)` | Selected list item / active nav state — 12% accent tint, distinct from hover (which is primary wash) so the two states don't blur together |

**Convention**: the wash concentration (6% → 10% → 12% →
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

## Decision: Design system token expansion (2026-06-27, PR-1)

**Context**: the pre-2026-06-27 token system had color +
typography covered, but spacing / radius / type-scale / motion
/ shadow values were scattered as raw px / ms values across
the 44 components. PR-1 + PR-2 + PR-3d swept 438 raw values
into token form, formalizing the project's de-facto scale
(4-based spacing, 4/6/8/12 radius, font-size ladder) and
adding the missing motion vocabulary (durations + easings).

**Decision**: added 6 new token families to
`app/src/style.css` `@theme` block:

- **Spacing scale** (8 tokens): `--space-0..8` = 0/4/8/12/16/20/24/32/48 px
- **Radius scale** (5 tokens): `--radius-sm/md/lg/xl/pill` = 4/6/8/12/999px
- **Type scale** (7 sizes + 4 leading + 4 weights): `--text-xs..2xl`, `--leading-tight/normal/relaxed/loose`, `--weight-regular/medium/semibold/bold`
- **Motion** (6 durations + 3 easings): `--duration-instant/fast/base/slow/pulse/blink` + `--ease-out/spring/decelerate`
- **Shadow scale** (4 + ring): `--shadow-xs/sm/md/lg` + `--shadow-ring`
- **3 state tints** (above section): `--color-bg-hover/active/selected`

**Rationale**:

- The 4-based spacing scale matches the existing project
  convention (the same 4/8/12/16/20/24/32 px values were
  used ad-hoc in 20+ files). The token layer is additive —
  visual rhythm is unchanged.
- The 100/150/240ms motion duration split aligns with the
  pre-existing `popover-pattern.md` modal/popover 150ms-enter
  / 100ms-leave convention. The added `--duration-slow`
  (240ms) absorbs the toast (was 200ms ad-hoc) and the
  subagent drawer slide (was 180ms ad-hoc). `--duration-pulse`
  (1800ms) absorbs the subagent breathing animation that
  was hard-coded twice (tool card left bar, drawer section
  spinner). **Modal** 另有专用档位 `--duration-modal-in/out`
  (200/150ms) + `--ease-modal-in` / `--ease-accelerate`
  (2026-07-02, task 07-02-modal-motion-rhythm)；popover/drawer
  仍用 `--duration-base/fast`。
- `--ease-out` is `cubic-bezier(0.16, 1, 0.3, 1)` — a
  Linear-style snappy decel, replacing the bare CSS
  `ease-out` keyword. Slightly "harder" feel (faster
  initial deceleration), but the popover-pattern.md spec
  doesn't document the exact curve, so the visual delta
  is minimal.
- The 3 state tints are deliberately `color-mix` at use
  time (not precomputed hex) so a future `--color-accent`
  or `--color-text-primary` change propagates without
  re-balancing the tints.

**Migration path** (consumed in PR-2): the 438 raw values
in components were swept by sed in a follow-up PR
(`PR-2`); component CSS now references tokens verbatim.
The `:where(button)` baseline transition (PR-3d) is the
only global CSS rule that consumes the motion tokens
outside component scope — it provides a fast-color
fallback for buttons without explicit transitions.

**When to revisit**:
- If the spacing scale needs a half-step (`6px`, `10px`,
  `14px`) for a niche component, do NOT add `--space-1-5`
  tokens — add the raw value with a comment explaining
  why (per the "Don't add a token for a one-off use"
  rule below).
- If the project ever migrates to Tailwind v4 utility
  classes for spacing, the `--space-*` tokens will collide
  with Tailwind's built-in `--spacing-*` (note the
  missing `ing`). The token names were chosen to AVOID
  the collision so this is a non-issue today.

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

### Type Scale (added 2026-06-27, PR-1)

| Token | Value | Use |
|---|---|---|
| `--text-2xs` | `10px` | Sub-caption / 角标 metadata (added 2026-06-27 PR2): `(edited)` label, MemoryPreview/DiffView metadata, ModeSelect/TriggerMenu/ModelSelect captions — 21 ad-hoc 10px values were swept into this token |
| `--text-xs` | `11px` | Mono metadata: chip labels, hint text, sidebar header (`SESSIONS`), status bar, caption |
| `--text-sm` | `12px` | Caption / hint / small button / form help |
| `--text-base` | `13px` | Form input, message body, dropdown option |
| `--text-md` | `14px` | Default app font (`:root font-size`), ChatInput editor body |
| `--text-lg` | `16px` | Section title, card title, chat input send button |
| `--text-xl` | `20px` | Empty-state hero title (`EmptyProjectState`, `chat-panel__empty`) |
| `--text-2xl` | `24px` | Reserved (not used yet — landing page / hero title) |

**Convention**: 11-12px is the "metadata" tier (mono, hints,
chips). 13-14px is the "content" tier (body, message
bubble, form input). 16-20px is the "title" tier (section
header, empty-state hero).

### Line Heights (added 2026-06-27, PR-1)

| Token | Value | Use |
|---|---|---|
| `--leading-tight` | `1.3` | Headings, dense chip rows |
| `--leading-normal` | `1.5` | Default body, form input |
| `--leading-relaxed` | `1.6` | Chat message bubble (was hard-coded `1.6` in `MessageItem.vue`) |
| `--leading-loose` | `1.75` | Reserved for long-form content (not used yet) |

The 1.6 (vs Tailwind v4's default 1.625) is intentional —
chat message rhythm is slightly tighter to keep adjacent
user/assistant turns visually grouped.

### Font Weights (added 2026-06-27, PR-1)

| Token | Value | Use |
|---|---|---|
| `--weight-regular` | `400` | Default body, message text |
| `--weight-medium` | `500` | Button label, chip emphasis, sidebar project path |
| `--weight-semibold` | `600` | Title, section header, strong button |
| `--weight-bold` | `700` | Reserved (not used — `--weight-semibold` carries all current emphasis needs) |

---

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

## Motion Vocabulary (added 2026-06-27, PR-1)

Six duration tokens + three easing tokens. The 100/150ms
split matches the pre-PR-1 `popover-pattern.md` modal /
popover convention; the rest (80ms instant, 240ms slow,
1800ms pulse, 1000ms blink) absorb the ad-hoc durations
that were sprinkled across the codebase (TriggerMenu
0.08s, AppShell toast 0.2s, subagent drawer 0.18s, tool
card breathing 1.8s, streaming cursor 1s).

### Durations

| Token | Value | Use |
|---|---|---|
| `--duration-instant` | `80ms` | TriggerMenu palette / super-fast feedback (the one place that benefits from a sub-100ms response) |
| `--duration-fast` | `100ms` | Modal / popover **leave**; default hover bg/color; list item :hover out; sidebar session :hover |
| `--duration-base` | `150ms` | Modal / popover **enter**; default form input focus; chat input focus-within ring |
| `--duration-slow` | `240ms` | Toast (was 200ms, slightly slower for "attention-grabbing" feel); subagent drawer slide (was 180ms + ease-out) |
| `--duration-pulse` | `1800ms` | Subagent breathing (tool card left bar `--color-tool-shell` pulse, drawer section live spinner) |
| `--duration-blink` | `1000ms` | Streaming cursor (the `▍` glyph on streaming assistant messages) |

### Easings

| Token | Value | Use |
|---|---|---|
| `--ease-out` | `cubic-bezier(0.16, 1, 0.3, 1)` | Workhorse: every hover, focus, list item, chip transition. Slightly "harder" than CSS `ease-out` keyword (faster initial decel). |
| `--ease-spring` | `cubic-bezier(0.34, 1.56, 0.64, 1)` | `:active` press feedback (button `translateY(0.5px)` on press — the slight overshoot gives a "physical" feel). Used in `EmptyProjectState.vue` add button. |
| `--ease-decelerate` | `cubic-bezier(0, 0, 0.2, 1)` | Subagent drawer slide (replaces the old `ease-out` keyword; gives a more "physical" slide-in feel). |

**Don't use `linear` for UI transitions** — keep linear
for the rotating spinner only (0.6s linear infinite is
the only `linear` transition left in the codebase, in
`ChatInput.vue` chat-input-spin keyframe; this is a
fallback that PR-3e will likely replace with a shimmer
in a follow-up).

### Modal / Popover Convention (updated 2026-06-27)

The pre-PR-1 convention `150ms enter / 100ms leave` is
preserved, but now expressed via tokens:

| Surface | Enter | Leave |
|---|---|---|
| Modal (centered overlay) | `var(--duration-base) var(--ease-out)` | `var(--duration-fast) ease-in` |
| Popover (anchored) | `var(--duration-base) var(--ease-out)` | `var(--duration-fast) ease-in` |
| Toast (AppShell) | `var(--duration-slow) var(--ease-out)` | (same) |
| Subagent drawer | `var(--duration-slow) var(--ease-decelerate)` | (same) |

See `popover-pattern.md` "Animation" section for the
canonical reference.

### List enter (TransitionGroup) — added 2026-06-27 PR3

Message list (`MessageList.vue`) uses Vue `<TransitionGroup>` for
new-message enter. Four non-obvious gotchas (all hit during PR3):

1. **`:deep()` required** — TransitionGroup adds the `*-enter-*` classes
   to the child **component's** root element (`MessageItem`'s `<li>`). A
   scoped `.msg-enter-active` compiles to `.msg-enter-active[data-v-ML]`
   which doesn't reach the class on the child root; `:deep(.msg-enter-active)`
   drops the attribute selector so it matches.
2. **`transition: ... !important` required** — `MessageItem`'s
   `.msg:not(.msg--editing):not(.msg--err)` carries specificity (0,4,0) with
   `transition: background-color`. `:deep(.msg-enter-active)` is only (0,2,0),
   so the background-color transition **wholly overrides** the enter
   opacity/transform transition (transition is a property-level override,
   not a per-property merge) → no fade, no slide. `!important` forces the
   enter transition during the enter window (no hover then, so losing the
   background-color transition is harmless).
3. **`appear` to animate the first mount** — TransitionGroup's `appear`
   defaults off, so the first message in an empty session (where
   `MessageList` mounts fresh via `v-else`) would NOT animate. Set `appear`.
4. **`overflow-x: hidden` on the scroll container** — enter uses `translateX`
   toward the list's outer edge; `overflow-y: auto` makes `overflow-x`
   implicitly `auto`, so the offset bubbles trigger a **horizontal
   scrollbar** that flashes during the animation. Explicit `overflow-x:
   hidden` clips the outer offset without showing a scrollbar.

**Direction**: user enters from the right (`translateX(+24px) → 0`),
assistant from the left (`translateX(-24px) → 0`) — each from its own
aligned side's outer edge. The outer offset is clipped by `overflow-x:
hidden`, but the bubble body's travel is clearly visible. Reference
implementation: `MessageList.vue` `.msg-enter-*`.

### Reduced Motion (added 2026-06-27, PR-1)

`app/src/style.css` includes a top-level `@media
(prefers-reduced-motion: reduce)` block that collapses
all `animation-duration` and `transition-duration` to
`0.01ms` for users with the OS setting on. Required for
WCAG 2.3.3 accessibility. Don't override this rule at
the component level — the global rule wins via
`!important`.

---

## Shadow Scale (added 2026-06-27, PR-1)

Four elevation tiers + a focus-ring token. Alphas are
tuned for the dark `--color-bg-app` (#0a0e14) — pure-black
shadows with the same RGB channel values would have ~0.3
luminance lift on a near-black background (read as flat
"outline"), so the dark-bg alpha scale is 0.32 / 0.4 /
0.5.

| Token | Value | Use |
|---|---|---|
| `--shadow-xs` | `0 1px 2px rgba(0, 0, 0, 0.32)` | Chip hover lift, small raised chip |
| `--shadow-sm` | `0 2px 4px rgba(0, 0, 0, 0.4)` | Popover, dropdown, subagent drawer "↓ N new" floating button |
| `--shadow-md` | `0 4px 12px rgba(0, 0, 0, 0.4)` | AppShell toast, larger popover |
| `--shadow-lg` | `0 8px 24px rgba(0, 0, 0, 0.5)` | Reserved intermediate elevation tier (between md dropdowns and xl modals). Pre-PR1 some modals used this; the modal family now uses `--shadow-xl`. |
| `--shadow-xl` | `0 16px 48px rgba(0, 0, 0, 0.5)` | **Modal / large dialog (largest tier)**. Added 2026-06-27 PR1 — 8 modals (Settings / Memory / AuditLog / Diff / Yolo / DeleteWorktree / MarkdownDetail / ConfirmDialog) previously hardcoded this exact value. See `popover-pattern.md` "modal = xl". |
| `--shadow-ring` | `0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent)` | Focus ring (chat input focus-within, form input :focus, AuditLog select open). Uses the 20% accent mix that the pre-PR-1 chat input ring used directly as `box-shadow`. |

### Shadow exceptions (2026-06-27 PR1)

The following shadows are deliberately NOT mapped to a token —
each is a one-off value (unique alpha or offset/blur combo) where
no tier fits without a visible change. They stay inline so a
future `grep "box-shadow: 0"` doesn't read them as drift; this
table is the authoritative "these are intentional" list.

| File | Value | Why kept (not tokenized) |
|---|---|---|
| `MessageList.vue:262` (scroll-to-bottom FAB) | `0 2px 8px rgba(0,0,0,0.18)` | Extra-light float for a small FAB; lighter than `--shadow-sm` |
| `SubagentDrawer.vue:896` (↓N new pill) | `0 2px 8px rgba(0,0,0,0.25)` | Same FAB family, slightly stronger |
| `SessionList.vue:568` (ctx menu) | `0 4px 16px rgba(0,0,0,0.2)` | Wider blur + lighter alpha than `--shadow-md` |
| `HiddenProjectsMenu.vue:178` | `0 8px 24px rgba(0,0,0,0.35)` | lg offset/blur but lighter alpha than `--shadow-lg` |
| `ChecklistCard.vue:269` | `0 6px 24px rgba(0,0,0,0.35)` | Between md/lg for the floating checklist panel |
| `ChecklistCard.vue:460` | `0 4px 14px rgba(0,0,0,0.3)` | Floating empty-state CTA inside the panel |
| `EmptyProjectState.vue:187` | `0 1px 0 color-mix(accent 35%)` | 1px inner highlight, not an elevation shadow |
| `MessageActionsMenu.vue:350` | `0 0 0 2px color-mix(accent 25%)` | 2px focus ring (deliberately thinner than `--shadow-ring`'s 3px) |
| `MemoryLayerItem.vue:240,249` | `0 0 0 2px color-mix(var(--color-status-success)/--warn 25%)` | Status-dot ring; colors tokenized in PR2 (`--color-status-success`/`--color-status-warn`), only the 2px ring form is non-token (vs `--shadow-ring`'s 3px) |

If a future refactor adds a `--shadow-fab` (light float) token
covering the FAB family (MessageList / SubagentDrawer), the first
two rows can retire.

**Don't add a non-ring shadow that uses the accent color**
(purple/violet glow). Pre-PR-1 some components had subtle
accent-tinted shadows; the dark-bg elevation ladder above
is more honest — the surface rises via black, not color.

---

## Icon Sizing

All icons go through the `Icon.vue` wrapper (the only component
that imports `@lucide/vue` / `@heroicons/vue`); components render
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

## Don't: Hardcode color / spacing / radius / motion / shadow / type values in component CSS

Component CSS MUST reference the tokens. Hardcoded
`#131822`, `8px`, `150ms`, `0 4px 12px rgba(0,0,0,0.4)`, or
`font-size: 14px` in a component file will silently drift
if the token is ever updated.

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
