<script setup lang="ts">
// WorkerMergeControls — Merge / Discard buttons for a completed
// worker's preserved branch (L3b PR4, 2026-06-27).
//
// Visible condition (strict, per PRD §"Requirements"):
//   status === 'completed' && worktreePath != null
//
// The buttons route through:
//   1. ConfirmDialog (二次确认 — both actions are destructive:
//      merge writes to the parent session branch + destroys the
//      worker worktree; discard deletes the worker branch +
//      worktree outright). Per popover-pattern.md, the Tauri
//      webview silently no-ops `window.confirm()` — we MUST use
//      the in-app ConfirmDialog component.
//   2. store.mergeWorker(runId) / store.discardWorker(runId)
//      (per-run spinner, see subagentRuns store).
//   3. Toast on success / conflict / error (via projectsStore
//      .showToast — the project's standard toast surface).
//
// Conflict handling: when `mergeWorker` returns
// `{ kind: 'conflict', files }`, the file list renders inline
// (read-only, no interaction) with a hint to resolve via git
// CLI then retry. The worker branch + worktree are PRESERVED
// on conflict (the backend reset to parent tip but kept the
// branch), so the Merge / Discard buttons stay visible — the
// user can resolve in git, then click Merge again.
//
// Per-run spinner isolation: reads `store.mergeStateByRunId.get(runId)`
// to drive button disabled + spinner rendering. A 5s merge on run
// A does NOT disable the discard button on run B (different
// drawers, different runIds).

import { computed, ref } from "vue";
import { useSubagentRunsStore } from "../../stores/subagentRuns";
import { useProjectsStore } from "../../stores/projects";
import { formatWorkerBranchLabel } from "../../utils/workerBranch";
import type { MergeResult } from "../../stores/subagentRuns.types";
import ConfirmDialog from "../common/ConfirmDialog.vue";
import Icon from "../Icon.vue";

const props = defineProps<{
  /** The worker run id (`subagent_runs.id`). */
  runId: string;
  /** The parent session id (`subagent_runs.parent_session_id`).
   *  Threaded to the store so a successful lazy auto-attach (06-30
   *  follow-up) can refresh the chat session list and flip the
   *  chat header's worktree chip from `none → active`. Without
   *  this prop the merge still works (the toast announces the
   *  side effect) but the chip stays stale until the next user
   *  action. The drawer passes the value from
   *  `run.parentSessionId` (the run row's DB column). */
  parentSessionId: string;
}>();

const store = useSubagentRunsStore();
const projects = useProjectsStore();

/** Effective worktree path for this runId. Read from the store's
 *  cached row (single source of truth — `mergeWorker` /
 *  `discardWorker` clear it on success, the drawer's parent
 *  also reads from the same cache). Reactively updates when the
 *  store row's `worktreePath` flips, so the component's
 *  visible-gate hides itself immediately on success without the
 *  parent having to re-thread a prop. */
const worktreePath = computed<string | null>(
  () => store.getRunCache.get(props.runId)?.worktreePath ?? null,
);

/** Effective status for this runId, read from the same cached
 *  row. The visible-gate requires `status === 'completed'`
 *  (strict per PRD §"Requirements" + §"Edge Cases": cancelled /
 *  error / incomplete workers MUST NOT show Merge/Discard —
 *  those branches may exist on disk but the worker exit-state
 *  signals the user shouldn't be re-introducing them into the
 *  parent). Reactively updates when the store row's status
 *  flips (e.g. parent side-effect re-classifies a worker). */
const status = computed<string | null>(
  () => store.getRunCache.get(props.runId)?.status ?? null,
);

/** Per-run spinner for THIS runId. `null` when no action is in
 *  flight. The buttons disable on truthy + show the spinner. */
const mergeState = computed(() => store.mergeStateByRunId.get(props.runId) ?? null);

const isMergeLoading = computed(() => mergeState.value?.kind === "merge");
const isDiscardLoading = computed(() => mergeState.value?.kind === "discard");
const anyLoading = computed(() => mergeState.value !== null);

/** Strict visible-gate per PRD §"Requirements" + §"Edge Cases":
 *  buttons render ONLY for completed workers with a preserved
 *  branch. Other terminal states (cancelled / error /
 *  incomplete) skip the gate even if a worktree_path is
 *  lingering on disk — the worker exit-state is the
 *  authoritative "is this user-actionable" signal, not the
 *  worker's on-disk presence. */
const visible = computed(
  () => worktreePath.value !== null && status.value === "completed",
);

/** Friendly branch label for the ConfirmDialog body
 *  (`Worker <short-hash>`). Empty when worktreePath is null (the
 *  component is hidden in that case anyway). */
const branchLabel = computed(() => formatWorkerBranchLabel(worktreePath.value));

/** Inline conflict file list (read from the last mergeWorker
 *  result). Cleared on the next action attempt. The list is
 *  stored locally to the component instance — opening a
 *  different drawer runId mounts a fresh component, so the
 *  conflict list doesn't bleed across runs. */
const conflictFiles = ref<string[] | null>(null);

/** ConfirmDialog state — `null` = closed; `"merge"` / `"discard"`
 *  = open for that action. */
const confirmKind = ref<"merge" | "discard" | null>(null);

function askMerge(): void {
  // Clear any prior conflict result before re-confirming.
  conflictFiles.value = null;
  confirmKind.value = "merge";
}

function askDiscard(): void {
  conflictFiles.value = null;
  confirmKind.value = "discard";
}

function cancelConfirm(): void {
  confirmKind.value = null;
}

/** Fire the actual merge IPC after the user confirms. */
async function doMerge(): Promise<void> {
  confirmKind.value = null;
  const result: MergeResult = await store.mergeWorker(
    props.runId,
    props.parentSessionId,
  );
  if (result.kind === "success") {
    conflictFiles.value = null;
    // 06-30 follow-up: differentiate the toast by whether the
    // backend had to lazily attach a worktree on the parent
    // session as a side effect of the merge. The plain "merged"
    //    toast covers the common case (parent was already
    //    Active); the new "merged and bound the parent
    //    workspace" toast covers the case where the user
    //    clicked merge on a parent session that never had a
    //    worktree attached (this is now transparently handled
    //    end-to-end, so the user no longer has to attach
    //    manually first).
    const msg = result.autoAttachedParent
      ? "已合并到父 session 分支,并自动绑定了父工作区"
      : "已合并到 session 分支";
    projects.showToast(msg, "info");
  } else if (result.kind === "conflict") {
    // Preserve the file list for inline display; the buttons
    // stay visible (the worker branch + worktree are intact).
    conflictFiles.value = result.files;
    projects.showToast(
      result.files.length > 0
        ? `合并冲突(${result.files.length} 个文件),请到 git CLI 解决后重试`
        : "合并冲突,请到 git CLI 解决后重试",
      "error",
    );
  } else {
    projects.showToast(`合并失败: ${result.message}`, "error");
  }
}

/** Fire the actual discard IPC after the user confirms. */
async function doDiscard(): Promise<void> {
  confirmKind.value = null;
  const result = await store.discardWorker(props.runId);
  if (result.kind === "success") {
    conflictFiles.value = null;
    projects.showToast("已丢弃 worker 分支", "info");
  } else {
    projects.showToast(`丢弃失败: ${result.message}`, "error");
  }
}
</script>

<template>
  <div v-if="visible" class="worker-merge-controls">
    <div class="worker-merge-controls__row">
      <button
        type="button"
        class="worker-merge-controls__btn worker-merge-controls__btn--merge"
        :disabled="anyLoading"
        :aria-label="`合并 ${branchLabel} 分支到 session 分支`"
        @click="askMerge"
      >
        <span
          v-if="isMergeLoading"
          class="worker-merge-controls__spinner"
          aria-hidden="true"
        />
        <Icon v-else name="git-merge" :size="12" />
        <span>合并</span>
      </button>
      <button
        type="button"
        class="worker-merge-controls__btn worker-merge-controls__btn--discard"
        :disabled="anyLoading"
        :aria-label="`丢弃 ${branchLabel} 分支`"
        @click="askDiscard"
      >
        <span
          v-if="isDiscardLoading"
          class="worker-merge-controls__spinner"
          aria-hidden="true"
        />
        <Icon v-else name="trash" :size="12" />
        <span>丢弃</span>
      </button>
    </div>

    <!-- Conflict display: read-only file list + git CLI hint.
         Rendered when the last mergeWorker call returned
         { kind: 'conflict' }. The list is non-interactive — the
         user resolves conflicts in their terminal then clicks
         Merge again. -->
    <div
      v-if="conflictFiles !== null"
      class="worker-merge-controls__conflict"
      role="alert"
    >
      <div class="worker-merge-controls__conflict-header">
        <Icon name="warn" :size="12" />
        <span>合并冲突,以下文件需手动解决</span>
      </div>
      <ul v-if="conflictFiles.length > 0" class="worker-merge-controls__conflict-list">
        <li v-for="(f, i) in conflictFiles" :key="`conflict-${i}`">
          <code>{{ f }}</code>
        </li>
      </ul>
      <p v-else class="worker-merge-controls__conflict-empty">
        (无文件列表 — 请到 git CLI 检查冲突)
      </p>
      <p class="worker-merge-controls__conflict-hint">
        在终端解决冲突后重试合并,或直接丢弃分支。
      </p>
    </div>

    <!-- ConfirmDialog (二次确认 — both actions destructive). -->
    <ConfirmDialog
      :open="confirmKind === 'merge'"
      title="合并 worker 分支"
      variant="warning"
      confirm-text="合并"
      @cancel="cancelConfirm"
      @confirm="doMerge"
    >
      <p>
        确认将 <strong>{{ branchLabel }}</strong> 的改动合并到当前
        session 分支?合并后 worker 分支与 worktree 会被销毁。
      </p>
    </ConfirmDialog>
    <ConfirmDialog
      :open="confirmKind === 'discard'"
      title="丢弃 worker 分支"
      variant="danger"
      confirm-text="丢弃"
      @cancel="cancelConfirm"
      @confirm="doDiscard"
    >
      <p>
        确认丢弃 <strong>{{ branchLabel }}</strong> 的改动?worker
        分支与 worktree 将被永久删除,无法恢复。
      </p>
    </ConfirmDialog>
  </div>
</template>

<style scoped>
.worker-merge-controls {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px 12px;
  border-top: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
}

.worker-merge-controls__row {
  display: flex;
  gap: 8px;
}

.worker-merge-controls__btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  color: var(--color-text-primary);
  font: inherit;
  font-family: var(--font-sans);
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out),
    border-color var(--duration-fast) var(--ease-out),
    color var(--duration-fast) var(--ease-out);
}

.worker-merge-controls__btn:hover:not(:disabled) {
  background: var(--color-bg-hover);
}

.worker-merge-controls__btn:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.worker-merge-controls__btn--merge {
  color: var(--color-tool-write);
  border-color: color-mix(in srgb, var(--color-tool-write) 40%, transparent);
}

.worker-merge-controls__btn--merge:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-tool-write) 12%, transparent);
}

.worker-merge-controls__btn--discard {
  color: var(--color-tool-error);
  border-color: color-mix(in srgb, var(--color-tool-error) 40%, transparent);
}

.worker-merge-controls__btn--discard:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
}

/* Spinner — reuses the project's rotation convention. The
   0.6s linear infinite matches ChatInput's chat-input-spin
   (the only `linear` animation in the codebase per
   design-tokens.md). Kept inline here because it's a 12px
   inline spinner, not a full loader surface. */
.worker-merge-controls__spinner {
  width: 12px;
  height: 12px;
  border-radius: var(--radius-pill);
  border: 1.5px solid currentColor;
  border-top-color: transparent;
  animation: worker-merge-controls-spin 0.6s linear infinite;
}

@keyframes worker-merge-controls-spin {
  to {
    transform: rotate(360deg);
  }
}

/* Conflict display — read-only file list + git CLI hint. Mirrors
   the DrawerPermissionAskCard's chrome (left-border tint +
   dark surface) so the visual reads as "warning, attention". */
.worker-merge-controls__conflict {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px 10px;
  border-radius: var(--radius-sm);
  border-left: 3px solid var(--color-tool-error);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
}

.worker-merge-controls__conflict-header {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--color-tool-error);
  font-family: var(--font-sans);
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
}

.worker-merge-controls__conflict-list {
  margin: 0;
  padding-left: 18px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-height: 140px;
  overflow-y: auto;
}

.worker-merge-controls__conflict-list code {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-primary);
  background: var(--color-bg-elevated);
  padding: 1px 4px;
  border-radius: 3px;
  word-break: break-all;
}

.worker-merge-controls__conflict-empty {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-style: italic;
}

.worker-merge-controls__conflict-hint {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}
</style>
