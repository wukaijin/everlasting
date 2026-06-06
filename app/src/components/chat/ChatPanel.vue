<script setup lang="ts">
// ChatPanel — the right-side main content area when a project is
// active. Renders a header (current cwd) above and the input
// region below; the middle is the MessageList. The empty state
// (no messages yet) shows a welcome with the current project's
// name and any git/legacy warnings.

import { computed } from "vue";
import { useChatStore } from "../../stores/chat";
import { useProjectsStore } from "../../stores/projects";
import MessageList from "./MessageList.vue";
import ChatInput from "./ChatInput.vue";

const chatStore = useChatStore();
const projectsStore = useProjectsStore();

const emit = defineEmits<{
  send: [text: string];
}>();

const hasMessages = computed(() => chatStore.messages.length > 0);

// Canonical cwd string. Displayed verbatim; the `~/` shortening
// referenced in Q5 requires a home-dir lookup that PR1's backend
// doesn't expose (the deviation is documented in the implement
// report).
const cwdDisplay = computed<string>(() => chatStore.currentCwd || "");

const currentProject = computed(() =>
  projectsStore.projectById(projectsStore.currentProjectId),
);
</script>

<template>
  <section class="chat-panel">
    <header class="chat-panel__header">
      <h1 class="chat-panel__title">Everlasting</h1>
      <span class="chat-panel__subtitle">vibe coding workbench</span>
      <div
        v-if="cwdDisplay"
        class="chat-panel__cwd"
        :title="cwdDisplay"
      >cwd: {{ cwdDisplay }}</div>
    </header>

    <main class="chat-panel__main">
      <div v-if="!hasMessages" class="chat-panel__empty">
        <p>输入一句话,跟 LLM 聊聊看 👋</p>
        <p class="chat-panel__empty-hint">中文输入测试 + 流式响应 + 工具调用</p>
        <p v-if="currentProject" class="chat-panel__empty-project">
          当前项目: <strong>{{ currentProject.name }}</strong>
          <span v-if="!currentProject.is_git_repo" class="chat-panel__empty-warn">
            ⚠️ 未启用 git 隔离
          </span>
          <span v-else-if="currentProject.is_legacy" class="chat-panel__empty-warn">
            📦 旧数据,自动归入
          </span>
        </p>
      </div>
      <MessageList v-else />
    </main>

    <ChatInput :sending="chatStore.sending" @send="emit('send', $event)" />
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
  align-items: baseline;
  gap: 12px;
  padding: 14px 20px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  flex-wrap: wrap;
  flex-shrink: 0;
}

.chat-panel__title {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.chat-panel__subtitle {
  font-size: 12px;
  color: var(--color-text-secondary);
}

.chat-panel__cwd {
  font-size: 11px;
  color: var(--color-text-secondary);
  font-family: var(--font-mono);
  background: var(--color-bg-elevated);
  padding: 2px 8px;
  border-radius: 4px;
  margin-left: auto;
  max-width: 50%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl; /* keep the tail (path tail) visible when truncated */
  text-align: left;
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
}

.chat-panel__empty-warn {
  margin-left: 6px;
  color: var(--color-tool-shell);
  font-size: 11px;
}
</style>
