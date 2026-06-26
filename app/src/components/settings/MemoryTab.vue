<script setup lang="ts">
// MemoryTab — Settings page tab content for the B5 memory preview.
//
// The Settings page is project-agnostic: it shows the 2 User
// layers (User CLAUDE.md + User AGENTS.md) only, since those
// are global across all projects. The Project layers are
// scoped to the active project and live behind the ProjectTabs
// "Memory" dropdown, not here.
//
// Wraps `MemoryPreview` with `kind="user"`. The preview handles
// loading / error / refresh state itself; this component is
// just the tab-content shell with a one-line description.

import MemoryPreview from "../memory/MemoryPreview.vue";
</script>

<template>
  <div class="memory-tab">
    <p class="memory-tab__intro">
      您的个人指令文件 — 在 session 启动时自动注入到对话上下文中(全局，所有项目可见)。
      点击展开预览;修改后 1 秒内自动重新加载。
    </p>
    <MemoryPreview kind="user" />
  </div>
</template>

<style scoped>
.memory-tab {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.memory-tab__intro {
  margin: 0 0 4px 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: 1.6;
}

.memory-tab__intro code {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 3px;
  padding: 0 4px;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-accent);
}
</style>
