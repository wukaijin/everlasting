<script setup lang="ts">
// ProjectTabs — top tab bar for switching between registered projects.
//
// Per Q7 (PROPOSAL §5.3):
//   - No tab count limit; horizontal overflow scroll.
//   - Min 100px / max 240px per tab (ellipsis on overflow).
//   - "+" button fixed at the right end.
// Per Q3 (PROPOSAL §5.3):
//   - Show a red "●" on a tab while its session is streaming.
// Per Q-resolutions (Q3 dispatch):
//   - "⚠️" 12px icon for non-git projects (tooltip: "非 git 项目 — session 可创建但无法附加 worktree").
//   - "📦" 12px icon for legacy/auto-default projects (tooltip: "旧数据,自动归入").
//   - "×" close button calls `hide_project` (data preserved).
//   - Selected tab gets a 2px Prussian blue underline + muted bg.
//
// D3 restyle: dark theme tokens. Active tab uses the Prussian-muted
// background and accent underline per spike-003.
//
// B5 follow-up (2026-06-10): the "Memory" entry was originally a
// hand-rolled dropdown attached to this tab bar. That popover had a
// `right: 0; min-width: 480px` overflow bug when the trigger was not
// at the viewport's right edge (the popover spilled off-screen to
// the left). 2026-06-11 follow-up
// (`06-11-memory-modal-appheader-entry`, the task ID is a pre-pivot
// name — see the spec for context) moved the entry to a Brain icon
// button in `ChatPanel.vue`'s header + a reka-ui Dialog modal. The
// modal has no positioning bug and is semantically cleaner (Memory
// is not a project; it lives next to the session context chips).
// All Memory state lives in `useMemoryStore`; this component no
// longer holds any Memory UI.

import { useProjectsStore } from "../stores/projects";
import Icon from "./Icon.vue";

const store = useProjectsStore();

defineProps<{
  /** Set of project ids that have a streaming session. The store
   *  hands this in; the tab bar is purely presentational. */
  streamingProjectIds: Set<string>;
}>();

function onTabClick(id: string) {
  void store.switchProject(id);
}

function onHide(id: string, e: MouseEvent) {
  e.stopPropagation();
  void store.hideProject(id);
}

async function onAdd() {
  await store.addProject();
}

function tabTooltip(p: {
  path: string;
  is_legacy: boolean;
  is_git_repo: boolean;
}): string {
  if (p.is_legacy) return `${p.path} (旧数据,自动归入)`;
  if (!p.is_git_repo) {
    return `${p.path} (非 git 项目 — session 可创建但无法附加 worktree)`;
  }
  return p.path;
}
</script>

<template>
  <div class="tabs">
    <div class="tabs__scroll">
      <div
        v-for="p in store.projects"
        :key="p.id"
        :class="['tab', { 'tab--active': p.id === store.currentProjectId }]"
        role="button"
        tabindex="0"
        :title="tabTooltip(p)"
        @click="onTabClick(p.id)"
        @keydown.enter="onTabClick(p.id)"
        @keydown.space.prevent="onTabClick(p.id)"
      >
        <span class="tab__name">{{ p.name }}</span>
        <span
          v-if="!p.is_git_repo && !p.is_legacy"
          class="tab__icon tab__icon--warn"
          title="非 git 项目,无法附加 worktree"
        >
          <Icon name="warn" :size="12" />
        </span>
        <span
          v-else-if="p.is_legacy"
          class="tab__icon tab__icon--legacy"
          title="旧数据,自动归入"
        >
          <Icon name="archive" :size="12" />
        </span>
        <span
          v-if="streamingProjectIds.has(p.id)"
          class="tab__streaming"
          title="正在生成"
        >●</span>
        <button
          class="tab__close"
          :title="'关闭 Tab(数据保留)'"
          :aria-label="`关闭 ${p.name}`"
          @click="(e) => onHide(p.id, e)"
        >
          <Icon name="x" :size="12" />
        </button>
      </div>
    </div>
    <button
      class="tabs__add"
      title="添加项目"
      :aria-label="'添加项目'"
      @click="onAdd"
    >
      <Icon name="plus" :size="16" />
    </button>
  </div>
</template>

<style scoped>
.tabs {
  display: flex;
  align-items: stretch;
  background: var(--color-bg-surface);
  height: 40px;
  flex-shrink: 0;
}

.tabs__scroll {
  display: flex;
  flex: 1;
  min-width: 0;
  overflow-x: auto;
  overflow-y: hidden;
}

.tabs__scroll::-webkit-scrollbar {
  height: 4px;
}

.tabs__scroll::-webkit-scrollbar-thumb {
  background: var(--color-bg-border);
  border-radius: 2px;
}

.tabs__scroll::-webkit-scrollbar-track {
  background: transparent;
}

.tab {
  position: relative;
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 100px;
  max-width: 240px;
  flex-shrink: 0;
  padding: 0 6px 0 10px;
  height: 100%;
  background: transparent;
  border: none;
  border-right: 1px solid var(--color-bg-border);
  cursor: pointer;
  font-size: 13px;
  color: var(--color-text-secondary);
  transition: background 0.1s, color 0.1s;
  font-family: inherit;
}

.tab:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.tab--active {
  background: var(--color-accent-muted);
  color: var(--color-text-primary);
}

.tab--active::after {
  content: "";
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 2px;
  background: var(--color-accent);
}

.tab__name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: left;
}

.tab__icon {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  line-height: 1;
}

.tab__icon--warn {
  color: var(--color-tool-shell);
}

.tab__icon--legacy {
  color: var(--color-text-muted);
}

.tab__streaming {
  color: var(--color-tool-error);
  font-size: 9px;
  flex-shrink: 0;
  line-height: 1;
  animation: pulse 1.4s ease-in-out infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}

.tab__close {
  flex-shrink: 0;
  width: 18px;
  height: 18px;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  line-height: 1;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.1s, background 0.1s, color 0.1s;
  padding: 0;
  font-family: inherit;
}

.tab:hover .tab__close,
.tab--active .tab__close {
  opacity: 1;
}

.tab__close:hover {
  background: var(--color-tool-error);
  color: #ffffff;
}

.tabs__add {
  flex-shrink: 0;
  width: 40px;
  height: 100%;
  background: transparent;
  border: none;
  border-left: 1px solid var(--color-bg-border);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--color-text-secondary);
  transition: background 0.1s, color 0.1s;
  font-family: inherit;
  padding: 0;
}

.tabs__add:hover {
  background: var(--color-accent-muted);
  color: var(--color-accent);
}
</style>
