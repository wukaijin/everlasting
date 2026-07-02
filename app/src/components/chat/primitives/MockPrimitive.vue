<script setup lang="ts">
// MockPrimitive — placeholder renderer for B9 generative UI primitives
// (Child A of 07-02-b9-generative-ui, 2026-07-02).
//
// Child A wires the use_ui plumbing (tool + registry + UiCard +
// MessageItem dispatch) but does NOT implement real renderers. Every
// primitive type renders via this mock, which dumps the `type` + raw
// payload, so the pipeline can be validated end-to-end (LLM calls
// use_ui → frontend dispatches by type → component mounts).
//
// Child B replaces the `code_block` registry entry with a real hljs
// renderer; Child C replaces `diff` with a DiffView-based renderer.
// MockPrimitive stays as the registry fallback for unknown types.

import type { UiPrimitive } from "../uiCard.types";

defineProps<{ primitive: UiPrimitive }>();
</script>

<template>
  <div class="ui-prim ui-prim--mock">
    <div class="ui-prim__head">
      <span class="ui-prim__type">{{ primitive.type }}</span>
      <span class="ui-prim__badge">mock primitive</span>
      <span v-if="primitive.title" class="ui-prim__title">{{ primitive.title }}</span>
    </div>
    <pre class="ui-prim__dump">{{ primitive }}</pre>
  </div>
</template>

<style scoped>
.ui-prim--mock {
  border: 1px dashed var(--color-bg-border);
  border-radius: var(--radius-md);
  padding: 8px 10px;
  background: var(--color-bg-surface);
  font-size: 12px;
}
.ui-prim__head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
}
.ui-prim__type {
  font-family: monospace;
  font-weight: 600;
  color: var(--color-text-primary);
}
.ui-prim__badge {
  color: var(--color-text-secondary);
  font-size: 11px;
}
.ui-prim__title {
  color: var(--color-text-secondary);
}
.ui-prim__dump {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-all;
  color: var(--color-text-secondary);
  font-family: monospace;
  font-size: 11px;
  max-height: 160px;
  overflow: auto;
}
</style>
