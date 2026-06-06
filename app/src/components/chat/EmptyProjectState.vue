<script setup lang="ts">
// EmptyProjectState — shown when no project is registered yet. Lets
// the user add a project (delegating to the projects store's native
// folder picker) or re-open a recently hidden project. This is
// pre-`ChatPanel` because the panel assumes a project is active.

import { useProjectsStore } from "../../stores/projects";

const projectsStore = useProjectsStore();

async function onAdd() {
  await projectsStore.addProject();
}

async function onUnhide(id: string) {
  await projectsStore.unhideProject(id);
}

async function onLoadHidden() {
  await projectsStore.loadHiddenProjects();
}
</script>

<template>
  <main class="empty-state">
    <div class="empty-state__card">
      <p class="empty-state__title">还没有项目</p>
      <p class="empty-state__hint">
        点上方「+ 添加项目」,从文件系统选个目录开始
      </p>
      <button class="empty-state__add" @click="onAdd">+ 添加项目</button>

      <div
        v-if="projectsStore.hiddenProjects.length > 0"
        class="hidden-projects"
      >
        <div class="hidden-projects__sep" />
        <div class="hidden-projects__title">最近隐藏的项目</div>
        <ul class="hidden-projects__list">
          <li
            v-for="p in projectsStore.hiddenProjects"
            :key="p.id"
            class="hidden-projects__item"
          >
            <span class="hidden-projects__name" :title="p.path">
              <span
                v-if="p.is_legacy"
                class="hidden-projects__icon"
                title="旧数据,自动归入"
              >📦</span>
              <span
                v-else-if="!p.is_git_repo"
                class="hidden-projects__icon hidden-projects__icon--warn"
                title="未启用 git 隔离"
              >⚠️</span>
              {{ p.name }}
            </span>
            <button class="hidden-projects__btn" @click="onUnhide(p.id)">
              重新打开
            </button>
          </li>
        </ul>
      </div>

      <button
        v-else
        class="empty-state__load-hidden"
        @click="onLoadHidden"
      >
        查看最近隐藏的项目
      </button>
    </div>
  </main>
</template>

<style scoped>
.empty-state {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 32px 16px;
}

.empty-state__card {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: var(--color-text-secondary);
  text-align: center;
  max-width: 480px;
  padding: 32px 16px;
}

.empty-state__card p {
  margin: 4px 0;
}

.empty-state__title {
  font-size: 18px;
  font-weight: 500;
  color: var(--color-text-primary);
  margin: 0 0 8px;
}

.empty-state__hint {
  font-size: 12px;
  color: var(--color-text-muted);
}

.empty-state__add {
  margin-top: 20px;
  padding: 10px 22px;
  border: 1px solid var(--color-accent);
  border-radius: 8px;
  background: var(--color-accent);
  color: #ffffff;
  font-size: 14px;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s;
  font-family: inherit;
}

.empty-state__add:hover {
  background: var(--color-accent-hover);
  border-color: var(--color-accent-hover);
}

.empty-state__load-hidden {
  margin-top: 20px;
  padding: 6px 12px;
  background: transparent;
  border: none;
  color: var(--color-text-secondary);
  font-size: 12px;
  cursor: pointer;
  text-decoration: underline;
  font-family: inherit;
}

.empty-state__load-hidden:hover {
  color: var(--color-accent);
}

.hidden-projects {
  width: 100%;
  margin-top: 24px;
  text-align: left;
}

.hidden-projects__sep {
  height: 1px;
  background: var(--color-bg-border);
  margin: 0 0 16px;
}

.hidden-projects__title {
  font-size: 12px;
  font-weight: 600;
  color: var(--color-text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin-bottom: 8px;
}

.hidden-projects__list {
  list-style: none;
  margin: 0;
  padding: 0;
}

.hidden-projects__item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 10px;
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  background: var(--color-bg-surface);
  margin-bottom: 6px;
  font-size: 13px;
}

.hidden-projects__name {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--color-text-primary);
}

.hidden-projects__icon {
  flex-shrink: 0;
  font-size: 12px;
}

.hidden-projects__icon--warn {
  color: var(--color-tool-shell);
}

.hidden-projects__btn {
  flex-shrink: 0;
  margin-left: 8px;
  padding: 4px 10px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-accent);
  font-size: 12px;
  cursor: pointer;
  transition: background 0.1s;
  font-family: inherit;
}

.hidden-projects__btn:hover {
  background: var(--color-accent-muted);
}
</style>
