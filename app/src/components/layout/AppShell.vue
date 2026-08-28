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

import { computed, onMounted } from "vue";
import { useProjectsStore } from "../../stores/projects";
import { useChatStore } from "../../stores/chat";
import { useScheduledTasksStore } from "../../stores/scheduledTasks";
import { registerKeybinding } from "../../utils/useKeyboard";
import { useSearchModal } from "../../composables/useSearchModal";
import AppHeader from "./AppHeader.vue";
import Sidebar from "./Sidebar.vue";
import TracePanel from "../trace/TracePanel.vue";
import ToastProvider from "../common/ToastProvider.vue";
import SearchModal from "../search/SearchModal.vue";
import { useMobileNav } from "../../composables/useMobileNav";

const projectsStore = useProjectsStore();
const chatStore = useChatStore();
const showSidebar = computed<boolean>(
  () => projectsStore.currentProjectId !== null,
);
// D2 (08-17-cross-session-search): global search modal. Cmd/Ctrl+K
// was repurposed from "focus the sidebar title filter" (that
// binding was removed from SessionList; the filter itself is
// untouched). AppShell owns the mount because the search must be
// reachable regardless of which view fills the main slot — same
// rationale as the TracePanel drawer above.
const { open: openSearch } = useSearchModal();
registerKeybinding({
  key: "k",
  ctrlOrMeta: true,
  handler: (e) => {
    e.preventDefault();
    openSearch();
  },
});
// 08-17 hotfix: Edge (and some Chrome setups) reserve Ctrl+K for
// the omnibox search and don't always let the page override it.
// Ctrl/Cmd+Shift+F is the escape-hatch alias (VS Code global-search
// muscle memory; no browser default binding). Both stay registered
// — Tauri desktop keeps the single-tap Ctrl+K.
registerKeybinding({
  key: "f",
  ctrlOrMeta: true,
  shiftKey: true,
  handler: (e) => {
    e.preventDefault();
    openSearch();
  },
});
// S5 移动端抽屉导航(module-level 单例 composable,与 useToast 同构):AppHeader
// 汉堡 toggle,Sidebar 读 mobileNavOpen + 选 session 自动 close,本组件渲染
// 遮罩(@click close)。桌面下 open 状态被 CSS 忽略(Sidebar 常驻,fixed 定位
// 只在 @media max-width:767px 生效)。
const { mobileNavOpen, close: closeMobileNav } = useMobileNav();

// F2 定时任务 (2026-08-28): 启动拉一次全量任务列表(设计 §7 ——
// session header「活跃任务」徽章的数据源)。fire-and-forget:失败仅
// 落 store.error(徽章降态不渲染),不阻塞首屏。管理面(Settings
// tab)挂载/变更时会再拉,保持徽章新鲜。
const scheduledTasksStore = useScheduledTasksStore();
onMounted(() => {
  void scheduledTasksStore.load();
});

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

    <!-- S5 移动端抽屉遮罩:仅 @media (max-width:767px) 显示(CSS display:none
         桌面)。v-if 双绑 showSidebar —— 无项目时 Sidebar 不存在,遮罩也不该
         出现(汉堡那时也隐藏,双保险)。 -->
    <transition name="sidebar-overlay">
      <div
        v-if="mobileNavOpen && showSidebar"
        class="app-shell__sidebar-overlay"
        @click="closeMobileNav"
      />
    </transition>

    <!-- E2 (harness trace pipeline, 2026-07-14): trace-viewer
         drawer. Mounts at the AppShell level (sibling of the
         main slot) so the timeline stays available even when the
         chat surface is hidden. The slide-in / slide-out
         transition lives inside the panel itself. -->
    <TracePanel />

    <!-- D2 (08-17-cross-session-search): global search dialog
         (Cmd/Ctrl+K). Mounted at AppShell level for the same
         reason as TracePanel. -->
    <SearchModal />

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

/* S5 移动端:URL bar 伸缩用 dvh(桌面 height:100vh 基线不动)。
   iOS Safari 100vh = large viewport(地址栏收起态),地址栏出现时底部被遮;
   100dvh 随 URL bar 自动收缩。见 design §4.1。 */
@media (max-width: 767px) {
  .app-shell {
    /* --visual-viewport-height 由 useMobileKeyboard 写入(Step 6):软键盘
       弹起时 = visualViewport.height(键盘上方),否则回退 --app-height(dvh)。 */
    height: var(--visual-viewport-height, var(--app-height));
  }
}

/* S5 移动端抽屉遮罩。桌面 display:none(零回归)。z-index 105:低于 Sidebar
   抽屉(110,盖住遮罩),高于 TracePanel(100)。 */
.app-shell__sidebar-overlay {
  display: none;
}
@media (max-width: 767px) {
  .app-shell__sidebar-overlay {
    display: block;
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: var(--z-drawer-overlay);
  }
}
.sidebar-overlay-enter-active,
.sidebar-overlay-leave-active {
  transition: opacity var(--duration-base) var(--ease-out);
}
.sidebar-overlay-enter-from,
.sidebar-overlay-leave-to {
  opacity: 0;
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
  z-index: var(--z-top);
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
