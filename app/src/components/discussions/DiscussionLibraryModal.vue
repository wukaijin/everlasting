<script setup lang="ts">
// DiscussionLibraryModal — GCE M4b (09-07-gce-m4b-discussion-search)
// 「讨论库」独立面板:浏览 / 检索历史群聊审议(含定时审议)的场级
// 命中(一场一行),并跳回该场完整会话。
//
// Wire contract: `GroupChatSessionHit`(snake_case,来自
// `list_group_chat_sessions` / `search_group_chat_discussions`)。
// 空关键词 = 全量浏览(R2);关键词命中 title / summary / task_name /
// participants 任一即中(LIKE 在 db 层,此处不做客户端过滤)。打开命中
// 复用 `chatStore.openSessionInProject`(SearchModal 同款,project-aware)。
//
// 弹窗形态 / debounce / 结果态照 SearchModal(250ms debounce + IME
// composition gate + stale-response seq guard +「typed-but-not-yet-
// searched」提示)。差异:本面板常驻「空关键词也列出全部场次」模式,分组
// 用会话列表的 今天/昨天/本周/更早 buckets。

import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogClose,
} from "reka-ui";
import { computed, nextTick, ref, watch } from "vue";
import { transport } from "../../transport";
import { useChatStore } from "../../stores/chat";
import type { GroupChatSessionHit } from "../../stores/chat.types";
import { useDiscussionLibrary } from "../../composables/useDiscussionLibrary";
import { useProjectsStore } from "../../stores/projects";
import {
  BUCKET_LABELS,
  BUCKET_ORDER,
  bucketKey,
  type BucketKey,
} from "../../utils/sessionGrouping";
import { scheduledStopReasonLabel } from "../../stores/streamController";
import { hitTimeLabel } from "../../utils/searchHits";
import Icon from "../Icon.vue";

const QUERY_DEBOUNCE_MS = 250;

const chatStore = useChatStore();
const projectsStore = useProjectsStore();
const { discussionLibraryOpen, close } = useDiscussionLibrary();

// --- browse/search state -----------------------------------------------
const query = ref("");
const hits = ref<GroupChatSessionHit[]>([]);
const loading = ref(false);
const loadError = ref<string | null>(null);
const projectFilter = ref<string | null>(null);
const stopReasonFilter = ref<string | null>(null);
// IME composition gate — mid-composition keystrokes must not trigger
// searches (same contract as SearchModal / TriggerMenu).
const isComposing = ref(false);
let debounceTimer: ReturnType<typeof setTimeout> | null = null;
let searchSeq = 0;

/** The keyword the LAST COMPLETED run executed (echoed in the
 *  result-status + empty line — "did it search?" stays answerable). */
const searchedQuery = ref("");
const trimmedQuery = computed(() => query.value.trim());
const hasQuery = computed(() => trimmedQuery.value.length > 0);
const staleQuery = computed(
  () => hasQuery.value && searchedQuery.value !== trimmedQuery.value,
);

/** Filter-chip option sources. Each dimension is refreshed ONLY from
 *  a run where that dimension was unset (SearchModal's 08-17 hotfix
 *  #2 lesson) — a project-filtered run must not re-derive the project
 *  chips from its own (already single-project) hits and strand the
 *  active chip. */
const availableProjects = ref<{ id: string; label: string }[]>([]);
const availableStatuses = ref<{ code: string; label: string }[]>([]);

function projectLabel(id: string): string {
  return projectsStore.projectById(id)?.name ?? id;
}

function refreshOptionsFrom(result: GroupChatSessionHit[]): void {
  if (projectFilter.value === null) {
    const seen = new Map<string, string>();
    for (const h of result) {
      if (!seen.has(h.project_id)) seen.set(h.project_id, projectLabel(h.project_id));
    }
    availableProjects.value = [...seen.entries()].map(([id, label]) => ({
      id,
      label,
    }));
  }
  if (stopReasonFilter.value === null) {
    const seen = new Map<string, string>();
    for (const h of result) {
      const code = h.stop_reason;
      if (!code) continue;
      if (!seen.has(code)) seen.set(code, scheduledStopReasonLabel(code));
    }
    // Stable chip order (by code) so the row doesn't reorder between runs.
    availableStatuses.value = [...seen.entries()]
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([code, label]) => ({ code, label }));
  }
}

async function runLibrary(): Promise<void> {
  const q = trimmedQuery.value;
  const seq = ++searchSeq;
  loading.value = true;
  loadError.value = null;
  try {
    const filters = {
      projectId: projectFilter.value,
      stopReason: stopReasonFilter.value,
    };
    const result = q
      ? await transport.invoke<GroupChatSessionHit[]>(
          "search_group_chat_discussions",
          { query: q, ...filters },
        )
      : await transport.invoke<GroupChatSessionHit[]>("list_group_chat_sessions", filters);
    if (seq !== searchSeq) return; // stale response — superseded
    hits.value = result;
    searchedQuery.value = q;
    refreshOptionsFrom(result);
  } catch (e) {
    if (seq !== searchSeq) return;
    loadError.value = e instanceof Error ? e.message : String(e);
    hits.value = [];
    searchedQuery.value = q;
  } finally {
    if (seq === searchSeq) loading.value = false;
  }
}

/** Enter = run NOW (skip the debounce tail). IME-composition Enter
 *  (candidate confirm) must not trigger. */
function onEnter(e: KeyboardEvent): void {
  if (e.isComposing) return;
  if (debounceTimer) clearTimeout(debounceTimer);
  void runLibrary();
}

watch(query, () => {
  if (isComposing.value) return;
  if (booting) return; // armed by the open watcher
  if (debounceTimer) clearTimeout(debounceTimer);
  debounceTimer = setTimeout(runLibrary, QUERY_DEBOUNCE_MS);
});

// Changing a filter re-runs server-side (cheap + keeps the hit pool
// consistent with the chips) — no debounce needed.
watch([projectFilter, stopReasonFilter], () => {
  if (booting) return;
  void runLibrary();
});

/** Boot guard for the open watcher (same shape as SearchModal's
 *  `bootingPrefill`): the open reset mutates query/filters, which
 *  queues the two watchers above — cleared on nextTick AFTER the
 *  queued flush, so it can never swallow a later user edit. */
let booting = false;

// Every open starts from a clean slate: full browse, no filters.
// `immediate` covers the open-before-mount path (tests / a future
// programmatic mount while already open) — real usage mounts closed
// at AppShell startup and the watcher fires on the later `open()`.
watch(
  discussionLibraryOpen,
  (open) => {
    if (!open) return;
    booting = true;
    query.value = "";
    hits.value = [];
    projectFilter.value = null;
    stopReasonFilter.value = null;
    availableProjects.value = [];
    availableStatuses.value = [];
    loadError.value = null;
    searchedQuery.value = "";
    void runLibrary();
    void nextTick(() => {
      booting = false;
    });
  },
  { immediate: true },
);

// --- presentation -------------------------------------------------------

/** Group the (backend already updated_at DESC) hits into the sidebar's
 *  今天/昨天/本周/更早 buckets. Empty buckets omitted (bucketKey /
 *  Map contract). */
const groupedHits = computed(() => {
  const out = new Map<BucketKey, GroupChatSessionHit[]>();
  for (const h of hits.value) {
    const key = bucketKey(h.updated_at, new Date());
    let arr = out.get(key);
    if (!arr) {
      arr = [];
      out.set(key, arr);
    }
    arr.push(h);
  }
  return out;
});

/** Render the first keyword occurrence in a title/summary with <mark>
 *  (mirrors SearchModal's `highlightParts` — no wire offsets). */
function highlightParts(text: string): Array<{ text: string; hit: boolean }> {
  const q = trimmedQuery.value.toLowerCase();
  const idx = q ? text.toLowerCase().indexOf(q) : -1;
  if (idx === -1 || q === "") return [{ text, hit: false }];
  const parts: Array<{ text: string; hit: boolean }> = [];
  if (idx > 0) parts.push({ text: text.slice(0, idx), hit: false });
  parts.push({ text: text.slice(idx, idx + q.length), hit: true });
  const rest = text.slice(idx + q.length);
  if (rest) parts.push({ text: rest, hit: false });
  return parts;
}

function dateLabel(iso: string): string {
  return hitTimeLabel(iso);
}

function statusLabel(code: string | null): string | null {
  return code ? scheduledStopReasonLabel(code) : null;
}

/** 「打开会话」——project-aware switch(SearchModal 同款,AC3)。 */
async function openSession(hit: GroupChatSessionHit): Promise<void> {
  close();
  await chatStore.openSessionInProject(hit.project_id, hit.session_id);
}

const bucketKeys = computed(() =>
  BUCKET_ORDER.filter((k) => (groupedHits.value.get(k)?.length ?? 0) > 0),
);
const hasResults = computed(() => hits.value.length > 0);
</script>

<template>
  <DialogRoot
    :open="discussionLibraryOpen"
    @update:open="(v: boolean) => { if (!v) close(); }"
  >
    <DialogPortal>
      <DialogOverlay class="discussion-lib__overlay" />
      <DialogContent
        class="discussion-lib"
        :aria-describedby="undefined"
        @pointerdown-outside="close"
      >
        <DialogTitle class="discussion-lib__sr-title">讨论库</DialogTitle>

        <header class="discussion-lib__bar">
          <Icon name="circle-stack" :size="16" />
          <input
            v-model="query"
            class="discussion-lib__input"
            type="text"
            placeholder="搜索历史审议的标题、结论、任务名或参与人,回车立即搜索"
            autocomplete="off"
            spellcheck="false"
            autofocus
            @compositionstart="isComposing = true"
            @compositionend="isComposing = false"
            @keydown.enter="onEnter"
          />
          <span v-if="loading" class="app-spinner discussion-lib__spinner" aria-label="加载中" />
          <DialogClose as-child>
            <button
              type="button"
              class="discussion-lib__close btn btn--ghost btn--icon"
              aria-label="关闭"
              @click="close"
            >
              <Icon name="x" :size="14" />
            </button>
          </DialogClose>
        </header>

        <!-- 筛选 chips:项目 + 收官状态。两行各自只在对应维度未筛时
             从结果刷新(SearchModal 08-17 hotfix #2 的教训)。 -->
        <div v-if="availableProjects.length > 0" class="discussion-lib__filters">
          <span class="discussion-lib__filter-label">项目</span>
          <button
            type="button"
            class="discussion-lib__chip btn btn--outline btn--pill btn--sm"
            :class="{ 'discussion-lib__chip--active': projectFilter === null }"
            @click="projectFilter = null"
          >
            全部
          </button>
          <button
            v-for="p in availableProjects"
            :key="p.id"
            type="button"
            class="discussion-lib__chip btn btn--outline btn--pill btn--sm"
            :class="{ 'discussion-lib__chip--active': projectFilter === p.id }"
            @click="projectFilter = p.id"
          >
            {{ p.label }}
          </button>
        </div>
        <div v-if="availableStatuses.length > 0" class="discussion-lib__filters">
          <span class="discussion-lib__filter-label">状态</span>
          <button
            type="button"
            class="discussion-lib__chip btn btn--outline btn--pill btn--sm"
            :class="{ 'discussion-lib__chip--active': stopReasonFilter === null }"
            @click="stopReasonFilter = null"
          >
            全部
          </button>
          <button
            v-for="s in availableStatuses"
            :key="s.code"
            type="button"
            class="discussion-lib__chip btn btn--outline btn--pill btn--sm"
            :class="{ 'discussion-lib__chip--active': stopReasonFilter === s.code }"
            @click="stopReasonFilter = s.code"
          >
            {{ s.label }}
          </button>
        </div>

        <div class="discussion-lib__results">
          <div v-if="loadError" class="discussion-lib__state discussion-lib__state--error">
            加载失败:{{ loadError }}
          </div>
          <div v-else-if="loading && !hasResults" class="discussion-lib__status">
            正在{{ hasQuery ? '搜索' : '加载' }}…
          </div>
          <!-- Typed but not yet searched (debounce window / IME hold) →
               distinct from "no results". -->
          <div v-else-if="staleQuery && !hasResults" class="discussion-lib__status">
            回车立即搜索 "{{ trimmedQuery }}"
          </div>

          <template v-else-if="hasResults">
            <div class="discussion-lib__status">
              <template v-if="hasQuery">
                找到 {{ hits.length }} 场匹配<template v-if="staleQuery"> · 回车搜索
                  "{{ trimmedQuery }}"</template
                >
              </template>
              <template v-else>共 {{ hits.length }} 场历史审议</template>
            </div>

            <section v-for="key in bucketKeys" :key="key" class="discussion-lib__section">
              <h3 class="discussion-lib__section-title">{{ BUCKET_LABELS[key] }}</h3>
              <div class="discussion-lib__rows">
                <button
                  v-for="h in groupedHits.get(key)"
                  :key="h.session_id"
                  type="button"
                  class="discussion-lib__row no-focus-ring"
                  :title="`打开会话:${h.title}`"
                  @click="openSession(h)"
                >
                  <span class="discussion-lib__row-head">
                    <span class="discussion-lib__row-title">
                      <template v-for="(part, i) in highlightParts(h.title)" :key="i"
                        ><mark v-if="part.hit">{{ part.text }}</mark
                        ><template v-else>{{ part.text }}</template></template
                      >
                    </span>
                    <span v-if="h.task_name" class="discussion-lib__task-badge" title="定时审议">
                      {{ h.task_name }}
                    </span>
                  </span>
                  <span class="discussion-lib__row-meta">
                    <template v-if="h.participants.length > 0"
                      >{{ h.participants.join("、") }} ·</template
                    >
                    <template v-if="h.stop_reason"
                      ><span class="discussion-lib__stop-badge">{{ statusLabel(h.stop_reason) }}</span
                      > ·</template
                    >
                    {{ dateLabel(h.updated_at) }}
                  </span>
                  <span v-if="h.discussion_summary" class="discussion-lib__summary">
                    <template v-for="(part, i) in highlightParts(h.discussion_summary)" :key="i"
                      ><mark v-if="part.hit">{{ part.text }}</mark
                      ><template v-else>{{ part.text }}</template></template
                    >
                  </span>
                  <span v-else class="discussion-lib__summary discussion-lib__summary--empty">
                    尚未生成总结(讨论进行中或未以 end_discussion 收官)
                  </span>
                </button>
              </div>
            </section>
          </template>

          <!-- Search ran, zero hits — different from the never-searched
               browse-empty state. -->
          <div v-else-if="hasQuery" class="discussion-lib__state">
            没有找到与 "{{ searchedQuery }}" 匹配的历史审议
          </div>
          <!-- Empty library (browse mode, genuinely nothing yet). -->
          <div v-else class="discussion-lib__state">
            还没有历史审议——在右上角发起一场群聊,或去设置里建定时审议
          </div>
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
.discussion-lib__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: var(--z-modal-overlay);
}

.discussion-lib {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 80vw;
  min-width: 560px;
  max-width: min(720px, calc(100vw - 40px));
  height: min(640px, 80vh);
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--shadow-xl);
  z-index: var(--z-modal);
  outline: none;
  animation: discussion-lib-zoom var(--duration-modal-in) var(--ease-modal-in) both;
}

.discussion-lib[data-state="closed"] {
  animation: discussion-lib-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards;
}

@keyframes discussion-lib-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes discussion-lib-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
}

/* reka-ui requires a DialogTitle for a11y; visually hidden. */
.discussion-lib__sr-title {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
}

.discussion-lib__bar {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.discussion-lib__input {
  flex: 1;
  min-width: 0;
  background: transparent;
  border: none;
  outline: none;
  color: var(--color-text-primary);
  font-size: var(--text-base);
  font-family: inherit;
  padding: var(--space-1) 0;
}

.discussion-lib__input::placeholder {
  color: var(--color-text-muted);
}

.discussion-lib__close {
  flex-shrink: 0;
}

/* Filter chip rows. Each row: fixed-width label + wrapping chips. */
.discussion-lib__filters {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--color-bg-border);
  overflow-x: auto;
  flex-shrink: 0;
  flex-wrap: wrap;
}

.discussion-lib__filter-label {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.discussion-lib__chip {
  padding: 2px 10px;
  white-space: nowrap;
  flex-shrink: 0;
}

.discussion-lib__chip--active {
  background: color-mix(in srgb, var(--color-accent) 16%, transparent);
  border-color: var(--color-accent);
  color: var(--color-accent-text);
}

.discussion-lib__results {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: var(--space-3) var(--space-3) var(--space-4);
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}

.discussion-lib__status {
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  padding: 0 var(--space-1);
  flex-shrink: 0;
}

.discussion-lib__section {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

/* Section header — same treatment as SearchModal's L1 (uppercase
   xs + semibold + vertical bar). */
.discussion-lib__section-title {
  margin: 0;
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-xs);
  font-weight: var(--weight-semibold);
  color: var(--color-text-secondary);
  padding: var(--space-2) var(--space-2) var(--space-1);
  border-bottom: 1px solid var(--color-bg-border);
}

.discussion-lib__section-title::before {
  content: "";
  width: 3px;
  height: 12px;
  background: var(--color-text-primary);
  border-radius: 2px;
  flex-shrink: 0;
}

.discussion-lib__rows {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  padding: 0 var(--space-1);
}

/* One 场 = one row; the whole row opens the session (real button,
   CH12-1a lesson from SearchModal). */
.discussion-lib__row {
  display: flex;
  flex-direction: column;
  align-items: stretch;
  gap: var(--space-1);
  width: 100%;
  text-align: left;
  background: transparent;
  border: none;
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-family: inherit;
  padding: var(--space-2);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out);
}

.discussion-lib__row:hover,
.discussion-lib__row:focus-visible {
  background: var(--color-bg-elevated);
  outline: none;
}

.discussion-lib__row-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

.discussion-lib__row-title {
  flex: 1;
  min-width: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 定时场任务名徽章:accent 描边小徽,与标题同排最右。 */
.discussion-lib__task-badge {
  flex-shrink: 0;
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  color: var(--color-accent-text);
  background: color-mix(in srgb, var(--color-accent) 16%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-accent) 45%, transparent);
  border-radius: var(--radius-sm);
  padding: 1px 8px;
  white-space: nowrap;
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 收官状态徽章:中性描边,表达"这场怎么结束的"。 */
.discussion-lib__stop-badge {
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 0 6px;
}

.discussion-lib__row-meta {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  display: flex;
  align-items: center;
  gap: var(--space-1);
  flex-wrap: wrap;
}

/* summary 预览:正文 supporting line,2-3 行 clamp(与 SearchModal
   snippet 同族)。命中 mark 唯一彩色。 */
.discussion-lib__summary {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-relaxed);
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
  overflow-wrap: anywhere;
}

.discussion-lib__summary--empty {
  color: var(--color-text-muted);
  font-style: italic;
}

.discussion-lib__summary mark,
.discussion-lib__row-title mark {
  background: color-mix(in srgb, var(--color-accent) 60%, var(--color-bg-app));
  color: var(--color-text-on-accent);
  border-radius: 3px;
  padding: 0 2px;
  font-weight: var(--weight-medium);
  opacity: 1;
}

.discussion-lib__state {
  margin: var(--space-3) auto 0;
  max-width: 360px;
  padding: var(--space-3) var(--space-4);
  border-radius: var(--radius-md);
  background: var(--color-bg-elevated);
  color: var(--color-text-secondary);
  font-size: var(--text-sm);
  text-align: center;
  border: 1px solid var(--color-bg-border);
}

.discussion-lib__state--error {
  background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
  color: var(--color-tool-error);
  border-color: color-mix(in srgb, var(--color-tool-error) 30%, transparent);
}
</style>
