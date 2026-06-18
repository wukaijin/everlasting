<script setup lang="ts">
// ChecklistCard — B12 floating overlay for the agent's self-tracking
// checklist (PR2 frontend, 2026-06-19).
//
// The card is a `position: absolute` overlay anchored to the
// ChatPanel's bottom-right (offset ABOVE the input bar). It does
// NOT live in the message stream and does NOT scroll with the
// messages — the checklist is per-request metadata, not a chat
// turn. z-index is below PermissionModal / other modals so the
// modal layer still sits on top when an approval is open.
//
// Two states:
//   - **Expanded**: the full checklist renders — one row per item
//     with a status marker (`[ ]` pending, `[~]` in_progress,
//     `[x]` done). The single `in_progress` item (enforced by the
//     store's coerce) gets a pulsing marker so the user can see
//     what the agent is currently working on.
//   - **Minimized**: a small floating ball showing `done/total`
//     (e.g. "2/5"). When an `in_progress` item is active, the ball
//     breathes (slow pulse) so the user knows work is ongoing
//     without expanding the card.
//
// Visibility:
//   - Empty checklist (`null` from the store, meaning no
//     `update_checklist` has been seen this run) → hidden entirely.
//   - Empty array (`[]` — the model cleared the list) → renders
//     the expanded view with an "empty" placeholder.
//   - All `done` → keep expanded with a green completion tint
//     (one-pulse visual cue that the run is wrapping up). The
//     user can still manually minimize.
//
// Expand/minimize is local UI state, persisted across re-renders
// within the same ChatPanel mount but reset on session switch
// (the card remounts when the parent ChatPanel re-renders for a
// different session). The card reads the checklist from the
// checklist store via the parent-passed `items` prop (so the
// component is dumb — easy to test).

import { computed, ref, watch } from "vue";
import type { ChecklistItem, ChecklistStatus } from "../../stores/checklist";
import Icon from "../Icon.vue";

const props = defineProps<{
  /** The current session's checklist items. `null` hides the card
   *  (no `update_checklist` seen this run). An empty array still
   *  renders (the model just cleared the list). */
  items: ChecklistItem[] | null;
}>();

/** Local UI state: expanded ⇄ minimized. Defaults to expanded so
 *  the user sees the checklist the moment the first update lands.
 *  The user can minimize; we keep the choice for the lifetime of
 *  this ChatPanel mount. */
const expanded = ref<boolean>(true);

/** Counts derived from the items list. Recomputed on every prop
 *  change. The store guarantees at most one `in_progress` (via
 *  `coerceAtMostOneInProgress`), but we count defensively — if a
 *  future regression lets two slip through, the count would
 *  reflect the raw input rather than crash. */
const total = computed<number>(() => props.items?.length ?? 0);
const doneCount = computed<number>(
  () => props.items?.filter((i) => i.status === "done").length ?? 0,
);
const inProgressCount = computed<number>(
  () => props.items?.filter((i) => i.status === "in_progress").length ?? 0,
);

/** Whether the run looks "complete" (all items done). Drives the
 *  green completion tint + the auto-minimize-after-a-beat UX. */
const allDone = computed<boolean>(
  () => total.value > 0 && doneCount.value === total.value,
);

/** Whether to show the card at all. Hidden when no checklist has
 *  been seen this run (`items === null`). An empty array still
 *  shows (the model explicitly cleared). */
const showCard = computed<boolean>(() => props.items !== null);

/** When the checklist first appears (null → non-null), expand
 *  automatically so the user sees it. Subsequent state changes
 *  (e.g. all-done) do NOT auto-collapse — the user's expand/
 *  minimize choice is respected. */
watch(
  () => props.items !== null,
  (nowVisible, wasVisible) => {
    if (nowVisible && !wasVisible) {
      expanded.value = true;
    }
  },
);

/** Toggle the expanded state. Wired to the header click + the
 *  minimize / expand buttons. */
function toggleExpanded(): void {
  expanded.value = !expanded.value;
}

/** Per-status marker character. Matches the Rust `render_checklist`
 *  fn's markers (`[ ]`, `[~]`, `[x]`) so the LLM-facing tool_result
 *  and the UI stay visually consistent. */
function statusMarker(status: ChecklistStatus): string {
  switch (status) {
    case "done":
      return "[x]";
    case "in_progress":
      return "[~]";
    case "pending":
    default:
      return "[ ]";
  }
}

/** Per-status CSS class for the row's left marker color. */
function statusClass(status: ChecklistStatus): string {
  switch (status) {
    case "done":
      return "checklist-card__marker--done";
    case "in_progress":
      return "checklist-card__marker--in-progress";
    case "pending":
    default:
      return "checklist-card__marker--pending";
  }
}
</script>

<template>
  <div
    v-if="showCard"
    :class="[
      'checklist-card',
      {
        'checklist-card--minimized': !expanded,
        'checklist-card--all-done': allDone,
        'checklist-card--active': inProgressCount > 0,
      },
    ]"
    role="region"
    aria-label="Agent 进度清单"
  >
    <!--
      Minimized floating ball. Shows `done/total` + a pulsing
      ring when an in_progress item is active. Clicking expands.
    -->
    <button
      v-if="!expanded"
      type="button"
      class="checklist-card__ball"
      :aria-label="`展开清单(${doneCount}/${total})`"
      :title="`展开进度清单 (${doneCount}/${total})`"
      @click="toggleExpanded"
    >
      <span class="checklist-card__ball-ring" />
      <span class="checklist-card__ball-icon">
        <Icon name="clipboard-list" :size="14" />
      </span>
      <span class="checklist-card__ball-count">{{ doneCount }}/{{ total }}</span>
    </button>

    <!--
      Expanded card. Header (title + progress + minimize button)
      above the items list. Each item is a row: status marker +
      content. The in_progress row gets the pulse animation.
    -->
    <div v-else class="checklist-card__panel">
      <header class="checklist-card__header">
        <span class="checklist-card__title">
          <Icon name="clipboard-list" :size="13" icon-class="checklist-card__title-icon" />
          进度清单
        </span>
        <span
          class="checklist-card__progress"
          :title="`${doneCount} 已完成 / ${total} 共计`"
        >
          {{ doneCount }}/{{ total }}
        </span>
        <button
          type="button"
          class="checklist-card__minimize"
          :title="'最小化'"
          aria-label="最小化清单"
          @click="toggleExpanded"
        >
          <Icon name="minus" :size="12" />
        </button>
      </header>

      <ul v-if="total > 0" class="checklist-card__items">
        <li
          v-for="(item, idx) in items"
          :key="idx"
          :class="[
            'checklist-card__item',
            `checklist-card__item--${item.status}`,
                          ]"
        >
          <span
            :class="['checklist-card__marker', statusClass(item.status)]"
            aria-hidden="true"
          >{{ statusMarker(item.status) }}</span>
          <span class="checklist-card__content">{{ item.content }}</span>
        </li>
      </ul>
      <div v-else class="checklist-card__empty">
        清单为空
      </div>
    </div>
  </div>
</template>

<style scoped>
/* The card is a `position: absolute` overlay inside ChatPanel.
   ChatPanel provides the positioning context (the card's parent
   is `.chat-panel`, which is `position: relative` via the flex
   column). We anchor to the bottom-right, offset above the
   ChatInput bar. */
.checklist-card {
  position: absolute;
  right: 16px;
  /* Offset above the ChatInput bar. The ChatInput height is
     ~120px on a typical viewport (textarea + chips row); we
     leave 12px of breathing room. The exact value isn't load-
     bearing — the card stacks above whatever's at the bottom. */
  bottom: 132px;
  z-index: 50;
  /* z-index is BELOW PermissionModal (which lives at z-index
     1000+ via Teleport to body). The card also sits below the
     scroll-to-bottom button (z-index 10 inside MessageList —
     but MessageList is in a separate stacking context so the
     card's z-index 50 wins over it; the user-perceived
     behavior is "card floats above the message list, below
     modals"). */
  font-family: var(--font-sans);
  color: var(--color-text-primary);
  pointer-events: auto;
}

/* Expanded panel shape. */
.checklist-card__panel {
  width: 280px;
  max-height: 60vh;
  display: flex;
  flex-direction: column;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 8px;
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.35);
  overflow: hidden;
}

/* All-done tint: a subtle green left border to signal "the run
   is wrapping up". The user can still minimize / expand; we
   don't auto-collapse. */
.checklist-card--all-done .checklist-card__panel {
  border-left: 3px solid var(--color-tool-write);
}

.checklist-card__header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  cursor: pointer;
  user-select: none;
}

.checklist-card__title {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex: 1;
  font-size: 12px;
  font-weight: 600;
  color: var(--color-text-primary);
  min-width: 0;
}

.checklist-card__title-icon {
  color: var(--color-text-secondary);
  flex-shrink: 0;
}

.checklist-card--all-done .checklist-card__title-icon {
  color: var(--color-tool-write);
}

.checklist-card__progress {
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
  padding: 1px 6px;
  background: var(--color-bg-app);
  border-radius: 4px;
  flex-shrink: 0;
}

.checklist-card__minimize {
  flex-shrink: 0;
  width: 20px;
  height: 20px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: 0;
  border-radius: 4px;
  color: var(--color-text-muted);
  cursor: pointer;
  transition: background 0.1s, color 0.1s;
}

.checklist-card__minimize:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.checklist-card__items {
  list-style: none;
  margin: 0;
  padding: 6px 4px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
  min-height: 0;
}

.checklist-card__item {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 4px 6px;
  border-radius: 4px;
  font-size: 12px;
  line-height: 1.45;
  color: var(--color-text-primary);
  transition: background 0.12s;
}

.checklist-card__item:hover {
  background: var(--color-bg-elevated);
}

/* Pending items get a slightly muted treatment so the eye is
   drawn to the in_progress and done rows. */
.checklist-card__item--pending {
  color: var(--color-text-secondary);
}

.checklist-card__item--done .checklist-card__content {
  text-decoration: line-through;
  text-decoration-color: var(--color-text-muted);
  color: var(--color-text-muted);
}

/* The in_progress row gets a subtle accent background so it
   stands out even before the marker pulse kicks in. */
.checklist-card__item--in-progress {
  background: color-mix(in srgb, var(--color-tool-shell) 8%, transparent);
}

.checklist-card__marker {
  font-family: var(--font-mono);
  font-size: 11px;
  font-weight: 600;
  flex-shrink: 0;
  user-select: none;
  line-height: 1.45;
  min-width: 24px;
}

.checklist-card__marker--pending {
  color: var(--color-text-muted);
}

.checklist-card__marker--done {
  color: var(--color-tool-write);
}

.checklist-card__marker--in-progress {
  color: var(--color-tool-shell);
  /* The pulse: 1.6s ease-in-out, infinite. The animation is
     on the marker glyph itself (not the whole row) so the
     user's eye is drawn to exactly the in-progress item. */
  animation: checklist-pulse 1.6s ease-in-out infinite;
}

@keyframes checklist-pulse {
  0%, 100% { opacity: 1; transform: scale(1); }
  50% { opacity: 0.5; transform: scale(0.94); }
}

.checklist-card__content {
  flex: 1;
  min-width: 0;
  word-break: break-word;
}

.checklist-card__empty {
  padding: 12px;
  text-align: center;
  font-size: 11px;
  color: var(--color-text-muted);
}

/* ---- Minimized floating ball ---- */
.checklist-card__ball {
  position: relative;
  width: 44px;
  height: 44px;
  padding: 0;
  border-radius: 50%;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  color: var(--color-text-secondary);
  cursor: pointer;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.3);
  transition: transform 0.12s, background 0.12s, border-color 0.12s;
  overflow: visible;
}

.checklist-card__ball:hover {
  background: var(--color-bg-elevated);
  border-color: var(--color-accent);
  transform: scale(1.06);
}

/* The ball's content: clipboard icon on top, count below. */
.checklist-card__ball-icon {
  position: absolute;
  top: 8px;
  left: 0;
  right: 0;
  margin: 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--color-text-secondary);
}

.checklist-card--all-done .checklist-card__ball-icon {
  color: var(--color-tool-write);
}

.checklist-card__ball-count {
  position: absolute;
  bottom: 6px;
  left: 0;
  right: 0;
  margin: 0 auto;
  font-size: 9px;
  font-family: var(--font-mono);
  font-weight: 600;
  color: var(--color-text-muted);
  line-height: 1;
}

/* Breathing ring around the ball when an in_progress item is
   active. Drawn as an absolutely-positioned ::before on the
   ring span so we can animate opacity + scale independently
   of the ball's hover transform. */
.checklist-card__ball-ring {
  position: absolute;
  inset: -3px;
  border-radius: 50%;
  pointer-events: none;
}

.checklist-card--active .checklist-card__ball-ring {
  border: 2px solid var(--color-tool-shell);
  animation: checklist-breathe 2.2s ease-in-out infinite;
}

@keyframes checklist-breathe {
  0%, 100% { opacity: 0.4; transform: scale(1); }
  50% { opacity: 0.9; transform: scale(1.18); }
}

/* When all done + minimized, swap the ring color to green
   for a calmer "done" cue. */
.checklist-card--all-done.checklist-card--active .checklist-card__ball-ring,
.checklist-card--all-done .checklist-card__ball-ring {
  border-color: var(--color-tool-write);
  animation: none;
  opacity: 0.5;
}
</style>
