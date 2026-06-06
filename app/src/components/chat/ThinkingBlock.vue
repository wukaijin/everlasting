<script setup lang="ts">
// ThinkingBlock — collapsible <details> element for the model's
// extended thinking text. Per spike-003 the left bar is a 3px
// violet accent (--color-tool-thinking) and the block uses the
// elevated surface for the body so it visually separates from
// the chat bubble.
//
// The block is collapsed by default — the header shows the
// estimated token count so the user can decide whether to expand.
//
// D6 polish (expanded body):
//   - Monospace body via `var(--font-mono)`, 12.5px, line-height 1.6
//   - Background nests one level deeper (`var(--color-bg-app)`) so
//     the body visually "drops in" from the summary chip
//   - Padding 12px 14px (matches the card-content grid)
//   - 1px border on sides + bottom (no top — connects to summary)
//   - max-height 360px, overflow-y: auto, custom thin scrollbar
//   - D6 also replaces the 💭 emoji summary icon with a heroicon.

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
  margin-bottom: 6px;
  max-width: 100%;
  border-left: 3px solid var(--color-tool-thinking);
  border-radius: 0 6px 6px 0;
  padding-left: 0;
}

.thinking__summary {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 999px;
  font-size: 11px;
  color: var(--color-text-secondary);
  cursor: pointer;
  user-select: none;
  list-style: none;
  font-family: var(--font-mono);
  transition: background 0.1s, border-color 0.1s;
  margin-left: 4px;
}

.thinking__summary::-webkit-details-marker {
  display: none;
}

.thinking__summary:hover {
  background: var(--color-bg-surface);
  border-color: var(--color-tool-thinking);
}

.thinking[open] .thinking__summary {
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
  border-bottom-color: transparent;
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
   reads as "inside" the summary chip. */
.thinking__body {
  margin: 0;
  padding: 12px 14px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-top: none;
  border-radius: 0 0 6px 6px;
  white-space: pre-wrap;
  word-break: break-word;
  font-size: 12.5px;
  line-height: 1.6;
  color: var(--color-text-secondary);
  font-family: var(--font-mono);
  max-height: 360px;
  overflow-y: auto;
  margin-left: 4px;
  /* Match the rounded corner on the right edge of the bar so the
     body sits flush with the violet accent. */
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
