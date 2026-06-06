<script setup lang="ts">
// Sidebar — left-side session list. D5 restructure: the header
// "会话 SESSIONS" + single "+" icon now lives in the Sidebar wrapper
// itself, with the new-session handler pulled up from SessionList.
// SessionList below is just the <ul> of session items.
//
// Per spike-003 the sidebar is 260px wide; the active session gets a
// Prussian-muted background tint and a 2px accent left border.

import { useChatStore } from "../../stores/chat";
import SessionList from "../SessionList.vue";

const chat = useChatStore();

function onNew() {
  void chat.createNewSession();
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
      >+</button>
    </div>
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
  font-size: 18px;
  line-height: 1;
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
</style>
