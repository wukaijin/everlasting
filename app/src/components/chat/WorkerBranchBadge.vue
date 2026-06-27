<script setup lang="ts">
// WorkerBranchBadge — small pill showing the worker's branch
// preservation state (L3b PR4, 2026-06-27).
//
// Derived from `status` + `worktreePath` (PRD §"Requirements"):
//   - 隔离中 (isolated)     — status === 'running' (worker in
//                              flight; the worktree is live)
//   - 已完成(保留分支)       — status === 'completed' &&
//                              worktreePath != null (worker
//                              finished with preserved changes)
//   - 已销毁 (destroyed)    — worktreePath == null (covers
//                              completed-but-merged/discarded,
//                              cancelled/error/incomplete, and
//                              swept rows). The badge is HIDDEN
//                              in this state by default (PRD
//                              §"Requirements": only render when
//                              there's something to show).
//
// Color tokens (see design-tokens.md — no one-off hex):
//   - isolated     → --color-tool-shell (amber — matches the
//                    drawer's running indicator)
//   - branch-kept  → --color-tool-write (emerald — the worker
//                    has changes worth merging)
//   - destroyed    → hidden (no badge)
//
// The badge is pure-presentation: it reads the two derived fields
// and renders the pill. The parent (SubagentDrawer) owns the
// derivation; this component just formats + styles.

import { computed } from "vue";
import type { SubagentStatus } from "../../stores/subagentRuns.types";

const props = defineProps<{
  status: SubagentStatus;
  worktreePath: string | null;
}>();

type BadgeState = "isolated" | "branch-kept" | "destroyed";

const META: Record<
  Exclude<BadgeState, "destroyed">,
  { label: string; color: string; bg: string }
> = {
  isolated: {
    label: "隔离中",
    color: "var(--color-tool-shell)",
    bg: "color-mix(in srgb, var(--color-tool-shell) 12%, transparent)",
  },
  "branch-kept": {
    label: "已完成 · 保留分支",
    color: "var(--color-tool-write)",
    bg: "color-mix(in srgb, var(--color-tool-write) 12%, transparent)",
  },
};

/** Derive the badge state. Returns `"destroyed"` for the hidden
 *  state (any non-isolated state with no worktree). */
const state = computed<BadgeState>(() => {
  if (props.status === "running") return "isolated";
  if (props.status === "completed" && props.worktreePath) {
    return "branch-kept";
  }
  return "destroyed";
});

const meta = computed(() => (state.value === "destroyed" ? null : META[state.value]));
</script>

<template>
  <span
    v-if="meta"
    class="worker-branch-badge"
    :style="{ color: meta.color, background: meta.bg, borderColor: meta.color }"
    role="status"
  >
    {{ meta.label }}
  </span>
</template>

<style scoped>
.worker-branch-badge {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  border: 1px solid;
  font-family: var(--font-sans);
  font-size: var(--text-2xs);
  font-weight: var(--weight-semibold);
  line-height: 1.4;
  white-space: nowrap;
  /* The badge is non-interactive — it's a status readout, not a
     button. The `user-select: none` keeps double-click selection
     from grabbing it. */
  user-select: none;
}
</style>
