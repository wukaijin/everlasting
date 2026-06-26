<script setup lang="ts">
// Sidebar — left-side session list. D5 restructure: the header
// "会话 SESSIONS" + single "+" icon now lives in the Sidebar wrapper
// itself, with the new-session handler pulled up from SessionList.
// SessionList below is just the <ul> of session items.
//
// Per spike-003 the sidebar is 260px wide; the active session gets a
// Prussian-muted background tint and a 2px accent left border.
//
// PR5 follow-up: the Settings entry moved from the bottom-of-content
// `StatusBar` (PR4) into a `.sidebar__footer` here. This keeps the
// sidebar symmetric — top has the "SESSIONS" title, bottom has the
// meta "设置" button — and stops the gear from being lost in the
// chat input's visual hierarchy at the bottom of the main pane.
//
// 2026-06-27 sidebar 搜索/密度/分组 (PR-of-PRs, 3 features):
//   The header now has THREE icon buttons in the top-right
//   (search-toggle / density-toggle / new-session) plus the
//   "会话 SESSIONS" title. Search is a toggled state — when active
//   the search input row replaces the title-row, and the three
//   buttons collapse to just the new-session (+) button so the
//   user can't accidentally open density/search while typing.
//   Density state is lifted to Sidebar so the icon reflects it
//   AND so the SessionList renders with the matching modifier
//   class.

import { ref } from "vue";
import { useChatStore } from "../../stores/chat";
import SessionList from "../SessionList.vue";
import SettingsModal from "../settings/SettingsModal.vue";
import Icon from "../Icon.vue";

const chat = useChatStore();

function onNew() {
  void chat.createNewSession();
}

const settingsOpen = ref(false);

function onSettingsClick() {
  settingsOpen.value = !settingsOpen.value;
}

// 2026-06-27 sidebar 搜索入口 + 密度切换: state lifted to
// Sidebar so both buttons can share their icons + the SessionList
// receives `searchActive` as a prop. localStorage persistence
// lives in SessionList (it owns the per-density CSS modifier).
//
// `searchActive` is a one-way flip from the Sidebar; SessionList
// emits `search-clear` to flip it back to false when the user
// presses Esc on an empty query or clicks the ✕ button (after
// the user has already cleared the query text).
const searchActive = ref<boolean>(false);

function toggleSearch() {
  searchActive.value = !searchActive.value;
}

/** Density toggle (comfortable / compact). Lifted to Sidebar so
 *  the icon can reflect the current density, but the persisted
 *  state is read/written by SessionList (single source of truth
 *  — both header button and list read the same localStorage key).
 *  We track the value here only to flip the icon; the actual
 *  CSS modifier on the list comes from SessionList's own state. */
type Density = "comfortable" | "compact";
const density = ref<Density>(
  (localStorage.getItem("everlasting:sessionDensity") as Density) ||
    "comfortable",
);

function toggleDensity() {
  density.value = density.value === "comfortable" ? "compact" : "comfortable";
  try {
    localStorage.setItem("everlasting:sessionDensity", density.value);
  } catch {
    // Same swallow as SessionList — localStorage may be unavailable
    // (private mode, quota). In-memory value still works this session.
  }
}

function onSearchClear() {
  searchActive.value = false;
}
</script>

<template>
  <aside class="sidebar">
    <!--
      2026-06-27 sidebar header 改造: title row + 3 icon buttons.
      When search is active, the buttons collapse to just the
      new-session (+) — the user is typing, not clicking toggles.
      The SessionSearchInput mounts INSIDE SessionList when
      searchActive=true (it's part of the list's template, not
      this header's), so the title row stays unchanged.
    -->
    <div class="sidebar__header">
      <span class="sidebar__title">会话 SESSIONS</span>
      <div class="sidebar__actions">
        <button
          v-if="!searchActive"
          class="sidebar__action"
          type="button"
          :title="density === 'compact' ? '切换为舒适密度' : '切换为紧凑密度'"
          :aria-label="
            density === 'compact' ? '切换为舒适密度' : '切换为紧凑密度'
          "
          @click="toggleDensity"
        >
          <Icon name="adjustments" :size="14" />
        </button>
        <button
          v-if="!searchActive"
          class="sidebar__action"
          type="button"
          title="搜索会话 (Cmd/Ctrl+K)"
          aria-label="搜索会话"
          @click="toggleSearch"
        >
          <Icon name="magnifying-glass" :size="14" />
        </button>
        <button
          class="sidebar__add"
          type="button"
          title="新建会话"
          aria-label="新建会话"
          @click="onNew"
        >
          <Icon name="plus" :size="16" />
        </button>
      </div>
    </div>
    <SessionList
      :search-active="searchActive"
      @search-clear="onSearchClear"
    />
    <div class="sidebar__footer">
      <button
        type="button"
        class="sidebar__settings"
        title="设置"
        aria-label="设置"
        @click="onSettingsClick"
      >
        <Icon name="cog-6-tooth" :size="18" />
        <span class="sidebar__settings-label">设置</span>
      </button>
    </div>
    <SettingsModal v-model:open="settingsOpen" />
  </aside>
</template>

<style scoped>
.sidebar {
  width: 260px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  background: var(--color-bg-surface);
  /* 2026-06-27 top-tab-bar boundary fix: border color bumped from
     `--color-bg-border` (#1e2530) to `--color-bg-border-strong`
     (#3b475a). Reason: bg-surface (#131822) → bg-app (#0a0e14) is
     only 4 luminance units, so a 1px border at the regular color is
     a 4-unit jump — invisible on dim displays and washed out by
     screenshot compression. The strong color is +13 luminance units
     from bg-app and reads consistently in every capture. */
  border-right: 1px solid var(--color-bg-border-strong);
  overflow: hidden;
}

/* 2026-06-27 top-tab-bar boundary fix: header height locked to 40px
   to match AppHeader / ChatPanel header. Previously `padding:
   14px 16px 10px` produced ~35-36px, which made the "SESSIONS" text
   baseline NOT align with the ChatPanel header's title-row baseline
   when both rows are adjacent. Locking height: 40px + align-items:
   center + adjusting padding gives a stable y-coordinate for the
   sidebar header text and a stable visual anchor at the bottom. */
.sidebar__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 40px;
  padding: 0 16px;
  flex-shrink: 0;
}

.sidebar__title {
  font-size: var(--text-xs);
  font-weight: var(--weight-semibold);
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

/* 2026-06-27 sidebar header 改造: actions container holds the
   3 icon buttons (density / search / new-session). Flex gap
   keeps a uniform 4px between buttons; the row right-aligns via
   the header's `justify-content: space-between`. */
.sidebar__actions {
  display: inline-flex;
  align-items: center;
  gap: 2px;
}

.sidebar__action {
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  border: none;
  background: transparent;
  color: var(--color-text-muted);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-family: inherit;
  padding: 0;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out);
}

.sidebar__action:hover {
  background: var(--color-bg-elevated);
  color: var(--color-accent);
}

.sidebar__add {
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  border: none;
  background: transparent;
  color: var(--color-text-muted);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-family: inherit;
  padding: 0;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out);
}

.sidebar__add:hover {
  background: var(--color-bg-elevated);
  color: var(--color-accent);
}

/* PR5: bottom-of-sidebar footer that holds the Settings entry.
   Pinned to the bottom via `margin-top: auto` so the SessionList
   fills the available height above it. */
.sidebar__footer {
  flex-shrink: 0;
  margin-top: auto;
  border-top: 1px solid var(--color-bg-border);
  padding: 8px 16px;
  display: flex;
  align-items: center;
  justify-content: flex-start;
}

.sidebar__settings {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  padding: 4px 6px;
  border-radius: 3px;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out);
}

.sidebar__settings:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.sidebar__settings-label {
  font-weight: var(--weight-medium);
}
</style>
