<script setup lang="ts">
// SubagentDrawer — B6 PR3 right-side drawer for worker subagent
// transcripts (R6).
//
// Implementation note (reka-ui version pin): the PRD specified
// reka-ui `Sheet`, but reka-ui@2.9.9 (this project's pinned version
// per `reka-ui-usage.md`) does NOT ship the `Sheet` primitive —
// only `Dialog` / `AlertDialog` / `Popover`. Rather than upgrade
// the version pin (out of scope), we compose the drawer out of the
// existing `Dialog*` primitives and theme the `DialogContent` as a
// right-side panel via CSS (fixed right + full height + slide-in
// transition). The result is functionally identical to a Sheet for
// our use case (right-anchored side panel, click-backdrop-to-close,
// Esc-to-close, focus trap, scroll lock). The reka-ui `:deep()`
// gotcha documented in `reka-ui-usage.md` still applies — the
// `DialogContent` portals to body, so our scoped CSS uses Vue 3.5's
// preserved `data-v-*` attribute selector (same approach
// `AuditLogModal.vue` + `MemoryModal.vue` take).
//
// UX summary (mirrors PRD R6 + AC5):
//   - open state bound to `store.openRunId !== null`
//   - header: status badge + subagentName + startedAt + finishedAt
//     (if any) + summary (if any)
//   - body: transcript list from
//     `store.liveTranscript.get(openRunId) ?? parse(getRunCache.transcriptJson) ?? []`
//   - per entry: kind badge (chat_event=gray / tool_call=blue /
//     tool_result=green / permission_ask=orange) + payload
//     `JSON.stringify(_, null, 2)` + timestamp
//   - "Show chat events" toggle: chat_event entries default hidden;
//     tool_call / tool_result / permission_ask always visible
//   - transcriptTruncated flag → "原 transcript 已截断 (head + tail)"
//   - empty state → "Worker is starting..."

import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DialogClose,
} from "reka-ui";
import Icon from "../Icon.vue";
import { extractToolResultDisplay } from "../../utils/messageFormat";
import {
  useSubagentRunsStore,
  coerceStatus,
  parseTranscriptJson,
  type TranscriptEntry,
  type TranscriptKind,
  type SubagentStatus,
} from "../../stores/subagentRuns";

const store = useSubagentRunsStore();

/** Drawer open state — reka-ui Dialog requires a writable ref. We
 *  bridge it to `store.openRunId` so the store is the single source
 *  of truth for "which run is open". When the dialog closes (Esc /
 *  backdrop / X), we propagate that into `closeDrawer()`. */
const open = computed<boolean>({
  get: () => store.openRunId !== null,
  set: (next: boolean) => {
    if (!next) store.closeDrawer();
  },
});

/** The cached full row for the currently-open run (header source).
 *  `undefined` while the run is opening but the `fetchRun` hasn't
 *  resolved yet (the header shows placeholders in that window). */
const run = computed(() => store.openRun);

/** Coerce the row's raw `status: string` into the typed union
 *  (Drift trap 1 — Row.status is a raw string, Summary.status is
 *  the typed enum; we unify here). Falls back to "running" if the
 *  row isn't loaded yet. */
const status = computed<SubagentStatus>(() =>
  run.value ? coerceStatus(run.value.status) : "running",
);

const STATUS_META: Record<
  SubagentStatus,
  { label: string; color: string }
> = {
  running: { label: "运行中", color: "var(--color-tool-shell)" },
  completed: { label: "完成", color: "var(--color-tool-write)" },
  cancelled: { label: "已停止", color: "var(--color-text-muted)" },
  error: { label: "出错", color: "var(--color-tool-error)" },
};

/** Transcript list for the drawer body. Priority per R6:
 *    1. `store.liveTranscript.get(openRunId)` (live stream)
 *    2. `parse(run.transcriptJson)` (DB cache from fetchRun)
 *    3. `[]`
 *  Both branches return `TranscriptEntry[]` (snake_case
 *  `payload_json`). */
const transcript = computed<TranscriptEntry[]>(() => {
  const rid = store.openRunId;
  if (!rid) return [];
  const live = store.liveTranscript.get(rid);
  if (live && live.length > 0) return live;
  const cached = run.value?.transcriptJson;
  return cached ? parseTranscriptJson(cached) : [];
});

/** Whether to render chat_event entries. Defaults to `false` per
 *  PRD decision #2 — the drawer hides the verbose delta stream and
 *  shows tool_call / tool_result / permission_ask by default. */
const showChatEvents = ref(false);

/** Visible transcript after applying the chat-event filter. */
const visibleTranscript = computed<TranscriptEntry[]>(() => {
  if (showChatEvents.value) return transcript.value;
  return transcript.value.filter((e) => e.kind !== "chat_event");
});

/** Whether the transcript was truncated at the 4 MiB cap on the
 *  backend. Drives the "原 transcript 已截断" banner. */
const truncated = computed<boolean>(() => {
  if (!run.value) return false;
  return (run.value.transcriptTruncated ?? 0) !== 0;
});

/** Empty state: worker just started, no transcript yet. */
const isEmpty = computed<boolean>(
  () => transcript.value.length === 0,
);

/** Kind badge metadata — color per PRD R6 (chat_event=gray /
 *  tool_call=blue / tool_result=green / permission_ask=orange).
 *  Reuses existing CSS color tokens so the palette stays unified. */
const KIND_META: Record<
  TranscriptKind,
  { label: string; color: string }
> = {
  chat_event: { label: "chat", color: "var(--color-text-muted)" },
  tool_call: { label: "call", color: "var(--color-accent)" },
  tool_result: { label: "result", color: "var(--color-tool-write)" },
  permission_ask: { label: "perm", color: "var(--color-tool-shell)" },
};

/** Format a transcript entry's `payload_json` (snake_case DB storage
 *  shape — Drift trap 2) as indented JSON.
 *
 *  B2 (2026-06-20): for `tool_result` entries, `payload_json.content`
 *  is the LLM-facing cwd envelope (REQ-16 — see
 *  `extractToolResultDisplay` in `utils/messageFormat.ts`), e.g.
 *  `'{"result":"...","cwd":"/data/wt"}'`. The previous blanket
 *  `JSON.stringify(payload_json, null, 2)` would re-stringify the
 *  envelope, producing `\"cwd\":\"...\"` escape noise and rendering
 *  the envelope JSON instead of the actual tool output. The same
 *  envelope is unwrapped by `ToolCallCard.vue` on the main panel
 *  (line 33-37); reusing the helper here keeps the two surfaces in
 *  sync. Non-`tool_result` kinds (tool_call / permission_ask /
 *  chat_event) keep the old JSON.stringify path — those don't carry
 *  the envelope shape. */
function formatPayload(entry: TranscriptEntry): string {
  if (entry.kind === "tool_result") {
    const raw = entry.payload_json?.content;
    if (typeof raw === "string") {
      return extractToolResultDisplay(raw);
    }
  }
  try {
    return JSON.stringify(entry.payload_json, null, 2);
  } catch {
    return String(entry.payload_json);
  }
}

// ---------------------------------------------------------------------------
// B6 PR3b (2026-06-20): live duration timer + auto-scroll polish
// ---------------------------------------------------------------------------

/** 100 ms cadence for the header duration counter. Smooth enough for
 *  the eye (10 Hz) without burning CPU. Cleared on drawer close +
 *  component unmount (see `watch(openRunId)` + `onUnmounted`). */
const TIMER_TICK_MS = 100;

/** Scroll distance (px) from the bottom that's still considered
 *  "at the bottom" for auto-follow purposes. Lets users scroll a
 *  few pixels up without immediately pausing the auto-follow. */
const SCROLL_BOTTOM_THRESHOLD_PX = 50;

const nowTick = ref(Date.now());
let tickerHandle: ReturnType<typeof setInterval> | null = null;

/** Milliseconds since the run's `startedAt`. Returns 0 if the row
 *  hasn't loaded yet (initial open window). */
const elapsedMs = computed<number>(() => {
  if (!run.value?.startedAt) return 0;
  const startedMs = new Date(run.value.startedAt).getTime();
  if (!Number.isFinite(startedMs)) return 0;
  return Math.max(0, nowTick.value - startedMs);
});

/** Status pill text — appends a duration suffix per state:
 *    - running → "running 8.2s" (live, updates every 100ms)
 *    - completed → "done in 12.4s" (terminal, computed once)
 *    - error → "failed at 4.2s" (terminal, wall-clock at error)
 *    - cancelled → "stopped at 3.1s" (terminal, wall-clock at cancel)
 *  Falls back to the plain label if the row hasn't loaded yet.
 *
 *  B1 (2026-06-20): the error / cancelled branches previously used
 *  `elapsedMs` (nowTick - startedMs) for the suffix, which kept
 *  ticking after the run finished. If the drawer stayed open after
 *  the worker failed (e.g. user reading the transcript), the badge
 *  drifted from "failed at 11.7s" (real wall-clock) to "failed at
 *  14281.9s" (4 hours). Fix: use `finishedAt - startedAt` for all
 *  terminal states (same formula as `completed`), giving a frozen
 *  duration that doesn't change while the drawer is open. The
 *  `terminalDurMs` helper holds the shared computation; the
 *  running branch stays on the live `elapsedMs`. */
const statusDisplay = computed<{ label: string; color: string; suffix: string }>(() => {
  const meta = STATUS_META[status.value];
  if (!run.value?.startedAt) {
    return { label: meta.label, color: meta.color, suffix: "" };
  }
  const startedMs = new Date(run.value.startedAt).getTime();
  const finishedAt = run.value.finishedAt;
  const finishedMs = finishedAt ? new Date(finishedAt).getTime() : null;
  const terminalDurMs =
    finishedMs !== null && Number.isFinite(finishedMs) && Number.isFinite(startedMs)
      ? Math.max(0, finishedMs - startedMs)
      : null;
  if (status.value === "running") {
    return { label: meta.label, color: meta.color, suffix: ` ${(elapsedMs.value / 1000).toFixed(1)}s` };
  }
  if (status.value === "completed" && terminalDurMs !== null) {
    return { label: meta.label, color: meta.color, suffix: ` ${(terminalDurMs / 1000).toFixed(1)}s` };
  }
  if (status.value === "error") {
    const suffix = terminalDurMs !== null ? ` at ${(terminalDurMs / 1000).toFixed(1)}s` : "";
    return { label: "failed", color: meta.color, suffix };
  }
  if (status.value === "cancelled") {
    const suffix = terminalDurMs !== null ? ` at ${(terminalDurMs / 1000).toFixed(1)}s` : "";
    return { label: meta.label, color: meta.color, suffix };
  }
  return { label: meta.label, color: meta.color, suffix: "" };
});

watch(
  () => store.openRunId,
  (rid) => {
    if (tickerHandle) {
      clearInterval(tickerHandle);
      tickerHandle = null;
    }
    if (rid) {
      nowTick.value = Date.now();
      tickerHandle = setInterval(() => {
        nowTick.value = Date.now();
      }, TIMER_TICK_MS);
    }
  },
  { immediate: true },
);

onUnmounted(() => {
  if (tickerHandle) {
    clearInterval(tickerHandle);
    tickerHandle = null;
  }
});

/** Ref to the scrollable body element. Used for auto-scroll. */
const bodyEl = ref<HTMLElement | null>(null);

/** Whether new transcript entries should auto-scroll into view.
 *  Pauses when the user scrolls up past the threshold. */
const autoFollow = ref<boolean>(true);

/** Count of new entries that arrived while the user was scrolled
 *  away from the bottom. Drives the floating "↓ N new" button. */
const newCount = ref<number>(0);

/** `scroll` event handler on the body. Updates `autoFollow` based
 *  on proximity to the bottom. The `< 50px` slack means small
 *  mouse-wheel ticks up don't immediately disable auto-follow. */
function onBodyScroll(e: Event): void {
  const el = e.target as HTMLElement;
  const atBottom =
    el.scrollHeight - el.scrollTop - el.clientHeight < SCROLL_BOTTOM_THRESHOLD_PX;
  autoFollow.value = atBottom;
  if (atBottom) {
    newCount.value = 0;
  }
}

/** Watch the rendered transcript count (NOT the full list — we
 *  hide chat_event entries by default; `visibleTranscript` is what
 *  the user sees). When a new entry arrives: if auto-follow is on,
 *  scroll to bottom; otherwise increment the newCount badge. */
watch(
  () => visibleTranscript.value.length,
  () => {
    if (autoFollow.value) {
      void nextTick(() => {
        if (bodyEl.value) {
          bodyEl.value.scrollTop = bodyEl.value.scrollHeight;
        }
      });
    } else {
      newCount.value += 1;
    }
  },
);

/** User clicked the "↗ jump to latest" header button OR the
 *  "↓ N new" floating button. Smooth-scroll to the bottom and
 *  re-enable auto-follow. The `scrollTo` feature check is a
 *  defensive guard for test environments (jsdom does NOT
 *  implement `Element.scrollTo`) — production browsers always
 *  have it. The `scrollTop` fallback has the same end-state,
 *  just without the smooth animation. */
function jumpToLatest(): void {
  if (!bodyEl.value) return;
  const target = bodyEl.value.scrollHeight;
  if (typeof bodyEl.value.scrollTo === "function") {
    bodyEl.value.scrollTo({ top: target, behavior: "smooth" });
  } else {
    bodyEl.value.scrollTop = target;
  }
  autoFollow.value = true;
  newCount.value = 0;
}
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="subagent-drawer__overlay" />
      <Transition name="subagent-drawer">
        <DialogContent
          v-if="open"
          class="subagent-drawer"
          aria-describedby="subagent-drawer-desc"
        >
          <DialogTitle class="subagent-drawer__sr-title">
            Worker subagent transcript
          </DialogTitle>
          <DialogDescription class="subagent-drawer__sr-desc">
            Live transcript of the worker subagent run.
          </DialogDescription>

          <!-- Header: status + name + timestamps + summary -->
          <header class="subagent-drawer__header">
            <div class="subagent-drawer__title-row">
              <span
                class="subagent-drawer__status"
                :style="{ color: statusDisplay.color, borderColor: statusDisplay.color }"
                :title="`Status: ${status}`"
              >{{ statusDisplay.label }}{{ statusDisplay.suffix }}</span>
              <span class="subagent-drawer__name">
                {{ run?.subagentName ?? "worker" }}
              </span>
              <!-- B6 PR3b (2026-06-20): jump-to-latest button.
                   Shown when auto-follow is paused (either because
                   the user scrolled up while running, OR because new
                   events arrived while they were scrolled away).
                   Placed in the header per Q5 decision D5. -->
              <button
                v-if="!autoFollow && visibleTranscript.length > 0"
                class="subagent-drawer__jump-latest"
                type="button"
                :title="newCount > 0 ? `跳到最新 (${newCount} 条新事件)` : '跳到最新'"
                aria-label="Jump to latest"
                @click="jumpToLatest"
              >
                <Icon name="arrow-down" :size="14" />
              </button>
              <DialogClose
                class="subagent-drawer__close"
                aria-label="Close"
              >
                <Icon name="x" :size="14" />
              </DialogClose>
            </div>
            <div
              v-if="run?.startedAt"
              class="subagent-drawer__meta"
            >
              <span>开始: {{ run.startedAt }}</span>
              <span v-if="run.finishedAt">结束: {{ run.finishedAt }}</span>
            </div>
            <p
              v-if="run?.summary"
              class="subagent-drawer__summary"
            >{{ run.summary }}</p>
          </header>

          <!-- Filter row: chat-event toggle + truncated notice -->
          <div class="subagent-drawer__filter-row">
            <label class="subagent-drawer__toggle">
              <input
                v-model="showChatEvents"
                type="checkbox"
              />
              <span>Show chat events</span>
            </label>
            <span
              v-if="truncated"
              class="subagent-drawer__truncated"
              title="原 transcript 超过 4 MiB,backend 已截断保留 head + tail"
            >
              原 transcript 已截断 (head + tail)
            </span>
          </div>

          <!-- Transcript list -->
          <div
            ref="bodyEl"
            class="subagent-drawer__body"
            @scroll="onBodyScroll"
          >
            <div v-if="isEmpty" class="subagent-drawer__empty">
              Worker is starting...
            </div>
            <ol v-else class="subagent-drawer__list">
              <li
                v-for="(entry, i) in visibleTranscript"
                :key="i"
                class="subagent-drawer__entry"
                :class="`subagent-drawer__entry--${entry.kind}`"
              >
                <span
                  class="subagent-drawer__kind"
                  :style="{ color: KIND_META[entry.kind].color, borderColor: KIND_META[entry.kind].color }"
                >{{ KIND_META[entry.kind].label }}</span>
                <pre class="subagent-drawer__payload">{{ formatPayload(entry) }}</pre>
              </li>
            </ol>
            <!-- B6 PR3b (2026-06-20): floating "↓ N new" button.
                 Appears at the body's bottom-center when auto-follow
                 is paused AND new entries arrived since the user
                 scrolled away. Clicking jumps to the latest entry +
                 resumes auto-follow. Hidden when autoFollow=true OR
                 the transcript is empty. -->
            <button
              v-if="!autoFollow && newCount > 0"
              class="subagent-drawer__new-events"
              type="button"
              @click="jumpToLatest"
            >↓ {{ newCount }} new</button>
          </div>
        </DialogContent>
      </Transition>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
/* The DialogContent / DialogOverlay portal to <body>, so the
   `:deep()` selector via Vue 3.5's preserved `data-v-*` attribute
   is unnecessary here — the portal children are still children of
   THIS component's render tree (reka-ui portals via Vue's own
   <Teleport>, which preserves the parent chain for styling). If
   theming breaks in a future reka-ui upgrade, fall back to the
   `:deep()` pattern used by AuditLogModal. */

.subagent-drawer__sr-title,
.subagent-drawer__sr-desc {
  position: absolute;
  width: 1px;
  height: 1px;
  margin: -1px;
  padding: 0;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}

.subagent-drawer__overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.35);
  z-index: 999;
}

.subagent-drawer {
  position: fixed;
  top: 0;
  right: 0;
  bottom: 0;
  width: min(480px, 90vw);
  background: var(--color-bg-surface);
  border-left: 1px solid var(--color-bg-border);
  box-shadow: -8px 0 24px rgba(0, 0, 0, 0.18);
  z-index: 1000;
  display: flex;
  flex-direction: column;
  font-family: var(--font-sans);
  color: var(--color-text-primary);
}

/* Slide-in animation */
.subagent-drawer-enter-active,
.subagent-drawer-leave-active {
  transition: transform 0.18s ease-out, opacity 0.18s ease-out;
}
.subagent-drawer-enter-from,
.subagent-drawer-leave-to {
  transform: translateX(24px);
  opacity: 0;
}

.subagent-drawer__header {
  padding: 12px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.subagent-drawer__title-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.subagent-drawer__status {
  padding: 2px 8px;
  border: 1px solid;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 600;
  background: color-mix(in srgb, currentColor 10%, transparent);
}

.subagent-drawer__name {
  font-weight: 600;
  font-size: 13px;
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.subagent-drawer__close {
  font: inherit;
  font-family: var(--font-sans);
  display: inline-flex;
  align-items: center;
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: 4px;
}
.subagent-drawer__close:hover {
  color: var(--color-text-primary);
  background: var(--color-bg-elevated);
}

/* B6 PR3b (2026-06-20): jump-to-latest button in the header.
   Matches the existing close button's footprint (padding / border-radius)
   for visual rhythm; uses --color-accent as the hint color so the user
   notices it without it screaming for attention. Only renders when
   auto-follow is paused. */
.subagent-drawer__jump-latest {
  font: inherit;
  font-family: var(--font-sans);
  display: inline-flex;
  align-items: center;
  background: transparent;
  border: 1px solid var(--color-accent);
  color: var(--color-accent);
  cursor: pointer;
  padding: 2px 6px;
  border-radius: 4px;
  flex-shrink: 0;
}
.subagent-drawer__jump-latest:hover {
  background: var(--color-accent);
  color: var(--color-bg-app);
}

/* B6 PR3b (2026-06-20): floating "↓ N new" button at the body's
   bottom-center. Sits above the scroll content; same --color-accent
   palette as the header jump-latest button so the two feel paired. */
.subagent-drawer__new-events {
  position: sticky;
  bottom: 8px;
  left: 50%;
  transform: translateX(-50%);
  margin: 8px auto 0;
  display: block;
  z-index: 1;
  font: inherit;
  font-family: var(--font-sans);
  font-size: 11px;
  font-weight: 600;
  padding: 4px 12px;
  border-radius: 999px;
  border: 1px solid var(--color-accent);
  background: var(--color-bg-surface);
  color: var(--color-accent);
  cursor: pointer;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.25);
}
.subagent-drawer__new-events:hover {
  background: var(--color-accent);
  color: var(--color-bg-app);
}

.subagent-drawer__meta {
  display: flex;
  gap: 12px;
  font-size: 11px;
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.subagent-drawer__summary {
  margin: 0;
  font-size: 12px;
  color: var(--color-text-secondary);
  line-height: 1.5;
  max-height: 100px;
  overflow-y: auto;
}

.subagent-drawer__filter-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 6px 16px;
  background: var(--color-bg-app);
  border-bottom: 1px solid var(--color-bg-border);
  font-size: 11px;
}

.subagent-drawer__toggle {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  user-select: none;
  color: var(--color-text-secondary);
}

.subagent-drawer__toggle input {
  margin: 0;
}

.subagent-drawer__truncated {
  color: var(--color-tool-shell);
  font-size: 10px;
  cursor: help;
}

.subagent-drawer__body {
  flex: 1;
  overflow-y: auto;
  padding: 8px 0;
}

.subagent-drawer__empty {
  padding: 24px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: 12px;
}

.subagent-drawer__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
}

.subagent-drawer__entry {
  padding: 6px 16px;
  display: flex;
  gap: 8px;
  align-items: flex-start;
  border-bottom: 1px solid var(--color-bg-border);
}
.subagent-drawer__entry:last-child {
  border-bottom: 0;
}

.subagent-drawer__kind {
  flex-shrink: 0;
  padding: 1px 6px;
  border: 1px solid;
  border-radius: 4px;
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 600;
  background: color-mix(in srgb, currentColor 10%, transparent);
  min-width: 48px;
  text-align: center;
}

.subagent-drawer__payload {
  margin: 0;
  flex: 1;
  min-width: 0;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--color-text-primary);
  white-space: pre-wrap;
  word-break: break-all;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  padding: 4px 6px;
  max-height: 200px;
  overflow-y: auto;
}
</style>
