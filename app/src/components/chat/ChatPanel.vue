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

import { computed, onUnmounted, ref } from "vue";
import { useChatStore, type SessionSummary } from "../../stores/chat";
import { useConfigStore } from "../../stores/config";
import { useProjectsStore } from "../../stores/projects";
import MessageList from "./MessageList.vue";
import ChatInput from "./ChatInput.vue";
import DiffView from "./DiffView.vue";
import Icon from "../Icon.vue";

const chatStore = useChatStore();
const projectsStore = useProjectsStore();
const configStore = useConfigStore();

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

function onKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape" && diffModalOpen.value) {
        closeDiffModal();
    }
}

if (typeof window !== "undefined") {
    window.addEventListener("keydown", onKeyDown);
    onUnmounted(() => window.removeEventListener("keydown", onKeyDown));
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

const diffButtonLabel = computed<string>(() => {
    const n = diffFileCount.value;
    if (n === null) return "diff";
    if (n === 0) return "diff (clean)";
    return `diff (${n})`;
});

const diffButtonTitle = computed<string>(() => {
    const sid = chatStore.currentSessionId;
    if (!sid) return "Switch to a session to view its diff";
    if (!currentProject.value?.is_git_repo) {
        return "This project isn't a git repo";
    }
    const n = diffFileCount.value;
    if (n === null) return "View the diff for this session";
    if (n === 0) return "No changes in this session yet";
    return `View ${n} ${n === 1 ? "file" : "files"} changed in this session`;
});
</script>

<template>
    <section class="chat-panel">
        <header class="chat-panel__header">
            <div class="chat-panel__title-row">
                <h1 class="chat-panel__title">{{ currentSessionTitle }}</h1>
                <span v-if="configStore.model" class="chat-panel__chip">
                    <Icon name="command-line" :size="12" />
                    {{ configStore.model }}
                </span>
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
                <button
                    v-if="chatStore.currentSessionId"
                    type="button"
                    class="chat-panel__chip chat-panel__chip--diff"
                    :title="diffButtonTitle"
                    @click="openDiffModal"
                >
                    <Icon name="document" :size="12" />
                    {{ diffButtonLabel }}
                </button>
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
                        未启用 git 隔离
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

.chat-panel__chip--diff {
    background: var(--color-accent-muted);
    color: var(--color-accent);
    border-color: var(--color-accent);
    border-width: 1px;
    border-style: solid;
    cursor: pointer;
    font: inherit;
    font-size: 11px;
}

.chat-panel__chip--diff:hover {
    background: var(--color-accent);
    color: var(--color-bg-app);
}

/* -----------------------------------------------------------------------
 * Diff modal (step 4 / PR3). Full-viewport overlay; the inner
 * .diff-modal is centered and sized to leave 40px margin on each
 * side. Scrolling happens inside .diff-modal__body so the
 * header + close button stay pinned.
 * --------------------------------------------------------------------- */
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
