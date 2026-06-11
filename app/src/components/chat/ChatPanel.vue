<script setup lang="ts">
// ChatPanel — the right-side main content area when a project is
// active. Renders a header (current session title + model + git
// indicator) above and the input region below; the middle is the
// MessageList. The empty state (no messages yet) shows a welcome
// with the current project's name and any git/legacy warnings.
//
// D6 header: replaced the static "Everlasting / vibe coding
// workbench / cwd" trio with a per-session header that shows the
// session title (or "新对话" when none) plus two small chips: the
// model name and the project's current git branch. The git chip
// is hidden when the project is not a git repo; otherwise it
// shows the branch name (e.g. `main`, `feature/foo`, or the
// literal `HEAD` for a detached-HEAD repo).
//
// PR1 spike-005 follow-up: header is now a 28px-tall compact row
// (padding 6px + content), the session title is 13px, and a new
// `.chat-panel__chip--cwd` chip is pushed to the right showing
// `chatStore.simplifiedCwd` (prepared by PR3; e.g. `~/code/foo`).
//
// Step 4 follow-up: the diff chip is replaced by a tri-state
// worktree chip with a dropdown menu (see `WorktreeChip.vue`):
//   - `none` (no worktree ever) → "attach worktree" button
//   - `active` (worktree bound)  → "diff (N)" + dropdown with
//     copy-path / copy-branch / detach / delete
//   - `detached` (was active)    → "上次 worktree" + dropdown
//     with the same actions (the file diff is from the stale
//     worktree on disk; the copy buttons still work; detach and
//     delete are still meaningful).
//
// 8-PR3 split: the worktree chip + dropdown moved to
// `WorktreeChip.vue`; the diff overlay moved to `DiffModal.vue`.
// This file now owns the session / project header state and the
// action handlers (attach / detach / delete + confirm modal),
// and delegates the chip + diff UI to the new components.

import { computed, onUnmounted, ref } from "vue";
import { useChatStore, type SessionSummary } from "../../stores/chat";
import { useProjectsStore } from "../../stores/projects";
import MessageList from "./MessageList.vue";
import ChatInput from "./ChatInput.vue";
import DeleteWorktreeConfirm from "./DeleteWorktreeConfirm.vue";
import WorktreeChip, { type WorktreeState } from "./WorktreeChip.vue";
import DiffModal from "./DiffModal.vue";
import MemoryModal from "../memory/MemoryModal.vue";
import Icon from "../Icon.vue";

const chatStore = useChatStore();
const projectsStore = useProjectsStore();

const emit = defineEmits<{
  send: [text: string];
}>();

const hasMessages = computed(() => chatStore.messages.length > 0);

/** PR5: forwarded to `chatStore.cancel()` so the parent can keep
 *  the ChatInput → ChatPanel → store flow symmetric with `send`. */
function onStop() {
  void chatStore.cancel();
}

/** The currently active session, if any. Looked up by id against
 *  the sessions list (the chat store only tracks the id; the full
 *  record lives in the list). */
const currentSession = computed<SessionSummary | null>(() => {
  const id = chatStore.currentSessionId;
  if (!id) return null;
  return chatStore.sessions.find((s) => s.id === id) ?? null;
});

/** Display title for the header: the session's stored title, or a
 *  "新对话" placeholder for the no-session-yet state. */
const currentSessionTitle = computed<string>(
  () => currentSession.value?.title || "新对话",
);

const currentProject = computed(() =>
  projectsStore.projectById(projectsStore.currentProjectId),
);

/** Git branch chip is rendered when the project is a git repo. The
 *  label is the project's `git_branch` (e.g. `main`, `feature/foo`).
 *  For detached-HEAD repos `git_branch` is the literal string
 *  `"HEAD"` — we render that as-is so the user can distinguish
 *  detached state from a real branch named "HEAD". v1 does not
 *  decorate detached HEAD with a short SHA. Falls back to the
 *  legacy static "git" tag if the project row hasn't been
 *  re-probed yet (older rows pre-PR2). */
const showGitChip = computed<boolean>(
  () => !!currentProject.value?.is_git_repo,
);

const gitBranchLabel = computed<string>(() => {
  const branch = currentProject.value?.git_branch;
  return branch && branch.length > 0 ? branch : "git";
});

// -----------------------------------------------------------------------
// Step 4 / PR3: session-level diff modal (state only — UI moved
// to `DiffModal.vue` in 8-PR3).
// -----------------------------------------------------------------------

const diffModalOpen = ref(false);
const diffLoading = ref(false);
const diffError = ref<string | null>(null);
const diffResult = ref<{ files: import("./DiffView.vue").FileDiff[] } | null>(
  null,
);

async function openDiffModal() {
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  diffModalOpen.value = true;
  diffError.value = null;
  diffResult.value = null;
  diffLoading.value = true;
  try {
    diffResult.value = await chatStore.fetchDiff(sid);
  } catch (e) {
    diffError.value = e instanceof Error ? e.message : String(e);
  } finally {
    diffLoading.value = false;
  }
}

function closeDiffModal() {
  diffModalOpen.value = false;
}

// -----------------------------------------------------------------------
// Step 4 follow-up: tri-state worktree chip + dropdown
// (UI moved to `WorktreeChip.vue` in 8-PR3; this file owns the
// state derivation + action handlers).
// -----------------------------------------------------------------------

/** Reactive count of files in the current session's diff. Reads
 *  the cache (no IPC) so the chip can show "diff (3 files)"
 *  before the user clicks to open the modal. Falls back to "diff"
 *  when nothing is cached yet OR for pre-step-4 sessions. */
const diffFileCount = computed<number | null>(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return null;
  const cached = chatStore.getDiff(sid);
  if (!cached) return null;
  return cached.files.length;
});

const worktreeState = computed<WorktreeState>(
  () => currentSession.value?.worktree_state ?? "none",
);

/** Per-state worktree chip label. Mirrors the PR3 (single
 *  "diff" button) UX for `active`, and adds two new shapes for
 *  `none` and `detached`. */
const worktreeChipLabel = computed<string>(() => {
  const state = worktreeState.value;
  if (state === "none") return "attach worktree";
  if (state === "detached") {
    const n = diffFileCount.value;
    if (n === null) return "上次 worktree";
    if (n === 0) return "上次 worktree (clean)";
    return `上次 worktree (${n})`;
  }
  // active
  const n = diffFileCount.value;
  if (n === null) return "diff";
  if (n === 0) return "diff (clean)";
  return `diff (${n})`;
});

const worktreeChipTitle = computed<string>(() => {
  const state = worktreeState.value;
  if (state === "none") {
    if (!currentProject.value?.is_git_repo) {
      return "This project isn't a git repo";
    }
    return "Attach a worktree to isolate this session's changes";
  }
  if (state === "detached") {
    return "This session has a detached worktree (preserved on disk)";
  }
  const n = diffFileCount.value;
  if (n === null) return "View the diff for this session";
  if (n === 0) return "No changes in this session yet";
  return `View ${n} ${n === 1 ? "file" : "files"} changed in this session`;
});

/** Show the worktree chip at all? The chip is hidden when no
 *  session is active. We DO render the chip for sessions on
 *  non-git projects: the "attach worktree" button is replaced
 *  with a disabled state in the menu (the backend refuses
 *  non-git attach). */
const showWorktreeChip = computed<boolean>(() => !!chatStore.currentSessionId);

const isStreaming = computed<boolean>(
  () => chatStore.isCurrentSessionStreaming,
);

/** The branch name for the active/detached session. The Rust
 *  side always names it `session/<session_id>` — re-deriving it
 *  client-side keeps the copy buttons honest. */
const branchName = computed<string>(
  () => `session/${chatStore.currentSessionId ?? ""}`,
);

/** The worktree path that's currently "live" for the session.
 *  Active: `worktree_path`. Detached: `last_worktree_path`.
 *  None: `null` (the chip's "copy path" menu item is hidden). */
const worktreePathForDisplay = computed<string | null>(() => {
  const s = currentSession.value;
  if (!s) return null;
  if (s.worktree_state === "active") return s.worktree_path;
  if (s.worktree_state === "detached") return s.last_worktree_path;
  return null;
});

/** Click on the chip itself: for `active` we open the diff; for
 *  `none` we attach; for `detached` we open the diff (the
 *  diff still reflects the on-disk state). The dropdown is the
 *  second-click path; single-click is the most common path so
 *  it goes straight to the primary action. */
function onChipClick() {
  const state = worktreeState.value;
  if (state === "none") {
    void onAttach();
    return;
  }
  // active or detached: open the diff modal directly.
  void openDiffModal();
}

async function onAttach() {
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  try {
    await chatStore.attachWorktree(sid);
    projectsStore.showToast("worktree 已附加", "info", 2000);
  } catch {
    // Toast already shown by the store on error.
  }
}

async function onDetach() {
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  try {
    await chatStore.detachWorktree(sid);
    projectsStore.showToast("worktree 已解绑", "info", 2000);
  } catch {
    // Toast already shown by the store on error.
  }
}

/** Delete worktree — confirm modal only for `active`+`has_diff`;
 *  one-click for the other two paths. */
const confirmDeleteOpen = ref(false);

function onDeleteClick() {
  const state = worktreeState.value;
  const hasDiff = state === "active" && (diffFileCount.value ?? 0) > 0;
  if (hasDiff) {
    confirmDeleteOpen.value = true;
    return;
  }
  void onDeleteConfirm();
}

async function onDeleteConfirm() {
  const sid = chatStore.currentSessionId;
  if (!sid) {
    confirmDeleteOpen.value = false;
    return;
  }
  confirmDeleteOpen.value = false;
  try {
    await chatStore.deleteWorktree(sid);
    projectsStore.showToast("worktree 已删除", "info", 2000);
  } catch {
    // Toast already shown by the store on error.
  }
}

function onDeleteCancel() {
  confirmDeleteOpen.value = false;
}

// -----------------------------------------------------------------------
// Memory entry (2026-06-11, `06-11-memory-modal-appheader-entry`)
// -----------------------------------------------------------------------
//
// The Memory entry was originally a hand-rolled popover on ProjectTabs;
// its `right: 0; min-width: 480px` anchor strategy spilled off-screen
// when the trigger wasn't at the viewport's right edge. The follow-up
// task moved it here — a Brain icon button next to WorktreeChip opens
// a reka-ui Dialog modal (`MemoryModal.vue`) showing the active
// project's CLAUDE.md / AGENTS.md.
//
// Implementation note: the button is only meaningful when a project is
// active. We gate on `projectsStore.currentProjectId` (matching the
// ProjectTabs dropdown's old visibility rule).

const memoryModalOpen = ref(false);

/** Esc key handling — closes whichever popup is on top: delete
 *  confirm → worktree dropdown → diff modal. Popovers inside
 *  `WorktreeChip` handle their own Esc when focused.
 *  (WorktreeChip's own keydown listener is local; we keep this
 *  here as a top-level fallback for when the chip doesn't catch
 *  the key first.) */
function onKeyDown(e: KeyboardEvent) {
  if (e.key === "Escape") {
    if (confirmDeleteOpen.value) {
      onDeleteCancel();
      return;
    }
    if (diffModalOpen.value) {
      closeDiffModal();
    }
  }
}

if (typeof window !== "undefined") {
  window.addEventListener("keydown", onKeyDown);
  onUnmounted(() => window.removeEventListener("keydown", onKeyDown));
}
</script>

<template>
  <section class="chat-panel">
    <header class="chat-panel__header">
      <div class="chat-panel__title-row">
        <h1 class="chat-panel__title">{{ currentSessionTitle }}</h1>
        <span
          v-if="showGitChip"
          class="chat-panel__chip chat-panel__chip--git"
          :title="`Current branch: ${gitBranchLabel}`"
        >
          <Icon name="refresh" :size="12" />
          {{ gitBranchLabel }}
        </span>
        <span
          v-if="chatStore.simplifiedCwd"
          class="chat-panel__chip chat-panel__chip--cwd"
          :title="chatStore.simplifiedCwd"
        >
          <Icon name="folder" :size="12" />
          {{ chatStore.simplifiedCwd }}
        </span>
        <!--
                  Step 4 follow-up: tri-state worktree chip with
                  dropdown. The chip itself is the primary action
                  (open diff / attach), the dropdown is for the
                  secondary actions (copy path / branch / detach /
                  delete). 8-PR3: UI extracted to `WorktreeChip.vue`.
                -->
        <WorktreeChip
          v-if="showWorktreeChip"
          :state="worktreeState"
          :chip-label="worktreeChipLabel"
          :chip-title="worktreeChipTitle"
          :branch-name="branchName"
          :path-for-display="worktreePathForDisplay"
          :is-streaming="isStreaming"
          @chip-click="onChipClick"
          @detach-click="onDetach"
          @delete-click="onDeleteClick"
        />
        <button
          v-if="projectsStore.currentProjectId"
          class="chat-panel__memory-btn"
          type="button"
          title="查看项目指令文件 (CLAUDE.md / AGENTS.md)"
          aria-label="Memory"
          @click="memoryModalOpen = true"
        >
          <Icon name="brain" :size="14" />
        </button>
      </div>
    </header>

    <main class="chat-panel__main">
      <!-- F4: loading spinner while switching sessions -->
      <div v-if="chatStore.sessionLoading" class="chat-panel__loading">
        <div class="chat-panel__spinner" />
      </div>
      <div v-else-if="!hasMessages" class="chat-panel__empty">
        <p>输入一句话,跟 LLM 聊聊看</p>
        <p class="chat-panel__empty-hint">中文输入测试 + 流式响应 + 工具调用</p>
        <p v-if="currentProject" class="chat-panel__empty-project">
          当前项目: <strong>{{ currentProject.name }}</strong>
          <span
            v-if="!currentProject.is_git_repo"
            class="chat-panel__empty-warn"
          >
            <Icon name="warn" :size="11" />
            非 git 项目,无法附加 worktree
          </span>
          <span
            v-else-if="currentProject.is_legacy"
            class="chat-panel__empty-warn"
          >
            <Icon name="archive" :size="11" />
            旧数据,自动归入
          </span>
        </p>
      </div>
      <MessageList v-else />
    </main>

    <ChatInput
      :sending="chatStore.isCurrentSessionStreaming"
      @send="emit('send', $event)"
      @stop="onStop"
    />

    <!--
          Step 4 / PR3: session-level diff modal. 8-PR3: UI
          extracted to `DiffModal.vue`. State (open / loading /
          error / result) stays here.
        -->
    <DiffModal
      :is-open="diffModalOpen"
      :is-loading="diffLoading"
      :error="diffError"
      :result="diffResult"
      @close="closeDiffModal"
    />

    <!--
          Step 4 follow-up: confirmation modal for delete_worktree.
          Rendered only when the user clicks Delete in the dropdown
          AND the session is `active` with at least one changed
          file. Other paths skip the confirm.
        -->
    <DeleteWorktreeConfirm
      :open="confirmDeleteOpen"
      :file-count="diffFileCount ?? 0"
      @cancel="onDeleteCancel"
      @confirm="onDeleteConfirm"
    />

    <!--
          Memory entry (2026-06-11). See the script comment above
          for context. The modal handles its own focus trap / ESC /
          outside-click close via reka-ui Dialog.
        -->
    <MemoryModal v-model:open="memoryModalOpen" />
  </section>
</template>

<style scoped>
.chat-panel {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
  min-width: 0;
  background: var(--color-bg-app);
}

.chat-panel__header {
  display: flex;
  align-items: center;
  padding: 6px 20px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  flex-shrink: 0;
}

.chat-panel__loading {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}

.chat-panel__spinner {
  width: 20px;
  height: 20px;
  border: 2px solid var(--color-bg-border);
  border-top-color: var(--color-accent);
  border-radius: 50%;
  animation: spin 0.6s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}
  min-width: 0;
}

.chat-panel__title-row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
  flex: 1;
  flex-wrap: wrap;
}

.chat-panel__title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 50vw;
}

.chat-panel__chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  color: var(--color-text-secondary);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  padding: 2px 8px;
  border-radius: 4px;
  font-family: var(--font-mono);
  white-space: nowrap;
}

.chat-panel__chip--git {
  color: var(--color-accent);
  border-color: var(--color-accent-muted);
}

.chat-panel__chip--cwd {
  margin-left: auto;
  max-width: 50%;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* Memory entry button (2026-06-11). Sits to the right of the
   WorktreeChip, after the cwd chip's `margin-left: auto` has
   pushed everything from cwd onward to the right. Visual matches
   the chip family (small, 11px-ish height) but uses an icon
   instead of text. */
.chat-panel__memory-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 24px;
  height: 22px;
  padding: 0;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-text-secondary);
  cursor: pointer;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
  font-family: inherit;
}

.chat-panel__memory-btn:hover {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.chat-panel__memory-btn:active {
  background: var(--color-bg-border);
}

.chat-panel__main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
  padding: 20px 4px 0px 20px;
  overflow: hidden;
}

.chat-panel__empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: var(--color-text-secondary);
  text-align: center;
  max-width: 480px;
  margin: auto;
  padding: 32px 16px;
  gap: 4px;
}

.chat-panel__empty-hint {
  font-size: 12px;
  color: var(--color-text-muted);
}

.chat-panel__empty-project {
  font-size: 12px;
  color: var(--color-text-secondary);
  margin-top: 12px;
  display: inline-flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: center;
}

.chat-panel__empty-warn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--color-tool-shell);
  font-size: 11px;
}
</style>
