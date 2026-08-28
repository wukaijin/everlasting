<script setup lang="ts">
// ProjectMemoryTab — Settings「项目」scope → 项目指令文件
// (2026-08-29 settings-shell 重构)。
//
// 薄封装:复用 MemoryPreview(kind="project")展示当前选中项目的
// CLAUDE.md + AGENTS.md 指令层。此前项目层只有 ProjectTabs 的
// Memory 下拉(MemoryModal)一个入口;Settings 的 Memory 分类刻意
// 只显示用户层 —— 项目 scope 落地后这里补上项目层入口,两者读同
// 一个 `memory` store,MemoryPreview 自理加载(mount 与 projectId
// 变化时自动 reload)。

import MemoryPreview from "../memory/MemoryPreview.vue";

defineProps<{
  /** 项目 scope 选择器当前选中的项目 id(null = 无可见项目)。 */
  projectId: string | null;
}>();
</script>

<template>
  <div class="project-memory-tab">
    <MemoryPreview v-if="projectId" kind="project" :project-id="projectId" />
    <p v-else class="project-memory-tab__empty">没有可选项目。</p>
  </div>
</template>

<style scoped>
.project-memory-tab {
  display: flex;
  flex-direction: column;
}

.project-memory-tab__empty {
  margin: 0;
  padding: var(--space-4);
  font-size: var(--text-sm);
  color: var(--color-text-muted);
  text-align: center;
}
</style>
