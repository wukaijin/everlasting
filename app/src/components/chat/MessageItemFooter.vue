<script setup lang="ts">
// MessageItemFooter — the bottom-of-bubble footer for a chat
// message row. Renders (in order):
//   1. Error row (if `error` is set) — red text with a warn icon
//      + an OPTIONAL `↻ 重试` button (visible when
//      `categoryRetryable(category)` resolves true).
//   2. F5 latency chip (assistant only, when not streaming and
//      `latency.totalMs` is set) — hover surfaces the three-line
//      breakdown (TTFB / 生成 / 端到端) via reka-ui Tooltip.
//
// This is the second of the two children extracted from
// `MessageItem.vue` on 2026-06-23. Per the task's ADR-2
// decision, the (edited) label stays in the parent — it sits
// inside the bubble div, visually distinct from the error /
// latency chips that hang below the bubble. The footer
// therefore has three visual surfaces (error row, retry button,
// latency chip).
//
// Why pure presentation (no store import for retry):
//   - Single source of truth: the parent (`MessageItem.vue`)
//     owns `chatStore` (provides `retryChat`) and decides when
//     the retry button is enabled.
//   - Testable in isolation: vitest can drive this component
//     with hand-built props and assert on the rendered DOM
//     without spinning up Pinia. See
//     `app/src/components/chat/MessageItemFooter.test.ts`.
//   - Mirrors the `<MessageActionsMenu>` and `<MessageItemEdit>`
//     conventions (parent orchestrates, child renders).
//
// A5 R2 (2026-07-17) retry button UX:
//   - Visible iff `categoryRetryable(error.category)` is true
//     (RateLimit / Server / Network). Auth / InvalidRequest /
//     no-category rows show no button.
//   - Click → `emit('retry', messageSeq)`; the parent
//     (`MessageItem.vue`) routes to `chatStore.retryChat`.
//   - Loading state: while the parent has a retry stream in
//     flight (`parent:streaming` reaches us via the
//     `retry-loading` prop), the button is disabled and shows
//     "重试中...". The parent toggles the prop via its own
//     watcher on the session's active stream.

import { computed } from "vue";
import {
  TooltipProvider,
  TooltipRoot,
  TooltipTrigger,
  TooltipPortal,
  TooltipContent,
  TooltipArrow,
} from "reka-ui";
import { abbreviateDuration } from "../../utils/duration";
import { categoryRetryable } from "../../utils/error";
import Icon from "../Icon.vue";

const props = withDefaults(
  defineProps<{
    /** The message's role. Latency is shown only for assistant
     *  rows (user rows have no measurable turn latency). The
     *  `error` row shows for either role. */
    role: "user" | "assistant";
    /** True while a chat stream is in flight. Hides the latency
     *  chip (the chip is in flux; the user is reading the
     *  bubble, not the footer). Does NOT hide the error row —
     *  a streaming turn can still surface an error before
     *  the latency lands. Also disables the retry button
     *  (defense against mid-stream double-click). */
    streaming: boolean;
    /** Per-message latency breakdown. Missing for pre-F5
     *  rows and for user-role / system-event rows. The chip
     *  renders only when `latency.totalMs` is set and the row
     *  is an assistant turn not currently streaming. */
    latency?: {
      ttfbMs?: number;
      genMs?: number;
      totalMs?: number;
    };
    /** Per-message error. Renders a small red row above the
     *  latency chip (or in place of it, if no latency is
     *  available). Missing for non-error rows. */
    error?: { message: string; category?: string };
    /** A5 R2 (2026-07-17): the message's seq. Used by the retry
     *  button's emit so the parent can call
     *  `chatStore.retryChat(sessionId, messageSeq)` without
     *  re-reading the message. Missing for tool-only rows
     *  that the row's footer wouldn't render for anyway. */
    messageSeq?: number;
    /** A5 R2: while a retry stream is in flight for THIS
     *  message, the parent sets this to true to flip the
     *  retry button into its loading state ("重试中..." +
     *  disabled). The parent uses its own `currentSessionId`
     *  watcher to clear it on `done` / `error`. */
    retryLoading?: boolean;
  }>(),
  {
    streaming: false,
    latency: undefined,
    error: undefined,
    messageSeq: undefined,
    retryLoading: false,
  },
);

const emit = defineEmits<{
  /** A5 R2: fired when the user clicks the `↻ 重试` button.
   *  Payload is the row's `messageSeq` so the parent can call
   *  `chatStore.retryChat(sessionId, messageSeq)` directly. */
  (e: "retry", messageSeq: number): void;
}>();

/** A5 R2: whether the retry button renders at all. True iff
 *  - the row carries an error, AND
 *  - the error's `category` is one of RateLimit/Server/Network
 *    (mirrors backend `AppError::retryable()` default), AND
 *  - the row is not mid-stream (defense: a stale error from
 *    before a retry that just landed should not be retry-able
 *    mid-stream). */
const canRetry = computed<boolean>(
  () =>
    !!props.error &&
    categoryRetryable(props.error.category) &&
    !props.streaming,
);

/** F5 chip visibility. Renders the bottom-right of the
 *  assistant bubble with a 1-decimal abbreviation of
 *  `totalMs` (e.g. "3.2s"). Hidden for:
 *  - user-role messages (only assistant turns have a latency)
 *  - messages without a `latency` object (pre-F5 rows)
 *  - messages mid-stream (`streaming` true; the chip is in
 *    flux and the user is reading the bubble, not the footer)
 *  - rows where `latency.totalMs` is not a number (a cancel
 *    path that left ttfbMs / genMs null while totalMs is
 *    also missing — show "—" in place of the chip) */
const showLatency = computed<boolean>(
  () =>
    props.role === "assistant" &&
    !props.streaming &&
    !!props.latency &&
    typeof props.latency.totalMs === "number",
);

/** The chip's visible label. Falls back to "—" when no
 *  `totalMs` is present (this branch is unreachable in
 *  practice because `showLatency` gates the render, but
 *  the function is exposed for any future caller that
 *  wants the formatted value directly). */
const latencyTotalLabel = computed<string>(() => {
  const t = props.latency?.totalMs;
  if (typeof t !== "number") return "—";
  return abbreviateDuration(t);
});

/** The three lines shown in the hover tooltip. Each is
 *  omitted (and the row hidden) when the value is
 *  undefined — the cancel / error path leaves ttfbMs /
 *  genMs null while totalMs is set, and the UI shows only
 *  the available rows. */
const latencyRows = computed<Array<{ label: string; value: string }>>(() => {
  const lat = props.latency;
  if (!lat) return [];
  const rows: Array<{ label: string; value: string }> = [];
  if (typeof lat.ttfbMs === "number") {
    rows.push({ label: "TTFB", value: abbreviateDuration(lat.ttfbMs) });
  }
  if (typeof lat.genMs === "number") {
    rows.push({ label: "生成", value: abbreviateDuration(lat.genMs) });
  }
  if (typeof lat.totalMs === "number") {
    rows.push({ label: "端到端", value: abbreviateDuration(lat.totalMs) });
  }
  return rows;
});

/** A5 R2: the retry button's text. Switches between the
 *  default label and the loading-state label when the parent
 *  flips `retryLoading`. */
const retryButtonLabel = computed<string>(() =>
  props.retryLoading ? "重试中..." : "↻ 重试",
);

/** A5 R2: the retry button's click handler. Emits the row's
 *  seq to the parent for orchestrating the
 *  `chatStore.retryChat` call. Defensive: if `messageSeq`
 *  is missing (which shouldn't happen for a row with an
 *  error, but defensively) we log + skip. */
function onRetryClick(): void {
  if (typeof props.messageSeq !== "number") return;
  if (props.retryLoading) return;
  emit("retry", props.messageSeq);
}
</script>

<template>
  <!--
    Error footer. Sits at the top of the footer block (above
    the latency chip when both are present) so the user sees
    the failure first, the latency second. The text is the
    `error.message` string from the ChatMessage.

    A5 R2 (2026-07-17): the `↻ 重试` button renders next to
    the error text when `canRetry` is true. The button uses
    inline-flex spacing; clicking it emits `retry(seq)` which
    the parent (MessageItem.vue) routes to
    `chatStore.retryChat(sessionId, messageSeq)`.
  -->
  <div
    v-if="error"
    class="msg__error"
    role="alert"
    data-testid="msg-error-row"
  >
    <Icon name="warn" :size="12" icon-class="msg__error-icon" />
    <span class="msg__error-text">{{ error.message }}</span>
    <button
      v-if="canRetry"
      type="button"
      class="msg__error-retry"
      :disabled="!!retryLoading"
      :data-testid="'msg-retry-button'"
      @click="onRetryClick"
    >
      {{ retryButtonLabel }}
    </button>
  </div>

  <!--
    F5 (LLM Latency Tracking): per-message latency chip. The
    chip is the TooltipTrigger; the tooltip content is the
    three-row breakdown (TTFB / 生成 / 端到端). The `delay-duration`
    of 150ms defers the open so quick mouse-passes don't
    trigger (matches the project-wide Tooltip convention
    documented in `.trellis/spec/frontend/reka-ui-usage.md`).
  -->
  <TooltipProvider v-if="showLatency">
    <TooltipRoot :delay-duration="150">
      <TooltipTrigger as-child>
        <span
          class="msg__latency"
          data-testid="msg-latency-chip"
        >{{ latencyTotalLabel }}</span>
      </TooltipTrigger>
      <TooltipPortal>
        <TooltipContent
          class="msg__latency-tooltip"
          :side-offset="4"
        >
          <div
            v-for="row in latencyRows"
            :key="row.label"
            class="msg__latency-tooltip-row"
            :data-testid="`msg-latency-tooltip-row-${row.label}`"
          >
            <span>{{ row.label }}</span>
            <span>{{ row.value }}</span>
          </div>
          <TooltipArrow class="msg__latency-tooltip-arrow" :size="6" />
        </TooltipContent>
      </TooltipPortal>
    </TooltipRoot>
  </TooltipProvider>
</template>

<style scoped>
.msg__error {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-top: 4px;
  padding: 0 14px;
  font-size: var(--text-sm);
  color: var(--color-tool-error);
}

.msg__error-icon {
  flex-shrink: 0;
}

.msg__error-text {
  /* 把 retry button 推到文本末尾靠右时,保留 text 的 ellipsis 行为 */
  white-space: pre-wrap;
  word-break: break-word;
}

/* A5 R2 (2026-07-17): retry button. Inline 在错误文本后,小尺寸 +
   8px 间距;复用工具行按钮风格(无新 design token)。disabled 态
   颜色退到 muted + cursor not-allowed。 */
.msg__error-retry {
  display: inline-flex;
  align-items: center;
  gap: 0;
  margin-left: 6px;
  padding: 2px 8px;
  background: transparent;
  border: 1px solid color-mix(in srgb, var(--color-tool-error) 50%, transparent);
  border-radius: var(--radius-sm);
  color: var(--color-tool-error);
  font-size: var(--text-xs);
  font-family: inherit;
  cursor: pointer;
  user-select: none;
  transition:
    background-color var(--duration-fast) var(--ease-out),
    border-color var(--duration-fast) var(--ease-out);
}
.msg__error-retry:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-tool-error) 10%, transparent);
  border-color: var(--color-tool-error);
}
.msg__error-retry:disabled {
  cursor: not-allowed;
  opacity: 0.6;
}

/* F5 (LLM Latency Tracking): per-message latency chip. Sits
   at the bottom-right of the assistant bubble. The chip
   itself is the TooltipTrigger; the tooltip content is the
   three-row breakdown (TTFB / 生成 / 端到端).

   Visual decisions:
   - 11px mono font to match the existing density (token
     usage chip in ChatInput uses the same).
   - 0.5px muted color so it doesn't fight the bubble for
     attention — the user sees it on glance but isn't
     pulled in.
   - Right-aligned via `align-self: flex-end` (the parent
     `li.msg` is `display: flex; flex-direction: column`,
     so the chip is the rightmost element of the bubble
     column). */
.msg__latency {
  display: inline-flex;
  align-items: center;
  align-self: flex-end;
  margin-top: 4px;
  padding: 0 6px;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  font-weight: var(--weight-semibold);
  color: var(--color-text-muted);
  cursor: help;
  border-radius: var(--radius-sm);
  user-select: none;
}

.msg__latency:hover {
  color: var(--color-text-secondary);
}

/* Tooltip content (reka-ui `TooltipContent` portal to body
   — must use :deep() per `.trellis/spec/frontend/reka-ui-usage.md`
   gotcha). The popover floats above the chip (default
   side is "top"). 11px mono, single-column row layout. */
:deep(.msg__latency-tooltip) {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: 6px 10px;
  min-width: 140px;
  z-index: 3000;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  animation: msg-latency-tooltip-enter var(--duration-base) var(--ease-out);
}

:deep(.msg__latency-tooltip-row) {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  padding: 1px 0;
}

:deep(.msg__latency-tooltip-row span:first-child) {
  color: var(--color-text-secondary);
}

:deep(.msg__latency-tooltip-arrow) {
  fill: var(--color-bg-surface);
  stroke: var(--color-bg-border);
}

@keyframes msg-latency-tooltip-enter {
  from {
    opacity: 0;
    transform: translateY(2px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}
</style>
