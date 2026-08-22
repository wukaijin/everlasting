<script setup lang="ts">
// SearchModal — D2 (08-17-cross-session-search) ① user-driven
// full-text search over ALL sessions/projects, mounted globally in
// AppShell. Opened via Cmd/Ctrl+K (repurposed from the sidebar
// title-filter focus; the sidebar filter itself is untouched) or
// the AppHeader search button (mobile-only entry).
//
// Two states in ONE dialog (avoids reka-ui portal nesting):
//   results — debounced query → `search_messages` IPC → title hits
//             (flat, first) + content hits grouped project→session.
//   preview — read-only `<SearchPreviewBody>` of the clicked hit's
//             session, scrolled to + highlighting the hit message.
//
// Wire contract: `MessageSearchHit` (snake_case, kind title|content);
// the frontend locates the query inside each snippet itself
// (lowercased indexOf) — no match offsets on the wire.

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
import type { MessageSearchHit } from "../../stores/chat.types";
import { useSearchModal } from "../../composables/useSearchModal";
import { hitTimeLabel, splitSnippetAt } from "../../utils/searchHits";
import Icon from "../Icon.vue";
import SearchPreviewBody from "./SearchPreviewBody.vue";

const QUERY_DEBOUNCE_MS = 250;
const RESULT_LIMIT = 50;

const chatStore = useChatStore();
const { searchModalOpen, close, consumePrefill } = useSearchModal();

// --- results state --------------------------------------------------------
const query = ref("");
const hits = ref<MessageSearchHit[]>([]);
const searching = ref(false);
const searchError = ref<string | null>(null);
const projectFilter = ref<string | null>(null);
// IME composition gate — mid-composition keystrokes must not
// trigger searches (same contract as the TriggerMenu triggers).
const isComposing = ref(false);
let debounceTimer: ReturnType<typeof setTimeout> | null = null;
let searchSeq = 0;

const titleHits = computed(() => hits.value.filter((h) => h.kind === "title"));
const contentHits = computed(() => hits.value.filter((h) => h.kind === "content"));

/** Projects available as filter chips. NOT derived from the current
 *  (possibly filtered) hit set — a filtered search returns only the
 *  selected project's hits, and deriving chips from that made the
 *  whole row vanish on the first chip click (08-17 hotfix #2) and
 *  stranded the filter on. Instead the chip source is refreshed
 *  only by UNFILTERED searches (which run on query change / open)
 *  and survives filtered re-searches. */
const availableProjects = ref<{ id: string; label: string }[]>([]);
function refreshAvailableProjects(unfilteredHits: MessageSearchHit[]): void {
  const seen = new Map<string, string>();
  for (const h of unfilteredHits) {
    if (!seen.has(h.project_id)) {
      seen.set(h.project_id, h.project_name ?? h.project_id);
    }
  }
  availableProjects.value = [...seen.entries()].map(([id, label]) => ({ id, label }));
}

/** Content hits grouped project → session (design §4.2). Within a
 *  session, first snippet + "还有 N 条" — the preview reveals the
 *  rest by scrolling to the clicked hit's seq. */
const contentGroups = computed(() => {
  const filtered = contentHits.value.filter(
    (h) => projectFilter.value === null || h.project_id === projectFilter.value,
  );
  const byProject = new Map<
    string,
    { projectId: string; projectLabel: string; sessions: Map<string, { hit: MessageSearchHit; extra: number }> }
  >();
  for (const h of filtered) {
    let pg = byProject.get(h.project_id);
    if (!pg) {
      pg = {
        projectId: h.project_id,
        projectLabel: h.project_name ?? h.project_id,
        sessions: new Map(),
      };
      byProject.set(h.project_id, pg);
    }
    const existing = pg.sessions.get(h.session_id);
    if (existing) {
      existing.extra += 1;
    } else {
      pg.sessions.set(h.session_id, { hit: h, extra: 0 });
    }
  }
  return [...byProject.values()].map((pg) => ({
    ...pg,
    sessions: [...pg.sessions.values()],
  }));
});

const visibleTitleHits = computed(() =>
  titleHits.value.filter(
    (h) => projectFilter.value === null || h.project_id === projectFilter.value,
  ),
);

const truncated = computed(() => hits.value.length >= RESULT_LIMIT);
const hasQuery = computed(() => query.value.trim().length > 0);
/** The query the LAST COMPLETED search ran with (echoed in the
 *  result-status line + empty state so "did it search?" is always
 *  answerable at a glance — the 08-17 user feedback: results felt
 *  indistinguishable from "nothing happened"). */
const searchedQuery = ref("");
const trimmedQuery = computed(() => query.value.trim());
/** True when the input holds a query the last completed search did
 *  NOT run — the debounce window, an IME hold, or post-search edits.
 *  Plugs the state-machine hole where this gap rendered as
 *  `没有找到与 "" 匹配` (08-17 hotfix #1: typed-but-not-yet-searched
 *  is a DIFFERENT state from searched-and-empty). */
const staleQuery = computed(
  () => hasQuery.value && searchedQuery.value !== trimmedQuery.value,
);

async function runSearch(): Promise<void> {
  const q = query.value.trim();
  if (!q) {
    hits.value = [];
    searchError.value = null;
    searchedQuery.value = "";
    return;
  }
  const seq = ++searchSeq;
  searching.value = true;
  searchError.value = null;
  try {
    const result = await transport.invoke<MessageSearchHit[]>("search_messages", {
      query: q,
      projectId: projectFilter.value,
      limit: RESULT_LIMIT,
    });
    if (seq !== searchSeq) return; // stale response — a newer query superseded it
    hits.value = result;
    searchedQuery.value = q;
    if (projectFilter.value === null) {
      refreshAvailableProjects(result);
    }
  } catch (e) {
    if (seq !== searchSeq) return;
    searchError.value = e instanceof Error ? e.message : String(e);
    hits.value = [];
    searchedQuery.value = q;
  } finally {
    if (seq === searchSeq) searching.value = false;
  }
}

/** Enter = search NOW (skip the debounce tail). IME-composition
 *  Enter (candidate confirm) must not trigger — `isComposing` is
 *  exactly that signal. */
function onEnter(e: KeyboardEvent): void {
  if (e.isComposing) return;
  if (debounceTimer) clearTimeout(debounceTimer);
  void runSearch();
}

watch(query, () => {
  if (isComposing.value) return;
  if (bootingPrefill) return; // armed by the open watcher, see below
  if (debounceTimer) clearTimeout(debounceTimer);
  debounceTimer = setTimeout(runSearch, QUERY_DEBOUNCE_MS);
});

// Changing the filter re-runs server-side (cheap + keeps the hit
// pool consistent with the chips) — no debounce needed.
watch(projectFilter, () => {
  if (bootingPrefill) return; // armed by the open watcher, see below
  void runSearch();
});

// Prefill boot guard: while the open watcher arms a prefill's
// query/projectFilter and fires its OWN immediate runSearch, the
// value mutations above queue these two watchers too — without the
// guard every prefill open would double-fire the IPC (immediate +
// debounced). The guard is cleared on nextTick, AFTER the queued
// pre-flush watchers have run, so it can never swallow a later
// user edit (a same-query reopen where the watchers DON'T fire
// still clears on nextTick — no leak, unlike a one-shot counter).
let bootingPrefill = false;

// Reset to a clean results state on every open (stale results from
// a previous open are noise, not context). With a pending prefill
// (D2②+ SearchHistoryCard CTA / programmatic opens) the query and
// project filter are pre-armed and the first search fires
// immediately — no debounce, no IME window on a programmatic open.
watch(searchModalOpen, (open) => {
  if (open) {
    const prefill = consumePrefill();
    bootingPrefill = !!prefill?.query;
    query.value = prefill?.query ?? "";
    hits.value = [];
    projectFilter.value = prefill?.projectId ?? null;
    availableProjects.value = [];
    searchError.value = null;
    searchedQuery.value = "";
    preview.value = null;
    if (prefill && prefill.query) {
      void runSearch();
      // Clear AFTER the queued query/projectFilter watchers have
      // been skipped in this flush (see `bootingPrefill` above).
      void nextTick(() => {
        bootingPrefill = false;
      });
    }
  }
});

// --- preview state --------------------------------------------------------
interface PreviewTarget {
  sessionId: string;
  sessionTitle: string;
  projectId: string;
  seq: number | null;
}
const preview = ref<PreviewTarget | null>(null);

function openPreview(hit: MessageSearchHit): void {
  preview.value = {
    sessionId: hit.session_id,
    sessionTitle: hit.session_title,
    projectId: hit.project_id,
    seq: hit.seq,
  };
}

/** "在主窗口打开" — project-aware switch (design §4.2 方案甲):
 *  `openSessionInProject` swaps the project first when needed so
 *  the sidebar list + lastSession bookkeeping stay correct. */
async function openInMainWindow(target: PreviewTarget): Promise<void> {
  close();
  await chatStore.openSessionInProject(target.projectId, target.sessionId);
}

/** Highlight the query inside a snippet. Returns [before, match,
 *  after] segments, case-insensitive first occurrence — the wire
 *  deliberately carries no offsets (design §2). */
// hit-rendering helpers extracted to utils/searchHits (shared with
// the D2②+ SearchHistoryCard); local wrappers keep the template's
// call sites stable.
function splitSnippet(snippet: string): [string, string | null, string] {
  return splitSnippetAt(snippet, query.value);
}

function timeLabel(iso: string): string {
  return hitTimeLabel(iso);
}
</script>

<template>
  <DialogRoot :open="searchModalOpen" @update:open="(v: boolean) => { if (!v) close(); }">
    <DialogPortal>
      <DialogOverlay class="search-modal__overlay" />
      <DialogContent
        class="search-modal"
        :aria-describedby="undefined"
        @pointerdown-outside="close"
      >
        <DialogTitle class="search-modal__sr-title">全局搜索</DialogTitle>

        <!-- ═══ state: results ═══ -->
        <template v-if="!preview">
          <header class="search-modal__bar">
            <Icon name="magnifying-glass" :size="16" />
            <input
              v-model="query"
              class="search-modal__input"
              type="text"
              placeholder="搜索所有会话的消息与标题,回车立即搜索"
              autocomplete="off"
              spellcheck="false"
              autofocus
              @compositionstart="isComposing = true"
              @compositionend="isComposing = false"
              @keydown.enter="onEnter"
            />
            <span v-if="searching" class="search-modal__spinner" aria-label="搜索中" />
            <DialogClose as-child>
              <button type="button" class="search-modal__close" aria-label="关闭" @click="close">
                <Icon name="x" :size="14" />
              </button>
            </DialogClose>
          </header>

          <div v-if="availableProjects.length > 1" class="search-modal__filters">
            <button
              type="button"
              class="search-modal__chip"
              :class="{ 'search-modal__chip--active': projectFilter === null }"
              @click="projectFilter = null"
            >
              全部
            </button>
            <button
              v-for="p in availableProjects"
              :key="p.id"
              type="button"
              class="search-modal__chip"
              :class="{ 'search-modal__chip--active': projectFilter === p.id }"
              @click="projectFilter = p.id"
            >
              {{ p.label }}
            </button>
          </div>

          <div class="search-modal__results">
            <div v-if="searchError" class="search-modal__state search-modal__state--error">
              搜索 "{{ searchedQuery }}" 失败:{{ searchError }}
            </div>
            <div v-else-if="!hasQuery" class="search-modal__state">
              输入关键词,跨项目检索全部会话
            </div>
            <div v-else-if="searching" class="search-modal__status">
              正在搜索 "{{ trimmedQuery }}"…
            </div>
            <!-- Typed but no completed search for it yet (debounce window /
                 IME hold / post-search edit) with nothing to show — NOT
                 "no results". -->
            <div v-else-if="staleQuery && hits.length === 0" class="search-modal__status">
              回车立即搜索 "{{ trimmedQuery }}"
            </div>
            <template v-else-if="hits.length > 0">
              <!-- Status line: makes "the search ran, here's how much it
                   found" explicit — without it the results quietly
                   replacing the placeholder is easy to miss. -->
              <div class="search-modal__status">
                找到 {{ hits.length }} 条命中(标题 {{ titleHits.length }} · 消息
                {{ contentHits.length }})<template v-if="staleQuery">
                  · 回车搜索 "{{ trimmedQuery }}"</template
                >
              </div>
<!-- title hits first (flat) -->
              <section v-if="visibleTitleHits.length > 0" class="search-modal__section">
                <h3 class="search-modal__section-title">会话标题</h3>
                <div class="search-modal__section-rows">
                  <button
                    v-for="h in visibleTitleHits"
                    :key="`t-${h.session_id}`"
                    type="button"
                    class="search-modal__row no-focus-ring"
                    @click="openInMainWindow({ sessionId: h.session_id, sessionTitle: h.session_title, projectId: h.project_id, seq: null })"
                  >
                    <span class="search-modal__row-title">{{ h.session_title }}</span>
                    <span class="search-modal__row-meta search-modal__row-meta--prefix-dot">
                      {{ h.project_name ?? h.project_id }} · {{ timeLabel(h.updated_at) }}
                    </span>
                  </button>
                </div>
              </section>

              <!-- content hits grouped project → session -->
              <section v-for="pg in contentGroups" :key="pg.projectId" class="search-modal__section">
                <h3 class="search-modal__section-title">{{ pg.projectLabel }}</h3>
                <div class="search-modal__section-rows">
                  <div v-for="s in pg.sessions" :key="s.hit.session_id" class="search-modal__session">
                    <div class="search-modal__session-head">
                      <span class="search-modal__row-title">{{ s.hit.session_title }}</span>
                      <span class="search-modal__row-meta">
                        {{ timeLabel(s.hit.updated_at) }}<template v-if="s.extra > 0"> · 还有 {{ s.extra }} 条</template>
                      </span>
                    </div>
                    <button type="button" class="search-modal__row search-modal__row--snippet" @click="openPreview(s.hit)">
                      <span class="search-modal__snippet">
                        <template v-if="s.hit.snippet">
                          <span v-if="splitSnippet(s.hit.snippet)[0]">{{ splitSnippet(s.hit.snippet)[0] }}</span><mark v-if="splitSnippet(s.hit.snippet)[1]">{{ splitSnippet(s.hit.snippet)[1] }}</mark><span>{{ splitSnippet(s.hit.snippet)[2] }}</span>
                        </template>
                      </span>
                    </button>
                  </div>
                </div>
              </section>

              <div v-if="truncated" class="search-modal__state search-modal__state--hint">
                仅显示前 {{ RESULT_LIMIT }} 条命中,试着用更具体的关键词
              </div>
            </template>
            <!-- Searched (query echoed) but zero hits — a DIFFERENT
                 message from the never-searched placeholder above. -->
            <div v-else class="search-modal__state">
              没有找到与 "{{ searchedQuery }}" 匹配的会话或消息
            </div>
          </div>
        </template>

        <!-- ═══ state: preview ═══ -->
        <template v-else>
          <header class="search-modal__bar search-modal__bar--preview">
            <button
              type="button"
              class="search-modal__back"
              aria-label="返回结果列表"
              @click="preview = null"
            >
              <Icon name="arrow-left" :size="16" />
            </button>
            <div class="search-modal__preview-title">
              <span class="search-modal__row-title">{{ preview.sessionTitle }}</span>
              <span class="search-modal__row-meta search-modal__row-meta--prefix-dot">只读预览</span>
            </div>
            <button
              type="button"
              class="search-modal__open-btn"
              @click="openInMainWindow(preview)"
            >
              在主窗口打开
            </button>
            <DialogClose as-child>
              <button type="button" class="search-modal__close" aria-label="关闭" @click="close">
                <Icon name="x" :size="14" />
              </button>
            </DialogClose>
          </header>
          <SearchPreviewBody :session-id="preview.sessionId" :target-seq="preview.seq" />
        </template>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
.search-modal__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: 2000;
}

.search-modal {
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
  z-index: 2001;
  /* 容器接 programmatic focus,整框上环无意义;内部控件由全局 :focus-visible 基线负责(style.css) */
  outline: none;
  animation: search-modal-zoom var(--duration-modal-in) var(--ease-modal-in) both;
}

.search-modal[data-state="closed"] {
  animation: search-modal-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards;
}

@keyframes search-modal-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes search-modal-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
}

/* reka-ui requires a DialogTitle for a11y; visually hidden. */
.search-modal__sr-title {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
}

.search-modal__bar {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.search-modal__input {
  flex: 1;
  min-width: 0;
  background: transparent;
  border: none;
  /* 焦点指示=输入光标 caret(裸搜索框,容器已提供上下文) */
  outline: none;
  color: var(--color-text-primary);
  font-size: var(--text-base);
  font-family: inherit;
  padding: var(--space-1) 0;
}

.search-modal__input::placeholder {
  color: var(--color-text-muted);
}

.search-modal__spinner {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  border: 2px solid var(--color-bg-border);
  border-top-color: var(--color-accent);
  animation: search-modal-spin 0.8s linear infinite;
  flex-shrink: 0;
}

@keyframes search-modal-spin {
  to { transform: rotate(360deg); }
}

.search-modal__close,
.search-modal__back {
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: var(--radius-sm);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.search-modal__close:hover,
.search-modal__back:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.search-modal__filters {
  display: flex;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--color-bg-border);
  overflow-x: auto;
  flex-shrink: 0;
}

.search-modal__chip {
  background: transparent;
  border: 1px solid var(--color-bg-border);
  border-radius: 999px;
  color: var(--color-text-secondary);
  font-size: var(--text-xs);
  font-family: inherit;
  padding: 2px 10px;
  cursor: pointer;
  white-space: nowrap;
  flex-shrink: 0;
}

.search-modal__chip--active {
  background: color-mix(in srgb, var(--color-accent) 16%, transparent);
  border-color: var(--color-accent);
  color: var(--color-accent-text);
}

.search-modal__results {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: var(--space-3) var(--space-3) var(--space-4);
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}

/* Search-status line (正在搜索… / 找到 N 条命中) — small but
   textual, so the search lifecycle is perceivable without watching
   for the tiny input spinner. */
.search-modal__status {
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  padding: 0 var(--space-1);
  flex-shrink: 0;
}

/* ── Visual hierarchy (08-17 design pass) ─────────────────────────────
   Three-level stacking — the previous version mixed --text-muted and
   --text-secondary within the same row so the project / session /
   meta lines bled together.

   L1 section header (e.g. "precaution-frontend"):
       primary tone + semibold + uppercase + a vertical accent bar;
       separates "which project am I looking at" from the hits below.
   L2 session title (within a project):
       primary tone + medium weight, the click target.
   L3 meta (project · date / 还有 N 条):
       tertiary muted, smaller, single line.
   Snippet row:
       same L1 primary tone but a notch smaller; the mark accent is
       the only color in the row so the eye lands there. */
.search-modal__section {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

/* L1 — project / "会话标题" group header. The accent bar on the
   left is the only shape that survives at low luminance levels
   (color alone couldn't carry hierarchy within secondary/muted). */
.search-modal__section-title {
  margin: 0;
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-xs);
  font-weight: var(--weight-semibold);
  color: var(--color-text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.08em;
  padding: var(--space-2) var(--space-2) var(--space-1);
  border-bottom: 1px solid var(--color-bg-border);
}

/* The accent stroke sits at the very left of the section so it
   reads as a "this section" marker without competing with the chips
   row above. We don't reach for --color-accent here on purpose —
   accent is reserved for actionable / "you are here" semantics. */
.search-modal__section-title::before {
  content: "";
  width: 3px;
  height: 12px;
  background: var(--color-text-primary);
  border-radius: 2px;
  flex-shrink: 0;
}

/* Indent session / row content so it visually nests under the
   section bar; pairs with the border-bottom divider above. */
.search-modal__section-rows {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  padding: 0 var(--space-1);
}

.search-modal__session {
  display: flex;
  flex-direction: column;
  border-radius: var(--radius-sm);
}

/* Title row in each session: small but unambiguously primary so the
   snippet below reads as "supporting text". */
.search-modal__session-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-2) 0;
}

.search-modal__row {
  display: flex;
  flex-direction: column;
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

.search-modal__row:hover,
.search-modal__row:focus-visible {
  background: var(--color-bg-elevated);
  /* 自有焦点替代::focus-visible 底色高亮(见下);基线环经模板 no-focus-ring 关闭 */
  outline: none;
}

/* L2 — session title (title-only hits use this directly). Medium
   weight gives a 1-point distinction from L3 below. */
.search-modal__row-title {
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* L3 — meta line. muted tone (small but distinct from the session
   title's primary); sits after a soft · separator so the eye parses
   "[title] · [meta]" as one field, not two unconnected strings. The
   ::before is opt-in via the modifier below — preview and session
   rows reuse the same class with multiple meta strings and need a
   free layout. */
.search-modal__row-meta {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.search-modal__row-meta--prefix-dot::before {
  content: "·";
  margin-right: 6px;
  color: var(--color-bg-border-strong);
}

.search-modal__row--snippet {
  padding-top: var(--space-1);
}

.search-modal__snippet {
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  line-height: var(--leading-relaxed);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  word-break: break-all;
  opacity: 0.82;
}

/* The highlight mark is the only color in the snippet — keeping
   it as the strongest visual anchor invites clicks. */
.search-modal__snippet mark {
  background: color-mix(in srgb, var(--color-accent) 28%, transparent);
  color: var(--color-text-primary);
  border-radius: 2px;
  padding: 1px 2px;
  font-weight: var(--weight-medium);
  opacity: 1;
}

/* State messages (placeholder / empty / error) — moved into the
   results column with a soft tinted background so the row looks
   like an explicit status, not "nothing rendered". */
.search-modal__state {
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

.search-modal__state--error {
  background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
  color: var(--color-tool-error);
  border-color: color-mix(in srgb, var(--color-tool-error) 30%, transparent);
}

.search-modal__state--hint {
  margin: 0;
  padding: var(--space-2);
  font-size: var(--text-xs);
  background: transparent;
  border: 0;
  text-align: left;
  color: var(--color-text-muted);
  max-width: none;
}

/* preview state */
.search-modal__bar--preview {
  gap: var(--space-2);
}

.search-modal__preview-title {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
}

.search-modal__preview-title .search-modal__row-title {
  flex: 1;
  min-width: 0;
}

.search-modal__open-btn {
  background: var(--color-accent);
  border: none;
  border-radius: var(--radius-sm);
  color: var(--color-text-on-accent);
  font-size: var(--text-xs);
  font-family: inherit;
  font-weight: var(--weight-medium);
  padding: var(--space-1) var(--space-3);
  cursor: pointer;
  white-space: nowrap;
  flex-shrink: 0;
}

.search-modal__open-btn:hover {
  filter: brightness(1.08);
}
</style>
