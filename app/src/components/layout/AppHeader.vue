<script setup lang="ts">
// AppHeader — top of the application. Picks the top-bar shell based on
// the runtime context, then fills it with the SHARED top-bar content
// (project tabs + hidden-projects menu + pending badge).
//
// Two shells, same slot contract:
//   - Tauri webview  → TitleBar (drag region + window controls + OS
//     platform detection). Calls `getCurrentWindow()` / `platform()`
//     which only exist under the Tauri runtime.
//   - Plain browser   → BrowserHeader (logo + slot + spacer only).
//     Strips all Tauri-only chrome; the browser owns window controls.
//
// Why the split (P2 browser-degrade fix, 2026-07-23): TitleBar's
// `<script setup>` calls `getCurrentWindow()` synchronously at the top
// level (not inside a try/catch). In a plain browser that throws →
// component setup crashes → the whole AppHeader subtree (including
// ProjectTabs, the project switcher) disappears. Routing browsers to
// BrowserHeader (which has zero `@tauri-apps/api` imports) avoids the
// crash entirely. `isTauriWebview()` gates the choice.
//
// The shared slot content is declared once here (not duplicated in
// either shell) so adding a top-bar element only touches this file.
//
// PR3 of `06-07-6-ui-bug-markdown-sse`: the red-dot "this project has
// a streaming session" set moved out of the chat store into the
// streamController. Multiple sessions in the same project can stream
// concurrently, but the project tab only needs to know whether *any*
// session under it is streaming — the controller's
// `streamingProjectIds` computed set is exactly that. We read it here
// directly (rather than going through the chat store facade) so
// changes to the chat store's API don't ripple into the project-tab
// UI.

import { computed } from "vue";
import { useStreamControllerStore } from "../../stores/streamController";
import { useProjectsStore } from "../../stores/projects";
import { isTauriWebview } from "../../transport/env";
import TitleBar from "./TitleBar.vue";
import BrowserHeader from "./BrowserHeader.vue";
import ProjectTabs from "../ProjectTabs.vue";
import HiddenProjectsMenu from "../HiddenProjectsMenu.vue";
import PendingBadge from "./PendingBadge.vue";
import Icon from "../Icon.vue";
import { useMobileNav } from "../../composables/useMobileNav";
import { useSearchModal } from "../../composables/useSearchModal";

const streamController = useStreamControllerStore();
const projectsStore = useProjectsStore();
const { mobileNavOpen, toggle: toggleMobileNav } = useMobileNav();
// D2 (08-17-cross-session-search): global search entry. Mobile: the
// old sidebar Cmd+K hop lived inside the nav drawer, which is
// invisible unless the drawer is open — the header button gives an
// always-visible trigger. Browser/PWA desktop (08-17 hotfix): also
// shown, because Edge/Chrome don't reliably let pages override
// Ctrl+K (omnibox search). Tauri desktop keeps it hidden (Ctrl+K
// works uncontested there).
const { open: openSearch } = useSearchModal();
const showSearchButton = !isTauriWebview();
// S5: 无项目时隐藏汉堡(与 Sidebar v-if="showSidebar" 对称,review P3-3)。
// 空状态本身有"+ 添加项目"入口,汉堡点了也没东西弹。
const showHamburger = computed(
  () => projectsStore.currentProjectId !== null,
);

// Resolve the shell component once at setup. `<component :is>` below
// picks TitleBar or BrowserHeader; both expose the same default slot
// and the shared content is injected either way.
const shell = isTauriWebview() ? TitleBar : BrowserHeader;
</script>

<template>
  <header class="app-header">
    <component :is="shell">
      <!-- S5 移动端汉堡(桌面 .app-header__menu-toggle display:none,
           移动端 inline-flex)。无项目时 v-if 隐藏(review P3-3)。 -->
      <button
        v-if="showHamburger"
        class="app-header__menu-toggle"
        type="button"
        aria-label="打开导航"
        :aria-expanded="mobileNavOpen"
        @click="toggleMobileNav"
      >
        <Icon name="bars-3" :size="20" />
      </button>
      <!-- D2: global search entry (mobile always; browser/PWA desktop
           too — see showSearchButton rationale). -->
      <button
        class="app-header__search-toggle"
        :data-shown="showSearchButton ? '' : undefined"
        type="button"
        aria-label="全局搜索"
        title="全局搜索 (Ctrl/Cmd+Shift+F)"
        @click="openSearch()"
      >
        <Icon name="magnifying-glass" :size="18" />
      </button>
      <ProjectTabs
        :streaming-project-ids="streamController.streamingProjectIds"
        class="app-header__project-tabs"
      />
      <!-- RULE-FrontProj-001 fix: surfaces a "已隐藏项目" entry
           in the main UI (not just the empty state). Mounts only
           when at least one hidden project exists; the menu itself
           loads the list on mount. -->
      <HiddenProjectsMenu />
      <!-- 2026-07-08 cross-session-pending-indicator (B档): global
           pending-interaction count across all sessions/projects.
           Hidden when count === 0 (self-managed inside the badge).
           2026-08-21: QuotaChip removed — the 5h-window quota panel
           moved into the ChatInput hint-row token popover
           (`ChatInputTokenUsage.vue`). -->
      <PendingBadge />
    </component>
  </header>
</template>

<style scoped>
/* AppHeader owns the top-of-body divider. Per 2026-06-27 top-tab-bar
   boundary fix: TitleBar used to carry `border-bottom` itself, which
   conflicted with ProjectTabs' active-state `::after` accent (both
   rendered at the same pixel band). Hoisting the border here gives
   ProjectTabs a stable "anchor" to draw its accent above (z-axis) the
   divider cleanly, and stops the divider from disappearing if a
   future child component ever changes height. BrowserHeader relies on
   the same anchor (it deliberately carries no border of its own). */
.app-header {
  flex-shrink: 0;
  background: var(--color-bg-surface);
  border-bottom: 1px solid var(--color-bg-border);
  /* 局部层:盖普通流内容,低于一切弹层(--z-raised 100 起) */
  z-index: 10;
}

/* S5 移动端汉堡按钮 + D2 全局搜索入口。
   汉堡:桌面 display:none(零回归);移动端 inline-flex,触摸目标 44×44。
   搜索按钮:移动端始终显示;浏览器/PWA 桌面也显示(`[data-shown]`,
   08-17 hotfix——Edge/Chrome 的 Ctrl+K 不总能让页面抢占,浏览器桌面需要
   可点击入口);Tauri 桌面隐藏(Ctrl+K 无冲突)。共享样式放基础层,
   display 差异在 media 外单独控制,44px 触摸目标只在移动端叠加
   (桌面 header 40px 高,不强制 44)。 */
.app-header__menu-toggle,
.app-header__search-toggle {
  display: none;
}
.app-header__search-toggle[data-shown] {
  display: inline-flex;
}
.app-header__menu-toggle,
.app-header__search-toggle,
.app-header__search-toggle[data-shown] {
  align-items: center;
  justify-content: center;
  width: 44px;
  height: 100%;
  background: transparent;
  border: none;
  color: var(--color-text-secondary);
  cursor: pointer;
  padding: 0;
  flex-shrink: 0;
  font-family: inherit;
  transition: background var(--duration-fast) var(--ease-out),
    color var(--duration-fast) var(--ease-out);
}
.app-header__menu-toggle:hover,
.app-header__search-toggle:hover,
.app-header__search-toggle[data-shown]:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}
@media (max-width: 767px) {
  .app-header__menu-toggle,
  .app-header__search-toggle {
    display: inline-flex;
    min-height: 44px;
  }
  /* S5:AppHeader 的 ProjectTabs 移动端隐藏(挪进 Sidebar 抽屉顶部,见
       .sidebar__project-tabs)。桌面常驻不动。 */
  .app-header__project-tabs {
    display: none;
  }
}
</style>
