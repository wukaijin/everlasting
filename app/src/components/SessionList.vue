<script setup lang="ts">
// SessionList — session list shown in the left sidebar when a project
// is active. The header and "+ 新对话" button are owned by
// Sidebar.vue; this component is just the <ul> of session items.
//
// D6 restructure: each item is now a TWO-LINE card instead of the
// single-line row we had in D5. Line 1 is the session title (CJK +
// English mix), line 2 is muted secondary info — the project name +
// a relative timestamp joined with a · separator. This gives the
// user visual context for which project each session belongs to,
// which is especially useful when multiple projects are loaded.
//
// Per Q4v2 (PROPOSAL §5.2): default to the 8 most-recently-updated
// sessions; if there are more, render a "查看更早的 N 个" button at
// the bottom that toggles to show the full list. This is purely
// view-side folding — no schema change, no archive state.

import { computed, ref } from "vue";
import { useChatStore, type SessionSummary } from "../stores/chat";
import { useProjectsStore } from "../stores/projects";
import { useStreamControllerStore } from "../stores/streamController";
import Icon from "./Icon.vue";

const store = useChatStore();
const projectsStore = useProjectsStore();
// PR4 (06-07-6-ui-bug-markdown-sse): the per-session streaming
// indicator subscribes to the controller's reactive
// `streamingSessionIds` Set. Pinia auto-unwraps the computed, so
// reading `streamingSessionIds` on the store proxy yields the
// `Set<string>` itself — when a session's `request_id` enters or
// leaves `activeRequests` (via `startRequest` / `finalizeRequest`),
// the recomputed Set re-runs and the matching `v-if` flips. This
// means the indicator updates whether the user is currently
// looking at the streaming session or some other session in the
// same project (AC6.2 / AC6.3 / AC6.4 — per-session independence).
const streamController = useStreamControllerStore();

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

/** Look up a session's project name via the projects store. The
 *  projects store is the source of truth for project metadata; the
 *  session record itself only carries `project_id`. Falls back to
 *  "—" if the project is hidden, missing, or the id is unknown. */
function projectNameFor(s: SessionSummary): string {
  const p = projectsStore.projectById(s.project_id);
  return p?.name ?? "—";
}

function onClick(id: string) {
  void store.switchSession(id);
}

function onDelete(id: string, e: MouseEvent) {
  e.stopPropagation();
  // PR3: replaced the old global `sending` with
  // `isCurrentSessionStreaming` — per-session guard. Other
  // sessions in the same project can still be streaming
  // concurrently; the guard is specifically about "is THIS
  // session streaming right now?".
  if (store.isCurrentSessionStreaming && id === store.currentSessionId) return;
  if (!confirm("删除此 session 及其所有消息？")) return;
  void store.deleteSession(id);
}

/** Coarse relative-time formatter. Buckets by age to keep the label
 *  short and glanceable (line 2 of a two-line card). Anything
 *  ≥ 7 days falls back to a localized date. */
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
      <div class="session-item__main">
        <div class="session-item__title-row">
          <span class="session-item__title">{{ s.title }}</span>
          <!--
            PR4: per-session streaming indicator. A small pulsing dot
            next to the title, matching the project-tab pattern in
            `ProjectTabs.vue::.tab__streaming`. Uses
            `--color-accent` (Prussian blue) instead of the project
            tab's red so the two indicators are visually distinct
            when the user is looking at a sidebar while the
            project tab also shows a red dot (the project tab means
            "this project has any streaming session", the session
            card means "this specific session is streaming").

            The dot is purely visual — no click handler, no
            aria-label beyond `aria-hidden` (the title is already
            present in the DOM; the dot is a redundant signal for
            sighted users at a glance, not a new information
            channel that needs announcing).

            `streamingSessionIds` is a reactive Set on the
            controller; `Set#has` is a method call, which Vue
            tracks through the Set's iteration protocol in
            reactive() proxies. So adding/removing a session
            id from the Set (via `startRequest` / `finalizeRequest`)
            flips the `v-if` automatically.
          -->
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

/* PR4: per-session streaming indicator. Mirrors the
   `ProjectTabs.vue::.tab__streaming` pattern (a small dot with
   pulse animation) so the visual language is consistent between
   the top tab bar and the sidebar. Differences:
   - Color: `--color-accent` (Prussian blue) rather than the
     project tab's `--color-tool-error` (red). Two reasons:
     (1) visual separation when both indicators are visible at
         once (project tab = "project has any streaming session",
         session card = "this specific session is streaming");
     (2) the session card already has a permanent blue dot in
         `__dot` when the session is active, so blue reads as
         "this session, which is already special" rather than
         "something is wrong".
   - Animation: `pulseDot` (1.5s loop, opacity 0.4→1.0). Slightly
     longer than the project tab's 1.4s so the two animations
     don't visually sync up when both are on screen at once.
   - Sizing: 7px circle (between 6 and 8px per the dispatch
     instructions; 7 is asymmetric in a way that's not
     meaningful — but matches the project tab's 9px font glyph
     well at the card's 13px title size).
   - The dot is a CSS-only circle (no glyph), so its color and
     alpha are independent — a future theme swap can recolor
     without touching the markup. */
.session-item__streaming {
  flex-shrink: 0;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--color-accent);
  animation: pulseDot 1.5s ease-in-out infinite;
  /* Aligns the dot with the title's optical center. The title
     uses font-size 13px with default line-height, so a tiny
     top nudge lines up the indicator with the middle of the
     x-height rather than the baseline. */
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
