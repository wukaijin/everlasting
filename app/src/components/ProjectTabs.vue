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
//   - "📦" 12px icon for legacy/auto-default projects (tooltip: "旧数据，自动归入").
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

import { ref } from "vue";
import { useProjectsStore } from "../stores/projects";
import Icon from "./Icon.vue";

const store = useProjectsStore();

defineProps<{
  /** Set of project ids that have a streaming session. The store
   *  hands this in; the tab bar is purely presentational. */
  streamingProjectIds: Set<string>;
}>();

// P2.4 D6: manual-path input model (browser-mode degrade when the
// native folder picker is unavailable over httpTransport).
const manualPath = ref("");
const manualPathBusy = ref(false);

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

async function onManualPathSubmit() {
  if (!manualPath.value.trim() || manualPathBusy.value) return;
  manualPathBusy.value = true;
  try {
    await store.addProjectByPath(manualPath.value);
    manualPath.value = "";
  } finally {
    manualPathBusy.value = false;
  }
}

function onManualPathCancel() {
  manualPath.value = "";
  store.cancelManualPath();
}

function tabTooltip(p: {
  path: string;
  is_legacy: boolean;
  is_git_repo: boolean;
}): string {
  if (p.is_legacy) return `${p.path} (旧数据，自动归入)`;
  if (!p.is_git_repo) {
    return `${p.path} (非 git 项目 - session 可创建但无法附加 worktree)`;
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
          title="非 git 项目，无法附加 worktree"
        >
          <Icon name="warn" :size="12" />
        </span>
        <span
          v-else-if="p.is_legacy"
          class="tab__icon tab__icon--legacy"
          title="旧数据，自动归入"
        >
          <Icon name="archive" :size="12" />
        </span>
        <span
          v-if="streamingProjectIds.has(p.id)"
          class="tab__streaming"
          title="正在生成"
        >●</span>
        <button
          class="tab__close btn btn--ghost btn--icon"
          :title="'关闭 Tab(数据保留)'"
          :aria-label="`关闭 ${p.name}`"
          @click="(e) => onHide(p.id, e)"
        >
          <Icon name="x" :size="12" />
        </button>
      </div>
    </div>
    <button
      class="tabs__add btn btn--ghost"
      title="添加项目"
      :aria-label="'添加项目'"
      @click="onAdd"
    >
      <Icon name="plus" :size="16" />
    </button>
    <!-- P2.4 D6: browser-mode manual-path entry. Rendered when the
         native folder picker is unavailable (httpTransport —
         `pick_project_dir` has no daemon route). The daemon
         validates the path's existence via `create_project`. -->
    <div v-if="store.manualPathOpen" class="manual-path">
      <input
        v-model="manualPath"
        class="manual-path__input"
        type="text"
        placeholder="/absolute/path/to/project"
        :disabled="manualPathBusy"
        autofocus
        @keydown.enter.prevent="onManualPathSubmit"
        @keydown.esc.prevent="onManualPathCancel"
      />
      <button
        class="manual-path__btn manual-path__btn--confirm btn btn--ghost btn--icon"
        :disabled="manualPathBusy || !manualPath.trim()"
        title="添加"
        @click="onManualPathSubmit"
      >
        <Icon name="check" :size="14" />
      </button>
      <button
        class="manual-path__btn manual-path__btn--cancel btn btn--ghost btn--icon"
        title="取消"
        @click="onManualPathCancel"
      >
        <Icon name="x" :size="14" />
      </button>
    </div>
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
  cursor: pointer;
  /* 08-24 裸值清理:13px = --text-base 等值,落 token。tab 本身是
     <div> 非按钮,样式保留本地(--active 选中态家族不合)。 */
  font-size: var(--text-base);
  color: var(--color-text-secondary);
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out);
}

.tab:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

/* 2026-06-27 top-tab-bar boundary fix: the right-border between
   tabs now ONLY appears on inactive tabs. Active tabs used to keep
   the 1px gray border-right, which combined with the active tab's
   2px accent bottom (`inset` box-shadow below) created a visual
   "L" at the active↔inactive junction that read as a stray edge.
   Dropping the right-border on the active tab gives the accent a
   clean horizontal terminus at the tab edge. */
.tab:not(.tab--active) {
  border-right: 1px solid var(--color-bg-border);
}

.tab--active {
  background: var(--color-accent-muted);
  color: var(--color-text-primary);
}

/* 2026-06-27 top-tab-bar boundary fix: the active-state accent is
   now an `inset` box-shadow rather than an absolutely-positioned
   `::after` at `bottom: 0`. Reason: the parent AppHeader now owns
   the 1px bottom border (single source of truth for the top
   divider), and a `::after { bottom: 0 }` would render UNDER the
   border on the same pixel band — producing a "double layer" where
   the accent bled into the divider. `inset 0 -2px 0` paints the
   accent INSIDE the tab's bottom 2px, so it sits ABOVE the divider
   on the z-axis and reads as a clean tab-selection underline. */
.tab--active {
  box-shadow: inset 0 -2px 0 var(--color-accent);
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
  color: var(--color-tool-error-text);
  font-size: 9px;
  flex-shrink: 0;
  line-height: 1;
  animation: pulse 1.4s ease-in-out infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}

/* 08-24 btn-family:18px 固定 icon 钮由 ghost·icon 家族承载;本地保留
   固定几何 + hover 显隐(opacity reveal,需 opacity transition 家族未含;
   原裸 3px radius / 0.1s 删,落家族 radius-sm / fast)。hover 红实底是
   tab 关闭语义特例(设计文档备案,不立变体),本地覆写。 */
.tab__close {
  flex-shrink: 0;
  width: 18px;
  height: 18px;
  padding: 0;
  line-height: 1;
  opacity: 0;
  transition: opacity var(--duration-fast) var(--ease-out);
}

.tab:hover .tab__close,
.tab--active .tab__close {
  opacity: 1;
}

.tab__close:hover {
  background: var(--color-tool-error);
  color: var(--color-text-on-accent);
}

/* 08-24 btn-family:+ 添加 tab 由 ghost 家族承载(本体透明,hover 收敛
   家族 wash);本地保留 40px 通高几何。 */
.tabs__add {
  flex-shrink: 0;
  width: 40px;
  height: 100%;
  padding: 0;
}

/* P2.4 D6: browser-mode manual-path entry. */
.manual-path {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 0 8px;
  flex-shrink: 0;
}

.manual-path__input {
  width: 240px;
  height: 26px;
  padding: 0 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-text-primary);
  font-family: var(--font-mono);
  font-size: 0.75rem;
}

.manual-path__input:focus {
  /* 自有焦点替代::focus 换 accent 边框(见下),无需 UA 默认环 */
  outline: none;
  border-color: var(--color-accent);
}

.manual-path__input:disabled {
  opacity: 0.6;
}

/* 08-24 btn-family:manual-path 确认/取消 26px icon 钮由 ghost·icon
   家族承载(原 1px 边/裸 4px radius/0.1s 删,落家族);本地保留固定
   几何。confirm 的 hover 绿字是"路径有效"语义特例,本地覆写。 */
.manual-path__btn {
  width: 26px;
  height: 26px;
  padding: 0;
}

.manual-path__btn--confirm:hover:not(:disabled) {
  color: var(--color-status-success);
}
</style>
