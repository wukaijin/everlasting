<script setup lang="ts">
// ChatInputHintRow — A4 + F5 + PR5: the bottom hint row of the
// ChatInput composer.
//
// Layout (locked, see ChatInput.vue PR5 header comment):
// - LEFT: LLM cumulative latency chip (clock icon + "Σ 1.2s" /
//   "—") with clickable popover breaking the running total into a
//   per-turn TTFB / Gen / Total list. Rendered via
//   `<ChatInputLatencyPopover>` (0 store import — props-only).
//   **Hidden when `currentSessionId` is null** (matches the A4
//   token chip's "no session → don't render" rule).
// - CENTER: per-session token usage chip with reka-ui `Tooltip`
//   hover-breakdown (input / cache_read / cache_creation / output).
//   Color thresholds: 50% yellow, 75% red; rendered inline as a
//   `chat-input__token-usage` span with `--{ok|warn|alert}`
//   modifier.
// - RIGHT: model picker popover (`<ModelSelect>` — opens UP).
//
// Extracted from `ChatInput.vue` (split refactor 2026-06-23). All
// `chat-input__hint*` + `chat-input__token*` CSS rules moved here.
// Composable-vs-component division: the latency chip + popover
// body now lives in `<ChatInputLatencyPopover>`; the parent (this
// file) only owns the "no session → don't render" gate and embeds
// the chip.
//
// **0 store import** — props only (tokenUsage / contextWindow /
// usageLevel / currentSessionId + totalMs / turns for the latency
// sub-component). Parent computes usageLevel from `tokenUsage`
// and `contextWindow` so this component is testable with synthetic
// props.
//
// CSS gotcha: `TooltipContent` portals to `<body>` via reka-ui's
// `TooltipPortal`. Per `.trellis/spec/frontend/reka-ui-usage.md`,
// `<style scoped>` selectors that target the portal child must
// be wrapped in `:deep(...)`. Vue 3.5 propagates `data-v-xxx` to
// portal children empirically, so plain scoped works too — but
// `:deep()` is the safe default (matches PermissionModal's
// defensive wrap style).

import { TooltipProvider, TooltipRoot, TooltipTrigger, TooltipPortal, TooltipContent, TooltipArrow } from "reka-ui";
import ChatInputLatencyPopover from "./ChatInputLatencyPopover.vue";
import ModelSelect from "./ModelSelect.vue";
import { abbreviateTokens, type TokenUsageLevel } from "../../utils/tokenUsage";
import type { LatencyInfo, SessionTokenUsage } from "../../stores/chat.types";

defineProps<{
  /** Per-session cumulative token usage. `null` = pre-A4 session or
   *  no usage recorded yet → render "—" + "升级前未统计" tooltip.
   *  Mirrors the Rust `SessionTokenUsage` shape exactly. */
  tokenUsage: SessionTokenUsage | null;
  /** The denominator for the percentage chip ("X% / 200K"). Pulled
   *  from the current model's `contextWindow` field in the catalog;
   *  the parent computes this from `modelsStore.defaultModel`. */
  contextWindow: number;
  /** Pre-computed color band derived from
   *  `tokenUsage.input_tokens / contextWindow`. The parent runs
   *  `tokenUsageLevel(pct)` so the boundary rules (49/50/74/75) live
   *  in `utils/tokenUsage.ts` and stay unit-testable. `null` for
   *  sessions without usage (renders plain "—", no color). */
  usageLevel: TokenUsageLevel | null;
  /** Active session id. When `null` the latency chip is hidden
   *  (no session = nothing to time); the token chip renders in
   *  empty state; the ModelSelect always renders. */
  currentSessionId: string | null;
  /** Cumulative Σ totalMs across all recorded turns. `null` for
   *  pre-F5 sessions / no recorded turns → latency chip renders "—".
   *  Forwarded to `<ChatInputLatencyPopover>`. */
  totalMs: number | null;
  /** Per-turn list for the latency popover. `null` = no session,
   *  `[]` = active session but no turns recorded yet. Forwarded
   *  to `<ChatInputLatencyPopover>`. */
  turns: LatencyInfo[] | null;
}>();
</script>

<template>
  <div class="chat-input__hint">
    <!-- F5 follow-up: LLM cumulative latency chip (LEFT).
         Rendered via the dedicated sub-component. Hidden when
         no session is active (matches the A4 token chip's
         "no session → don't render" rule). -->
    <ChatInputLatencyPopover
      v-if="currentSessionId"
      :total-ms="totalMs"
      :turns="turns"
    />
    <!-- A4: token usage chip. Render-mode depends on
         whether the session has accumulated any usage:
         - null → "—" with the "升级前未统计" tooltip
         - non-null → the percentage line; tooltip breaks
           the four counters down.
         Color thresholds are 50% (yellow) and 75% (red);
         see `usageLevel` prop. -->
    <TooltipProvider>
      <TooltipRoot>
        <TooltipTrigger as-child>
          <span
            class="chat-input__token-usage"
            :class="{
              [`chat-input__token-usage--${usageLevel}`]: usageLevel,
            }"
          >
            <template v-if="tokenUsage">
              {{ abbreviateTokens(tokenUsage.input_tokens) }}
              ·
              {{
                Math.min(
                  100,
                  Math.round(
                    (tokenUsage.input_tokens / contextWindow) * 100,
                  ),
                )
              }}% / {{ abbreviateTokens(contextWindow) }}
            </template>
            <template v-else>—</template>
          </span>
        </TooltipTrigger>
        <TooltipPortal>
          <TooltipContent class="chat-input__token-tooltip" :side-offset="6">
            <template v-if="tokenUsage">
              <div class="chat-input__token-tooltip-row">
                <span>input</span>
                <span>{{ abbreviateTokens(tokenUsage.input_tokens) }}</span>
              </div>
              <div class="chat-input__token-tooltip-row">
                <span>cache_read</span>
                <span>{{ abbreviateTokens(tokenUsage.cache_read_input_tokens) }}</span>
              </div>
              <div class="chat-input__token-tooltip-row">
                <span>cache_creation</span>
                <span>{{ abbreviateTokens(tokenUsage.cache_creation_input_tokens) }}</span>
              </div>
              <div class="chat-input__token-tooltip-row">
                <span>output</span>
                <span>{{ abbreviateTokens(tokenUsage.output_tokens) }}</span>
              </div>
            </template>
            <template v-else>
              <div class="chat-input__token-tooltip-empty">升级前未统计</div>
            </template>
            <TooltipArrow class="chat-input__token-tooltip-arrow" :size="6" />
          </TooltipContent>
        </TooltipPortal>
      </TooltipRoot>
    </TooltipProvider>
    <!-- PR5: model picker popover (upward-opening) attached to
         the right edge of the hint row. Replaces the
         bottom-of-content `StatusBar` from PR4. -->
    <ModelSelect />
  </div>
</template>

<style scoped>
.chat-input__hint {
  margin-top: 8px;
  padding: 0 6px;
  font-size: 11px;
  color: var(--color-text-muted);
  user-select: none;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

/* A4 (Token Usage Tracking): the per-session token usage
   chip in the hint row. The chip is a TooltipTrigger
   (reka-ui); the trigger itself has no role, the span is
   the visual target. The three color states map to the
   threshold ladder:
   - ok (0-49%): subtle green tint, still readable on dark
   - warn (50-74%): amber, calls attention
   - alert (75%+): red, stops the eye */
.chat-input__token-usage {
  display: inline-flex;
  align-items: center;
  padding: 0 6px;
  font-size: 11px;
  font-family: var(--font-mono);
  white-space: nowrap;
  cursor: help;
  border-radius: 4px;
  color: var(--color-text-muted);
  transition: color 0.15s;
  user-select: none;
}

.chat-input__token-usage--ok {
  color: #4ade80; /* green-400 — readable on dark, doesn't shout */
}

.chat-input__token-usage--warn {
  color: #fbbf24; /* amber-400 — matches --color-tool-shell family */
}

.chat-input__token-usage--alert {
  color: var(--color-tool-error);
}

/* Tooltip content (reka-ui `TooltipContent` portal to body
   — must use :deep() per `.trellis/spec/frontend/reka-ui-usage.md`
   gotcha). The popover floats above the trigger (default
   side is "top" since the chat input is at the bottom of the
   viewport). */
:deep(.chat-input__token-tooltip) {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  padding: 8px 10px;
  min-width: 180px;
  z-index: 3000;
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  animation: chat-input-tooltip-enter 150ms ease-out;
}

:deep(.chat-input__token-tooltip-row) {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  padding: 2px 0;
}

:deep(.chat-input__token-tooltip-row span:first-child) {
  color: var(--color-text-secondary);
}

:deep(.chat-input__token-tooltip-empty) {
  color: var(--color-text-muted);
  text-align: center;
  padding: 2px 0;
}

:deep(.chat-input__token-tooltip-arrow) {
  fill: var(--color-bg-surface);
  stroke: var(--color-bg-border);
}

@keyframes chat-input-tooltip-enter {
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
