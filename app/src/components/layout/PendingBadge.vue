<script setup lang="ts">
// PendingBadge — top-bar global "N pending interactions" indicator
// (B档 of 2026-07-08 `cross-session-pending-indicator`). Shows the
// total count of pending mode_change / ask_user_question across ALL
// sessions and projects (`questionCards.pendingBySession.size`), so
// a user working in session A perceives that session B — possibly
// in ANOTHER project, whose sessions aren't in the sidebar — has a
// pending interaction. This is the cross-project-visible surface
// that A档 (sidebar row badge, current-project only) cannot cover.
//
// Mounted inside AppHeader's TitleBar slot (after HiddenProjectsMenu),
// so it rides the tab-bar row. `align-self:center` keeps it from
// being stretched by titlebar__content's `align-items:stretch`.
//
// Click → switch to the most-recent pending session IN THE CURRENT
// project (per Q4: same-project jump only — `chatStore.sessions`
// holds only the current project's sessions, so cross-project
// targets are naturally skipped; avoids session→project mapping).
// When no in-project pending exists (all pendings are in other
// projects), the click is a no-op (the badge still informs via its
// count + tooltip). Hidden entirely when count === 0.
//
// Reactive: count is a computed over `pendingBySession` (a reactive
// Map), so it updates live as pendings register/resolve — no extra
// wiring needed when a user answers a card (removePending → Map
// shrink → count drops → badge hides at 0).

import { computed } from "vue";
import { useQuestionCardsStore } from "../../stores/questionCards";
import { useChatStore } from "../../stores/chat";

const qc = useQuestionCardsStore();
const chatStore = useChatStore();

/** Total pending interactions across all sessions/projects. */
const count = computed(() => qc.pendingBySession.size);

/** Most-recent pending session id WITHIN the current project
 *  (`chatStore.sessions` holds only the current project's sessions;
 *  cross-project pendings are absent and thus skipped). Picks the
 *  highest-`ts` pending among in-project sessions. `null` when none
 *  of the current project's sessions have a pending interaction. */
const targetInProject = computed<string | null>(() => {
  let best: { id: string; ts: number } | null = null;
  for (const s of chatStore.sessions) {
    const entry = qc.getPending(s.id);
    if (!entry) continue;
    const ts = entry.payload.ts;
    if (best === null || ts > best.ts) {
      best = { id: s.id, ts };
    }
  }
  return best?.id ?? null;
});

function onClick(): void {
  const target = targetInProject.value;
  if (target) {
    void chatStore.switchSession(target);
  }
}
</script>

<template>
  <button
    v-if="count > 0"
    class="pending-badge"
    type="button"
    :title="`${count} 个会话有待处理交互${targetInProject ? '(点击查看)' : ''}`"
    @click="onClick"
  >
    <span class="pending-badge__dot" aria-hidden="true" />
    <span class="pending-badge__count">{{ count }}</span>
  </button>
</template>

<style scoped>
.pending-badge {
  align-self: center;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin: 0 6px;
  padding: 2px 9px;
  border: none;
  border-radius: 999px;
  background: color-mix(in srgb, var(--color-accent) 18%, transparent);
  color: var(--color-accent);
  font-size: var(--text-2xs);
  font-weight: var(--weight-medium);
  font-variant-numeric: tabular-nums;
  line-height: 1;
  cursor: pointer;
  flex-shrink: 0;
}

.pending-badge:hover {
  background: color-mix(in srgb, var(--color-accent) 30%, transparent);
}

.pending-badge__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-accent);
  animation: pending-pulse 1.5s ease-in-out infinite;
}

.pending-badge__count {
  line-height: 1;
}

@keyframes pending-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.4;
  }
}
</style>
