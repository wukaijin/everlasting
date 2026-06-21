<script setup lang="ts">
// DrawerThinkingBlock — drawer-side wrapper around the shared
// `ThinkingBlock` visual primitive.
//
// PR4 of the subagent-drawer redesign (2026-06-21). Per PRD R6 +
// Decision 1: the drawer shares the main panel's VISUAL primitives
// (ThinkingBlock / ToolCallCard-rendered-by-ToolInputBody+ToolOutputBody)
// but maintains its OWN data→view rendering path. This wrapper is
// the adapter between the drawer's accumulator output
// (`ThinkingSection`, from `stores/subagentRuns.ts`) and the main
// panel's `ThinkingBlock` props (`blocks: ThinkingBlockInfo[]`).
//
// Why a wrapper instead of mounting `<ThinkingBlock>` directly from
// the drawer:
//   - `ThinkingBlock` consumes `ThinkingBlockInfo[]` (one entry per
//     Anthropic content block, with `signature`). The drawer's
//     accumulator flattens `thinking_delta` SSE chunks into a single
//     concatenated `text` string on a `ThinkingSection` (no
//     per-block signature, because the worker's transcript is
//     display-only — the signature is irrelevant once the worker
//     turn has been persisted; see PR2 `RunAccumulator.feed`).
//   - The conversion is `section.text → [{ text: section.text,
//     signature: "" }]` (single-element array; `thinkingDisplayText`
//     on the underlying block does `blocks.map(b=>b.text).join("\n\n")`
//     which is a no-op on a single-element array).
//   - Keeping the conversion INSIDE this wrapper keeps PR5's
//     `DrawerSection` a pure layout shell (no data-shape knowledge).
//
// `streaming` / `showStreamingHint` are derived from `section.closed`
// by default (an open thinking segment is still streaming) but can
// be overridden by the parent for edge cases (e.g. a finished run
// whose last thinking block never saw a `signature_delta`).
//
// `thinkingDurationMs` is intentionally NOT surfaced — the drawer's
// accumulator does not track per-segment wall-clock duration (the
// worker's per-turn duration is captured by the backend, not the
// accumulator). The underlying `ThinkingBlock` header falls back
// to "Thought for —", which reads honestly as "duration unknown"
// for a worker transcript segment.

import { computed } from "vue";
import type { ThinkingBlockInfo } from "../../stores/chat";
import type { ThinkingSection } from "../../stores/subagentRuns";
import ThinkingBlock from "./ThinkingBlock.vue";

const props = withDefaults(
  defineProps<{
    /** The drawer accumulator's thinking segment. The wrapper reads
     *  `text` (concatenated thinking_delta chunks) and `closed`
     *  (whether a `signature_delta` has been seen for this block). */
    section: ThinkingSection;
    /** Override the streaming-derived `showStreamingHint`. Defaults
     *  to `!section.closed` (an open segment is still streaming).
     *  Set explicitly when the parent knows the worker has finished
     *  (e.g. drawer reads a completed run from DB cache — even an
     *  unclosed segment should NOT show "streaming…" in that case).
     *
     *  ⚠️ Vue 3 boolean-casting gotcha: a bare `?: boolean` prop
     *  gets coerced to `false` when absent (Vue's Boolean casting
     *  rule — see https://vuejs.org/guide/components/props.html#boolean-casting).
     *  `default: undefined` below disables that coercion so the
     *  "absent vs. explicitly-false" distinction survives, which
     *  the `showHint` computed below relies on. */
    showStreamingHint?: boolean | undefined;
  }>(),
  { showStreamingHint: undefined },
);

/** Convert the drawer segment's flat text into the
 *  `ThinkingBlockInfo[]` shape `ThinkingBlock` expects. A single
 *  element suffices — `thinkingDisplayText` does `.join("\n\n")`
 *  on the array, which is a no-op for length 1. The `signature`
 *  is empty because the drawer's accumulator doesn't preserve it
 *  (display-only; the signature only matters for the outbound
 *  Anthropic payload, which the worker transcript isn't). */
const blocks = computed<ThinkingBlockInfo[]>(() => [
  { text: props.section.text, signature: "" },
]);

/** A segment that hasn't seen its `signature_delta` yet is still
 *  being streamed by the worker. Pass that hint down to the
 *  underlying `ThinkingBlock` so the header shows "streaming…"
 *  while the model is still producing thinking text. Parent
 *  override wins (e.g. drawer showing a completed run from DB
 *  cache where the segment was left open). */
const showHint = computed<boolean>(() => {
  if (typeof props.showStreamingHint === "boolean") {
    return props.showStreamingHint;
  }
  return !props.section.closed;
});
</script>

<template>
  <ThinkingBlock
    :blocks="blocks"
    :show-streaming-hint="showHint"
  />
</template>
