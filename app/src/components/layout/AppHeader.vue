<script setup lang="ts">
// AppHeader — wraps the top project tab bar. Per D4 the TitleBar will
// be merged into this same row (single-line fusion: TitleBar 28px +
// ProjectTabs 16-20px). For D3 the header is a thin pass-through so
// the D4 step can edit a single file.

import { useChatStore } from "../../stores/chat";
import ProjectTabs from "../ProjectTabs.vue";

const chatStore = useChatStore();
</script>

<template>
  <header class="app-header">
    <ProjectTabs :streaming-project-ids="chatStore.streamingProjectIds" />
  </header>
</template>

<style scoped>
.app-header {
  flex-shrink: 0;
  background: var(--color-bg-surface);
  border-bottom: 1px solid var(--color-bg-border);
  z-index: 10;
}
</style>
