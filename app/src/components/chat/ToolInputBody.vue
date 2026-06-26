<script setup lang="ts">
// ToolInputBody — shared body component for tool_call input.
//
// FT-F-001 PR1 (2026-06-20): extracted from `ToolCallCard.vue`
// so the same rendering can be reused by the main chat panel AND
// the future `<SubagentDrawer>` (FT-F-001 stage 2) when it
// routes `tool_call` transcript entries to typed cards.
//
// Per D1/D2/D3 decisions:
//   - 1 component, no variant prop (D3 — variant multi-explosion)
//   - decoupled data props `{ name, input }` (D2 — not a typed
//     `ToolCallInfo` wrapper, so the drawer can pass its raw
//     `payload_json.name` / `payload_json.input` directly)
//   - no store dependency (D3 — store lives in the outer wrapper)
//   - scoped CSS using existing `--color-*` tokens (D7)
//
// Visual contract: matches `ToolCallCard.vue:578-581` exactly
// (the old inline `<details>` block it replaces). The collapsed
// state is the default `<details>` behavior — input is hidden
// until the user clicks to expand (matches "input section stays
// collapsed by default" per `ToolCallCard.vue` file header).

defineProps<{
  name: string;
  input: Record<string, unknown>;
}>();
</script>

<template>
  <details class="tool-input-body">
    <summary>input</summary>
    <pre class="tool-input-body__pre">{{ JSON.stringify(input, null, 2) }}</pre>
  </details>
</template>

<style scoped>
.tool-input-body {
  margin-top: 6px;
}

.tool-input-body summary {
  cursor: pointer;
  color: var(--color-text-secondary);
  font-size: var(--text-xs);
  user-select: none;
  list-style: none;
}

.tool-input-body summary::-webkit-details-marker {
  display: none;
}

.tool-input-body summary::before {
  content: "▸ ";
  color: var(--color-text-muted);
}

.tool-input-body[open] summary::before {
  content: "▾ ";
}

.tool-input-body summary:hover {
  color: var(--color-text-primary);
}

.tool-input-body__pre {
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
</style>
