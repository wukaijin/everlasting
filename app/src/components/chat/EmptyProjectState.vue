<script setup lang="ts">
// EmptyProjectState — shown when no project is registered yet. Lets
// the user add a project (delegating to the projects store's native
// folder picker) or re-open a recently hidden project. This is
// pre-`ChatPanel` because the panel assumes a project is active.
//
// D5 polish: larger title in primary text color, a subtle icon row
// above the title, and a more prominent primary button (Prussian
// blue, larger). The hidden-projects section is now a compact,
// two-column list (name + path) with subtle dividers.
//
// RULE-FrontProj-001 fix: auto-load hidden projects on mount so the
// "最近隐藏的项目" list shows up immediately instead of forcing the
// user to click "查看最近隐藏的项目" first.

import { onMounted } from "vue";
import { useProjectsStore } from "../../stores/projects";
import Icon from "../Icon.vue";

const projectsStore = useProjectsStore();

onMounted(async () => {
  // Best-effort load; if the IPC fails the user can still click
  // the fallback "查看最近隐藏的项目" button below to retry.
  try {
    await projectsStore.loadHiddenProjects();
  } catch {
    // Silent — the fallback button remains usable.
  }
});

async function onAdd() {
  await projectsStore.addProject();
}

async function onUnhide(id: string) {
  await projectsStore.unhideProject(id);
}

async function onLoadHidden() {
  // Kept as a fallback for the "hiddenProjects.length === 0" branch
  // (i.e. the IPC failed on mount and the user wants to retry).
  await projectsStore.loadHiddenProjects();
}
</script>

<template>
  <main class="empty-state">
    <div class="empty-state__inner">
      <div class="empty-state__icon" aria-hidden="true">
        <Icon name="archive" :size="24" icon-class="empty-state__icon-glyph" />
      </div>
      <h1 class="empty-state__title">还没有项目</h1>
      <p class="empty-state__hint">
        添加一个项目目录,开始与 LLM 协作编码
      </p>
      <button class="empty-state__add" @click="onAdd">
        <Icon name="plus" :size="16" icon-class="empty-state__add-plus" />
        添加项目
      </button>

      <div
        v-if="projectsStore.hiddenProjects.length > 0"
        class="hidden-projects"
      >
        <div class="hidden-projects__sep" />
        <div class="hidden-projects__header">
          <span class="hidden-projects__title">最近隐藏的项目</span>
          <span class="hidden-projects__count">{{ projectsStore.hiddenProjects.length }}</span>
        </div>
        <ul class="hidden-projects__list">
          <li
            v-for="p in projectsStore.hiddenProjects"
            :key="p.id"
            class="hidden-projects__item"
          >
            <div class="hidden-projects__meta">
              <div class="hidden-projects__name-row">
                <span
                  v-if="p.is_legacy"
                  class="hidden-projects__icon"
                  title="旧数据,自动归入"
                >
                  <Icon name="archive" :size="13" />
                </span>
                <span
                  v-else-if="!p.is_git_repo"
                  class="hidden-projects__icon hidden-projects__icon--warn"
                  title="非 git 项目,无法附加 worktree"
                >
                  <Icon name="warn" :size="13" />
                </span>
                <span
                  v-else
                  class="hidden-projects__icon"
                >
                  <Icon name="archive" :size="13" />
                </span>
                <span class="hidden-projects__name" :title="p.name">{{ p.name }}</span>
              </div>
              <div class="hidden-projects__path" :title="p.path">{{ p.path }}</div>
            </div>
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
  background: var(--color-bg-app);
}

.empty-state__inner {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  text-align: center;
  max-width: 480px;
  width: 100%;
  padding: 16px;
}

.empty-state__icon {
  width: 56px;
  height: 56px;
  border-radius: 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  margin-bottom: 20px;
}

.empty-state__icon-glyph {
  color: var(--color-accent);
}

.empty-state__title {
  font-size: 20px;
  font-weight: 600;
  color: var(--color-text-primary);
  margin: 0 0 6px;
  letter-spacing: -0.01em;
}

.empty-state__hint {
  font-size: 13px;
  color: var(--color-text-muted);
  margin: 0 0 24px;
  line-height: 1.5;
}

.empty-state__add {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 11px 22px;
  border: 1px solid var(--color-accent);
  border-radius: 8px;
  background: var(--color-accent);
  color: #ffffff;
  font-size: 14px;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s, transform 0.05s;
  font-family: inherit;
  box-shadow: 0 1px 0 color-mix(in srgb, var(--color-accent) 35%, transparent);
}

.empty-state__add:hover {
  background: var(--color-accent-hover);
  border-color: var(--color-accent-hover);
}

.empty-state__add:active {
  transform: translateY(1px);
}

.empty-state__add-plus {
  line-height: 1;
  font-weight: 400;
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
  margin-top: 32px;
  text-align: left;
}

.hidden-projects__sep {
  height: 1px;
  background: var(--color-bg-border);
  margin: 0 0 12px;
}

.hidden-projects__header {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  margin-bottom: 8px;
  padding: 0 4px;
}

.hidden-projects__title {
  font-size: 11px;
  font-weight: 600;
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.hidden-projects__count {
  font-size: 11px;
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
  background: var(--color-bg-elevated);
  padding: 1px 6px;
  border-radius: 4px;
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
  gap: 12px;
  padding: 10px 12px;
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  background: var(--color-bg-surface);
  margin-bottom: 6px;
  font-size: 13px;
  transition: border-color 0.1s;
}

.hidden-projects__item:hover {
  border-color: var(--color-accent-muted);
}

.hidden-projects__meta {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.hidden-projects__name-row {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.hidden-projects__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  color: var(--color-text-secondary);
}

.hidden-projects__icon--warn {
  color: var(--color-tool-shell);
}

.hidden-projects__name {
  color: var(--color-text-primary);
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hidden-projects__path {
  font-size: 11px;
  color: var(--color-text-muted);
  font-family: var(--font-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl; /* keep the tail (path tail) visible when truncated */
  text-align: left;
}

.hidden-projects__btn {
  flex-shrink: 0;
  padding: 5px 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-accent);
  font-size: 12px;
  cursor: pointer;
  transition: background 0.1s, border-color 0.1s;
  font-family: inherit;
}

.hidden-projects__btn:hover {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
}
</style>
