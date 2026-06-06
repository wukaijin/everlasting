<script setup lang="ts">
// ChatWindow — top-level chat orchestrator. Decomposed during D3:
// renders EmptyProjectState (no project active) or ChatPanel (project
// + session + chat). This file owns data initialization (config +
// projects load on mount) and the "no project active" guard before
// delegating to the chat store.

import { computed, onMounted } from "vue";
import { useChatStore } from "../stores/chat";
import { useConfigStore } from "../stores/config";
import { useProjectsStore } from "../stores/projects";
import ChatPanel from "./chat/ChatPanel.vue";
import EmptyProjectState from "./chat/EmptyProjectState.vue";

const store = useChatStore();
const config = useConfigStore();
const projectsStore = useProjectsStore();

onMounted(async () => {
  config.load();
  await projectsStore.loadProjects();
  // Restore last active project (Q1 / PROPOSAL §5.5). The chat
  // store's watcher in `chat.ts` will load sessions for the
  // selected project; we just need to choose which one.
  const lastId = config.lastActiveProjectId;
  if (lastId && projectsStore.projects.find((p) => p.id === lastId)) {
    projectsStore.currentProjectId = lastId;
  } else if (projectsStore.projects.length > 0) {
    projectsStore.currentProjectId = projectsStore.projects[0].id;
  }
  // If neither condition holds, the empty state will show. The
  // chat store's watcher fires for `currentProjectId = null` and
  // just clears sessions.
});

const showEmptyState = computed<boolean>(
  () => projectsStore.currentProjectId === null,
);

async function onSend(text: string) {
  if (!projectsStore.currentProjectId) {
    // Defensive: the empty state should make this unreachable,
    // but if some race gets us here, surface a toast instead of
    // silently failing. The PR1 backend's `create_session` would
    // also reject an empty project_id.
    projectsStore.showToast("请先添加项目", "warn");
    return;
  }
  await store.send(text);
}
</script>

<template>
  <div class="chat-window">
    <EmptyProjectState v-if="showEmptyState" />
    <ChatPanel v-else @send="onSend" />
  </div>
</template>

<style scoped>
.chat-window {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: var(--color-bg-app);
}
</style>
