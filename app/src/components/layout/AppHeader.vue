<script setup lang="ts">
// AppHeader — top of the application, single-row fusion of TitleBar
// (drag region + window controls) and ProjectTabs (per PRD decision #4
// & research/tauri-titlebar-patterns.md).
//
// Layout (left → right, all on one 40px-tall row):
//   - macOS: 80px traffic-light spacer (drag) | ProjectTabs (interactive)
//     | flexible drag-region spacer | (no self-drawn controls — macOS
//     uses the native red lights)
//   - Windows / Linux / WSLg: 0 left pad | ProjectTabs (interactive)
//     | flexible drag-region spacer | 3 self-drawn min/max/close buttons
//
// The drag region is owned by TitleBar. ProjectTabs' root is wrapped
// in a div with `data-tauri-drag-region="false"` (TitleBar's slot
// wrapper does this) so the tabs stay clickable and the horizontal
// scroll inside `.tabs__scroll` continues to work.
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
import TitleBar from "./TitleBar.vue";
import ProjectTabs from "../ProjectTabs.vue";
import HiddenProjectsMenu from "../HiddenProjectsMenu.vue";

const streamController = useStreamControllerStore();
</script>

<template>
  <header class="app-header">
    <TitleBar>
      <ProjectTabs :streaming-project-ids="streamController.streamingProjectIds" />
      <!-- RULE-FrontProj-001 fix: surfaces a "已隐藏项目" entry
           in the main UI (not just the empty state). Mounts only
           when at least one hidden project exists; the menu itself
           loads the list on mount. -->
      <HiddenProjectsMenu />
    </TitleBar>
  </header>
</template>

<style scoped>
/* AppHeader owns the top-of-body divider. Per 2026-06-27 top-tab-bar
   boundary fix: TitleBar used to carry `border-bottom` itself, which
   conflicted with ProjectTabs' active-state `::after` accent (both
   rendered at the same pixel band). Hoisting the border here gives
   ProjectTabs a stable "anchor" to draw its accent above (z-axis) the
   divider cleanly, and stops the divider from disappearing if a
   future child component ever changes height. */
.app-header {
  flex-shrink: 0;
  background: var(--color-bg-surface);
  border-bottom: 1px solid var(--color-bg-border);
  z-index: 10;
}
</style>
