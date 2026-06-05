<script setup lang="ts">
// SessionList — session list shown in the left sidebar when a project
// is active. Extracted from ChatWindow.vue (PR2 scope) so the
// project tab bar can sit above it cleanly.
//
// Per Q4v2 (PROPOSAL §5.2): default to the 8 most-recently-updated
// sessions; if there are more, render a "查看更早的 N 个" button at
// the bottom that toggles to show the full list. This is purely
// view-side folding — no schema change, no archive state.
//
// Per Q5: the session's `current_cwd` is shown in the chat header
// (see ChatWindow.vue), NOT in the session list rows. The PR1
// backend exposes it on `SessionSummary` so the data is available;
// the row keeps `title` + `preview` only for visual simplicity.

import { computed, ref } from "vue";
import { useChatStore, type SessionSummary } from "../stores/chat";

const store = useChatStore();

const DEFAULT_VISIBLE = 8;
const expanded = ref(false);

const visibleSessions = computed<SessionSummary[]>(() => {
  const all = store.sessions;
  if (expanded.value || all.length <= DEFAULT_VISIBLE) {
    return all;
  }
  return all.slice(0, DEFAULT_VISIBLE);
});

const hiddenCount = computed<number>(() => {
  const total = store.sessions.length;
  if (expanded.value || total <= DEFAULT_VISIBLE) return 0;
  return total - DEFAULT_VISIBLE;
});

function onClick(id: string) {
  void store.switchSession(id);
}

function onDelete(id: string, e: MouseEvent) {
  e.stopPropagation();
  if (store.sending && id === store.currentSessionId) return;
  if (!confirm("删除此 session 及其所有消息？")) return;
  void store.deleteSession(id);
}

function onNew() {
  void store.createNewSession();
}
</script>

<template>
  <div class="sidebar-list">
    <div class="sidebar-list__header">
      <span class="sidebar-list__title">Sessions</span>
    </div>
    <button class="sidebar-list__new" @click="onNew">+ 新对话</button>
    <ul class="sidebar-list__items">
      <li
        v-for="s in visibleSessions"
        :key="s.id"
        :class="[
          'session-item',
          { 'session-item--active': s.id === store.currentSessionId },
        ]"
        @click="onClick(s.id)"
      >
        <div class="session-item__main">
          <div class="session-item__title">{{ s.title }}</div>
          <div v-if="s.preview" class="session-item__preview">{{ s.preview }}</div>
        </div>
        <button
          class="session-item__delete"
          title="删除"
          @click="(e) => onDelete(s.id, e)"
        >×</button>
      </li>
      <li v-if="store.sessions.length === 0" class="session-empty">
        还没有对话，点上方按钮开始
      </li>
      <li v-else-if="hiddenCount > 0" class="session-more">
        <button class="session-more__btn" @click="expanded = true">
          查看更早的 {{ hiddenCount }} 个
        </button>
      </li>
      <li v-else-if="expanded && store.sessions.length > DEFAULT_VISIBLE" class="session-more">
        <button class="session-more__btn" @click="expanded = false">
          收起
        </button>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.sidebar-list {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.sidebar-list__header {
  padding: 14px 16px 8px;
}

.sidebar-list__title {
  font-size: 12px;
  font-weight: 600;
  color: #6b7280;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.sidebar-list__new {
  margin: 0 12px 8px;
  padding: 8px 12px;
  border: 1px solid #d1d5db;
  border-radius: 6px;
  background: #ffffff;
  color: #1f2328;
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  text-align: left;
  transition: background 0.15s;
  font-family: inherit;
}

.sidebar-list__new:hover {
  background: #f9fafb;
  border-color: #9ca3af;
}

.sidebar-list__items {
  list-style: none;
  margin: 0;
  padding: 0 8px 8px;
  overflow-y: auto;
  flex: 1;
}

.session-item {
  display: flex;
  align-items: flex-start;
  gap: 4px;
  padding: 8px 10px;
  margin-bottom: 2px;
  border-radius: 6px;
  cursor: pointer;
  transition: background 0.1s;
}

.session-item:hover {
  background: #e5e7eb;
}

.session-item--active {
  background: #ffffff;
  border: 1px solid #d1d5db;
}

.session-item--active:hover {
  background: #ffffff;
}

.session-item__main {
  flex: 1;
  min-width: 0;
}

.session-item__title {
  font-size: 13px;
  font-weight: 500;
  color: #1f2328;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-item__preview {
  font-size: 11px;
  color: #6b7280;
  margin-top: 2px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-item__delete {
  flex-shrink: 0;
  width: 20px;
  height: 20px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: #9ca3af;
  font-size: 16px;
  line-height: 1;
  cursor: pointer;
  opacity: 0;
  transition: all 0.1s;
  padding: 0;
  font-family: inherit;
}

.session-item:hover .session-item__delete {
  opacity: 1;
}

.session-item__delete:hover {
  background: #fca5a5;
  color: #ffffff;
}

.session-empty {
  padding: 16px 12px;
  font-size: 12px;
  color: #9ca3af;
  text-align: center;
}

.session-more {
  padding: 6px 12px;
  text-align: center;
}

.session-more__btn {
  width: 100%;
  background: transparent;
  border: 1px solid #e5e7eb;
  border-radius: 6px;
  padding: 6px 8px;
  color: #6b7280;
  font-size: 12px;
  cursor: pointer;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
  font-family: inherit;
}

.session-more__btn:hover {
  background: #f3f4f6;
  color: #1f2328;
  border-color: #d1d5db;
}
</style>
