<script setup lang="ts">
// WorkerTextTimeline — drawer-local lifecycle view for a worker run's
// `chat_event` transcript entries.
//
// FT-F-001 stage 2 (2026-06-20): per D7, the drawer's chat_event entries
// route here instead of being stringified into a `<pre>` blob. The
// component renders a compact timeline of the worker's lifecycle
// milestones (`start` / `done`), ignoring the noisy delta / thinking /
// tool_call / tool_result / error chat_event kinds (those are either
// already represented by their own transcript entries or redundant).
//
// Per R3 + D7 + Q3:
//   - props `{ events: TranscriptEntry[] }` — a subset of the drawer's
//     transcript filtered to `kind === "chat_event"`. The component does
//     NOT re-filter by kind (the parent already routed by kind); but it
//     DOES filter by `payload_json.kind` (the inner ChatEvent variant)
//     because a single chat_event transcript entry can carry any of the
//     9 inner kinds (`start` / `delta` / `thinking_delta` /
//     `signature_delta` / `redacted_thinking_delta` / `tool_call` /
//     `tool_result` / `done` / `error`).
//   - inner kinds we render: `start`, `done` (done carries `stop_reason`)
//   - inner kinds we ignore: everything else (delta noise is already
//     represented by the worker's stream; tool_call/tool_result have
//     their own typed-card transcript entries; error is surfaced by the
//     FT-F-005 failure banner in the drawer header).
//   - token usage is NOT displayed here (Q3 decision — the drawer header
//     already surfaces the `tokenUsageJson` aggregate; duplicating per-
//     event `usage` would be noise).
//
// Visual: plain div timeline (NOT reka-ui), one row per lifecycle
// milestone, with a small status dot. Reuses existing `--color-*` tokens
// per design-tokens.md (no hardcoded hex). Compact padding to fit the
// drawer's 480px width.

import { computed } from "vue";
import type { TranscriptEntry } from "../../stores/subagentRuns.types";

/** A lifecycle milestone extracted from the chat_event stream. */
interface Milestone {
  /** Marker kind — only `start` / `done` qualify (see filter). */
  inner: "start" | "done";
  /** `done` carries `stop_reason` (end_turn / max_turns / tool_use /
   *  stop_sequence / cancelled). Surfaced inline; falls back to a
   *  generic label when absent. */
  stopReason?: string;
}

const props = defineProps<{
  events: TranscriptEntry[];
}>();

/** Pull the lifecycle milestones out of the chat_event subset. `start`
 *  and `done` are the only inner kinds we render. Unknown / missing
 *  `payload_json.kind` values are silently dropped (defensive — the
 *  Rust ChatEvent enum may grow new variants we haven't mapped yet). */
const milestones = computed<Milestone[]>(() => {
  const out: Milestone[] = [];
  for (const e of props.events) {
    const inner = e.payload_json?.kind;
    if (inner === "start") {
      out.push({ inner: "start" });
    } else if (inner === "done") {
      const reason = e.payload_json?.stop_reason;
      out.push({
        inner: "done",
        stopReason: typeof reason === "string" ? reason : undefined,
      });
    }
    // Other inner kinds are intentionally ignored (delta / thinking_*
    // / signature_delta / redacted_thinking_delta / tool_call /
    // tool_result / error). See file header for rationale.
  }
  return out;
});
</script>

<template>
  <div class="worker-text-timeline">
    <div
      v-for="(m, i) in milestones"
      :key="i"
      class="worker-text-timeline__row"
      :class="`worker-text-timeline__row--${m.inner}`"
    >
      <span class="worker-text-timeline__dot"></span>
      <span class="worker-text-timeline__label">
        <template v-if="m.inner === 'start'">agent 开始响应</template>
        <template v-else>
          agent 完成<span
            v-if="m.stopReason"
            class="worker-text-timeline__reason"
          > · {{ m.stopReason }}</span>
        </template>
      </span>
    </div>
    <p
      v-if="milestones.length === 0"
      class="worker-text-timeline__empty"
    >无 lifecycle 事件</p>
  </div>
</template>

<style scoped>
.worker-text-timeline {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 4px 0;
  font-family: var(--font-sans);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}

.worker-text-timeline__row {
  display: flex;
  align-items: center;
  gap: 6px;
  line-height: 1.4;
}

.worker-text-timeline__dot {
  flex-shrink: 0;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-text-muted);
}

/* start = neutral grey dot (worker spun up).
   done = green dot (worker reached a terminal ChatEvent). */
.worker-text-timeline__row--done .worker-text-timeline__dot {
  background: var(--color-tool-write);
}

.worker-text-timeline__label {
  min-width: 0;
}

.worker-text-timeline__reason {
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.worker-text-timeline__empty {
  margin: 0;
  color: var(--color-text-muted);
  font-style: italic;
}
</style>
