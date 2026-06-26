<script setup lang="ts">
// DrawerSection — generic collapsible container for the drawer's
// thinking / tools / reply segments.
//
// PR5 of the subagent-drawer redesign (2026-06-21). Per PRD R14 +
// R15 + R16 + Q5 + Technical Approach: the drawer body is a stack
// of segments. Each segment has a chip-style header (icon + label
// + count) and a collapsible body that the parent fills via the
// default slot.
//
// Why a generic `DrawerSection` (vs three specialised components):
//   - All three segments share the same collapse / expand behavior,
//     the same chip layout, and the same live-indicator slot. A
//     generic shell + per-segment parent-provided content keeps
//     the visual contract in ONE place.
//   - The differences (icon, label, default-open state, live
//     indicator kind) are pure props — no behavioral fork needed.
//
// Live indicator (PRD R17-R19): the `live` prop flips the chip's
// right-hand side between two states:
//   - `live === true`  → spinner icon + entry count + elapsed time
//                        (e.g. "⚡ 3 · 4.2s"); updates every 100ms
//                        via the drawer's `nowTick` ticker.
//   - `live === false` → static "✓ Completed · X.Xs" OR "N entries"
//                        depending on whether `finalDurationMs` is
//                        provided.
//
// The live indicator is OWNED by this component (not the parent):
// the drawer passes `live` + `entryCount` + `elapsedMs` /
// `finalDurationMs` and this component formats the chip. Keeping
// formatting centralized means the three segments use IDENTICAL
// chip styling (no per-segment CSS drift).
//
// Default-open state: per PRD R16, Thinking defaults to collapsed;
// Tools + Reply default to expanded. The parent sets
// `:default-open="false"` for Thinking and lets the default (`true`)
// apply to the other two. The state is component-local (NOT
// persisted) — reopening the drawer resets to defaults (PRD Out of
// Scope #4).

import { ref, computed } from "vue";
import Icon from "../Icon.vue";
import { abbreviateDuration } from "../../utils/duration";

const props = withDefaults(
  defineProps<{
    /** Segment type — drives the header icon + label. */
    type: "thinking" | "tools" | "reply";
    /** Number of entries inside the segment body. Shown in the
     *  chip as "N entries" / "N tools" / "N chars". `0` hides
     *  the count portion (the chip still shows the icon + label). */
    entryCount?: number;
    /** Whether the segment is currently live (worker running).
     *  When true, the chip shows the spinner + elapsed time;
     *  when false, it shows the static count + (optionally)
     *  `✓ X.Xs` if `finalDurationMs` is provided. */
    live?: boolean;
    /** Live elapsed time in ms (the drawer's `elapsedMs` or
     *  `terminalDurMs`). Drives the spinner chip's "X.Xs" suffix
     *  while `live === true`. */
    elapsedMs?: number;
    /** Terminal duration in ms — when provided AND `live === false`,
     *  the chip shows "✓ Completed · X.Xs". Omit for reply segment
     *  (which doesn't have a per-segment duration concept). */
    finalDurationMs?: number;
    /** Override the header label. Defaults to a per-type label
     *  ("Thinking" / "Tools" / "Reply"). */
    label?: string;
    /** Initial expanded state. `true` (expanded) by default; the
     *  drawer passes `false` for the Thinking segment per PRD R16. */
    defaultOpen?: boolean;
  }>(),
  {
    entryCount: 0,
    live: false,
    elapsedMs: 0,
    finalDurationMs: undefined,
    label: undefined,
    defaultOpen: true,
  },
);

const open = ref<boolean>(props.defaultOpen);

function toggle(): void {
  open.value = !open.value;
}

/** Per-type icon name (heroicons key in Icon.vue registry). */
const iconName = computed<string>(() => {
  switch (props.type) {
    case "thinking":
      return "thinking";
    case "tools":
      return "wrench";
    case "reply":
      return "document";
  }
});

/** Per-type default label. */
const fallbackLabel = computed<string>(() => {
  switch (props.type) {
    case "thinking":
      return "Thinking";
    case "tools":
      return "Tools";
    case "reply":
      return "Reply";
  }
});

const displayLabel = computed<string>(() => props.label ?? fallbackLabel.value);

/** Unit suffix for the entry count. "chars" for thinking (chars
 *  of thinking text), "tools" for tools, "lines" for reply. */
const countUnit = computed<string>(() => {
  switch (props.type) {
    case "thinking":
      return "chars";
    case "tools":
      return "tools";
    case "reply":
      return "lines";
  }
});

/** Formatted live-elapsed chip text. Per PRD R17 the live indicator
 *  shows "spinner + 数字 + 耗时" — the 数字 is the entry count
 *  (tools / chars / lines per segment type) and the 耗时 is the
 *  elapsed seconds. e.g. "3 · 4.2s" while the worker is running.
 *  Empty-segment guard: when `entryCount === 0` (worker just
 *  started, no entries yet), show only the elapsed time so the
 *  chip doesn't render a misleading "0 ·". */
const liveChipText = computed<string>(() => {
  const sec = (props.elapsedMs / 1000).toFixed(1);
  return props.entryCount > 0 ? `${props.entryCount} · ${sec}s` : `${sec}s`;
});

/** Formatted terminal chip text. e.g. "✓ Completed · 4.2s" when
 *  the worker has finished. */
const finalChipText = computed<string>(() => {
  if (typeof props.finalDurationMs !== "number") {
    // No terminal duration → just show the count.
    return props.entryCount > 0 ? `${props.entryCount} ${countUnit.value}` : "";
  }
  return `✓ ${abbreviateDuration(props.finalDurationMs)}`;
});
</script>

<template>
  <section class="drawer-section" :data-type="type">
    <button
      type="button"
      class="drawer-section__header"
      :aria-expanded="open"
      @click="toggle"
    >
      <span class="drawer-section__chevron" :class="{ 'drawer-section__chevron--closed': !open }">
        <Icon name="chevron-down" :size="12" />
      </span>
      <span class="drawer-section__icon">
        <Icon :name="iconName" :size="12" />
      </span>
      <span class="drawer-section__label">{{ displayLabel }}</span>
      <span v-if="entryCount > 0 && !live && finalDurationMs === undefined" class="drawer-section__count">
        {{ entryCount }} {{ countUnit }}
      </span>
      <span v-if="live" class="drawer-section__live-chip" :title="`Worker running · ${liveChipText}`">
        <span class="drawer-section__live-spinner" aria-hidden="true" />
        <span class="drawer-section__live-text">{{ liveChipText }}</span>
      </span>
      <span v-else-if="typeof finalDurationMs === 'number'" class="drawer-section__final-chip">
        {{ finalChipText }}
      </span>
    </button>
    <div v-if="open" class="drawer-section__body">
      <slot />
    </div>
  </section>
</template>

<style scoped>
.drawer-section {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  margin-bottom: 8px;
  overflow: hidden;
}

.drawer-section__header {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 8px 12px;
  background: transparent;
  border: 0;
  cursor: pointer;
  font: inherit;
  font-family: var(--font-sans);
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  text-align: left;
}

.drawer-section__header:hover {
  background: var(--color-bg-elevated);
}

.drawer-section__chevron {
  display: inline-flex;
  align-items: center;
  transition: transform var(--duration-base) var(--ease-out);
  color: var(--color-text-muted);
  flex-shrink: 0;
}

/* Closed: rotate -90deg so the chevron points right (collapsed).
   Open: default downward orientation. */
.drawer-section__chevron--closed {
  transform: rotate(-90deg);
}

.drawer-section__icon {
  display: inline-flex;
  align-items: center;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.drawer-section__label {
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  flex: 1;
  min-width: 0;
}

.drawer-section__count {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  flex-shrink: 0;
}

/* Live indicator chip — spinner + elapsed time. Only visible while
   the worker is running (live === true). */
.drawer-section__live-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 1px 6px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--color-tool-shell) 18%, transparent);
  color: var(--color-tool-shell);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  flex-shrink: 0;
}

.drawer-section__live-spinner {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  border: 1.5px solid currentColor;
  border-top-color: transparent;
  animation: drawer-section-spin 0.8s linear infinite;
}

@keyframes drawer-section-spin {
  to {
    transform: rotate(360deg);
  }
}

.drawer-section__live-text {
  /* visually centered next to the spinner */
}

/* Terminal chip — "✓ Completed · X.Xs". Shown when the worker has
   finished and `finalDurationMs` is provided. */
.drawer-section__final-chip {
  display: inline-flex;
  align-items: center;
  padding: 1px 6px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--color-tool-write) 18%, transparent);
  color: var(--color-tool-write);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  flex-shrink: 0;
}

.drawer-section__body {
  padding: 8px 12px 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  border-top: 1px solid var(--color-bg-border);
}

/* When the body is a <details>-style ThinkingBlock (no card chrome),
   strip the section's inner padding so the block bleeds to the
   section's edges. Applied via :first-child / :last-child rules
   so a single child fills the body cleanly. */
.drawer-section__body > :first-child {
  margin-top: 0;
}

.drawer-section__body > :last-child {
  margin-bottom: 0;
}
</style>
