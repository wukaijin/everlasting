<!-- Moved from design-tokens.md 2026-09-19 (doc-split) -->

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

**Don't use `linear` for UI transitions** — keep linear for
rotation only. Since 2026-08-23 (task 08-23-spinner-skeleton-primitive)
the canonical rotation lives in `app/src/style.css` shared primitives:
`@keyframes app-spin` consumed by `.app-spinner` (rings, 0.8s linear;
size ladder `--2xs` 8px / `--xs` 10px / default 14px / `--sm` 12px /
`--lg` 20px, track `--color-bg-border` + accent arc, `--inline` variant
= currentColor arc with transparent top for in-button use) and
`.icon-spin` (icon/svg rotation, 1s linear). Components must not
declare their own spin keyframes — keep only positional CSS locally.
The old per-site keyframes (search-modal-spin, chat-input-spin,
checklist-spin, …) are gone; `ChatInput.vue`'s 0.6s spinner was dead
code removed in the same task.

### Skeleton shimmer convention (added 2026-08-23)

`@keyframes skeleton-shimmer` (background-position 200% → -200%,
`background-size: 200% 100%`, 1.5s ease-in-out infinite) is the single
skeleton animation. Gradient stops stay per-component because the start
color must sit one step off the parent background: ChatPanel bubbles
(bg-app parent) use elevated→border-strong; TurnTimeline cards
(elevated parent cards) use surface→elevated. Never unify stops across
different parent surfaces — share only the keyframes and rhythm.

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

