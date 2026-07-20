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
//
// E2 (harness trace pipeline, 2026-07-14): the right-side
// `<TracePanel>` drawer mounts as a top-level sibling of the
// main slot. The drawer is `v-if` gated on
// `traceStore.panelOpen`; the slide-in / slide-out
// transition lives inside the panel itself (mirrors
// SubagentDrawer). Per design §5, mounting the panel at
// the AppShell level (not inside ChatWindow) keeps the
// trace viewer available even when the empty-project
// state is showing — the user can review trace history
// for any session regardless of the current chat
// surface.

import { computed } from "vue";
import { useProjectsStore } from "../../stores/projects";
import { useChatStore } from "../../stores/chat";
import AppHeader from "./AppHeader.vue";
import Sidebar from "./Sidebar.vue";
import TracePanel from "../trace/TracePanel.vue";
import ToastProvider from "../common/ToastProvider.vue";

const projectsStore = useProjectsStore();
const chatStore = useChatStore();
const showSidebar = computed<boolean>(
  () => projectsStore.currentProjectId !== null,
);

/** Toast click handler. For cross-session pending-interaction
 *  toasts (`sessionId` set): if the target session belongs to the
 *  current project (present in `chatStore.sessions`), switch to it
 *  before dismissing so the user lands on the inline card.
 *  Project-operation toasts carry no `sessionId` and just dismiss —
 *  preserving pre-existing behavior. Cross-project targets are NOT
 *  in `sessions`, so they dismiss without jumping (per Q4:
 *  same-project jump only; avoids needing session→project mapping). */
async function onToastClick(): Promise<void> {
  const sid = projectsStore.toast?.sessionId;
  if (sid && chatStore.sessions.some((s) => s.id === sid)) {
    await chatStore.switchSession(sid);
  }
  projectsStore.dismissToast();
}
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

    <!-- E2 (harness trace pipeline, 2026-07-14): trace-viewer
         drawer. Mounts at the AppShell level (sibling of the
         main slot) so the timeline stays available even when
         the chat surface is hidden. The slide-in / slide-out
         transition lives inside the panel itself. -->
    <TracePanel />

    <!-- A5 R1 (2026-07-17): reka-ui Toast viewport for global
         error routing. Mounts at AppShell level so all 5 stub
         categories (Auth/RateLimit/Server/Network) render the
         same toast viewport (InvalidRequest stays console.warn).
         The `useToast` composable owns the queue + dedupe;
         `useErrorBus.routeByCategory` is the only consumer in
         main.ts. Single instance per app lifetime (Pinia-style
         singleton, mirrored on `useErrorBus`). -->
    <ToastProvider />

    <transition name="toast">
      <div
        v-if="projectsStore.toast"
        :class="['toast', `toast--${projectsStore.toast.kind}`]"
        @click="onToastClick"
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
  bottom: var(--space-6);
  left: 50%;
  transform: translateX(-50%);
  /* half-step padding (10/18 not on the 4-based scale) — kept raw
     per design-tokens.md "don't add a half-step token" rule. */
  padding: 10px 18px;
  border-radius: var(--radius-lg);
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  font-size: var(--text-base);
  box-shadow: var(--shadow-md);
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
  color: var(--color-text-on-accent);
}

.toast--info {
  background: var(--color-accent);
  color: var(--color-text-on-accent);
}

.toast-enter-active,
.toast-leave-active {
  transition: opacity var(--duration-slow) var(--ease-out), transform var(--duration-slow) var(--ease-out);
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translate(-50%, 8px);
}
</style>
