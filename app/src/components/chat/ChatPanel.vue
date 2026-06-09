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
// worktree chip with a dropdown menu:
//   - `none` (no worktree ever) → "attach worktree" button
//   - `active` (worktree bound)  → "diff (N)" + dropdown with
//     copy-path / copy-branch / detach / delete
//   - `detached` (was active)    → "上次 worktree" + dropdown
//     with the same actions (the file diff is from the stale
//     worktree on disk; the copy buttons still work; detach and
//     delete are still meaningful).

import { computed, onUnmounted, ref } from "vue";
import { useChatStore, type SessionSummary } from "../../stores/chat";
import { useProjectsStore } from "../../stores/projects";
import MessageList from "./MessageList.vue";
import ChatInput from "./ChatInput.vue";
import DiffView from "./DiffView.vue";
import DeleteWorktreeConfirm from "./DeleteWorktreeConfirm.vue";
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
// Step 4 / PR3: session-level diff modal
// -----------------------------------------------------------------------

const diffModalOpen = ref(false);
const diffLoading = ref(false);
const diffError = ref<string | null>(null);
const diffResult = ref<{ files: import("./DiffView.vue").FileDiff[] } | null>(null);

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
// -----------------------------------------------------------------------

/** The dropdown that opens from the worktree chip. Closes on
 *  outside-click and on Escape. State is local to the chip — the
 *  chat store doesn't need to know whether the menu is open. */
const worktreeMenuOpen = ref(false);
const worktreeMenuRoot = ref<HTMLElement | null>(null);

function toggleWorktreeMenu() {
    worktreeMenuOpen.value = !worktreeMenuOpen.value;
}

function closeWorktreeMenu() {
    worktreeMenuOpen.value = false;
}

function onDocumentClick(e: MouseEvent) {
    if (!worktreeMenuOpen.value) return;
    const target = e.target as Node | null;
    if (worktreeMenuRoot.value && target && !worktreeMenuRoot.value.contains(target)) {
        worktreeMenuOpen.value = false;
    }
}

if (typeof document !== "undefined") {
    document.addEventListener("click", onDocumentClick);
    onUnmounted(() => document.removeEventListener("click", onDocumentClick));
}

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

/** Per-state worktree chip label. Mirrors the PR3 (single
 *  "diff" button) UX for `active`, and adds two new shapes for
 *  `none` and `detached`. */
const worktreeChipLabel = computed<string>(() => {
    const state = currentSession.value?.worktree_state ?? "none";
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
    const state = currentSession.value?.worktree_state ?? "none";
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

const isStreaming = computed<boolean>(() => chatStore.isCurrentSessionStreaming);

/** The branch name for the active/detached session. The Rust
 *  side always names it `session/<session_id>` — re-deriving it
 *  client-side keeps the copy buttons honest. */
const branchName = computed<string>(() =>
    `session/${chatStore.currentSessionId ?? ""}`,
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

/** "Copy <label>" — uses `navigator.clipboard.writeText` with a
 *  fallback for non-secure contexts. The toast goes through the
 *  projects store (the existing toast system in `AppShell.vue`).
 *  The operations are read-only; we explicitly do NOT disable
 *  them when `isStreaming` is true (REQ-26). */
async function copyToClipboard(value: string, label: string) {
    try {
        if (navigator.clipboard?.writeText) {
            await navigator.clipboard.writeText(value);
        } else {
            // Fallback: legacy `document.execCommand("copy")` for
            // non-secure contexts (some embedded webviews).
            const ta = document.createElement("textarea");
            ta.value = value;
            ta.setAttribute("readonly", "");
            ta.style.position = "absolute";
            ta.style.left = "-9999px";
            document.body.appendChild(ta);
            ta.select();
            document.execCommand("copy");
            document.body.removeChild(ta);
        }
        projectsStore.showToast(`已复制 ${label}`, "info", 2000);
    } catch (e) {
        projectsStore.showToast(`复制失败: ${String(e)}`, "error");
    }
}

function onCopyWorktreePath() {
    const p = worktreePathForDisplay.value;
    if (!p) return;
    void copyToClipboard(p, "worktree path");
    closeWorktreeMenu();
}

function onCopyBranchName() {
    if (!chatStore.currentSessionId) return;
    void copyToClipboard(branchName.value, "branch name");
    closeWorktreeMenu();
}

/** Click on the chip itself: for `active` we open the diff; for
 *  `none` we attach; for `detached` we open the diff (the
 *  diff still reflects the on-disk state). The dropdown is the
 *  second-click path; single-click is the most common path so
 *  it goes straight to the primary action. */
function onChipClick() {
    const state = currentSession.value?.worktree_state ?? "none";
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
    closeWorktreeMenu();
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
    closeWorktreeMenu();
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
    const state = currentSession.value?.worktree_state ?? "none";
    const hasDiff =
        state === "active" && (diffFileCount.value ?? 0) > 0;
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
    closeWorktreeMenu();
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

/** Disabled-state predicates for the dropdown menu items.
 *  Detach/delete are disabled while streaming (REQ-13); the copy
 *  buttons are NOT (REQ-26). Attach is allowed mid-stream. */
const detachDisabled = computed<boolean>(() => isStreaming.value);
const deleteDisabled = computed<boolean>(() => isStreaming.value);

const worktreeState = computed(() => currentSession.value?.worktree_state ?? "none");

function onKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
        if (confirmDeleteOpen.value) {
            onDeleteCancel();
            return;
        }
        if (worktreeMenuOpen.value) {
            closeWorktreeMenu();
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
                  delete).
                -->
                <div
                    v-if="showWorktreeChip"
                    ref="worktreeMenuRoot"
                    class="chat-panel__worktree"
                >
                    <button
                        type="button"
                        class="chat-panel__chip chat-panel__chip--worktree"
                        :title="worktreeChipTitle"
                        @click="onChipClick"
                    >
                        <Icon name="document" :size="12" />
                        {{ worktreeChipLabel }}
                    </button>
                    <button
                        v-if="worktreeState !== 'none'"
                        type="button"
                        class="chat-panel__chip chat-panel__chip--worktree-toggle"
                        :aria-label="'worktree options'"
                        :title="'worktree options'"
                        @click.stop="toggleWorktreeMenu"
                    >
                        <Icon
                            :name="worktreeMenuOpen ? 'chevron-down' : 'chevron-right'"
                            :size="12"
                        />
                    </button>
                    <div
                        v-if="worktreeMenuOpen && worktreeState !== 'none'"
                        class="chat-panel__menu"
                        role="menu"
                    >
                        <button
                            v-if="worktreePathForDisplay"
                            type="button"
                            class="chat-panel__menu-item"
                            role="menuitem"
                            @click="onCopyWorktreePath"
                        >
                            <Icon name="document" :size="12" />
                            复制 worktree path
                        </button>
                        <button
                            type="button"
                            class="chat-panel__menu-item"
                            role="menuitem"
                            @click="onCopyBranchName"
                        >
                            <Icon name="refresh" :size="12" />
                            复制 branch name
                        </button>
                        <div class="chat-panel__menu-sep" />
                        <button
                            type="button"
                            class="chat-panel__menu-item"
                            role="menuitem"
                            :disabled="detachDisabled"
                            @click="onDetach"
                        >
                            <Icon name="minus" :size="12" />
                            解绑 (detach)
                        </button>
                        <button
                            type="button"
                            class="chat-panel__menu-item chat-panel__menu-item--danger"
                            role="menuitem"
                            :disabled="deleteDisabled"
                            @click="onDeleteClick"
                        >
                            <Icon name="warn" :size="12" />
                            删除 worktree
                        </button>
                    </div>
                </div>
                <!--
                  The legacy diff button is gone — replaced by the
                  worktree chip above. The "attach" path is folded
                  into the chip's primary click (no worktree →
                  click chip → attach).
                -->
            </div>
        </header>

        <main class="chat-panel__main">
            <div v-if="!hasMessages" class="chat-panel__empty">
                <p>输入一句话,跟 LLM 聊聊看</p>
                <p class="chat-panel__empty-hint">
                    中文输入测试 + 流式响应 + 工具调用
                </p>
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
          Step 4 / PR3: session-level diff modal. Triggered by the
          "diff" chip in the header. Closes on backdrop click, on
          the close button, or on Esc. Renders the DiffView
          component with the session's cached diff.
        -->
        <div
            v-if="diffModalOpen"
            class="diff-modal-backdrop"
            @click.self="closeDiffModal"
        >
            <div
                class="diff-modal"
                role="dialog"
                aria-modal="true"
                aria-label="Session diff"
            >
                <header class="diff-modal__header">
                    <h2 class="diff-modal__title">
                        Session diff
                        <span v-if="diffResult" class="diff-modal__count">
                            ({{ diffResult.files.length }}
                            {{ diffResult.files.length === 1 ? "file" : "files" }})
                        </span>
                    </h2>
                    <button
                        type="button"
                        class="diff-modal__close"
                        @click="closeDiffModal"
                        aria-label="Close"
                    >
                        <Icon name="x" :size="14" />
                    </button>
                </header>
                <div class="diff-modal__body">
                    <div v-if="diffLoading" class="diff-modal__loading">
                        Loading diff…
                    </div>
                    <div v-else-if="diffError" class="diff-modal__error">
                        {{ diffError }}
                    </div>
                    <DiffView v-else-if="diffResult" :files="diffResult.files" />
                </div>
            </div>
        </div>

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

.chat-panel__main {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    padding: 20px;
    padding-right: 4px;
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

/* -----------------------------------------------------------------------
 * Step 4 follow-up: tri-state worktree chip + dropdown
 * ------------------------------------------------------------------- */

.chat-panel__worktree {
    position: relative;
    display: inline-flex;
    align-items: stretch;
}

.chat-panel__chip--worktree {
    background: var(--color-accent-muted);
    color: var(--color-accent);
    border-color: var(--color-accent);
    border-width: 1px;
    border-style: solid;
    cursor: pointer;
    font: inherit;
    font-size: 11px;
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
    border-right: 0;
}

.chat-panel__chip--worktree:hover {
    background: var(--color-accent);
    color: var(--color-bg-app);
}

.chat-panel__chip--worktree-toggle {
    background: var(--color-accent-muted);
    color: var(--color-accent);
    border: 1px solid var(--color-accent);
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    cursor: pointer;
    font: inherit;
    font-size: 11px;
    padding: 2px 4px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    margin-left: 0;
}

.chat-panel__chip--worktree-toggle:hover {
    background: var(--color-accent);
    color: var(--color-bg-app);
}

.chat-panel__menu {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    background: var(--color-bg-surface);
    border: 1px solid var(--color-bg-border);
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
    min-width: 200px;
    z-index: 100;
    padding: 4px;
    display: flex;
    flex-direction: column;
}

.chat-panel__menu-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    background: transparent;
    border: 0;
    color: var(--color-text-primary);
    font: inherit;
    font-size: 12px;
    text-align: left;
    cursor: pointer;
    border-radius: 4px;
}

.chat-panel__menu-item:hover:not(:disabled) {
    background: var(--color-bg-elevated);
}

.chat-panel__menu-item:disabled {
    color: var(--color-text-muted);
    cursor: not-allowed;
}

.chat-panel__menu-item--danger {
    color: var(--color-tool-error);
}

.chat-panel__menu-item--danger:hover:not(:disabled) {
    background: var(--color-bg-elevated);
}

.chat-panel__menu-sep {
    height: 1px;
    background: var(--color-bg-border);
    margin: 4px 0;
}

/* -----------------------------------------------------------------------
 * Diff modal (step 4 / PR3). Full-viewport overlay; the inner
 * .diff-modal is centered and sized to leave 40px margin on each
 * side. Scrolling happens inside .diff-modal__body so the
 * header + close button stay pinned.
 * -------------------------------------------------------------------- */
.diff-modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    padding: 40px;
}

.diff-modal {
    background: var(--color-bg-surface);
    border: 1px solid var(--color-bg-border);
    border-radius: 8px;
    width: 100%;
    max-width: 1100px;
    max-height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
}

.diff-modal__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 16px;
    border-bottom: 1px solid var(--color-bg-border);
    background: var(--color-bg-elevated);
    flex-shrink: 0;
}

.diff-modal__title {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--color-text-primary);
    display: inline-flex;
    align-items: baseline;
    gap: 8px;
}

.diff-modal__count {
    font-size: 11px;
    color: var(--color-text-muted);
    font-weight: 400;
}

.diff-modal__close {
    background: transparent;
    border: 0;
    color: var(--color-text-muted);
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
}

.diff-modal__close:hover {
    background: var(--color-bg-border);
    color: var(--color-text-primary);
}

.diff-modal__body {
    flex: 1;
    overflow-y: auto;
    padding: 12px 16px;
    background: var(--color-bg-app);
}

.diff-modal__loading,
.diff-modal__error {
    padding: 24px;
    text-align: center;
    color: var(--color-text-muted);
    font-size: 13px;
}

.diff-modal__error {
    color: var(--color-tool-error);
}
</style>
