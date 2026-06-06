<script setup lang="ts">
// ToolCallCard — a single tool invocation with its input / output
// blocks. Per spike-003 the card has a 3px left bar that switches
// color by tool name: read_file → cyan, write_file → emerald,
// shell → amber. The error state (the matching tool_result reports
// is_error) flips the bar to red and tints the card body.
//
// D5 restructure: the header is a single line — icon + tool name +
// file path on the left, status on the right. Matches the spike-003
// reference (ui-A.png). The input section stays collapsed by default;
// the output is shown directly when present (not inside <details>)
// so the user sees the result immediately. Long input / output is
// capped at ~200px tall and overflows with scroll.

import { computed } from "vue";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat";
import {
  formatToolInput,
  truncateOutput,
  toolAccentVar,
  toolIcon,
} from "../../utils/messageFormat";
import Icon from "../Icon.vue";

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

/** Best-effort file path for display in the header. Most tools pass
 *  `path` in their input; shell uses `command` which is too long to
 *  fit, so we leave it out. Non-string values are guarded. */
const filePath = computed<string | null>(() => {
  const input = props.call.input;
  if (!input) return null;
  const p = input.path;
  if (typeof p === "string" && p.length > 0) return p;
  return null;
});

const statusText = computed<string>(() => {
  if (isError.value) return "error";
  if (hasResult.value) return "done";
  return "running…";
});

/** Map the run state to a heroicon name for the status indicator.
 *  "running" uses an animated ellipsis (handled by CSS); the other
 *  two are static check / X marks. */
const statusIconName = computed<string>(() => {
  if (isError.value) return "x";
  if (hasResult.value) return "check";
  return "ellipsis";
});
</script>

<template>
  <div
    :class="['tool-card', { 'tool-card--error': isError, 'tool-card--running': !hasResult && !isError }]"
    :style="{ borderLeftColor: accent }"
  >
    <div class="tool-card__header">
      <div class="tool-card__title">
        <span class="tool-card__icon">
          <Icon :name="toolIcon(call.name)" :size="14" />
        </span>
        <span class="tool-card__name">{{ call.name }}</span>
        <span v-if="filePath" class="tool-card__path" :title="filePath">
          · {{ filePath }}
        </span>
      </div>
      <div class="tool-card__status">
        <span
          :class="['tool-card__status-icon', { 'tool-card__status-icon--running': !hasResult && !isError }]"
        >
          <Icon :name="statusIconName" :size="14" />
        </span>
        <span>{{ statusText }}</span>
      </div>
    </div>

    <details v-if="call.input && Object.keys(call.input).length" class="tool-card__details">
      <summary>input</summary>
      <pre class="tool-card__pre tool-card__pre--input">{{ formatToolInput(call) }}</pre>
    </details>

    <div v-if="result" class="tool-card__output">
      <pre class="tool-card__pre tool-card__pre--output">{{ truncateOutput(result.content) }}</pre>
    </div>
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

.tool-card--running {
  border-left-color: var(--color-tool-shell);
}

.tool-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
}

.tool-card__title {
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
  flex: 1;
  overflow: hidden;
  white-space: nowrap;
}

.tool-card__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  color: var(--color-text-secondary);
}

.tool-card--error .tool-card__icon {
  color: var(--color-tool-error);
}

.tool-card__name {
  font-weight: 600;
  color: var(--color-text-primary);
}

.tool-card--error .tool-card__name {
  color: var(--color-tool-error);
}

.tool-card__path {
  color: var(--color-text-secondary);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
  flex: 1;
}

.tool-card__status {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.tool-card__status-icon {
  display: inline-flex;
  align-items: center;
  line-height: 1;
}

.tool-card__status-icon--running {
  animation: tool-card-pulse 1.4s ease-in-out infinite;
}

@keyframes tool-card-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}

.tool-card--error .tool-card__status {
  color: var(--color-tool-error);
}

.tool-card__details {
  margin-top: 6px;
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

.tool-card__output {
  margin-top: 6px;
}

.tool-card__pre {
  margin: 0;
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
