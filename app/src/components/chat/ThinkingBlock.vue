<script setup lang="ts">
// ThinkingBlock — collapsible <details> element for the model's
// extended thinking text. Per spike-003 the left bar is a 3px
// violet accent (--color-tool-thinking). The block is collapsed by
// default — the header shows how long the thinking phase took
// (e.g. "Thought for 1.4s") so the user can decide whether to
// expand. F5 follow-up: the previous "X tokens" label was
// replaced with an actual wall-clock measurement captured by the
// streaming `streamController` (see `RequestState.thinkingStartedAt`
// / `thinkingDurationMs`); the token count is no longer surfaced
// here because the user's "did this take a long time?" question
// is answered by time, not by content size.
//
// Bug-fix v3 layout (the previous "pill summary + detached body"
// combo left a visible seam and made the violet 3px bar look like it
// wasn't connected to anything). We now render a single unified card:
//   - Root <details> is the card, with the 3px violet left bar and
//     rounded right corners; `overflow: hidden` clips children to
//     the card so the body never sticks out past the radius.
//   - The summary is a full-width header band, not a pill, so its
//     left edge sits flush against the violet bar (no more 4px
//     inset gap). When expanded, a 1px hairline divider separates
//     the header from the body.
//   - The body still nests one level deeper (`--color-bg-app`) so
//     the expanded block reads as "inside" the header — a
//     Linear-meets-Cursor style inline disclosure, not two separate
//     elements stuck together.

import { computed } from "vue";
import type { ThinkingBlockInfo } from "../../stores/chat.types";
import { thinkingDisplayText } from "../../utils/messageFormat";
import { abbreviateDuration } from "../../utils/duration";
import Icon from "../Icon.vue";

const props = defineProps<{
  blocks: ThinkingBlockInfo[];
  /** Whether the underlying message is currently streaming. When
   *  true and there's no visible text yet, the header shows a
   *  "streaming…" hint so the user knows the model is thinking. */
  streaming?: boolean;
  /** Show the "streaming…" hint independently of `streaming` —
   *  used when the model is producing thinking but no text yet. */
  showStreamingHint?: boolean;
  /** F5 follow-up: how long the model spent in the thinking
   *  phase for this turn (ms). Set on the assistant message
   *  by `streamController` from `RequestState.thinkingDurationMs`.
   *  `undefined` for messages that never entered the thinking
   *  phase, or after a page reload (in-memory only, no DB
   *  column) — the header falls back to "—" in that case. */
  thinkingDurationMs?: number;
}>();

const text = computed(() => thinkingDisplayText(props.blocks));
// F5 follow-up: the header label. Uses `abbreviateDuration`
// (same formatter as the F5 per-message latency chip) so the
// scale matches the rest of the chat: "1.4s", "32.4s", "1m 23s".
// Undefined → "—" (consistent with the F5 cumulative chip's
// "pre-F5 / no data" fallback; we don't show "0.0s" for
// "never thought" because that's misleading).
const headerLabel = computed(() => {
  if (typeof props.thinkingDurationMs === "number") {
    return `Thought for ${abbreviateDuration(props.thinkingDurationMs)}`;
  }
  return "Thought for —";
});
</script>

<template>
  <details class="thinking">
    <summary class="thinking__summary">
      <span class="thinking__icon" aria-hidden="true">
        <Icon name="thinking" :size="12" />
      </span>
      <span>{{ headerLabel }}</span>
      <span v-if="blocks.length > 1" class="thinking__count">
        · {{ blocks.length }} blocks
      </span>
      <span v-if="showStreamingHint" class="thinking__streaming">
        streaming…
      </span>
    </summary>
    <pre class="thinking__body">{{ text }}</pre>
  </details>
</template>

<style scoped>
.thinking {
  position: relative;
  margin-bottom: 0px;
  max-width: 100%;
  border-left: 3px solid var(--color-tool-thinking);
  border-radius: 0 6px 6px 0;
  overflow: hidden;
  background: var(--color-bg-app);
}

.thinking__summary {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  background: var(--color-bg-elevated);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  cursor: pointer;
  user-select: none;
  list-style: none;
  font-family: var(--font-mono);
  transition: background var(--duration-fast) var(--ease-out);
}

.thinking__summary::-webkit-details-marker {
  display: none;
}

.thinking__summary:hover {
  background: var(--color-bg-surface);
}

/* When expanded, a 1px hairline separates the header from the body
   so the two backgrounds don't blur into each other. */
.thinking[open] .thinking__summary {
  border-bottom: 1px solid var(--color-bg-border);
}

.thinking__icon {
  display: inline-flex;
  align-items: center;
  color: var(--color-tool-thinking);
}

.thinking__count {
  color: var(--color-text-muted);
}

.thinking__streaming {
  margin-left: 2px;
  color: var(--color-accent);
  font-weight: var(--weight-medium);
}

/* Expanded body: monospace, padded, capped height with thin
   dark scrollbar. Background nests one level deeper so the body
   reads as "inside" the summary header. */
.thinking__body {
  margin: 0;
  padding: 12px 14px;
  background: var(--color-bg-app);
  white-space: pre-wrap;
  word-break: break-word;
  font-size: 12.5px;
  line-height: 1.6;
  color: var(--color-text-secondary);
  font-family: var(--font-mono);
  max-height: 360px;
  overflow-y: auto;
  scrollbar-width: thin;
  scrollbar-color: var(--color-bg-border) transparent;
}

.thinking__body::-webkit-scrollbar {
  width: 6px;
}

.thinking__body::-webkit-scrollbar-thumb {
  background: var(--color-bg-border);
  border-radius: 3px;
}

.thinking__body::-webkit-scrollbar-track {
  background: transparent;
}
</style>
