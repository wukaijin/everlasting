<script setup lang="ts">
// AppShell — top-level layout. Per spike-003 + PRD: 40px tab bar
// (AppHeader) + flex body with optional sidebar (left) + main slot
// (right). The toast lives fixed at the bottom-center, outside the
// flex flow.
//
// Sidebar visibility follows the same rule ChatWindow used: visible
// iff a project is active. The empty state (no project) is rendered
// inside the slot (ChatWindow) so the user can hit "+ 添加项目"
// from the same surface.

import { computed } from "vue";
import { useProjectsStore } from "../../stores/projects";
import AppHeader from "./AppHeader.vue";
import Sidebar from "./Sidebar.vue";

const projectsStore = useProjectsStore();
const showSidebar = computed<boolean>(
  () => projectsStore.currentProjectId !== null,
);
</script>

<template>
  <div class="app-shell">
    <AppHeader />

    <div class="app-shell__body">
      <Sidebar v-if="showSidebar" />

      <main class="app-shell__main">
        <slot />
      </main>
    </div>

    <transition name="toast">
      <div
        v-if="projectsStore.toast"
        :class="['toast', `toast--${projectsStore.toast.kind}`]"
        @click="projectsStore.dismissToast"
      >
        {{ projectsStore.toast.message }}
      </div>
    </transition>
  </div>
</template>

<style scoped>
.app-shell {
  display: flex;
  flex-direction: column;
  height: 100vh;
  background: var(--color-bg-app);
  color: var(--color-text-primary);
  font-family: var(--font-sans);
}

.app-shell__body {
  flex: 1;
  display: flex;
  min-height: 0;
}

.app-shell__main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  background: var(--color-bg-app);
}

/* --- Toast (Q8v2 / minimal fixed bottom-center div) --- */

.toast {
  position: fixed;
  bottom: 24px;
  left: 50%;
  transform: translateX(-50%);
  padding: 10px 18px;
  border-radius: 8px;
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  font-size: 13px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
  cursor: pointer;
  max-width: 80vw;
  z-index: 9999;
  border: 1px solid var(--color-bg-border);
}

.toast--warn {
  background: var(--color-tool-shell);
  color: var(--color-bg-app);
}

.toast--error {
  background: var(--color-tool-error);
  color: #ffffff;
}

.toast--info {
  background: var(--color-accent);
  color: #ffffff;
}

.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.2s, transform 0.2s;
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translate(-50%, 8px);
}
</style>
