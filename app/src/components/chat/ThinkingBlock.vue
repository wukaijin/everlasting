<script setup lang="ts">
// ThinkingBlock — collapsible <details> element for the model's
// extended thinking text. Per spike-003 the left bar is a 3px
// violet accent (--color-tool-thinking). The block is collapsed by
// default — the header shows the estimated token count so the user
// can decide whether to expand.
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
import type { ThinkingBlockInfo } from "../../stores/chat";
import {
  thinkingDisplayText,
  estimateThinkingTokens,
} from "../../utils/messageFormat";
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
}>();

const text = computed(() => thinkingDisplayText(props.blocks));
const tokens = computed(() => estimateThinkingTokens(props.blocks));
</script>

<template>
  <details class="thinking">
    <summary class="thinking__summary">
      <span class="thinking__icon" aria-hidden="true">
        <Icon name="thinking" :size="12" />
      </span>
      <span>Thought for {{ tokens }} tokens</span>
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
  margin-bottom: 6px;
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
  font-size: 11px;
  color: var(--color-text-secondary);
  cursor: pointer;
  user-select: none;
  list-style: none;
  font-family: var(--font-mono);
  transition: background 0.1s;
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
  font-weight: 500;
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
