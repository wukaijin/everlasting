<script setup lang="ts">
// TurnTimeline — the trace viewer's main axis. Renders the
// per-seq turn cards in ASC order, plus a footer card for
// the "ungrouped audit events" bucket (rows whose `turnSeq`
// is `null` — pre-v7 historical audit data, or IPC-handler
// audit writes that have no turn-loop context).
//
// Pulls the timeline data from `useTraceStore` (a single
// function call returns the sorted array). The `loading`
// flag drives a skeleton; the `error` flag drives an
// inline error chip with a 重试 button.

import { computed } from "vue";
import Icon from "../Icon.vue";
import { useTraceStore } from "../../stores/traceStore";
import TurnCard from "./TurnCard.vue";

const traceStore = useTraceStore();

/** Sorted list of all TurnTraces for the current session,
 *  excluding the ungrouped audit bucket. */
const sortedTraces = computed(() => traceStore.tracesForCurrentSession());

/** Audit rows with no turnSeq (rendered as a footer card). */
const ungroupedAudits = computed(() => traceStore.ungroupedAuditEvents());

/** True when the timeline has nothing to show — either no
 *  traces at all, or only the ungrouped bucket (no turn
 *  loop ran yet, only IPC-handler audits). */
const isEmpty = computed<boolean>(
  () => sortedTraces.value.length === 0 && ungroupedAudits.value.length === 0,
);

/** True when the ungrouped bucket should render as a
 *  synthetic "未关联 turn" card. The card component is
 *  reused for this case (`seq === Number.MAX_SAFE_INTEGER`
 *  triggers the "未关联 turn" label). */
const ungroupedTrace = computed(() => {
  if (ungroupedAudits.value.length === 0) return null;
  return {
    id: 0,
    sessionId: traceStore.currentSessionId ?? "",
    seq: Number.MAX_SAFE_INTEGER,
    createdAt: ungroupedAudits.value[0]?.ts ?? "",
    auditEvents: ungroupedAudits.value,
  };
});

/** Retry the 回看 load. The error state is cleared on
 *  retry. The store's `loadHistory` re-fetches both IPCs. */
async function onRetry(): Promise<void> {
  if (!traceStore.currentSessionId) return;
  await traceStore.loadHistory(traceStore.currentSessionId);
}
</script>

<template>
  <div class="turn-timeline">
    <!-- Loading skeleton -->
    <div v-if="traceStore.loading" class="turn-timeline__skeleton">
      <div class="turn-timeline__skeleton-card" />
      <div class="turn-timeline__skeleton-card" />
      <div class="turn-timeline__skeleton-card" />
    </div>

    <!-- Error state -->
    <div v-else-if="traceStore.error" class="turn-timeline__error">
      <Icon name="warn" :size="14" class="turn-timeline__error-icon" />
      <span class="turn-timeline__error-text">
        加载失败: {{ traceStore.error }}
      </span>
      <button
        type="button"
        class="turn-timeline__retry"
        @click="onRetry"
      >
        重试
      </button>
    </div>

    <!-- Empty state -->
    <div v-else-if="isEmpty" class="turn-timeline__empty">
      <Icon name="info" :size="14" />
      <span>该 session 暂无 trace 记录</span>
    </div>

    <!-- Main timeline (turn-seq ASC) -->
    <div v-else class="turn-timeline__list">
      <TurnCard
        v-for="t in sortedTraces"
        :key="t.seq"
        :trace="t"
      />

      <!-- Ungrouped audit bucket (pre-v7 historical) -->
      <TurnCard
        v-if="ungroupedTrace"
        :key="`ungrouped-${ungroupedTrace.auditEvents?.length ?? 0}`"
        :trace="ungroupedTrace"
      />
    </div>
  </div>
</template>

<style scoped>
.turn-timeline {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  min-width: 0;
}

.turn-timeline__list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
}

.turn-timeline__empty {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 24px 12px;
  color: var(--color-text-muted);
  font-size: var(--text-sm);
  justify-content: center;
}

.turn-timeline__error {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px;
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-tool-error) 35%, transparent);
  border-left: 3px solid var(--color-tool-error);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
}

.turn-timeline__error-icon {
  color: var(--color-tool-error);
  flex-shrink: 0;
}

.turn-timeline__error-text {
  flex: 1;
  word-break: break-all;
}

.turn-timeline__retry {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 2px 10px;
  font-size: var(--text-xs);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out);
}

.turn-timeline__retry:hover {
  background: var(--color-bg-border);
}

.turn-timeline__skeleton {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
}

.turn-timeline__skeleton-card {
  height: 60px;
  background: linear-gradient(
    90deg,
    var(--color-bg-surface) 0%,
    var(--color-bg-elevated) 50%,
    var(--color-bg-surface) 100%
  );
  background-size: 200% 100%;
  border-radius: var(--radius-md);
  animation: skeleton-pulse 1.4s ease-in-out infinite;
}

@keyframes skeleton-pulse {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}
</style>
