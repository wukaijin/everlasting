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
// B5 follow-up (2026-06-10): added the "Memory" dropdown trigger
// on the right of the project tab list. The dropdown is the
// project-layer entry point (the 2 Project CLAUDE.md / AGENTS.md
// files for the active project). User-layer memory is exposed via
// the Settings page's "Memory" tab.

import { computed, onUnmounted, ref, watch } from "vue";

import { useProjectsStore } from "../stores/projects";
import { useMemoryStore } from "../stores/memory";
import Icon from "./Icon.vue";
import MemoryPreview from "./memory/MemoryPreview.vue";

const store = useProjectsStore();
const memoryStore = useMemoryStore();

defineProps<{
  /** Set of project ids that have a streaming session. The store
   *  hands this in; the tab bar is purely presentational. */
  streamingProjectIds: Set<string>;
}>();

function onTabClick(id: string) {
  void store.switchProject(id);
  // Close any open memory dropdown when switching projects — the
  // dropdown's contents are project-scoped, so leaving it open
  // would show stale state during the brief window before the
  // new project's layers load.
  memoryMenuOpen.value = false;
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

// --- Memory dropdown (B5) --------------------------------------------
// Hand-rolled popover per `.trellis/spec/frontend/popover-pattern.md`.
// The Memory dropdown shows the 2 Project layers for the active
// project. We re-use the existing `MemoryPreview` component with
// `kind="project"` so the panel and the dropdown render identically.

const memoryMenuOpen = ref(false);
const memoryMenuRoot = ref<HTMLElement | null>(null);

const activeProjectId = computed<string | null>(
  () => store.currentProjectId,
);

function toggleMemoryMenu() {
  memoryMenuOpen.value = !memoryMenuOpen.value;
}

function onMemoryDocumentClick(e: MouseEvent) {
  if (!memoryMenuOpen.value) return;
  const target = e.target as Node | null;
  if (memoryMenuRoot.value && target && !memoryMenuRoot.value.contains(target)) {
    memoryMenuOpen.value = false;
  }
}

function onMemoryKeydown(e: KeyboardEvent) {
  if (memoryMenuOpen.value && e.key === "Escape") {
    memoryMenuOpen.value = false;
  }
}

if (typeof document !== "undefined") {
  document.addEventListener("click", onMemoryDocumentClick);
  document.addEventListener("keydown", onMemoryKeydown);
  onUnmounted(() => {
    document.removeEventListener("click", onMemoryDocumentClick);
    document.removeEventListener("keydown", onMemoryKeydown);
  });
}

// Eagerly load the memory layers for the active project when the
// dropdown opens (idempotent — the store skips re-fetches when
// the project id is unchanged). This makes the dropdown's first
// open feel instant on the second+ open; on the first open the
// user sees the "加载中" state for ~50ms.
watch(memoryMenuOpen, (open) => {
  if (open && activeProjectId.value) {
    void memoryStore.loadForProject(activeProjectId.value);
  }
});
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
    <div
      v-if="store.currentProjectId"
      ref="memoryMenuRoot"
      class="tabs__memory"
    >
      <button
        class="tabs__memory-trigger"
        :class="{ 'tabs__memory-trigger--open': memoryMenuOpen }"
        type="button"
        :title="'查看项目 memory (CLAUDE.md / AGENTS.md)'"
        :aria-label="'Memory'"
        :aria-expanded="memoryMenuOpen"
        @click="toggleMemoryMenu"
      >
        <Icon name="document" :size="14" />
        <span class="tabs__memory-label">Memory</span>
        <Icon
          :name="memoryMenuOpen ? 'chevron-up' : 'chevron-down'"
          :size="10"
        />
      </button>
      <div v-if="memoryMenuOpen" class="tabs__memory-popover">
        <div class="tabs__memory-popover-inner">
          <MemoryPreview kind="project" :project-id="activeProjectId" />
        </div>
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

/* ------------------------------------------------------------------- */
/* Memory dropdown (B5 follow-up)                                       */
/* ------------------------------------------------------------------- */

.tabs__memory {
  position: relative;
  flex-shrink: 0;
  display: flex;
  align-items: center;
}

.tabs__memory-trigger {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 100%;
  padding: 0 10px;
  background: transparent;
  border: none;
  border-left: 1px solid var(--color-bg-border);
  cursor: pointer;
  color: var(--color-text-secondary);
  font-family: inherit;
  font-size: 12px;
  transition: background 0.1s, color 0.1s;
}

.tabs__memory-trigger:hover,
.tabs__memory-trigger--open {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.tabs__memory-label {
  font-weight: 500;
}

.tabs__memory-popover {
  position: absolute;
  top: 100%;
  right: 0;
  z-index: 1500;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
  min-width: 480px;
  max-width: 600px;
  max-height: 70vh;
  overflow: hidden;
  animation: memory-popover-slide 150ms ease-out;
}

.tabs__memory-popover-inner {
  max-height: 70vh;
  overflow-y: auto;
  padding: 14px;
}

@keyframes memory-popover-slide {
  from { opacity: 0; transform: translateY(-4px); }
  to   { opacity: 1; transform: translateY(0); }
}
</style>
