<script setup lang="ts">
// UiCard — container for a `use_ui` tool's primitives (B9 Child A,
// 2026-07-02). Mounted as a sibling BELOW `<ToolCallCard>` in
// MessageItem's tool stream (same dispatch pattern as
// `<AskUserQuestionCard>` — see MessageItem.vue's `visibleToolCalls`
// v-for).
//
// Reads `call.input.primitives` directly (the primitive data lives
// in the tool_use input; parent prd D2 — non-blocking, no separate
// IPC event, unlike ask_user_question's `tool:question` channel).
// Renders each primitive via the component registry; an unknown type
// degrades to the fallback (MockPrimitive) rather than crashing the
// message stream.

import { computed } from "vue";
import type { Component } from "vue";
import type { ToolCallInfo } from "../../stores/chat.types";
import { resolveUiPrimitive } from "./uiPrimitiveRegistry";
import type { UiPrimitive } from "./uiCard.types";

const props = defineProps<{ call: ToolCallInfo }>();

/** The primitives carried in the tool_use input. Defensive: if the
 *  shape is wrong (missing / non-array), render nothing rather than
 *  crash — the backend executor already validated, but a stale or
 *  hand-edited message could still mismatch. */
const primitives = computed<UiPrimitive[]>(() => {
  const raw = props.call.input?.primitives;
  return Array.isArray(raw) ? (raw as UiPrimitive[]) : [];
});

/** Resolve a primitive to its renderer component (registry or fallback). */
function rendererFor(p: UiPrimitive): Component {
  return resolveUiPrimitive(p.type);
}
</script>

<template>
  <div v-if="primitives.length" class="ui-card">
    <component
      v-for="(p, i) in primitives"
      :key="i"
      :is="rendererFor(p)"
      :primitive="p"
    />
  </div>
</template>

<style scoped>
.ui-card {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
</style>
