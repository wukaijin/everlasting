<script setup lang="ts">
// WorkerAskBanner — compact pill rendered above the chat panel when
// one or more subagent worker tool_use asks are awaiting the user's
// decision.
//
// PR2 of RULE-FrontSubagent-003 (2026-06-22). The user-locked
// decisions (from the brainstorm that produced this task):
//
//   1. Modal CANNOT be global — multi-session concurrency would make
//      a global modal ambiguous (which session is asking?). The
//      banner is NON-BLOCKING: it doesn't steal focus, doesn't trap
//      input, doesn't overlay the chat.
//   2. The drawer is the primary surface. The banner is a FALLBACK
//      for when the drawer is closed — clicking the banner opens the
//      drawer for the most-recent pending worker run.
//   3. Worker asks are cross-session isolated: the banner reads
//      `permissions.pendingWorkerCountForSession(currentSessionId)`
//      so it only surfaces asks belonging to the CURRENTLY-ACTIVE
//      chat session. Other sessions' asks don't leak into this
//      banner (the user can see them by switching sessions, which
//      will surface their own banner if they have pending asks).
//
// Visual contract:
//   - Amber accent (`--color-tool-shell`) — matches the drawer's
//     PermissionAsk left-border tint, so the color reads as
//     "permission ask" across surfaces.
//   - Pill shape (border-radius: 999px), compact padding (4px 10px).
//   - No icon (keep it text-only + lightweight); the amber tint is
//     enough signal.
//   - `cursor: pointer` + hover brighten — clickable.
//
// The banner is single-instance per ChatPanel mount (reads
// `chatStore.currentSessionId` reactively; the count updates as
// worker asks arrive / resolve). No global state, no portal — it
// sits inline in the ChatPanel header area.

import { computed } from "vue";
import { usePermissionsStore } from "../../stores/permissions";
import { useSubagentRunsStore } from "../../stores/subagentRuns";
import { useChatStore } from "../../stores/chat";

const permissions = usePermissionsStore();
const subagentRuns = useSubagentRunsStore();
const chat = useChatStore();

/** Number of live worker asks belonging to the currently-active
 *  chat session. Drives the banner's `v-if`. */
const count = computed<number>(() =>
  permissions.pendingWorkerCountForSession(chat.currentSessionId ?? ""),
);

/** Worker run ids with live pending asks for the current session.
 *  Used by `openMostRecent` to pick the drawer target. */
const runIds = computed<string[]>(() =>
  permissions.pendingWorkerRunIdsForSession(chat.currentSessionId ?? ""),
);

/** Click handler — open the drawer for the most recent pending
 *  worker run. The runIds list is in insertion order (Map iteration
 *  is insertion-order in JS), so `[0]` is the oldest pending ask.
 *  That's a reasonable default (FIFO — surface the one that's been
 *  waiting longest); a future enhancement could sort by elapsed time
 *  but MVP doesn't need that. */
function openMostRecent(): void {
  const id = runIds.value[0];
  if (id) {
    void subagentRuns.openDrawer(id);
  }
}
</script>

<template>
  <button
    v-if="count > 0"
    class="worker-ask-banner"
    type="button"
    :title="`${count} 个 worker 正在等待审批 — 点击展开 drawer`"
    :aria-label="`${count} 个 worker 待审批`"
    @click="openMostRecent"
  >
    <span class="worker-ask-banner__icon" aria-hidden="true">⏳</span>
    <span class="worker-ask-banner__text">
      {{ count }} 个 worker 待审批
    </span>
    <span class="worker-ask-banner__action">展开 →</span>
  </button>
</template>

<style scoped>
.worker-ask-banner {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  border-radius: 999px;
  border: 1px solid var(--color-tool-shell);
  background: color-mix(in srgb, var(--color-tool-shell) 12%, transparent);
  color: var(--color-tool-shell);
  font-family: var(--font-sans);
  font-size: var(--text-xs);
  font-weight: var(--weight-semibold);
  cursor: pointer;
  transition: filter var(--duration-fast) var(--ease-out), background var(--duration-fast) var(--ease-out);
  /* Non-blocking: doesn't steal focus from the chat input. */
  user-select: none;
}

.worker-ask-banner:hover {
  filter: brightness(1.08);
  background: color-mix(in srgb, var(--color-tool-shell) 20%, transparent);
}

.worker-ask-banner__icon {
  font-size: var(--text-sm);
  line-height: 1;
}

.worker-ask-banner__text {
  /* Keep the text on one line — the count fits in 2-3 chars max. */
  white-space: nowrap;
}

.worker-ask-banner__action {
  font-weight: var(--weight-medium);
  opacity: 0.85;
}
</style>
