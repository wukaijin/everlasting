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
</script>

<template>
  <aside class="sidebar">
    <div class="sidebar__header">
      <span class="sidebar__title">会话 SESSIONS</span>
      <button
        class="sidebar__add"
        title="新建会话"
        aria-label="新建会话"
        @click="onNew"
      >
        <Icon name="plus" :size="16" />
      </button>
    </div>
    <SessionList />
    <div class="sidebar__footer">
      <button
        type="button"
        class="sidebar__settings"
        title="设置"
        aria-label="设置"
        @click="onSettingsClick"
      >
        <Icon name="cog" :size="12" />
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
  border-right: 1px solid var(--color-bg-border);
  overflow: hidden;
}

.sidebar__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 16px 10px;
  flex-shrink: 0;
}

.sidebar__title {
  font-size: 11px;
  font-weight: 600;
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.sidebar__add {
  width: 22px;
  height: 22px;
  border-radius: 4px;
  border: none;
  background: transparent;
  color: var(--color-text-muted);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-family: inherit;
  padding: 0;
  transition: background 0.1s, color 0.1s;
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
  font-size: 11px;
  padding: 4px 6px;
  border-radius: 3px;
  transition: background 0.1s, color 0.1s;
}

.sidebar__settings:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.sidebar__settings-label {
  font-weight: 500;
}
</style>
