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
// model name and a git placeholder. The git chip is intentionally
// a static "git" tag today — the backend doesn't yet expose a real
// branch name on the project; the chip will swap to a real branch
// string once the Rust side grows a `git_branch` column.

import { computed } from "vue";
import { useChatStore, type SessionSummary } from "../../stores/chat";
import { useConfigStore } from "../../stores/config";
import { useProjectsStore } from "../../stores/projects";
import MessageList from "./MessageList.vue";
import ChatInput from "./ChatInput.vue";
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
 *  label is a static "git" — the backend doesn't yet expose the
 *  real branch name, so we don't fabricate one. Once the Rust
 *  project schema grows `git_branch`, replace this string with
 *  `currentProject.value.git_branch` (or similar). */
const showGitChip = computed<boolean>(
  () => !!currentProject.value?.is_git_repo,
);
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
        <span v-if="showGitChip" class="chat-panel__chip chat-panel__chip--git">
          <Icon name="refresh" :size="12" />
          git
        </span>
      </div>
    </header>

    <main class="chat-panel__main">
      <div v-if="!hasMessages" class="chat-panel__empty">
        <p>输入一句话,跟 LLM 聊聊看</p>
        <p class="chat-panel__empty-hint">中文输入测试 + 流式响应 + 工具调用</p>
        <p v-if="currentProject" class="chat-panel__empty-project">
          当前项目: <strong>{{ currentProject.name }}</strong>
          <span v-if="!currentProject.is_git_repo" class="chat-panel__empty-warn">
            <Icon name="warn" :size="11" />
            未启用 git 隔离
          </span>
          <span v-else-if="currentProject.is_legacy" class="chat-panel__empty-warn">
            <Icon name="archive" :size="11" />
            旧数据,自动归入
          </span>
        </p>
      </div>
      <MessageList v-else />
    </main>

    <ChatInput :sending="chatStore.sending" @send="emit('send', $event)" @stop="onStop" />
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
  padding: 14px 20px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  flex-shrink: 0;
  min-width: 0;
}

.chat-panel__title-row {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
  flex-wrap: wrap;
}

.chat-panel__title {
  margin: 0;
  font-size: 15px;
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

.chat-panel__main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
  padding: 20px;
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
