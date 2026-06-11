<script setup lang="ts">
// SessionList — session list shown in the left sidebar when a project
// is active. The header and "+ 新对话" button are owned by
// Sidebar.vue; this component is just the <ul> of session items.
//
// D1: added right-click context menu (reka-ui DropdownMenu) with
// rename / color tag / delete actions, plus double-click-to-rename
// inline editing on the title. Color tag renders as a 10% background
// tint on inactive cards.

import { computed, nextTick, ref } from "vue";
import { useChatStore, type SessionSummary } from "../stores/chat";
import { useProjectsStore } from "../stores/projects";
import { useStreamControllerStore } from "../stores/streamController";
import { COLOR_PALETTE, colorTagHex, hexToRgba } from "../utils/colorTag";
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
} from "reka-ui";
import Icon from "./Icon.vue";

const store = useChatStore();
const projectsStore = useProjectsStore();
const streamController = useStreamControllerStore();

const DEFAULT_VISIBLE = 8;
const expanded = ref(false);

// --- D1: right-click context menu state ---
const contextSessionId = ref<string | null>(null);
const contextMenuOpen = ref(false);
// Position for the context menu
const menuX = ref(0);
const menuY = ref(0);

// --- D1: inline rename state ---
const editingId = ref<string | null>(null);
const editingTitle = ref("");
const editInput = ref<HTMLInputElement | null>(null);

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

function projectNameFor(s: SessionSummary): string {
  const p = projectsStore.projectById(s.project_id);
  return p?.name ?? "—";
}

function onClick(id: string) {
  if (editingId.value === id) return;
  void store.switchSession(id);
}

function onDelete(id: string, e: MouseEvent) {
  e.stopPropagation();
  if (store.isCurrentSessionStreaming && id === store.currentSessionId) return;
  if (!confirm("删除此 session 及其所有消息？")) return;
  void store.deleteSession(id);
}

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

// --- D1: context menu handlers ---

function onContextMenu(e: MouseEvent, id: string) {
  e.preventDefault();
  if (editingId.value) return;
  contextSessionId.value = id;
  menuX.value = e.clientX;
  menuY.value = e.clientY;
  contextMenuOpen.value = true;
}

function contextRename() {
  const id = contextSessionId.value;
  if (!id) return;
  const s = store.sessions.find((x) => x.id === id);
  if (!s) return;
  startEditing(id, s.title);
}

async function startEditing(id: string, currentTitle: string) {
  editingId.value = id;
  editingTitle.value = currentTitle;
  contextMenuOpen.value = false;
  await nextTick();
  editInput.value?.focus();
  editInput.value?.select();
}

function commitEdit() {
  const id = editingId.value;
  if (!id) return;
  const trimmed = editingTitle.value.trim();
  if (trimmed) {
    void store.renameSession(id, trimmed);
  }
  editingId.value = null;
}

function cancelEdit() {
  editingId.value = null;
}

function contextSetColor(tag: number | null) {
  const id = contextSessionId.value;
  if (!id) return;
  void store.setSessionColor(id, tag);
}

function contextDelete() {
  const id = contextSessionId.value;
  if (!id) return;
  if (store.isCurrentSessionStreaming && id === store.currentSessionId) return;
  if (!confirm("删除此 session 及其所有消息？")) return;
  void store.deleteSession(id);
}

// --- D1: color helpers ---

function cardStyle(s: SessionSummary): Record<string, string> {
  const isActive = s.id === store.currentSessionId;
  if (isActive || s.color_tag === null) return {};
  const hex = colorTagHex(s.color_tag);
  if (!hex) return {};
  return { backgroundColor: hexToRgba(hex, 0.1), borderLeftColor: hex };
}
</script>

<template>
  <ul class="session-list">
    <li
      v-for="s in visibleSessions"
      :key="s.id"
      :class="['session-item', { 'session-item--active': s.id === store.currentSessionId }]"
      :style="cardStyle(s)"
      @click="onClick(s.id)"
      @dblclick="startEditing(s.id, s.title)"
      @contextmenu="onContextMenu($event, s.id)"
    >
      <div class="session-item__main">
        <div class="session-item__title-row">
          <input
            v-if="editingId === s.id"
            ref="editInput"
            v-model="editingTitle"
            class="session-item__edit-input"
            maxlength="80"
            @keydown.enter="commitEdit"
            @keydown.escape="cancelEdit"
            @blur="commitEdit"
            @click.stop
          />
          <span v-else class="session-item__title">{{ s.title }}</span>
          <span
            v-if="streamController.streamingSessionIds.has(s.id)"
            class="session-item__streaming"
            aria-hidden="true"
            title="正在生成"
          />
        </div>
        <div class="session-item__meta">
          <span class="session-item__project">{{ projectNameFor(s) }}</span>
          <span v-if="formatTime(s.updated_at)" class="session-item__sep">·</span>
          <span v-if="formatTime(s.updated_at)" class="session-item__time">
            {{ formatTime(s.updated_at) }}
          </span>
        </div>
      </div>
      <span class="session-item__dot" aria-hidden="true" />
      <button
        class="session-item__delete"
        title="删除"
        aria-label="删除会话"
        @click="(e) => onDelete(s.id, e)"
      >
        <Icon name="x" :size="12" />
      </button>
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

  <!-- D1: right-click context menu -->
  <DropdownMenuRoot v-model:open="contextMenuOpen">
    <DropdownMenuPortal>
      <DropdownMenuContent
        class="ctx-menu"
        :side-offset="4"
        :style="{ position: 'fixed', left: menuX + 'px', top: menuY + 'px' }"
        @clickoutside="contextMenuOpen = false"
      >
        <DropdownMenuItem class="ctx-menu__item" @click="contextRename">
          重命名
        </DropdownMenuItem>
        <DropdownMenuSub>
          <DropdownMenuSubTrigger class="ctx-menu__item ctx-menu__item--sub">
            标记颜色
          </DropdownMenuSubTrigger>
          <DropdownMenuPortal>
            <DropdownMenuSubContent class="ctx-menu ctx-menu--palette">
              <button
                v-for="(hex, idx) in COLOR_PALETTE"
                :key="idx"
                class="palette-dot"
                :class="{ 'palette-dot--active': store.sessions.find(s => s.id === contextSessionId)?.color_tag === idx }"
                :style="{ backgroundColor: hex }"
                :title="`颜色 ${idx + 1}`"
                @click="contextSetColor(idx)"
              />
              <div class="ctx-menu__separator" />
              <DropdownMenuItem class="ctx-menu__item" @click="contextSetColor(null)">
                取消标记
              </DropdownMenuItem>
            </DropdownMenuSubContent>
          </DropdownMenuPortal>
        </DropdownMenuSub>
        <DropdownMenuSeparator class="ctx-menu__separator" />
        <DropdownMenuItem class="ctx-menu__item ctx-menu__item--danger" @click="contextDelete">
          删除
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenuPortal>
  </DropdownMenuRoot>
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
  gap: 2px;
}

.session-item {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 8px 10px;
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

.session-item__main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.session-item__title-row {
  display: flex;
  align-items: center;
  min-width: 0;
}

.session-item__title {
  font-size: 13px;
  font-weight: 500;
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
  min-width: 0;
}

.session-item__edit-input {
  flex: 1;
  min-width: 0;
  font-size: 13px;
  font-weight: 500;
  color: var(--color-text-primary);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-accent);
  border-radius: 3px;
  padding: 1px 4px;
  outline: none;
  font-family: inherit;
}

.session-item__meta {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--color-text-muted);
  min-width: 0;
  overflow: hidden;
}

.session-item__project {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 500;
}

.session-item__sep {
  flex-shrink: 0;
  color: var(--color-text-muted);
}

.session-item__time {
  flex-shrink: 0;
  font-variant-numeric: tabular-nums;
}

.session-item__dot {
  flex-shrink: 0;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--color-tool-write);
  margin-top: 6px;
  order: -1;
}

.session-item--active .session-item__dot {
  background: var(--color-accent);
}

.session-item__streaming {
  flex-shrink: 0;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--color-accent);
  animation: pulseDot 1.5s ease-in-out infinite;
  margin-top: 1px;
}

@keyframes pulseDot {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}

.session-item__delete {
  flex-shrink: 0;
  width: 20px;
  height: 20px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
  justify-content: center;
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

<!-- D1: context menu styles must be non-scoped because reka-ui
     DropdownMenu renders via a portal (outside the component DOM
     tree), so scoped styles cannot reach it. -->
<style>
.ctx-menu {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  padding: 4px;
  min-width: 140px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2);
  z-index: 9999;
}

.ctx-menu--palette {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 8px;
  min-width: 160px;
}

.ctx-menu__item {
  display: flex;
  align-items: center;
  width: 100%;
  padding: 6px 10px;
  font-size: 13px;
  color: var(--color-text-primary);
  border-radius: 4px;
  cursor: pointer;
  border: none;
  background: transparent;
  font-family: inherit;
  text-align: left;
}

.ctx-menu__item:hover,
.ctx-menu__item[data-highlighted] {
  background: var(--color-accent-muted);
}

.ctx-menu__item--sub {
  justify-content: space-between;
}

.ctx-menu__item--danger:hover,
.ctx-menu__item--danger[data-highlighted] {
  background: rgba(220, 53, 69, 0.12);
  color: var(--color-tool-error);
}

.ctx-menu__separator {
  height: 1px;
  background: var(--color-bg-border);
  margin: 4px 0;
}

.palette-dot {
  width: 20px;
  height: 20px;
  border-radius: 50%;
  border: 2px solid transparent;
  cursor: pointer;
  transition: border-color 0.1s, transform 0.1s;
}

.palette-dot:hover {
  transform: scale(1.15);
}

.palette-dot--active {
  border-color: var(--color-text-primary);
}
</style>
