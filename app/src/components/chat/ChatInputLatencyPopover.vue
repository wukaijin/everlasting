<script setup lang="ts">
// ChatInputLatencyPopover — F5 follow-up: LLM cumulative latency chip
// + clickable popover showing per-turn TTFB / Gen / Total breakdown.
//
// Extracted from `ChatInput.vue` (split refactor 2026-06-23) into a
// self-contained chip + popover component. Hand-rolled popover pattern
// per `.trellis/spec/frontend/popover-pattern.md` — identical shape to
// `ModelSelect.vue` / `ModeSelect.vue` / `TriggerMenu.vue`:
//   - `open` ref + `root` ref (wrap trigger + popover)
//   - `onDocumentClick` closes on outside-click (root.contains)
//   - `onKeyDown` closes on Esc
//   - popover geometry: `bottom: calc(100% + 4px)` (chip sits at the
//     bottom of the chat panel — opening down would clip the viewport)
//   - 150ms / 100ms enter/leave Transition (fade + slide up)
//   - `aria-haspopup="dialog"` + `aria-expanded` on trigger
//
// Why a separate component (vs. inline in ChatInput):
//   - The popover has ~150 lines of template + style + logic that don't
//     belong in the chat composer (which is dominated by CodeMirror
//     wiring). Splitting lowers ChatInput.vue from 1834 → ~1000 lines.
//   - 0 store import — props-only contract makes this trivially
//     unit-testable (LatencyPopover.test.ts mounts with synthetic
//     `totalMs` + `turns`, no Pinia / Tauri mocks).
//   - The popover body is presentation-only (no IPC, no dispatch); the
//     chip is a pure render of `totalMs` + `turns`.

import { computed, onUnmounted, ref } from "vue";
import Icon from "../Icon.vue";
import { abbreviateDuration } from "../../utils/duration";
import type { LatencyInfo } from "../../stores/chat.types";

const props = defineProps<{
  /** Cumulative Σ totalMs across all recorded turns. `null` for
   *  pre-F5 sessions or sessions with no recorded turns. The chip
   *  renders "—" when this is null; the popover summary row
   *  mirrors the same null vs number distinction. */
  totalMs: number | null;
  /** Per-turn list. `null` = "no session active" (chip hidden by
   *  parent — this component itself always renders when mounted).
   *  `[]` = active session but no turns recorded yet (chip renders
   *  "—"; popover shows the "本次 session 还没有 LLM 耗时数据"
   *  empty footer). */
  turns: LatencyInfo[] | null;
}>();

// === Popover state (hand-rolled per .trellis/spec/frontend/popover-pattern.md) ===

const open = ref(false);
const root = ref<HTMLElement | null>(null);

function toggle() {
  open.value = !open.value;
}

/** Click outside the popover root closes it. Mirrors `ModelSelect`
 *  and the worktree dropdown. */
function onDocumentClick(e: MouseEvent) {
  if (!open.value) return;
  const target = e.target as Node | null;
  if (root.value && target && !root.value.contains(target)) {
    open.value = false;
  }
}

/** Esc closes the popover. Bound on `window` because the trigger
 *  button may not have focus when the popover is open. */
function onKeyDown(e: KeyboardEvent) {
  if (e.key === "Escape" && open.value) {
    open.value = false;
  }
}

if (typeof document !== "undefined") {
  document.addEventListener("click", onDocumentClick);
}
if (typeof window !== "undefined") {
  window.addEventListener("keydown", onKeyDown);
}
onUnmounted(() => {
  if (typeof document !== "undefined") {
    document.removeEventListener("click", onDocumentClick);
  }
  if (typeof window !== "undefined") {
    window.removeEventListener("keydown", onKeyDown);
  }
});

// === Derived display values (pure, no store dependency) ===

/** Total ms formatted for the chip + summary "累计" row. */
const totalLabel = computed(() =>
  props.totalMs !== null ? abbreviateDuration(props.totalMs) : "—",
);

/** Number of recorded turns (drives the "轮次" row + the list). */
const turnCount = computed(() => props.turns?.length ?? 0);

/** Average totalMs across recorded turns. Computed live from the
 *  per-turn list (no separate counter needed). Returns `null`
 *  when no turns have been recorded. */
const average = computed<number | null>(() => {
  const t = props.turns;
  if (!t || t.length === 0) return null;
  let sum = 0;
  let count = 0;
  for (const x of t) {
    if (typeof x.totalMs === "number") {
      sum += x.totalMs;
      count++;
    }
  }
  return count > 0 ? sum / count : null;
});

const averageLabel = computed(() =>
  average.value !== null ? abbreviateDuration(average.value) : "—",
);
</script>

<template>
  <div ref="root" class="chat-input__latency">
    <button
      type="button"
      class="chat-input__latency-chip"
      :class="{ 'chat-input__latency-chip--open': open }"
      :aria-haspopup="'dialog'"
      :aria-expanded="open"
      :title="
        totalMs !== null
          ? '点击查看本次 session LLM 累计耗时明细'
          : '本次 session 还没有 LLM 耗时数据'
      "
      @click="toggle"
    >
      <Icon name="clock" :size="11" />
      <span class="chat-input__latency-label">LLM</span>
      <span class="chat-input__latency-value">{{ totalLabel }}</span>
    </button>
    <Transition name="chat-input-latency-popover">
      <div
        v-if="open"
        class="chat-input__latency-popover"
        role="dialog"
        aria-label="LLM 累计耗时明细"
      >
        <div class="chat-input__latency-popover-header">
          <Icon name="clock" :size="11" />
          <span>本次 session LLM 累计耗时</span>
        </div>
        <div class="chat-input__latency-popover-summary">
          <div class="chat-input__latency-popover-row">
            <span>累计</span>
            <span class="chat-input__latency-popover-strong">{{ totalLabel }}</span>
          </div>
          <div class="chat-input__latency-popover-row">
            <span>轮次</span>
            <span>{{ turnCount }}</span>
          </div>
          <div class="chat-input__latency-popover-row">
            <span>平均</span>
            <span>{{ averageLabel }}</span>
          </div>
        </div>
        <div
          v-if="turns && turns.length > 0"
          class="chat-input__latency-popover-list"
        >
          <div
            v-for="(turn, i) in turns"
            :key="i"
            class="chat-input__latency-popover-turn"
          >
            <div class="chat-input__latency-popover-turn-head">
              <span>turn {{ i + 1 }}</span>
              <span class="chat-input__latency-popover-strong">
                {{ turn.totalMs !== undefined ? abbreviateDuration(turn.totalMs) : "—" }}
              </span>
            </div>
            <div class="chat-input__latency-popover-turn-detail">
              <span>TTFB</span>
              <span>{{ turn.ttfbMs !== undefined ? abbreviateDuration(turn.ttfbMs) : "—" }}</span>
            </div>
            <div class="chat-input__latency-popover-turn-detail">
              <span>gen</span>
              <span>{{ turn.genMs !== undefined ? abbreviateDuration(turn.genMs) : "—" }}</span>
            </div>
          </div>
        </div>
        <div v-else class="chat-input__latency-popover-empty">
          本次 session 还没有 LLM 耗时数据
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.chat-input__latency {
  position: relative;
  display: inline-flex;
  flex-shrink: 0;
}

.chat-input__latency-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--color-text-muted);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  cursor: pointer;
  user-select: none;
  font: inherit;
  font-family: var(--font-mono);
  font-size: 11px;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}

.chat-input__latency-chip:hover {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-text-primary);
}

.chat-input__latency-chip--open {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-text-primary);
}

.chat-input__latency-label {
  color: var(--color-text-secondary);
}

.chat-input__latency-value {
  color: var(--color-text-primary);
  font-weight: 600;
}

/* Popover floats above the chip (chip sits at the bottom of the
   chat panel; opening down would clip the viewport). Width fits the
   longest "0.0s · 0.0s · 0.0s" line without overflow; the list
   area scrolls when there are too many turns. */
.chat-input__latency-popover {
  position: absolute;
  bottom: calc(100% + 4px);
  top: auto;
  left: 0;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  min-width: 220px;
  max-width: 280px;
  max-height: 320px;
  z-index: 200;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  font-size: 11px;
  color: var(--color-text-primary);
  font-family: var(--font-mono);
}

.chat-input__latency-popover-header {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 10px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--color-text-muted);
  padding-bottom: 4px;
  border-bottom: 1px solid var(--color-bg-border);
}

.chat-input__latency-popover-summary {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.chat-input__latency-popover-row {
  display: flex;
  justify-content: space-between;
  gap: 16px;
}

.chat-input__latency-popover-row > span:first-child {
  color: var(--color-text-secondary);
}

.chat-input__latency-popover-strong {
  color: var(--color-text-primary);
  font-weight: 600;
}

.chat-input__latency-popover-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  overflow-y: auto;
  max-height: 200px;
  padding-top: 4px;
  border-top: 1px solid var(--color-bg-border);
}

.chat-input__latency-popover-turn {
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 4px 0;
}

.chat-input__latency-popover-turn + .chat-input__latency-popover-turn {
  border-top: 1px dashed var(--color-bg-border);
}

.chat-input__latency-popover-turn-head {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  font-weight: 500;
}

.chat-input__latency-popover-turn-detail {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  color: var(--color-text-secondary);
  padding-left: 8px;
}

.chat-input__latency-popover-empty {
  color: var(--color-text-muted);
  text-align: center;
  padding: 6px 0;
}

/* Open/close animation. The popover opens UP, so it slides
   from translateY(4px) (slightly below the final position) up
   into place. Exit reverses. */
.chat-input-latency-popover-enter-active,
.chat-input-latency-popover-leave-active {
  transition: opacity 150ms ease-out, transform 150ms ease-out;
  transform-origin: bottom left;
}

.chat-input-latency-popover-enter-from,
.chat-input-latency-popover-leave-to {
  opacity: 0;
  transform: translateY(4px);
}

.chat-input-latency-popover-leave-active {
  transition-duration: 100ms;
  transition-timing-function: ease-in;
}
</style>
