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

import { extractErrorMessage } from "../../utils/useErrorBus";
import { computed, onUnmounted, ref, watch } from "vue";
import { useChatStore } from "../../stores/chat";
import type { SessionSummary } from "../../stores/chat.types";
import { useProjectsStore } from "../../stores/projects";
import { useChecklistStore } from "../../stores/checklist";
import { useQuestionCardsStore } from "../../stores/questionCards";
import { useMemoryStore } from "../../stores/memory";
import {
  useReviewStateStore,
} from "../../stores/reviewState";
import { transport } from "../../transport";
import type { CurrentTaskInfo } from "../../types/review-state";
import { useTraceStore } from "../../stores/traceStore";
import MessageList from "./MessageList.vue";
import ChatInput from "./ChatInput.vue";
import DeleteWorktreeConfirm from "./DeleteWorktreeConfirm.vue";
import WorktreeChip, { type WorktreeState } from "./WorktreeChip.vue";
import DiffModal from "./DiffModal.vue";
import MemoryModal from "../memory/MemoryModal.vue";
import RuntimeMemoryModal from "../memory/RuntimeMemoryModal.vue";
import GroupChatConfigModal from "./GroupChatConfigModal.vue";
import AuditLogModal from "../audit/AuditLogModal.vue";
import PermissionGrantsModal from "../permissions/PermissionGrantsModal.vue";
import ChecklistCard from "./ChecklistCard.vue";
import AskUserQuestionCard from "./AskUserQuestionCard.vue";
import WorkerAskBanner from "./WorkerAskBanner.vue";
import ReviewMatrix from "./ReviewMatrix.vue";
import Icon from "../Icon.vue";

const chatStore = useChatStore();
const projectsStore = useProjectsStore();
const checklistStore = useChecklistStore();
const questionCardsStore = useQuestionCardsStore();
const memoryStore = useMemoryStore();
const reviewStateStore = useReviewStateStore();
const traceStore = useTraceStore();

const emit = defineEmits<{
  send: [text: string];
}>();

const hasMessages = computed(() => chatStore.messages.length > 0);

/** C2+ loop-intervention pending for the current session, if any.
 *  The backend registers a `LoopIntervention` PendingInteraction
 *  (via `tool:question` event with `tool_use_id=loop_intervention_N`)
 *  when the agent loop hits ≥3 consecutive loop-detection hits. We
 *  render it as a FLOATING card here (not under a tool_use block)
 *  because the synthetic `tool_use_id` has no matching
 *  `ask_user_question` tool_use in the message stream — anchoring
 *  under a tool_use (as MessageItem does for real asks) would never
 *  match and silently drop the intervention (2026-07-28 incident,
 *  session e8a1ad96…). */
const loopIntervention = computed(
  () => {
    const sid = chatStore.currentSessionId;
    if (!sid) return null;
    const pending = questionCardsStore.getPending(sid);
    return pending && pending.kind === "loop_intervention"
      ? pending.payload
      : null;
  },
);

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

// Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E8):
// reactive indicators for the chat header. `isGroupChat` gates
// the group-chat indicator + "编辑参与者" button (and the
// edit-mode modal mounted below). `groupChatParticipants`
// is the per-session parsed roster that feeds the modal's
// `initialParticipants` prop for the edit flow.
const isGroupChat = computed(
  () => currentSession.value?.session_type === "group_chat",
);
const groupChatParticipants = computed(
  () => chatStore.currentSessionParticipants,
);

const groupChatEditOpen = ref<boolean>(false);
function openGroupChatEdit() {
  if (!isGroupChat.value) return;
  groupChatEditOpen.value = true;
}

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
 *  re-probed yet (older rows pre-PR2).
 *
 *  2026-06-27 polish: the previous fallback was the literal string
 *  `"git"` (e.g. `git` shown in the header chip). That read as
 *  "this project's branch is named 'git'" — confusing. The fallback
 *  now renders `—` (em-dash) with a tooltip explaining the row
 *  hasn't been probed yet, so the chip reads as "branch unknown"
 *  rather than "branch = 'git'". The pre-existing "HEAD" string
 *  for detached-HEAD repos still passes through unchanged (it's a
 *  real git concept and useful to surface). */
const showGitChip = computed<boolean>(
  () => !!currentProject.value?.is_git_repo,
);

const gitBranchLabel = computed<string>(() => {
  const branch = currentProject.value?.git_branch;
  return branch && branch.length > 0 ? branch : "—";
});

/** Tooltip for the branch chip. Surfaces the "branch unknown"
 *  explanation when the project row hasn't been probed yet, so
 *  the `—` fallback doesn't read as a missing data bug. */
const gitBranchTooltip = computed<string>(() => {
  const branch = currentProject.value?.git_branch;
  if (branch && branch.length > 0) return `Current branch: ${branch}`;
  return "Branch unknown — project row not yet probed (open the project again or restart the app)";
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
    diffError.value = e instanceof Error ? e.message : extractErrorMessage(e);
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

/** D (2026-06-30): publish the session branch into `main` (local
 *  only, no push). The store surfaces success/conflict toasts. */
async function onPublish() {
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  try {
    await chatStore.publishSessionToMain(sid);
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

// -----------------------------------------------------------------------
// 07-06 (am-observability-panel B4/R2b): real-time recall chip
// -----------------------------------------------------------------------
//
// The agent loop emits `ChatEvent::Recall { hits }` at the FTS
// recall site (turn start) + each pitfall-recall site (on tool
// dispatch). `streamController.handleChatEvent` routes the hits
// into `memoryStore.recallHitsBySession`; this chip reads the
// current session's slice. The chip renders ABOVE MessageList
// (MessageList has no banner slot — see chat.md; ChatPanel is the
// designated host for cross-cutting overlays).
//
// The chip is collapsed by default ("🧠 本次召回 N 条"); clicking
// expands a per-source group (fts / pitfall) of titles (design D7).
// The slice is cleared on each new user message (startRequest), so
// it reflects "本次召回" not a running total.
const recallHits = computed(() =>
  memoryStore.recallHitsForSession(chatStore.currentSessionId),
);
const recallExpanded = ref(false);
const ftsHits = computed(() =>
  recallHits.value.filter((h) => h.source === "fts"),
);
const pitfallHits = computed(() =>
  recallHits.value.filter((h) => h.source === "pitfall"),
);

// 07-06 (am-observability-panel B3/R3): the RuntimeMemoryModal host
// wiring. MemoryPreview emits `manage(id)` on row click; the
// `onMemoryManage` handler resolves the row + opens this modal.
// `runtimeMemoryModalOpen` gates the dialog; `managedMemoryId`
// carries the row's SQLite auto-id (resolved to the full row via
// computed `managedMemory`). The modal closes if the row vanishes
// (e.g. deleted from another surface).
const runtimeMemoryModalOpen = ref(false);
const managedMemoryId = ref<number | null>(null);
const managedMemory = computed(
  () =>
    memoryStore.runtimeMemories.find(
      (m) => m.id === managedMemoryId.value,
    ) ?? null,
);

function onMemoryManage(id: number) {
  managedMemoryId.value = id;
  runtimeMemoryModalOpen.value = true;
}

// -----------------------------------------------------------------------
// Audit entry (C4 audit-log UI, 2026-06-14 PR2)
// -----------------------------------------------------------------------
//
// A shield icon button next to the Memory button opens the
// AuditLogModal. The modal is bound to the CURRENT session (not
// the project), so it's `v-if`'d on `chatStore.currentSessionId`
// — Memory uses `projectsStore.currentProjectId` because memory
// files live at the project level, whereas audit events live at
// the session level. When the user switches session while the
// modal is open, the `watch(currentSessionId)` below closes it
// (PRD edge case "切 session 时 Modal 开着" → 关闭 Modal,换
// 上下文).

const auditModalOpen = ref(false);

// -----------------------------------------------------------------------
// Permission-grant management entry (task 07-01-permission-grant-list-ui)
// -----------------------------------------------------------------------
//
// A key icon button next to the Audit button opens the
// PermissionGrantsModal. Same session-scoped gating as audit
// (v-if on currentSessionId) — grants live at the session level.
// The watcher below closes it on session switch.
const grantsModalOpen = ref(false);

// -----------------------------------------------------------------------
// B12 Checklist (PR2 frontend, 2026-06-19): the current session's
// checklist. The card reads this off the checklist store + current
// session id. `null` hides the card (no update_checklist seen this
// run); an empty array renders the empty placeholder. Switching
// sessions is handled by the `currentSessionId` dependency — the
// computed re-evaluates and the card reflects the new session's
// checklist (or hides if none).
// -----------------------------------------------------------------------
const currentChecklist = computed(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return null;
  return checklistStore.getChecklist(sid);
});

watch(
  () => chatStore.currentSessionId,
  () => {
    // Switching session while the audit modal is open closes it
    // so the next open rebinds to the new session (the modal's
    // `boundSessionId` reads `chatStore.currentSessionId` at
    // open time, so closing here forces the next open to use
    // the new id).
    if (auditModalOpen.value) {
      auditModalOpen.value = false;
    }
    // Same close-on-switch for the grants modal (task 07-01).
    if (grantsModalOpen.value) {
      grantsModalOpen.value = false;
    }
    // C2 (review visualization view, 2026-07-26): tear down the
    // review-state subscription on session switch so a stale
    // debounce can't land on the new session. The new session's
    // review-state (if any) is started by the watcher below
    // (`watchEffect` on the review-session gate).
    reviewStateStore.stop();
  },
  { immediate: true },
);

/** C2 (review visualization view, 2026-07-26): gate for whether
 *  the `<ReviewMatrix>` panel should render at all. Three
 *  conditions:
 *    1. the session has `workflow_enabled` (a workflow session),
 *    2. the session's `plugin_name === "review"` (the review
 *       workflow — dev sessions don't render the panel),
 *    3. `reviewStateStore.state` is loaded OR `error` is set
 *       (missing file → both null → panel hidden silently, per
 *       PRD R5 + design §5).
 *
 *  Non-review sessions: zero impact (the watcher below never
 *  calls `start`, so the store stays empty). */
const isReviewSession = computed<boolean>(
  () =>
    !!currentSession.value &&
    !!currentSession.value.workflow_enabled &&
    currentSession.value.plugin_name === "review",
);

const shouldShowReviewMatrix = computed<boolean>(
  () => isReviewSession.value && (!!reviewStateStore.state || !!reviewStateStore.error),
);

/** Start the review-state subscription for the active session.
 *  Called from the watcher below whenever `isReviewSession`
 *  flips true (mount / switch into a review session). Reads the
 *  current task slug via `get_current_task_slug` (frontend has no
 *  task-slug state of its own — design §10.1). */
async function startReviewState(): Promise<void> {
  const cwd = currentSession.value?.current_cwd ?? "";
  if (!cwd) return;
  try {
    const info = await transport.invoke<CurrentTaskInfo | null>(
      "get_current_task_slug",
      { projectPath: cwd },
    );
    if (!info) return; // no active task — panel stays hidden
    await reviewStateStore.start(info.slug, cwd);
  } catch (e) {
    // IPC failure (daemon down / project lookup). Stay silent —
    // the panel just doesn't render. The user can switch sessions
    // to retry.
    console.error("ChatPanel: get_current_task_slug failed:", e);
  }
}

// Watch the review-session gate. On flip-to-true, start; on
// flip-to-false, stop. `immediate: true` covers the first mount
// (the watcher fires synchronously with the current value).
watch(
  isReviewSession,
  (isReview) => {
    if (isReview) {
      void startReviewState();
    } else {
      reviewStateStore.stop();
    }
  },
  { immediate: true },
);

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

// C2: tear down the review-state subscription on unmount so a
// pending debounce can't fire after the panel is gone.
onUnmounted(() => reviewStateStore.stop());
</script>

<template>
  <section class="chat-panel">
    <header class="chat-panel__header">
      <div class="chat-panel__title-row">
        <h1 class="chat-panel__title">{{ currentSessionTitle }}</h1>
        <!--
          Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E8/F5):
          show a "群聊 (N participants)" indicator chip on the
          session title row when the active session is a
          group_chat session. Tells the user at a glance which
          session type is active (the title alone is identical
          to a classic chat title). Clicking the chip opens
          the edit modal — same affordance as the dedicated
          "编辑参与者" button (below), so the user has 2
          discoverable paths to the same action.
        -->
        <button
          v-if="isGroupChat"
          class="chat-panel__chip chat-panel__chip--group-chat mobile-hide-group-chat"
          type="button"
          title="编辑参与者"
          aria-label="编辑群聊参与者"
          data-testid="chat-panel-group-chat-edit"
          @click="openGroupChatEdit"
        >
          <Icon name="users" :size="12" />
          群聊 ({{ groupChatParticipants?.length ?? 0 }} 参与者)
        </button>
        <span
          v-if="showGitChip"
          class="chat-panel__chip chat-panel__chip--git mobile-hide-git"
          :title="gitBranchTooltip"
        >
          <Icon name="refresh" :size="12" />
          {{ gitBranchLabel }}
        </span>
        <span
          v-if="chatStore.simplifiedCwd"
          class="chat-panel__chip chat-panel__chip--cwd mobile-hide-cwd"
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
          class="mobile-hide-worktree"
          :state="worktreeState"
          :chip-label="worktreeChipLabel"
          :chip-title="worktreeChipTitle"
          :branch-name="branchName"
          :path-for-display="worktreePathForDisplay"
          :is-streaming="isStreaming"
          @chip-click="onChipClick"
          @publish-click="onPublish"
          @detach-click="onDetach"
          @delete-click="onDeleteClick"
        />
        <div class="chat-panel__title-actions">
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
        <!--
                  08-14 ux-polish-r1 WP3 3.3(评审 C2):图标区按作用域分组
                  —— memory 是项目级入口,gated on currentProjectId;后面
                  audit/trace/grants 是会话级 inspector 组,gated on
                  currentSessionId(各自的 v-if 注释里已写明)。两者之间放
                  1px 竖分隔线。v-if 要求两端都渲染,避免无项目/无会话时
                  悬空一条线。移动端分组逻辑不变,分隔线保留(与 WP1 收纳
                  规则无冲突:收纳只隐藏 chips,不动图标区)。
                -->
        <span
          v-if="projectsStore.currentProjectId && chatStore.currentSessionId"
          class="chat-panel__action-divider"
          aria-hidden="true"
        />
        <!--
                  C4 audit-log entry (2026-06-14 PR2). Sits next to
                  the Memory button but is gated on the CURRENT
                  SESSION (not project) — audit events are scoped to
                  a session via the `session_audit_events.session_id`
                  FK. The watcher above closes the modal if the user
                  switches session while it's open.
                -->
        <button
          v-if="chatStore.currentSessionId"
          class="chat-panel__audit-btn"
          type="button"
          title="查看会话审计日志"
          aria-label="Audit"
          @click="auditModalOpen = true"
        >
          <Icon name="shield-check" :size="14" />
        </button>
        <!--
                  E2 (harness trace pipeline, 2026-07-14): trace
                  timeline toggle. Sits next to the audit button —
                  same session scope (reads currentSessionId), same
                  chip-family icon button. Opens the `<TracePanel>`
                  right-side drawer for the current session; the
                  drawer renders both the live in-flight trace
                  events (per-turn compaction / loop hint / workflow
                  breadcrumb) and the 回看 history from
                  `turn_trace` + `session_audit_events.turn_seq`.
                  The drawer itself is mounted at the AppShell
                  level (sibling of the main slot) so it survives
                  session-switch and the empty-state mount. This
                  button is just the toggle.
                -->
        <button
          v-if="chatStore.currentSessionId"
          class="chat-panel__trace-btn"
          type="button"
          :title="traceStore.panelOpen ? '关闭 trace 时间线' : '打开 trace 时间线'"
          :aria-label="traceStore.panelOpen ? 'Close trace' : 'Open trace'"
          @click="traceStore.togglePanel()"
        >
          <Icon name="chart" :size="14" />
        </button>
        <!--
                  Permission-grant management entry (task
                  07-01-permission-grant-list-ui). Sits next to the
                  Audit button — same session scope, same chip-family
                  icon button. Opens the PermissionGrantsModal which
                  lists + revokes the session's "always allow" rows.
                -->
        <button
          v-if="chatStore.currentSessionId"
          class="chat-panel__grants-btn"
          type="button"
          title="管理「始终允许」放行"
          aria-label="Permissions"
          @click="grantsModalOpen = true"
        >
          <Icon name="key" :size="14" />
        </button>
        <!--
                  PR2 RULE-FrontSubagent-003 (2026-06-22): worker ask
                  banner. Sits next to the audit button — same visual
                  row, same session scope (reads currentSessionId).
                  Non-blocking: clicks open the SubagentDrawer for
                  the most-recent pending worker ask; doesn't steal
                  focus or overlay the chat. Hidden when no worker
                  asks are pending for this session.
                -->
        <WorkerAskBanner />
        </div>
      </div>
    </header>

    <!-- 07-06 (am-observability-panel B4/R2b): real-time recall
         chip. Renders only when the current session has accumulated
         recall hits this turn (hitCount > 0). Sits between the
         header and the message list — a thin banner so it doesn't
         push the conversation down. Collapsed by default; click
         expands per-source groups (fts / pitfall) of titles.
         Cleared on each new user message (startRequest). -->
    <div
      v-if="recallHits.length > 0"
      class="chat-panel__recall"
      :class="{ 'chat-panel__recall--expanded': recallExpanded }"
    >
      <button
        type="button"
        class="chat-panel__recall-toggle"
        @click="recallExpanded = !recallExpanded"
      >
        <Icon name="brain" :size="12" />
        本次召回 {{ recallHits.length }} 条
        <Icon :name="recallExpanded ? 'chevron-up' : 'chevron-down'" :size="10" />
      </button>
      <div v-if="recallExpanded" class="chat-panel__recall-groups">
        <div v-if="ftsHits.length > 0" class="chat-panel__recall-group">
          <span class="chat-panel__recall-source chat-panel__recall-source--fts">
            语义
          </span>
          <ul class="chat-panel__recall-list">
            <li
              v-for="(h, i) in ftsHits"
              :key="`fts-${i}-${h.memory_id}`"
              class="chat-panel__recall-item"
            >
              {{ h.title }}
            </li>
          </ul>
        </div>
        <div v-if="pitfallHits.length > 0" class="chat-panel__recall-group">
          <span class="chat-panel__recall-source chat-panel__recall-source--pitfall">
            陷阱
          </span>
          <ul class="chat-panel__recall-list">
            <li
              v-for="(h, i) in pitfallHits"
              :key="`pitfall-${i}-${h.memory_id}`"
              class="chat-panel__recall-item"
            >
              {{ h.title }}
            </li>
          </ul>
        </div>
      </div>
    </div>

    <main class="chat-panel__main">
      <!-- C2 (review visualization view, 2026-07-26): the
           `<ReviewMatrix>` panel renders above the message list
           for review workflow sessions. Pure display — the
           `shouldShowReviewMatrix` gate ensures dev / non-review
           sessions see zero impact. -->
      <ReviewMatrix v-if="shouldShowReviewMatrix" />
      <!-- F4: loading state while switching sessions.
           PR-3e (2026-06-27): replaced the 0.6s rotating 20px
           spinner (which left the entire message area blank
           while loading) with a 3-placeholder skeleton list.
           The skeleton mirrors the visual shape of a real
           user → assistant turn (right-aligned short user
           bubble + two left-aligned assistant bubbles of
           varied width) so the user sees the "list structure"
           loading rather than a void. Shimmer animation uses
           a 1.5s linear-gradient sweep that collapses to
           static under prefers-reduced-motion (PR-1 @media). -->
      <div v-if="chatStore.sessionLoading" class="chat-panel__skeleton" aria-busy="true">
        <div class="chat-panel__skeleton-msg chat-panel__skeleton-msg--user">
          <div class="chat-panel__skeleton-bubble chat-panel__skeleton-bubble--short" />
        </div>
        <div class="chat-panel__skeleton-msg chat-panel__skeleton-msg--assistant">
          <div class="chat-panel__skeleton-bubble chat-panel__skeleton-bubble--wide" />
          <div class="chat-panel__skeleton-bubble chat-panel__skeleton-bubble--narrow" />
        </div>
      </div>
      <div v-else-if="!hasMessages" class="chat-panel__empty">
        <div class="chat-panel__empty-icon" aria-hidden="true">
          <Icon name="thinking" :size="28" />
        </div>
        <p class="chat-panel__empty-title">开始对话</p>
        <p class="chat-panel__empty-hint">描述任务，跟 LLM 聊聊看</p>
        <p v-if="currentProject" class="chat-panel__empty-project">
          当前项目: <strong>{{ currentProject.name }}</strong>
          <span
            v-if="!currentProject.is_git_repo"
            class="chat-panel__empty-warn"
          >
            <Icon name="warn" :size="12" />
            非 git 项目，无法附加 worktree
          </span>
          <span
            v-else-if="currentProject.is_legacy"
            class="chat-panel__empty-warn"
          >
            <Icon name="archive" :size="12" />
            旧数据，自动归入
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
      Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E8):
      edit-mode modal. Mounted unconditionally (the `v-model:open`
      + the `v-if` gating on the chip above keep it from stealing
      focus from the classic-chat path). When the user opens it,
      we pass the current sessionId + the parsed participants
      roster; the modal handles validation + IPC + reload.
      The `updated` event fires after a successful overwrite —
      the controller's messages themselves don't change, so no
      explicit refresh is needed from the modal side.
    -->
    <GroupChatConfigModal
      v-if="isGroupChat"
      v-model:open="groupChatEditOpen"
      mode="edit"
      :session-id="chatStore.currentSessionId ?? undefined"
      :initial-participants="groupChatParticipants ?? undefined"
    />

    <!--
          Memory entry (2026-06-11). See the script comment above
          for context. The modal handles its own focus trap / ESC /
          outside-click close via reka-ui Dialog.
        -->
    <MemoryModal v-model:open="memoryModalOpen" @manage="onMemoryManage" />

    <!-- 07-06 (am-observability-panel B3): runtime-memory detail +
         management modal. Opened when the user clicks a runtime row
         inside MemoryModal (MemoryPreview emits `manage` → MemoryModal
         forwards → `onMemoryManage` here resolves the row + opens).
         Nested inside MemoryModal's Dialog portal; reka-ui 2.9.9
         supports nested Dialogs (focus trap moves to the innermost). -->
    <RuntimeMemoryModal
      v-model:open="runtimeMemoryModalOpen"
      :memory="managedMemory"
    />

    <!--
          C4 audit-log modal (2026-06-14 PR2). See the script
          comment above for context. The modal handles its own
          focus trap / ESC / outside-click close via reka-ui
          Dialog. The watcher on `chatStore.currentSessionId`
          closes the modal on session switch.
        -->
    <AuditLogModal v-model:open="auditModalOpen" />

    <!--
          Permission-grant management modal (task 07-01). Same reka-ui
          Dialog shell + load-on-open watcher pattern as AuditLogModal.
          The watcher on `chatStore.currentSessionId` closes it on
          session switch.
        -->
    <PermissionGrantsModal v-model:open="grantsModalOpen" />

    <!--
          B12 Checklist (PR2 frontend, 2026-06-19). Floating
          overlay anchored to the ChatPanel's bottom-right, above
          the input bar. Reads the current session's checklist
          from the checklist store. Hidden when no checklist
          exists for the session (the store returns `null`).
          z-index is below PermissionModal / modals (the card
          uses z-index 50; modals teleport to body at z-index
          1000+).
        -->
    <ChecklistCard :items="currentChecklist" />

    <!--
          C2+ loop-intervention floating card (2026-07-28 fix). When
          the agent loop hits ≥3 consecutive loop-detection hits, the
          backend registers a `LoopIntervention` pending interaction
          and emits it on `tool:question`. This card surfaces the
          "终止 loop / 继续" choice as a TOP-anchored overlay (above
          the message list) so the user always sees it. It reuses
          `<AskUserQuestionCard>` verbatim — the resolve flow
          (`resolveToolQuestion`) is identical and the backend's C2+
          `select!{rx}` arm already interprets the answer. Pre-fix
          this intervention was invisible: the synthetic
          `tool_use_id=loop_intervention_N` matched no real tool_use
          block, so MessageItem's inline card never rendered.
    -->
    <div v-if="loopIntervention" class="chat-panel__loop-intervention">
      <AskUserQuestionCard
        :session-id="chatStore.currentSessionId ?? ''"
        :tool-use-id="loopIntervention.tool_use_id"
        :questions="loopIntervention.questions"
        state="pending"
      />
    </div>
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
  /* B12 Checklist (PR2 frontend): serve as the positioning
     context for the absolute-positioned `<ChecklistCard>`
     overlay. Without `relative`, the card's `position:
     absolute` would resolve against the nearest positioned
     ancestor (or the viewport), pulling the card out of the
     ChatPanel's flow. */
  position: relative;
}

/* 2026-06-27 top-tab-bar boundary fix: header height locked to 40px
   to match AppHeader's 40px (TitleBar) row. Previously the header
   shrunk to its content (~25px: 6+6 padding + 13px text), which made
   the ChatPanel header's bottom border sit ~15px ABOVE the Sidebar
   header's bottom border (which lives on `.sidebar__footer` deep
   down, not the header) — but more importantly the ChatPanel header
   divider ended up floating without a stable anchor to the AppHeader
   divider above it. Fixing to a fixed 40px gives the divider line a
   consistent y-coordinate across the top chrome and lets the
   `align-items: center` rule center the title row vertically so the
   text baseline aligns with where the user's eye expects it. */
.chat-panel__header {
  display: flex;
  align-items: center;
  padding: 0 20px;
  height: 40px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  flex-shrink: 0;
  min-width: 0;
}

.chat-panel__loading {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}

/* PR-3e (2026-06-27): skeleton list. The skeleton is 3 gray
   placeholder bubbles laid out to mirror a real user →
   assistant turn. The 1.5s linear-gradient shimmer
   (background-position 200% → -200%) gives the standard
   "content is loading" affordance without the dated
   rotating-spinner look. Bubble widths vary (short 35% /
   wide 70% / narrow 45%) so the placeholder doesn't look
   like a uniform stripe — a common AI tell. */
.chat-panel__skeleton {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: var(--space-5);
  padding: var(--space-5) var(--space-5);
  overflow: hidden;
}

.chat-panel__skeleton-msg {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.chat-panel__skeleton-msg--user {
  align-items: flex-end;
}

.chat-panel__skeleton-msg--assistant {
  align-items: flex-start;
}

.chat-panel__skeleton-bubble {
  height: 12px;
  border-radius: var(--radius-md);
  background: linear-gradient(
    90deg,
    var(--color-bg-elevated) 0%,
    var(--color-bg-border-strong) 50%,
    var(--color-bg-elevated) 100%
  );
  background-size: 200% 100%;
  animation: skeleton-shimmer 1.5s ease-in-out infinite;
}

.chat-panel__skeleton-bubble--short {
  width: 35%;
}

.chat-panel__skeleton-bubble--wide {
  width: 70%;
}

.chat-panel__skeleton-bubble--narrow {
  width: 45%;
}

@keyframes skeleton-shimmer {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}

/* PR-3e (2026-06-27): the old .chat-panel__loading + spinner
   classes are kept as no-op (the v-if was removed) so any
   future test that still references the spinner class
   doesn't 404. The keyframe is also kept for the same
   reason. They render nothing because the v-if no longer
   targets them; remove in a follow-up if no test references. */
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

.chat-panel__title-row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
  flex: 1;
  flex-wrap: wrap;
}

.chat-panel__title-actions {
  /* S6a 真机迭代:把 4 个 icon 按钮 + WorkerAskBanner 包进不缩容器,
     防止它们挤压标题文本(title 已有 max-width:50vw + ellipsis,
     但按钮作为 flex item 默认可被压缩,长标题时按钮变窄挤 text)。
     gap 8px 对齐 title-row 的桌面间距(桌面布局不变)。 */
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
  min-width: 0;
}

/* 08-14 ux-polish-r1 WP3 3.3(评审 C2):图标区功能分组分隔线 ——
   memory(项目级)与 audit/trace/grants(会话级)之间。1px 竖线、高度
   低于按钮(16px vs 22px),轻量不抢焦点;作为 flex item 吃 title-actions
   的 gap(桌面 8px → 分组断口 17px,移动端 12px → 25px,断口天然大于
   组内间距,分组语义可读)。 */
.chat-panel__action-divider {
  width: 1px;
  height: 16px;
  background: var(--color-bg-border);
  align-self: center;
  flex-shrink: 0;
}

.chat-panel__title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 50vw;
  /* Two flex fixes, working together:
     1. `min-width: 0` unlocks the shrink path — flex children default
        to `min-width: auto`, which makes an <h1> refuse to shrink
        below its intrinsic content width. With a long Chinese session
        title that intrinsic width blows past 50vw and pushes the
        right-side chips (git branch, cwd, worktree, memory, audit,
        grants) off-screen — `max-width` and `text-overflow: ellipsis`
        silently no-op without this.
     2. `flex: 1 1 0` makes the title actively grow into the leftover
        row space (after the fixed-size chips + buttons claim theirs)
        and start ellipsizing only when content actually overflows.
        Without this, the title sits at its intrinsic width and just
        gives up — the row ends up lopsided with the cwd chip flush
        right but the title not filling the left half. */
  flex: 1 1 0;
  min-width: 0;
}

.chat-panel__chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  padding: 2px 8px;
  border-radius: var(--radius-sm);
  font-family: var(--font-mono);
  white-space: nowrap;
}

.chat-panel__chip--git {
  color: var(--color-accent);
  border-color: var(--color-accent-muted);
  /* Same flex-shrink rationale as the title: the git branch chip is
     short ("main", "feature/foo") so it normally doesn't need to
     shrink, but locking it down explicitly keeps it from being
     squeezed by the title-row's wrap. */
  flex-shrink: 0;
}

.chat-panel__chip--cwd {
  margin-left: auto;
  max-width: 50%;
  overflow: hidden;
  text-overflow: ellipsis;
  /* Mirrors the title fix: without min-width: 0 the flex item's
     intrinsic content width (~ 280px for a full /usr/local/code/.../foo
     path) overrides the 50% cap and crowds the buttons. */
  min-width: 0;
}

/* Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E8): the
   group-chat indicator chip doubles as the edit button. Uses
   the same chip shell as the git/cwd chips but with a button
   affordance (cursor + hover state) so the user knows it's
   clickable. The "群聊 (N participants)" text gives the user
   the participant count at a glance — clicking opens the
   edit-mode modal. */
.chat-panel__chip--group-chat {
  cursor: pointer;
  background: transparent;
  font: inherit;
  color: var(--color-text);
  border-color: var(--color-accent-muted);
  flex-shrink: 0;
  /* Override the default chip "span" cursor in the title-row's
     flex layout — buttons in flex rows sometimes inherit
     `text` cursor on certain browsers. */
  cursor: pointer;
}
.chat-panel__chip--group-chat:hover {
  background: var(--color-bg-hover);
  color: var(--color-accent);
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
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out);
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

/* C4 audit-log entry button (2026-06-14 PR2). Sits to the right
   of the Memory button. Visually identical to the memory button
   (chip-family icon button) so the two read as a pair. */
.chat-panel__audit-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 24px;
  height: 22px;
  padding: 0;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out);
  font-family: inherit;
}

.chat-panel__audit-btn:hover {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.chat-panel__audit-btn:active {
  background: var(--color-bg-border);
}

/* Permission-grant management entry (task 07-01). Visually
   identical to the audit/memory buttons — the three read as a
   chip-family group of "session inspector" actions. */
.chat-panel__grants-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 24px;
  height: 22px;
  padding: 0;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out);
  font-family: inherit;
}

.chat-panel__grants-btn:hover {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.chat-panel__grants-btn:active {
  background: var(--color-bg-border);
}

/* E2 (harness trace pipeline, 2026-07-14): trace timeline
   toggle button. Sits in the same chip-family as the
   audit / grants buttons (24x22, 1px border, --color-bg-elevated).
   Same hover behavior — accent-muted + accent border. */
.chat-panel__trace-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 24px;
  height: 22px;
  padding: 0;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out);
  font-family: inherit;
}

.chat-panel__trace-btn:hover {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.chat-panel__trace-btn:active {
  background: var(--color-bg-border);
}

.chat-panel__main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
  /* PR4 (2026-06-27): symmetric L/R (--space-5 both sides; was
     20px/4px — the 4px right was a scrollbar-gutter hack now handled
     by `scrollbar-gutter: stable` on .messages). Bottom --space-2
     gives the message list breathing room above the input row. */
  padding: var(--space-5) var(--space-5) var(--space-2) var(--space-5);
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
  padding: var(--space-7) var(--space-4);
  gap: var(--space-2);
}

/* PR-3c (2026-06-27): icon-led empty state. The 64px container
   mirrors EmptyProjectState's "还没有项目" hero for visual
   consistency across the app's "no content yet" surfaces. The
   icon color is accent (Prussian blue) so the empty state
   reads as "this is an interactive area" rather than "error
   / disabled". Container has a subtle border + elevated bg
   so the icon doesn't float on the chat-panel background. */
.chat-panel__empty-icon {
  width: 64px;
  height: 64px;
  border-radius: var(--radius-xl);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  margin-bottom: var(--space-3);
  color: var(--color-accent);
}

.chat-panel__empty-title {
  font-size: var(--text-lg);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  margin: 0;
  letter-spacing: -0.01em;
}

.chat-panel__empty-hint {
  font-size: var(--text-sm);
  color: var(--color-text-muted);
  margin: 0;
}

.chat-panel__empty-project {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  margin-top: var(--space-3);
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
  justify-content: center;
}

.chat-panel__empty-warn {
  display: inline-flex;
  align-items: center;
  gap: var(--space-1);
  color: var(--color-tool-shell);
  font-size: var(--text-xs);
}

/* 07-06 (am-observability-panel B4/R2b): real-time recall chip.
   Thin banner between header + message list. Compact (single row
   collapsed); expands a grouped title list on click. Uses accent
   tint so it reads as informational, not an error. */
.chat-panel__recall {
  border-bottom: 1px solid var(--color-bg-border);
  background: color-mix(in srgb, var(--color-accent) 4%, transparent);
  font-size: var(--text-xs);
}
.chat-panel__recall-toggle {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  padding: 4px 16px;
  border: none;
  background: transparent;
  color: var(--color-accent);
  cursor: pointer;
  font-size: var(--text-xs);
  font-family: inherit;
}
.chat-panel__recall-toggle:hover {
  background: color-mix(in srgb, var(--color-accent) 8%, transparent);
}
.chat-panel__recall-groups {
  padding: 0 16px 6px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.chat-panel__recall-group {
  display: flex;
  align-items: flex-start;
  gap: 6px;
}
.chat-panel__recall-source {
  flex-shrink: 0;
  padding: 0 5px;
  border-radius: 3px;
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  line-height: 1.5;
}
.chat-panel__recall-source--fts {
  color: var(--color-accent);
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
}
.chat-panel__recall-source--pitfall {
  color: var(--color-tool-error);
  background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
}
.chat-panel__recall-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}
.chat-panel__recall-item {
  /* 08-14 ux-polish-r1 WP2(评审 B3):召回标题是常驻信息型文字(非角标),
     10px muted 低对比 → 升 --text-xs。 */
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* C2+ loop-intervention floating card: top-anchored overlay so the
   user always notices the agent is stuck. Warning-tinted border
   distinguishes it from a normal ask_user_question card. z-index
   above the checklist overlay (50) but below modals (1000+). */
.chat-panel__loop-intervention {
  position: absolute;
  top: var(--space-3);
  left: 50%;
  transform: translateX(-50%);
  z-index: 60;
  width: min(560px, calc(100% - var(--space-4) * 2));
  padding: var(--space-3);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-status-warn, #f59e0b);
  border-radius: var(--radius-md, 8px);
  box-shadow: var(--shadow-md, 0 4px 12px rgba(0, 0, 0, 0.15));
}

/* S6a 主聊天视图 header 瘦身(08-13-mobile-chat-view)。桌面样式块零改动,
   全部移动端规则放 @media (max-width: 767px) 内 + 新增 <360px 降级档。
   对应 prd A1/A2/D1/D2/D4/D5:
   - cwd chip / git chip / WorktreeChip 手机端无场景(DEC-2),display:none
     隐藏(不动 showGitChip/showWorktreeChip script 逻辑,桌面还要用)。
     隐藏类用单类选择器;WorktreeChip 是子组件根节点,scoped 下用 :deep()
     命中(见 .trellis/spec/frontend/reka-ui-usage.md scoped/portal 约定)。
   - 标题行 flex-wrap:wrap 在窄屏堆成三层(D1)→ nowrap + 标题 ellipsis,
     标题不再被挤到行首贴边(D4)。
   - 4 图标按钮(memory/audit/trace/grants)视觉 32px(真机反馈 44px 视觉
     过大;低频按钮 32px 足够,桌面 24px 的合理放大);header 高度 40px
     (桌面同高,紧凑)。见 .trellis/spec/frontend/responsive-mobile.md
     §6 DEC-6 修正。
   08-14 ux-polish-r1 WP1(评审 A1/A3):
   - 4 图标按钮触控目标 ≥44px:视觉仍是 32×32,通过 ::after 透明外扩
     (inset -6px,32+6×2=44)把 hit area 撑到 44px(DEC-6 "44px 只给主
     操作"的修正——不动视觉,只扩命中区,两者不再冲突)。
   - title-actions gap 0→12px:相邻按钮的 44px 外扩区不再互相重叠
     (重叠时后 DOM 的 ::after 盖住前者,实际命中退化回 ~32px)。
   - 群聊 chip(编辑参与者入口,低频)移动端隐藏(mobile-hide-group-chat,
     §1.4 约定);编辑参与者回桌面端操作。桌面块零改动。 */
@media (max-width: 767px) {
  .chat-panel__header {
    padding: 0 8px;
    height: 40px;
  }
  .chat-panel__title-row {
    flex-wrap: nowrap;
    gap: 4px;
  }
  .chat-panel__title-actions {
    gap: 12px;
  }
  /* 真机迭代(2026-08-13):移动端去掉 max-width:50vw —— 桌面靠 50vw
     上限控制标题宽度,但手机端 actions 已 flex-shrink:0 不缩,标题
     自然占满剩余宽度再 ellipsis(overflow:hidden + nowrap 在桌面块
     已有)。max-width 拿掉让标题在窄屏获得更充分的展示宽度。 */
  .chat-panel__title {
    max-width: none;
  }
  /* 真机反馈:桌面 20px 左右边距在手机过宽 → 收窄为 12px 12px 8px
     (消息气泡 max-width 88%,左右 12px 让内容区更贴近屏幕边缘)。 */
  .chat-panel__main {
    padding: var(--space-3) var(--space-3) var(--space-2);
  }
  .mobile-hide-cwd,
  .mobile-hide-git,
  .mobile-hide-group-chat,
  :deep(.mobile-hide-worktree) {
    display: none;
  }
  .chat-panel__memory-btn,
  .chat-panel__audit-btn,
  .chat-panel__trace-btn,
  .chat-panel__grants-btn {
    width: 32px;
    height: 32px;
    /* ::after 外扩锚点(桌面块保持 static,零改动) */
    position: relative;
  }
  /* 触控命中区 ≥44px(32px 视觉 + 6px 透明外扩 ×2)。-6px 是 32→44 的
     唯一解,不入 spacing scale(半步值,见 design-tokens.md "Don't add
     --space-1-5" 例外条款)。 */
  .chat-panel__memory-btn::after,
  .chat-panel__audit-btn::after,
  .chat-panel__trace-btn::after,
  .chat-panel__grants-btn::after {
    content: "";
    position: absolute;
    inset: -6px;
  }
}

/* S6a 窄屏再降级(D8/D9):360px 以下主占位问题已由 767px 档解决,这里只做
   A 组已有改动的强化档 —— 标题字号再收紧(design §3.5)。写在 767px 档之后
   (后者优先,天然覆盖,单类选择器无特异性冲突)。 */
@media (max-width: 359px) {
  .chat-panel__title {
    font-size: var(--text-sm);
  }
}
</style>
