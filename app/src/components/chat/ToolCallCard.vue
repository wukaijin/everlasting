<script setup lang="ts">
// ToolCallCard — a single tool invocation with its input / output
// blocks. Per spike-003 the card has a 3px left bar that switches
// color by tool name: read_file → cyan, write_file → emerald,
// shell → amber. The error state (the matching tool_result
// reports is_error) flips the bar to red and tints the card body.
//
// Header shows the tool name + status. "Input" details are
// collapsed by default (typical case is short); "Output" details
// are open by default when a result is present so the user can
// see the result immediately.

import { computed } from "vue";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat";
import {
  formatToolInput,
  truncateOutput,
  toolAccentVar,
} from "../../utils/messageFormat";

const props = defineProps<{
  call: ToolCallInfo;
  result?: ToolResultInfo;
}>();

const accent = computed(() => {
  if (props.result?.isError) return "var(--color-tool-error)";
  return toolAccentVar(props.call.name);
});

const isError = computed(() => !!props.result?.isError);
const hasResult = computed(() => !!props.result);

const status = computed<string>(() => {
  if (isError.value) return "✗ error";
  if (hasResult.value) return "✓ done";
  return "⏳ running…";
});
</script>

<template>
  <div
    :class="['tool-card', { 'tool-card--error': isError }]"
    :style="{ borderLeftColor: accent }"
  >
    <div class="tool-card__header">
      <span class="tool-card__name">{{ call.name }}</span>
      <span class="tool-card__status">{{ status }}</span>
    </div>
    <details class="tool-card__details">
      <summary>input</summary>
      <pre class="tool-card__pre">{{ formatToolInput(call) }}</pre>
    </details>
    <details v-if="result" class="tool-card__details" open>
      <summary>output</summary>
      <pre class="tool-card__pre">{{ truncateOutput(result.content) }}</pre>
    </details>
  </div>
</template>

<style scoped>
.tool-card {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-text-muted);
  border-radius: 6px;
  padding: 8px 12px;
  font-size: 12px;
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  max-width: 100%;
}

.tool-card--error {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}

.tool-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.tool-card__name {
  font-weight: 600;
  color: var(--color-text-primary);
}

.tool-card--error .tool-card__name {
  color: var(--color-tool-error);
}

.tool-card__status {
  font-size: 11px;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.tool-card--error .tool-card__status {
  color: var(--color-tool-error);
}

.tool-card__details {
  margin-top: 4px;
}

.tool-card__details summary {
  cursor: pointer;
  color: var(--color-text-secondary);
  font-size: 11px;
  user-select: none;
  list-style: none;
}

.tool-card__details summary::-webkit-details-marker {
  display: none;
}

.tool-card__details summary::before {
  content: "▸ ";
  color: var(--color-text-muted);
}

.tool-card__details[open] summary::before {
  content: "▾ ";
}

.tool-card__details summary:hover {
  color: var(--color-text-primary);
}

.tool-card__pre {
  margin: 4px 0 0;
  padding: 6px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 200px;
  overflow-y: auto;
  font-size: 11px;
  line-height: 1.4;
  color: var(--color-text-primary);
  font-family: var(--font-mono);
}
</style>
