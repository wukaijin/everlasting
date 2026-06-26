<script setup lang="ts">
// SessionList — session list shown in the left sidebar when a project
// is active. The header and "+ 新对话" button are owned by
// Sidebar.vue; this component is just the <ul> of session items.
//
// D1: added right-click context menu (reka-ui DropdownMenu) with
// rename / color tag / delete actions, plus double-click-to-rename
// inline editing on the title. Color tag renders as a 10% background
// tint on inactive cards.
//
// 2026-06-27 sidebar 搜索/密度/分组 (PR-of-PRs, 3 features):
//   - Search: title-substring filter via SessionSearchInput. When
//     `searchActive` is true, the list renders flat (groups
//     suppressed). Empty query returns to the grouped view.
//   - Density: comfortable (default) vs compact toggle. Reduces
//     session-item padding + font sizes + drops the project name.
//     State persisted in localStorage so the user's choice
//     survives a reload.
//   - Grouping: sessions split into 今天 / 昨天 / 本周 / 更早
//     buckets by updated_at. Today expanded by default; other
//     groups start collapsed. State persisted in localStorage.
//   - Cmd/Ctrl+K focuses the search input from anywhere via
//     `registerKeybinding` (capture-phase so it beats focus
//     traps inside the chat input / CodeMirror).
//
// The three features compose: search overrides grouping (flat
// filtered list), density applies to both grouped and flat modes,
// and grouping still shows a "查看更早的 N 个" disclosure at the
// bottom of each group's expanded slice when the group has more
// than 8 items (unchanged from pre-PR behavior, just scoped per
// group now).

import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useChatStore } from "../stores/chat";
import type { SessionSummary } from "../stores/chat.types";
import { useProjectsStore } from "../stores/projects";
import { useStreamControllerStore } from "../stores/streamController";
import { usePermissionsStore } from "../stores/permissions";
import { COLOR_PALETTE, colorTagHex, hexToRgba } from "../utils/colorTag";
import {
  BUCKET_LABELS,
  BUCKET_ORDER,
  filterByQuery,
  groupSessions,
  type BucketKey,
} from "../utils/sessionGrouping";
import { registerKeybinding } from "../utils/useKeyboard";
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
import ConfirmDialog from "./common/ConfirmDialog.vue";
import SessionGroupHeader from "./SessionGroupHeader.vue";
import SessionSearchInput from "./SessionSearchInput.vue";

const store = useChatStore();
const projectsStore = useProjectsStore();
const streamController = useStreamControllerStore();
const permStore = usePermissionsStore();

// --- 2026-06-27 search/density/grouping state ---
//
// `searchActive` is the v-model toggle — true when the search
// input row is mounted. `searchQuery` is the v-model of the input
// itself. They are independent: the parent (Sidebar) flips
// `searchActive` on/off when the user clicks the 🔍 icon; the user
// types into the input while active.
const props = withDefaults(
  defineProps<{
    searchActive?: boolean;
  }>(),
  { searchActive: false },
);

const emit = defineEmits<{
  (e: "search-clear"): void;
  (e: "search-input-ref", el: HTMLInputElement | null): void;
}>();

const searchQuery = ref<string>("");

/** Density variant. localStorage-persisted so the user's choice
 *  survives a reload. Default "comfortable" matches the pre-PR
 *  layout (no regression for existing users). */
type Density = "comfortable" | "compact";
const DENSITY_LS_KEY = "everlasting:sessionDensity";
const density = ref<Density>(
  (localStorage.getItem(DENSITY_LS_KEY) as Density) || "comfortable",
);
watch(density, (v) => {
  try {
    localStorage.setItem(DENSITY_LS_KEY, v);
  } catch {
    // localStorage may be unavailable (private mode, quota
    // exceeded). Silently swallow — the in-memory value still
    // works for this session.
  }
});

/** Per-group collapsed state. Default: 今天 expanded, others
 *  collapsed. Persisted in localStorage so the user's manual
 *  expand/collapse choices survive a reload. Stored as a Set for
 *  O(1) lookup in the template. */
const GROUP_COLLAPSED_LS_KEY = "everlasting:sessionGroupsCollapsed";
function loadCollapsedGroups(): Set<BucketKey> {
  try {
    const raw = localStorage.getItem(GROUP_COLLAPSED_LS_KEY);
    if (!raw) return new Set(["yesterday", "thisWeek", "older"]);
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      return new Set(parsed.filter((k): k is BucketKey =>
        BUCKET_ORDER.includes(k as BucketKey),
      ));
    }
  } catch {
    // fall through to default
  }
  return new Set(["yesterday", "thisWeek", "older"]);
}
const collapsedGroups = ref<Set<BucketKey>>(loadCollapsedGroups());

watch(collapsedGroups, (s) => {
  try {
    localStorage.setItem(
      GROUP_COLLAPSED_LS_KEY,
      JSON.stringify(Array.from(s)),
    );
  } catch {
    // same swallow as density
  }
}, { deep: true });

function toggleGroup(key: BucketKey) {
  const next = new Set(collapsedGroups.value);
  if (next.has(key)) {
    next.delete(key);
  } else {
    next.add(key);
  }
  collapsedGroups.value = next;
}

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

// --- F3: delete confirmation state ---
const confirmOpen = ref(false);
const pendingDeleteId = ref<string | null>(null);

/** Current "now" for bucket classification. Reactive `ref(Date)`
 *  so the buckets re-evaluate when the user keeps the app open
 *  across midnight (a session written yesterday becomes "今天"
 *  the next day). Updated on mount + on a 60s interval. */
const now = ref<Date>(new Date());
let nowTimer: ReturnType<typeof setInterval> | null = null;
onMounted(() => {
  nowTimer = setInterval(() => {
    now.value = new Date();
  }, 60_000);
});
onUnmounted(() => {
  if (nowTimer) clearInterval(nowTimer);
});

/** Search-filtered session list. When `searchQuery` is empty
 *  (whitespace-stripped) we return the input array so the sidebar
 *  stays in grouped mode; non-empty query returns the filtered
 *  subset for the flat-render branch. */
const searchedSessions = computed<SessionSummary[]>(() =>
  filterByQuery(store.sessions, searchQuery.value),
);

/** Grouped view (search empty). Each bucket's array preserves the
 *  input order from `store.sessions` (which the chat store keeps
 *  sorted by updated_at desc). */
const groupedSessions = computed<Map<BucketKey, SessionSummary[]>>(() =>
  groupSessions(searchedSessions.value, now.value),
);

/** Per-group sliced view for the "see more" disclosure. We slice
 *  each bucket to 8 items + track overflow count per bucket so
 *  the disclosure button shows the correct "see N more" label. */
const slicedGroups = computed<
  Map<BucketKey, { visible: SessionSummary[]; hidden: number }>
>(() => {
  const out = new Map<BucketKey, { visible: SessionSummary[]; hidden: number }>();
  for (const [key, items] of groupedSessions.value) {
    if (items.length <= DEFAULT_VISIBLE || expanded.value) {
      out.set(key, { visible: items, hidden: 0 });
    } else {
      out.set(key, {
        visible: items.slice(0, DEFAULT_VISIBLE),
        hidden: items.length - DEFAULT_VISIBLE,
      });
    }
  }
  return out;
});

/** `true` when search is active AND has a non-whitespace query.
 *  In that mode we render a flat list (no group headers). */
const flatFilterMode = computed<boolean>(
  () => searchQuery.value.trim().length > 0,
);

/** Total sessions matching the current filter (used by the
 *  "0 matches" empty state). */
const matchCount = computed<number>(() => searchedSessions.value.length);

/** Total hidden count across all groups in grouped mode — used
 *  by the bottom "收起" disclosure to know whether the expand
 *  is doing anything across the whole sidebar. */
const totalHidden = computed<number>(() => {
  let n = 0;
  for (const slice of slicedGroups.value.values()) {
    n += slice.hidden;
  }
  return n;
});

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
  requestDelete(id);
}

function requestDelete(id: string) {
  const s = store.sessions.find((x) => x.id === id);
  // Empty session (no preview) → delete directly
  if (!s || !s.preview) {
    void store.deleteSession(id);
    return;
  }
  pendingDeleteId.value = id;
  confirmOpen.value = true;
}

function onConfirmDelete() {
  const id = pendingDeleteId.value;
  confirmOpen.value = false;
  pendingDeleteId.value = null;
  if (id) void store.deleteSession(id);
}

function onCancelDelete() {
  confirmOpen.value = false;
  pendingDeleteId.value = null;
}

// --- D1: color helpers ---

function cardStyle(s: SessionSummary): Record<string, string> {
  const isActive = s.id === store.currentSessionId;
  if (isActive || s.color_tag === null) return {};
  const hex = colorTagHex(s.color_tag);
  if (!hex) return {};
  return { backgroundColor: hexToRgba(hex, 0.1), borderLeftColor: hex };
}

function projectNameFor(s: SessionSummary): string {
  const p = projectsStore.projectById(s.project_id);
  return p?.name ?? "无标题";
}

function onClick(id: string) {
  if (editingId.value === id) return;
  void store.switchSession(id);
}

function onDelete(id: string, e: MouseEvent) {
  e.stopPropagation();
  if (store.isCurrentSessionStreaming && id === store.currentSessionId) return;
  requestDelete(id);
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

/** Clear the search query and tell the parent to close the
 *  search input row. Wired to SessionSearchInput's `clear`
 *  event AND its Esc-when-empty path. */
function clearSearch() {
  searchQuery.value = "";
  emit("search-clear");
}

/** Global Cmd/Ctrl+K → focus the search input. Capture-phase
 *  registration so the key is caught before focus is anywhere
 *  else in the app (CodeMirror inside ChatInput would
 *  otherwise swallow the keystroke). Disabled while the input
 *  is already focused (no-op cycle). */
const searchInputRef = ref<InstanceType<typeof SessionSearchInput> | null>(null);
registerKeybinding({
  key: "k",
  ctrlOrMeta: true,
  enabled: () => props.searchActive && document.activeElement?.tagName !== "INPUT",
  handler: () => {
    searchInputRef.value?.focus();
  },
});
watch(() => props.searchActive, (active) => {
  if (active) {
    // After the input mounts, focus it (the child also focuses
    // on its own onMounted; this is a belt-and-braces guard
    // against the parent flipping `searchActive` from outside
    // Vue's mount cycle, e.g. via the global Cmd/Ctrl+K handler).
    nextTick(() => searchInputRef.value?.focus());
  }
});
</script>

<template>
  <SessionSearchInput
    v-if="searchActive"
    ref="searchInputRef"
    v-model="searchQuery"
    @clear="clearSearch"
  />

  <!-- Flat filter mode: query is non-empty. Skip group headers
       and render matches as a single list. -->
  <ul
    v-if="flatFilterMode"
    :class="['session-list', `session-list--${density}`]"
  >
    <li
      v-for="s in searchedSessions"
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
          <span
            v-if="permStore.hasPending(s.id)"
            class="session-item__pending-approval"
            title="有待审批的工具调用，切到此会话处理"
          >
            <Icon name="shield-check" :size="12" />
          </span>
        </div>
        <div class="session-item__meta">
          <span v-if="density === 'comfortable'" class="session-item__project">{{ projectNameFor(s) }}</span>
          <span v-if="density === 'comfortable' && formatTime(s.updated_at)" class="session-item__sep">·</span>
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
    <li v-if="matchCount === 0" class="session-empty">
      <Icon name="thinking" :size="20" />
      <span class="session-empty__title">没有匹配的会话</span>
      <span class="session-empty__hint">试试别的关键字</span>
    </li>
  </ul>

  <!-- Grouped mode: search empty. Render 4 buckets (today /
       yesterday / thisWeek / older) with collapsible headers.
       Buckets with 0 sessions are skipped (groupSessions omits
       empty entries from the Map). -->
  <ul
    v-else
    :class="['session-list', `session-list--${density}`]"
  >
    <template v-for="key in BUCKET_ORDER" :key="key">
      <template v-if="slicedGroups.get(key)">
        <SessionGroupHeader
          :label="BUCKET_LABELS[key]"
          :count="slicedGroups.get(key)!.visible.length"
          :collapsed="collapsedGroups.has(key)"
          @toggle="toggleGroup(key)"
        />
        <template v-if="!collapsedGroups.has(key)">
          <li
            v-for="s in slicedGroups.get(key)!.visible"
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
                <span
                  v-if="permStore.hasPending(s.id)"
                  class="session-item__pending-approval"
                  title="有待审批的工具调用，切到此会话处理"
                >
                  <Icon name="shield-check" :size="12" />
                </span>
              </div>
              <div class="session-item__meta">
                <span v-if="density === 'comfortable'" class="session-item__project">{{ projectNameFor(s) }}</span>
                <span v-if="density === 'comfortable' && formatTime(s.updated_at)" class="session-item__sep">·</span>
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
          <li
            v-if="slicedGroups.get(key)!.hidden > 0"
            class="session-more"
          >
            <button class="session-more__btn" @click="expanded = true">
              查看更早的 {{ slicedGroups.get(key)!.hidden }} 个
            </button>
          </li>
        </template>
      </template>
    </template>

    <li v-if="store.sessions.length === 0" class="session-empty">
      <Icon name="thinking" :size="20" />
      <span class="session-empty__title">还没有对话</span>
      <span class="session-empty__hint">点上方 + 开始</span>
    </li>
    <li v-else-if="expanded && totalHidden > 0" class="session-more">
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

  <!-- F3: delete session confirmation -->
  <ConfirmDialog
    :open="confirmOpen"
    title="确认删除 session?"
    confirm-text="确认删除"
    @confirm="onConfirmDelete"
    @cancel="onCancelDelete"
  >
    <p>此 session 及其所有消息将被永久删除，无法撤销。</p>
  </ConfirmDialog>
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

/* PR-3b (2026-06-27): selected-state visual hierarchy.
   Four states with clear delta:
     default   → transparent bg, 2px transparent border-left
     hover     → 6% primary wash + 2px transparent border-left
     selected  → 12% accent wash + 2px accent border-left
     selected:hover → 16% accent wash (tactile feedback)
   The wash concentration (6% → 12% → 16%) gives the user a clean
   read of "this row is interactive" → "this row is the current
   session" → "this row is being pressed". */
.session-item {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 8px 10px;
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: background-color var(--duration-fast) var(--ease-out),
              border-left-color var(--duration-fast) var(--ease-out);
  border-left: 2px solid transparent;
  min-width: 0;
}

/* 2026-06-27 sidebar 密度切换 (PR-of-PRs, 3 features): compact
   density tightens padding + drops the project name + reduces
   font sizes so ~2x more sessions fit in the same vertical space.
   Triggered by `.session-list--compact` on the parent <ul>; the
   item itself doesn't need a modifier class. Comfortable (the
   default) inherits the base `.session-item` styles above. */
.session-list--compact .session-item {
  padding: 4px 8px;
  gap: 6px;
}

.session-list--compact .session-item__title {
  font-size: var(--text-sm);
}

.session-list--compact .session-item__meta {
  font-size: var(--text-2xs);
}

.session-list--compact .session-item__dot {
  width: 6px;
  height: 6px;
  margin-top: 5px;
}

.session-list--compact .session-item__streaming {
  width: 5px;
  height: 5px;
}

.session-item:hover {
  background: var(--color-bg-hover);
}

.session-item--active {
  background: var(--color-bg-selected);
  border-left-color: var(--color-accent);
}

.session-item--active:hover {
  background: color-mix(in srgb, var(--color-accent) 16%, transparent);
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
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
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
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
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
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  min-width: 0;
  overflow: hidden;
}

.session-item__project {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: var(--weight-medium);
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

/* 2026-06-16: marks sessions with a pending permission ask so the
   user sees which session is blocked even after switching away
   (the inline approval card only renders for the current session). */
.session-item__pending-approval {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  color: var(--color-tool-shell);
  animation: pulseDot 1.5s ease-in-out infinite;
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
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  opacity: 0;
  transition: all var(--duration-fast) var(--ease-out);
  padding: 0;
  font-family: inherit;
}

.session-item:hover .session-item__delete,
.session-item--active .session-item__delete {
  opacity: 1;
}

.session-item__delete:hover {
  background: var(--color-tool-error);
  color: var(--color-text-on-accent);
}

.session-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-1);
  padding: var(--space-5) var(--space-3);
  text-align: center;
  color: var(--color-text-muted);
  list-style: none;
}

.session-empty > .icon {
  color: var(--color-accent);
  margin-bottom: var(--space-1);
}

.session-empty__title {
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
}

.session-empty__hint {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

.session-more {
  padding: 6px 12px;
  text-align: center;
}

.session-more__btn {
  width: 100%;
  background: transparent;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  padding: 6px 8px;
  color: var(--color-text-secondary);
  font-size: var(--text-sm);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out);
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
  border-radius: var(--radius-md);
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
  font-size: var(--text-base);
  color: var(--color-text-primary);
  border-radius: var(--radius-sm);
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
  transition: border-color var(--duration-fast) var(--ease-out), transform var(--duration-fast) var(--ease-out);
}

.palette-dot:hover {
  transform: scale(1.15);
}

.palette-dot--active {
  border-color: var(--color-text-primary);
}
</style>
