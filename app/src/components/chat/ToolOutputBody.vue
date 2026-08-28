<script setup lang="ts">
// ToolOutputBody — shared body component for tool_result output.
//
// FT-F-001 PR1 (2026-06-20): extracted from `ToolCallCard.vue`
// so the same rendering can be reused by the main chat panel AND
// the future `<SubagentDrawer>` (FT-F-001 stage 2) when it
// routes `tool_result` transcript entries to typed cards.
//
// Per D1/D2/D3 decisions:
//   - 1 component, no variant prop (D3)
//   - decoupled data props `{ content, isError, durationMs? }` (D2)
//   - no store dependency (D3)
//   - scoped CSS using existing `--color-*` tokens (D7)
//
// Visual contract: matches `ToolCallCard.vue:583-586` exactly
// (the old inline output `<details>` block it replaces):
//   - cwd envelope (`{result, cwd}`) auto-unwrapped via
//     `extractToolResultDisplay` so the user sees the actual
//     tool output, not the raw JSON
//   - long output truncated via `truncateOutput(max=500)`
//   - human-readable size hint in summary
//     ("<n> chars" / "X.XK chars" / "X.XM chars")
//   - F5 duration chip REMOVED (2026-08-29 ui-visual-polish): the
//     header (ToolCallHeader) already renders the same
//     `result.durationMs` next to ✓ done — the output-row repeat
//     read as "output · 225 chars · 0.1s · 0.1s" noise. The
//     `durationMs` prop is gone; pre-F5 callers passing it can
//     drop the binding.
//   - isError adds red-tinted pre border
//
// Pre-F5 rows / pre-F5 cards: summary just shows size, unchanged.

import { computed } from "vue";
import { extractToolResultDisplay, truncateOutput } from "../../utils/messageFormat";

const props = defineProps<{
  content: string;
  isError: boolean;
}>();

/** Display-only view of the tool result content. Strips the cwd
 *  envelope (`{result, cwd}` — see REQ-16 in prd.md) so the body
 *  shows the actual tool output, not the raw JSON. Same helper
 *  as the main panel; preserved verbatim per FT-F-001 R2. */
const display = computed<string>(() =>
  extractToolResultDisplay(props.content),
);

/** Truncated view for the `<pre>`. The 500-char cap matches the
 *  old inline `truncateOutput(displayContent)` behavior. */
const truncated = computed<string>(() => truncateOutput(display.value, 500));

/** Human-readable size label for the summary. Char count (not
 *  UTF-8 bytes) because tool results in this app are always text
 *  and chars read more honestly. "chars" suffix omitted under 1024
 *  (just a bare count reads fine for small outputs); the suffix
 *  reappears for K/M to disambiguate. */
const sizeLabel = computed<string>(() => {
  const n = display.value.length;
  if (n < 1024) return `${n} chars`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}K chars`;
  return `${(n / 1024 / 1024).toFixed(1)}M chars`;
});
</script>

<template>
  <details class="tool-output-body" :class="{ 'tool-output-body--error': isError }">
    <summary>
      output · {{ sizeLabel }}
    </summary>
    <pre
      class="tool-output-body__pre"
      :class="{ 'tool-output-body__pre--error': isError }"
    >{{ truncated }}</pre>
  </details>
</template>

<style scoped>
.tool-output-body {
  margin-top: 6px;
}

.tool-output-body summary {
  cursor: pointer;
  color: var(--color-text-secondary);
  font-size: var(--text-xs);
  user-select: none;
  list-style: none;
}

.tool-output-body summary::-webkit-details-marker {
  display: none;
}

.tool-output-body summary::before {
  content: "▸ ";
  color: var(--color-text-muted);
}

.tool-output-body[open] summary::before {
  content: "▾ ";
}

.tool-output-body summary:hover {
  color: var(--color-text-primary);
}

.tool-output-body__pre {
  margin: 0;
  padding: 6px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 200px;
  overflow-y: auto;
  font-size: var(--text-xs);
  line-height: 1.4;
  color: var(--color-text-primary);
  font-family: var(--font-mono);
}

.tool-output-body__pre--error {
  border-color: var(--color-tool-error);
  color: var(--color-tool-error-text);
}

/* S6a 折叠块移动端紧凑(08-13-mobile-chat-view)。prd A3:展开后空内容占满
   一屏 → summary 行移动端 padding 收紧、内容容器横向可滚(长代码/长行横向
   滚动不撑破布局)。桌面块零改动;不改字号(design §3.4 只约定 padding +
   overflow-x,10px mono 在 320px 反而伤可读性)。 */
@media (max-width: 767px) {
  .tool-output-body {
    margin-top: 2px;
  }
  .tool-output-body summary {
    padding: 0 2px;
  }
  .tool-output-body__pre {
    overflow-x: auto;
  }
}
</style>
