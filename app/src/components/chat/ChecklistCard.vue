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
//     with a status ICON (`circle` pending, `loader` in_progress,
//     `check-mini` done). The single `in_progress` item (enforced by the
//     store's coerce) gets a spinning marker so the user can see
//     what the agent is currently working on.
//   - **Minimized**: a small floating ball showing `done/total`
//     (e.g. "2/5"). When an `in_progress` item is active, the ball
//     breathes (slow pulse) so the user knows work is ongoing
//     without expanding the card.
//
// NOTE on decoupling (2026-06-19): the UI uses ICONS while the
// Rust `render_checklist` fn keeps its `[ ]`/`[~]`/`[x]` TEXT
// markers. This is intentional — the LLM (tool_result + ephemeral
// injection) and the human (this card) see different renderings
// of the same status. Do NOT re-couple them: the text markers are
// token-cheap and survive markdown round-trips for the model; the
// icons are a human affordance. The two layers agree only on the
// 3-state `status` enum, not its visual representation.
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

/** Per-status icon name. The UI is intentionally decoupled from the
 *  Rust `render_checklist` fn's text markers (`[ ]`/`[~]`/`[x]`) —
 *  the LLM-facing layer keeps text (token-cheap, markdown-safe);
 *  this card uses icons (human affordance). See the file-top NOTE. */
function statusIcon(status: ChecklistStatus): string {
    switch (status) {
        case "done":
            return "check-mini"; // lucide Check
        case "in_progress":
            return "loader"; // lucide LoaderCircle; CSS checklist-spin rotates it
        case "pending":
        default:
            return "circle"; // lucide Circle
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
            <span class="checklist-card__ball-count"
                >{{ doneCount }}/{{ total }}</span
            >
        </button>

        <!--
      Expanded card. Header (title + progress + minimize button)
      above the items list. Each item is a row: status marker +
      content. The in_progress row gets the pulse animation.
    -->
        <div v-else class="checklist-card__panel">
            <header class="checklist-card__header">
                <span class="checklist-card__title">
                    <Icon
                        name="clipboard-list"
                        :size="16"
                        icon-class="checklist-card__title-icon"
                    />
                    <span class="ml-2"> 进度清单 </span>
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
                        :class="[
                            'checklist-card__marker',
                            statusClass(item.status),
                        ]"
                        aria-hidden="true"
                    >
                        <Icon :name="statusIcon(item.status)" :size="16" />
                    </span>
                    <span class="checklist-card__content">{{
                        item.content
                    }}</span>
                </li>
            </ul>
            <div v-else class="checklist-card__empty">清单为空</div>
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
    right: 20px;
    /* Offset above the ChatInput bar. The ChatInput height is
     ~120px on a typical viewport (textarea + chips row); we
     leave 12px of breathing room. The exact value isn't load-
     bearing — the card stacks above whatever's at the bottom. */
    bottom: 156px;
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
    border-radius: var(--radius-lg);
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
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
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
    font-size: var(--text-xs);
    font-family: var(--font-mono);
    color: var(--color-text-secondary);
    padding: 1px 6px;
    background: var(--color-bg-app);
    border-radius: var(--radius-sm);
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
    border-radius: var(--radius-sm);
    color: var(--color-text-muted);
    cursor: pointer;
    transition:
        background var(--duration-fast) var(--ease-out),
        color var(--duration-fast) var(--ease-out);
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
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    line-height: 1.45;
    color: var(--color-text-primary);
    transition: background var(--duration-fast) var(--ease-out);
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
    flex-shrink: 0;
    line-height: 1.45;
    min-width: 24px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
}

.checklist-card__marker--pending {
    color: var(--color-text-muted);
}

.checklist-card__marker--done {
    color: var(--color-tool-write);
}

.checklist-card__marker--in-progress {
    color: var(--color-tool-shell);
}

/* The spin animation runs on the SVG ITSELF (not the marker
   span), with `transform-box: fill-box` so the rotation pivots
   around the SVG's own bounding-box center. Spinning the outer
   span uses its border-box center, which can drift from the
   icon's true geometric center (the Icon wrapper adds sub-pixel
   geometry), making the spinner wobble / orbit off-center.
   fill-box pins the pivot to the icon itself — steady, centered
   rotation. */
.checklist-card__marker--in-progress :deep(svg) {
    transform-box: fill-box;
    transform-origin: 50% 50%;
    animation: checklist-spin 1s linear infinite;
}

@keyframes checklist-spin {
    from {
        transform: rotate(0deg);
    }
    to {
        transform: rotate(360deg);
    }
}

.checklist-card__content {
    flex: 1;
    min-width: 0;
    word-break: break-word;
}

.checklist-card__empty {
    padding: 12px;
    text-align: center;
    font-size: var(--text-xs);
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
    transition:
        transform var(--duration-fast) var(--ease-out),
        background var(--duration-fast) var(--ease-out),
        border-color var(--duration-fast) var(--ease-out);
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
    font-weight: var(--weight-semibold);
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
    0%,
    100% {
        opacity: 0.4;
        transform: scale(1);
    }
    50% {
        opacity: 0.9;
        transform: scale(1.18);
    }
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
