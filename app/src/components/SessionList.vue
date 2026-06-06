<script setup lang="ts">
// SessionList — session list shown in the left sidebar when a project
// is active. D5 restructure: the header and "+ 新对话" button are now
// owned by Sidebar.vue; this component is just the <ul> of session
// items. Each item is a single line: title (truncated) + status dot
// + relative timestamp + hover-revealed delete button. Matches the
// spike-003 reference (ui-A.png).
//
// Per Q4v2 (PROPOSAL §5.2): default to the 8 most-recently-updated
// sessions; if there are more, render a "查看更早的 N 个" button at
// the bottom that toggles to show the full list. This is purely
// view-side folding — no schema change, no archive state.

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

/** Coarse relative-time formatter. Buckets by age to keep the label
 *  short and glanceable (the right side of a single-line row).
 *  Anything ≥ 7 days falls back to a localized date. */
function formatTime(iso: string): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const diff = Date.now() - t;
  const min = 60 * 1000;
  const hr = 60 * min;
  const day = 24 * hr;
  if (diff < min) return "刚刚";
  if (diff < hr) return `${Math.floor(diff / min)} 分钟前`;
  if (diff < day) return `${Math.floor(diff / hr)} 小时前`;
  if (diff < 2 * day) return "昨天";
  if (diff < 7 * day) return `${Math.floor(diff / day)} 天前`;
  const d = new Date(t);
  const y = d.getFullYear();
  const mo = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${y}-${mo}-${dd}`;
}
</script>

<template>
  <ul class="session-list">
    <li
      v-for="s in visibleSessions"
      :key="s.id"
      :class="['session-item', { 'session-item--active': s.id === store.currentSessionId }]"
      @click="onClick(s.id)"
    >
      <span class="session-item__title">{{ s.title }}</span>
      <span class="session-item__dot" aria-hidden="true" />
      <span class="session-item__time">{{ formatTime(s.updated_at) }}</span>
      <button
        class="session-item__delete"
        title="删除"
        aria-label="删除会话"
        @click="(e) => onDelete(s.id, e)"
      >×</button>
    </li>
    <li v-if="store.sessions.length === 0" class="session-empty">
      还没有对话,点上方 + 开始
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
</template>

<style scoped>
.session-list {
  list-style: none;
  margin: 0;
  padding: 0 8px 8px;
  overflow-y: auto;
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.session-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  margin-bottom: 2px;
  border-radius: 6px;
  cursor: pointer;
  transition: background 0.1s;
  border-left: 2px solid transparent;
  min-width: 0;
}

.session-item:hover {
  background: var(--color-bg-elevated);
}

.session-item--active {
  background: var(--color-accent-muted);
  border-left-color: var(--color-accent);
}

.session-item--active:hover {
  background: var(--color-accent-muted);
}

.session-item__title {
  flex: 1;
  min-width: 0;
  font-size: 13px;
  font-weight: 500;
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-item__dot {
  flex-shrink: 0;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-tool-write);
}

.session-item--active .session-item__dot {
  background: var(--color-accent);
}

.session-item__time {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
}

.session-item__delete {
  flex-shrink: 0;
  width: 20px;
  height: 20px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--color-text-muted);
  font-size: 16px;
  line-height: 1;
  cursor: pointer;
  opacity: 0;
  transition: all 0.1s;
  padding: 0;
  font-family: inherit;
}

.session-item:hover .session-item__delete,
.session-item--active .session-item__delete {
  opacity: 1;
}

.session-item__delete:hover {
  background: var(--color-tool-error);
  color: #ffffff;
}

.session-empty {
  padding: 16px 12px;
  font-size: 12px;
  color: var(--color-text-muted);
  text-align: center;
}

.session-more {
  padding: 6px 12px;
  text-align: center;
}

.session-more__btn {
  width: 100%;
  background: transparent;
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  padding: 6px 8px;
  color: var(--color-text-secondary);
  font-size: 12px;
  cursor: pointer;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
  font-family: inherit;
}

.session-more__btn:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  border-color: var(--color-accent);
}
</style>
