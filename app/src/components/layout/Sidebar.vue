<script setup lang="ts">
// Sidebar — left-side session list. Wraps the existing
// SessionList (extracted during 3b-1) with the dark-theme chrome
// (background + right border + fixed width per reference image A).
//
// Per spike-003 the sidebar is 260px wide; the active session gets
// a Prussian-muted background tint and a 2px accent left border.

import SessionList from "../SessionList.vue";
</script>

<template>
  <aside class="sidebar">
    <SessionList />
  </aside>
</template>

<style scoped>
.sidebar {
  width: 260px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  background: var(--color-bg-surface);
  border-right: 1px solid var(--color-bg-border);
  overflow: hidden;
}
</style>
