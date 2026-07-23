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

import { useStreamControllerStore } from "../../stores/streamController";
import { isTauriWebview } from "../../transport/env";
import TitleBar from "./TitleBar.vue";
import BrowserHeader from "./BrowserHeader.vue";
import ProjectTabs from "../ProjectTabs.vue";
import HiddenProjectsMenu from "../HiddenProjectsMenu.vue";
import PendingBadge from "./PendingBadge.vue";

const streamController = useStreamControllerStore();

// Resolve the shell component once at setup. `<component :is>` below
// picks TitleBar or BrowserHeader; both expose the same default slot
// and the shared content is injected either way.
const shell = isTauriWebview() ? TitleBar : BrowserHeader;
</script>

<template>
  <header class="app-header">
    <component :is="shell">
      <ProjectTabs :streaming-project-ids="streamController.streamingProjectIds" />
      <!-- RULE-FrontProj-001 fix: surfaces a "已隐藏项目" entry
           in the main UI (not just the empty state). Mounts only
           when at least one hidden project exists; the menu itself
           loads the list on mount. -->
      <HiddenProjectsMenu />
      <!-- 2026-07-08 cross-session-pending-indicator (B档): global
           pending-interaction count across all sessions/projects.
           Hidden when count === 0 (self-managed inside the badge). -->
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
  z-index: 10;
}
</style>
