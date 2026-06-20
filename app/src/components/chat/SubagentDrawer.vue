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
//   - per entry: B6 PR3 redesign (2026-06-21) — each entry
//     passes through `pairTranscript()` to merge adjacent
//     tool_call + tool_result into a single `.tool-card` matching
//     the main panel's visual language. chat_event /
//     permission_ask remain standalone. Call+result pairs render
//     the same .tool-card structure (icon + name + path | status-
//     icon + status-text + duration) used by ToolCallCard.
//   - "Show chat events" toggle: chat_event entries default hidden;
//     tool_call / tool_result / permission_ask always visible
//   - transcriptTruncated flag → "原 transcript 已截断 (head + tail)"
//   - empty state → "Worker is starting..."

import { computed, nextTick, onUnmounted, reactive, ref, watch } from "vue";
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
import ToolInputBody from "./ToolInputBody.vue";
import ToolOutputBody from "./ToolOutputBody.vue";
import PermissionAskBody from "./PermissionAskBody.vue";
import WorkerTextTimeline from "./WorkerTextTimeline.vue";
import {
  useSubagentRunsStore,
  coerceStatus,
  parseTranscriptJson,
  type TranscriptEntry,
  type SubagentStatus,
} from "../../stores/subagentRuns";
import { useChatStore } from "../../stores/chat";
import type {
  PermissionAsk,
  Risk,
} from "../../stores/permissions";
import { formatTime } from "../../utils/time";
import {
  pairTranscript,
  isErrorResult,
  type BufferedTranscriptEntry,
} from "../../utils/transcriptPairing";
import { toolAccentVar, toolIcon } from "../../utils/messageFormat";
import { abbreviateDuration } from "../../utils/duration";

const store = useSubagentRunsStore();
const chatStore = useChatStore();

/** FT-F-001 stage 2 (2026-06-20): repo root for the historical-mode
 *  PermissionAskBody path badge. Q2 decision — we assume the worker
 *  runs under the same project root as the parent session's cwd (the
 *  common case). Edge case: a worker running in a different cwd will
 *  show an inaccurate 仓库内 / 仓库外 badge; accepted as an edge-case
 *  tradeoff for the simpler drawer API (no per-worker cwd tracking). */
const repoRoot = computed<string>(() => chatStore.currentCwd);

/** FT-F-001 stage 2 (2026-06-20): synthesize a `PermissionAsk` from a
 *  drawer transcript entry's `payload_json`. The body component takes
 *  the typed `PermissionAsk` shape (camelCase), so we map field-by-
 *  field. Rid / sessionId / toolUseId are best-effort strings here —
 *  the historical-mode card never fires onRespond, so these IDs are
 *  purely informational (they DO drive the path badge via `ask.path`).
 *
 *  Cross-layer drift note (2026-06-20 check phase): the Rust
 *  `PermissionAskPayload` carries `#[serde(rename_all = "camelCase")]`
 *  (see `app/src-tauri/src/agent/permissions/mod.rs:406`), so the
 *  stored `payload_json` actually has camelCase keys
 *  (`sessionId` / `toolUseId` / `toolName` / `toolInput`). The PRD's
 *  snake_case claim was wrong — `ToolCallPayload` / `ToolResultPayload`
 *  are snake_case (no `rename_all`), but `PermissionAskPayload` is
 *  camelCase. We read BOTH spellings defensively (camelCase first per
 *  production reality, snake_case as fallback) so the drawer keeps
 *  rendering correctly if either layer is ever refactored. */
function synthesizeAsk(p: Record<string, unknown>): PermissionAsk {
  return {
    rid: String(p.rid ?? ""),
    sessionId: String(p.sessionId ?? p.session_id ?? ""),
    toolUseId: String(p.toolUseId ?? p.tool_use_id ?? ""),
    toolName: String(p.toolName ?? p.tool_name ?? ""),
    toolInput: (p.toolInput ?? p.tool_input ?? {}) as Record<string, unknown>,
    risk: p.risk as Risk,
    reason: p.reason as string | undefined,
    path: p.path as string | undefined,
  };
}

// -----------------------------------------------------------------------
// B6 PR3 redesign (2026-06-21): per-entry accessors for the
// `tool-card` template. Each is a thin read-only helper that pulls
// a field out of a `TranscriptEntry`'s `payload_json` with a
// defensive default. The template needs to know the tool name,
// file path, input, and output content for the header / body of
// the rendered card; these helpers centralize the type coercions.
// -----------------------------------------------------------------------

/** Pull the tool name out of a `tool_call`'s `payload_json`. Falls
 *  back to the result's name (for orphan results) or empty string
 *  for non-tool entries. Defensive against missing / non-string
 *  values (pre-redesign rows). For `permission_ask` entries, the
 *  `PermissionAskPayload` Rust struct has `toolName` / `tool_name`
 *  fields; we read both spellings (the PR1 `synthesizeAsk` helper
 *  does the same — keep the two in lockstep). */
function toolNameOf(e: TranscriptEntry): string {
  if (e.kind === "tool_call" || e.kind === "tool_result") {
    const n = e.payload_json?.name;
    if (typeof n === "string" && n.length > 0) return n;
    return "";
  }
  if (e.kind === "permission_ask") {
    // permission_ask payload_json may carry either `toolName`
    // (camelCase — production shape per the Rust
    // `PermissionAskPayload` `#[serde(rename_all = "camelCase")])`
    // or `tool_name` (snake_case fallback).
    const n = e.payload_json?.toolName ?? e.payload_json?.tool_name;
    if (typeof n === "string" && n.length > 0) return n;
    return "";
  }
  return "";
}

/** Pull the file path out of a `tool_call`'s `payload_json.input.path`.
 *  Returns null when the input has no `path` (shell, web_fetch, etc.)
 *  or when the value is non-string. */
function filePathOf(e: TranscriptEntry): string | null {
  if (e.kind !== "tool_call" && e.kind !== "tool_result") return null;
  const input = e.payload_json?.input as Record<string, unknown> | undefined;
  if (!input) return null;
  const p = input.path;
  if (typeof p === "string" && p.length > 0) return p;
  return null;
}

/** Pull the input object out of a `tool_call`'s `payload_json.input`. */
function inputOf(e: TranscriptEntry): Record<string, unknown> | null {
  if (e.kind !== "tool_call") return null;
  const input = e.payload_json?.input;
  if (input && typeof input === "object" && !Array.isArray(input)) {
    return input as Record<string, unknown>;
  }
  return null;
}

/** Pull the content string out of a `tool_result`'s `payload_json.content`. */
function contentOf(e: TranscriptEntry): string {
  if (e.kind !== "tool_result") return "";
  const c = e.payload_json?.content;
  return typeof c === "string" ? c : "";
}

/** Pull the `duration_ms` number out of a `tool_result`'s
 *  `payload_json.duration_ms`. Returns undefined for pre-redesign
 *  rows (no field) — `ToolOutputBody` treats undefined as "omit
 *  duration chip" per its file header. */
function durationMsOf(e: TranscriptEntry): number | undefined {
  if (e.kind !== "tool_result") return undefined;
  const d = e.payload_json?.duration_ms;
  if (typeof d === "number" && Number.isFinite(d) && d >= 0) return d;
  return undefined;
}

/** Human-readable duration label for the header. Returns "" for
 *  pre-redesign rows (no duration). */
function durationOf(e: TranscriptEntry): string {
  const d = durationMsOf(e);
  if (d === undefined) return "";
  return abbreviateDuration(d);
}

/** Standalone-entry accent color (for the 3px left border). The
 *  sub-kinds each have a different color so the user can scan a
 *  long transcript at a glance: chat_event = muted gray,
 *  permission_ask = amber, orphan tool_call = amber (matches
 *  the pending-call color), orphan tool_result = the tool's
 *  accent. */
function standaloneAccent(e: TranscriptEntry): string {
  if (e.kind === "chat_event") return "var(--color-text-muted)";
  if (e.kind === "permission_ask") return "var(--color-tool-shell)";
  if (e.kind === "tool_call") return "var(--color-tool-shell)";
  if (e.kind === "tool_result") {
    if (isErrorResult(e)) return "var(--color-tool-error)";
    return toolAccentVar(toolNameOf(e));
  }
  return "var(--color-text-muted)";
}

/** Standalone-entry name to render in the header. For chat_event
 *  and permission_ask, surface a short label. For tool_call /
 *  tool_result, fall through to the tool name (matches the paired
 *  card's header). */
function standaloneName(e: TranscriptEntry): string {
  if (e.kind === "chat_event") return "chat event";
  if (e.kind === "permission_ask") {
    const toolName = toolNameOf(e);
    return toolName ? `${toolName} (ask collapsed)` : "permission ask";
  }
  if (e.kind === "tool_call") return toolNameOf(e) || "tool call";
  if (e.kind === "tool_result") return toolNameOf(e) || "tool result";
  return "";
}

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

/** B6 PR3 redesign (2026-06-21): a non-reactive Map tracking the
 *  first-seen timestamp of each pending tool_call so the 30s
 *  timeout flush can age out calls across `pairTranscript`
 *  re-invocations. Lives for the drawer's lifetime; cleared on
 *  drawer close (the next open starts fresh). See the
 *  `pairTranscript` docstring for the cross-invocation contract. */
const pendingFirstSeenAt = reactive(new Map<string, number>());

/** Visible transcript after applying the chat-event filter. */
const visibleTranscript = computed<TranscriptEntry[]>(() => {
  if (showChatEvents.value) return transcript.value;
  return transcript.value.filter((e) => e.kind !== "chat_event");
});

/** B6 PR3 redesign (2026-06-21): paired / pending_call /
 *  standalone buffer view of `visibleTranscript`. Recomputed on
 *  every `nowTick` tick (5s cadence) so pending calls naturally
 *  age out from `pending_call` to `standalone` even without new
 *  transcript events arriving. */
const bufferedTranscript = computed<BufferedTranscriptEntry[]>(() =>
  pairTranscript(visibleTranscript.value, Date.now(), pendingFirstSeenAt),
);

/** FT-F-004 (2026-06-21): chat_event entries the default filter
 *  hides. Surfaced as a "+N chat hidden" hint next to the event
 *  count in the filter row ONLY while chat events are hidden — it
 *  nudges the user to expand. Once they tick "Show chat events",
 *  visibleTranscript includes chat and this drops to 0, hiding the
 *  hint. Computed as transcript.length − visibleTranscript.length
 *  so it stays correct regardless of how the filter is expressed. */
const hiddenChatCount = computed<number>(() =>
  showChatEvents.value
    ? 0
    : transcript.value.length - visibleTranscript.value.length,
);

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

/** FT-F-001 stage 2 (2026-06-20): the drawer no longer stringifies
 *  `payload_json` into a `<pre>` blob. Each transcript entry routes to
 *  its typed-card body component (see the `<li>` branches in the
 *  template). The old `formatPayload` + `extractToolResultDisplay`
 *  import have been removed; envelope-unwrapping + truncation now live
 *  inside `ToolOutputBody.vue` (PR1 shared body).
 *
 *  B6 PR3 redesign (2026-06-21): the old per-entry `.subagent-drawer__kind`
 *  badge is gone (the new `.tool-card` container has a richer
 *  status row instead). The `KIND_META` color/label constant has
 *  been removed; the relevant color mappings are now per-sub-kind
 *  in `standaloneAccent()` and the new template's `toolAccentVar`
 *  lookups. */

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

/** Failure-reason banner shown in the header for terminal error /
 *  cancelled runs (2026-06-20, FT-F-005). Returns `null` for
 *  `running` / `completed` states (no banner) or when the row
 *  hasn't loaded yet.
 *
 *  - `error` + non-empty `summary` → "Worker exited with error: <summary>",
 *    truncated to 80 chars + "…" if longer (the `summary` field carries
 *    the worker error text — see `agent/subagent.rs:format_dispatch_result`,
 *    Error arm).
 *  - `error` + empty/null `summary` → "Worker exited unexpectedly at X.Xs"
 *    using the frozen duration from `statusDisplay.suffix` (same
 *    `terminalDurMs` formula as the badge — see B1 hotfix).
 *  - `cancelled` → "Worker stopped by user at X.Xs" generic message
 *    (the schema doesn't record whether the cancel came from user
 *    Stop vs system timeout; out of scope per FT-F-005 prd §"Out of
 *    Scope").
 *
 *  Reuses `statusDisplay.suffix` for the duration string so the
 *  banner stays consistent with the badge ("failed at 11.7s" +
 *  "Worker exited unexpectedly at 11.7s" share the same number). */
const bannerText = computed<{ kind: "error" | "warning"; text: string } | null>(() => {
  if (!run.value) return null;
  if (status.value === "error") {
    const summary = run.value.summary;
    if (summary && summary.length > 0) {
      const truncated = summary.length > 80 ? summary.slice(0, 80) + "…" : summary;
      return { kind: "error", text: `Worker exited with error: ${truncated}` };
    }
    return { kind: "error", text: `Worker exited unexpectedly${statusDisplay.value.suffix}` };
  }
  if (status.value === "cancelled") {
    return { kind: "warning", text: `Worker stopped by user${statusDisplay.value.suffix}` };
  }
  return null;
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
      // 100ms cadence drives BOTH the header duration counter
      // AND the pairing-layer timeout flush (the buffered
      // transcript re-runs on every nowTick tick; the 30s
      // pending timeout advances by ~100ms per tick, which is
      // more than enough granularity for a "未完成" card
      // falling out of the pending window).
      tickerHandle = setInterval(() => {
        nowTick.value = Date.now();
      }, TIMER_TICK_MS);
    } else {
      // Drawer closed — drop the pending-call map so the next
      // open starts fresh (a new runId won't accidentally inherit
      // a stale "received at" from the previous run).
      pendingFirstSeenAt.clear();
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
 *  hide chat_event entries by default; `bufferedTranscript` is
 *  what the user actually sees, since each paired call+result
 *  collapses to a single card). When a new entry arrives: if
 *  auto-follow is on, scroll to bottom; otherwise increment the
 *  newCount badge. */
watch(
  () => bufferedTranscript.value.length,
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
            <!-- FT-F-005 (2026-06-20): failure-reason banner. Renders
                 only for terminal error / cancelled states (bannerText
                 computed returns null otherwise). Sits between the
                 status badge row and the timestamp row so the reason
                 reads as the natural next piece of context after
                 "failed at Ns" / "已停止 at Ns". The banner reuses the
                 status badge's duration suffix (statusDisplay.suffix)
                 for consistency. -->
            <div
              v-if="bannerText"
              :class="[
                'subagent-drawer__banner',
                `subagent-drawer__banner--${bannerText.kind}`,
              ]"
              role="status"
              :aria-label="bannerText.text"
            >
              <Icon name="warn" :size="14" />
              <span class="subagent-drawer__banner-text">{{ bannerText.text }}</span>
            </div>
            <div
              v-if="run?.startedAt"
              class="subagent-drawer__meta"
            >
              <!-- FT-F-004 (2026-06-21): raw ISO8601 → local HH:MM:SS
                   via formatTime (UTC→local conversion lives in the
                   helper — slicing the raw string would show UTC and
                   drift ~8h). Both timestamps keep the `clock` icon
                   for a unified "this is a time field" affordance. -->
              <span class="subagent-drawer__meta-time">
                <Icon name="clock" :size="11" />
                开始 {{ formatTime(run.startedAt) }}
              </span>
              <span
                v-if="run.finishedAt"
                class="subagent-drawer__meta-time"
              >
                <Icon name="clock" :size="11" />
                结束 {{ formatTime(run.finishedAt) }}
              </span>
            </div>
            <p
              v-if="run?.summary"
              class="subagent-drawer__summary"
            >{{ run.summary }}</p>
          </header>

          <!-- Filter row: chat-event toggle + truncated notice -->
          <div class="subagent-drawer__filter-row">
            <div class="subagent-drawer__filter-left">
              <label class="subagent-drawer__toggle">
                <input
                  v-model="showChatEvents"
                  type="checkbox"
                />
                <span>Show chat events</span>
              </label>
              <!-- FT-F-004 (2026-06-21): event count + hidden-chat
                   hint. visibleTranscript.length is the count the
                   user actually sees (tool_call/result/perm by
                   default; +chat once the toggle is on). The
                   "+N chat hidden" suffix nudges expansion and
                   disappears once chat events are shown. -->
              <span class="subagent-drawer__event-count">
                {{ visibleTranscript.length }} events<span
                  v-if="hiddenChatCount > 0"
                > · +{{ hiddenChatCount }} chat hidden</span>
              </span>
            </div>
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
              <!-- B6 PR3 redesign (2026-06-21): each entry in
                   `bufferedTranscript` is rendered as a `.tool-card`
                   matching the main panel's visual language. Three
                   branches per the pairing layer's return shape:
                     - `paired` (call + result merged): one card with
                       header (icon + name + path | status + duration)
                       + body (ToolInputBody + ToolOutputBody).
                     - `pending_call`: amber-bordered card with a
                       pulsing "未完成" indicator (still within 30s
                       timeout).
                     - `standalone`: chat_event (muted) /
                       permission_ask (amber) / orphan call (amber
                       "未完成" past timeout) / orphan result
                       (standard). Each sub-kind keeps its own
                       accent. -->
              <li
                v-for="(b, i) in bufferedTranscript"
                :key="b.kind === 'paired' ? `pair-${b.tool_use_id}` : b.kind === 'pending_call' ? `pend-${b.tool_use_id}` : `solo-${i}`"
                class="subagent-drawer__entry-wrapper"
              >
                <!-- PAIRED: call + result merged into one card. -->
                <div
                  v-if="b.kind === 'paired'"
                  class="tool-card"
                  :class="{
                    'tool-card--error': isErrorResult(b.result),
                  }"
                  :style="{
                    borderLeftColor: isErrorResult(b.result)
                      ? 'var(--color-tool-error)'
                      : toolAccentVar(toolNameOf(b.call)),
                  }"
                >
                  <div class="tool-card__header">
                    <div class="tool-card__title">
                      <span class="tool-card__icon">
                        <Icon :name="toolIcon(toolNameOf(b.call))" :size="14" />
                      </span>
                      <span class="tool-card__name">{{ toolNameOf(b.call) }}</span>
                      <span
                        v-if="filePathOf(b.call)"
                        class="tool-card__path"
                        :title="filePathOf(b.call) ?? ''"
                      >· {{ filePathOf(b.call) }}</span>
                    </div>
                    <div class="tool-card__status">
                      <span
                        class="tool-card__status-icon"
                        :class="{ 'tool-card__status-icon--error': isErrorResult(b.result) }"
                      >
                        <Icon :name="isErrorResult(b.result) ? 'x' : 'check'" :size="14" />
                      </span>
                      <span>{{ isErrorResult(b.result) ? 'error' : 'done' }}</span>
                      <span class="tool-card__duration">{{ durationOf(b.result) }}</span>
                    </div>
                  </div>
                  <ToolInputBody
                    v-if="inputOf(b.call) && Object.keys(inputOf(b.call) ?? {}).length > 0"
                    :name="toolNameOf(b.call)"
                    :input="inputOf(b.call) ?? {}"
                  />
                  <ToolOutputBody
                    :content="contentOf(b.result)"
                    :is-error="isErrorResult(b.result)"
                    :duration-ms="durationMsOf(b.result)"
                  />
                </div>

                <!-- PENDING CALL: call is in flight, no result yet
                     (within 30s timeout). Amber left-border + a
                     pulsing icon to signal "still working". -->
                <div
                  v-else-if="b.kind === 'pending_call'"
                  class="tool-card tool-card--running"
                  :style="{ borderLeftColor: 'var(--color-tool-shell)' }"
                >
                  <div class="tool-card__header">
                    <div class="tool-card__title">
                      <span class="tool-card__icon">
                        <Icon :name="toolIcon(toolNameOf(b.call))" :size="14" />
                      </span>
                      <span class="tool-card__name">{{ toolNameOf(b.call) }}</span>
                      <span
                        v-if="filePathOf(b.call)"
                        class="tool-card__path"
                        :title="filePathOf(b.call) ?? ''"
                      >· {{ filePathOf(b.call) }}</span>
                    </div>
                    <div class="tool-card__status">
                      <span class="tool-card__status-icon tool-card__status-icon--running">
                        <Icon name="ellipsis" :size="14" />
                      </span>
                      <span>running…</span>
                    </div>
                  </div>
                  <ToolInputBody
                    v-if="inputOf(b.call) && Object.keys(inputOf(b.call) ?? {}).length > 0"
                    :name="toolNameOf(b.call)"
                    :input="inputOf(b.call) ?? {}"
                  />
                </div>

                <!-- STANDALONE: chat_event / permission_ask / orphan
                     call (past 30s timeout) / orphan result. Each
                     sub-kind keeps its own accent. -->
                <div
                  v-else
                  class="tool-card"
                  :class="{
                    'tool-card--error': isErrorResult(b.entry),
                    'tool-card--orphan-call': b.entry.kind === 'tool_call' && !contentOf(b.entry) && !inputOf(b.entry),
                  }"
                  :style="{ borderLeftColor: standaloneAccent(b.entry) }"
                >
                  <div class="tool-card__header">
                    <div class="tool-card__title">
                      <span class="tool-card__icon">
                        <Icon
                          v-if="b.entry.kind === 'tool_call' || b.entry.kind === 'tool_result'"
                          :name="toolIcon(toolNameOf(b.entry))"
                          :size="14"
                        />
                        <Icon
                          v-else-if="b.entry.kind === 'permission_ask'"
                          name="shield-check"
                          :size="14"
                        />
                        <Icon v-else name="chat" :size="14" />
                      </span>
                      <span class="tool-card__name">{{ standaloneName(b.entry) }}</span>
                    </div>
                    <div
                      v-if="b.entry.kind === 'tool_result'"
                      class="tool-card__status"
                    >
                      <span
                        class="tool-card__status-icon"
                        :class="{ 'tool-card__status-icon--error': isErrorResult(b.entry) }"
                      >
                        <Icon :name="isErrorResult(b.entry) ? 'x' : 'check'" :size="14" />
                      </span>
                      <span>{{ isErrorResult(b.entry) ? 'error' : 'done' }}</span>
                      <span class="tool-card__duration">{{ durationOf(b.entry) }}</span>
                    </div>
                    <div
                      v-else-if="b.entry.kind === 'tool_call'"
                      class="tool-card__status"
                    >
                      <span
                        class="tool-card__status-icon"
                        style="color: var(--color-tool-shell)"
                      >
                        <Icon name="warn" :size="14" />
                      </span>
                      <span>未完成</span>
                    </div>
                  </div>
                  <ToolInputBody
                    v-if="b.entry.kind === 'tool_call' && inputOf(b.entry) && Object.keys(inputOf(b.entry) ?? {}).length > 0"
                    :name="toolNameOf(b.entry)"
                    :input="inputOf(b.entry) ?? {}"
                  />
                  <ToolOutputBody
                    v-else-if="b.entry.kind === 'tool_result'"
                    :content="contentOf(b.entry)"
                    :is-error="isErrorResult(b.entry)"
                    :duration-ms="durationMsOf(b.entry)"
                  />
                  <PermissionAskBody
                    v-else-if="b.entry.kind === 'permission_ask'"
                    mode="historical"
                    :ask="synthesizeAsk(b.entry.payload_json)"
                    :repo-root="repoRoot"
                  />
                  <WorkerTextTimeline
                    v-else-if="b.entry.kind === 'chat_event'"
                    :events="[b.entry]"
                  />
                </div>
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
  width: min(640px, 90vw);
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

/* FT-F-005 (2026-06-20): failure-reason banner. Always-visible
   inline warning strip in the header for terminal error / cancelled
   runs. Two color variants (--error red + --warning amber) using
   existing design tokens per spec/design-tokens.md — no hardcoded
   hex. Left 3px accent bar + ⚠ icon + text in a single row; wraps
   gracefully on narrow viewports (the drawer's max-width is 640px).
   Reuses `Icon name="warn"` (ExclamationTriangleIcon) already in
   the Icon.vue registry — no new icon import. */
.subagent-drawer__banner {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  padding: 6px 8px;
  border-radius: 4px;
  border-left: 3px solid currentColor;
  font-family: var(--font-sans);
  font-size: 11px;
  line-height: 1.4;
  background: color-mix(in srgb, currentColor 8%, transparent);
  word-break: break-word;
}
.subagent-drawer__banner--error {
  color: var(--color-tool-error);
}
.subagent-drawer__banner--warning {
  color: var(--color-tool-shell);
}
.subagent-drawer__banner-text {
  flex: 1;
  min-width: 0;
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

/* FT-F-004 (2026-06-21): wrapper for a clock icon + formatted
   timestamp so the two stay vertically centered in the mono
   meta row. Inherits color/font from __meta. */
.subagent-drawer__meta-time {
  display: inline-flex;
  align-items: center;
  gap: 3px;
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

/* FT-F-004 (2026-06-21): groups the chat-event toggle + the event
   count on the filter row's left side (the truncated notice stays
   right via space-between). */
.subagent-drawer__filter-left {
  display: flex;
  align-items: center;
  gap: 12px;
}

/* FT-F-004 (2026-06-21): "N events · +X chat hidden" counter. Mono
   font keeps the count visually distinct from the toggle label;
   --color-text-muted so it reads as secondary metadata. */
.subagent-drawer__event-count {
  color: var(--color-text-muted);
  font-family: var(--font-mono);
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
  gap: 8px;
  padding: 8px 12px;
}

.subagent-drawer__entry-wrapper {
  /* Wrapper around each .tool-card entry. No visual styling itself;
     the inner .tool-card owns the card surface. The list-level gap
     (8px) provides visual separation between consecutive cards
     (was: 1px border-bottom on the old per-kind rows). */
  list-style: none;
}

/* B6 PR3 redesign (2026-06-21): duplicate of the .tool-card visual
   contract from `ToolCallCard.vue`. The project uses plain CSS
   (no SCSS), so we can't share the rules via @import. Instead we
   re-declare the minimum subset here. The two definitions stay
   lockstep manually (see the design system contract in
   .trellis/spec/frontend/design-tokens.md §"Modal Tokens" — when
   the tool-card primitive grows new variants, both files must
   update).

   This is the explicit "回退" path from
   .trellis/tasks/06-21-redesign-subagent-drawer-entry-as-toolcard-style/prd.md
   §"复用 ToolCard 样式的策略": "直接在 drawer 里 `:deep(.tool-card) { ... }`
   复制最小子集" (the spec acknowledges the duplication as the
   trade-off for plain-CSS toolchain).

   All properties below mirror `ToolCallCard.vue` 1:1 so the
   drawer and main panel produce visually identical cards.
   Variables resolve via :root (the design tokens in style.css). */
.tool-card {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-text-muted);
  border-radius: 6px;
  padding: 8px 12px;
  font-size: 12px;
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  max-width: 100%;
}

.tool-card--error {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}

.tool-card--running {
  border-left-color: var(--color-tool-shell);
}

.tool-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
}

.tool-card__title {
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
  flex: 1;
  overflow: hidden;
  white-space: nowrap;
}

.tool-card__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  color: var(--color-text-secondary);
}

.tool-card--error .tool-card__icon {
  color: var(--color-tool-error);
}

.tool-card__name {
  font-weight: 600;
  color: var(--color-text-primary);
}

.tool-card--error .tool-card__name {
  color: var(--color-tool-error);
}

.tool-card__path {
  color: var(--color-text-secondary);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
  flex: 1;
}

.tool-card__status {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.tool-card__status-icon {
  display: inline-flex;
  align-items: center;
  line-height: 1;
}

.tool-card__status-icon--running {
  animation: tool-card-pulse 1.4s ease-in-out infinite;
}

.tool-card__status-icon--error {
  color: var(--color-tool-error);
}

@keyframes tool-card-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}

.tool-card--error .tool-card__status {
  color: var(--color-tool-error);
}

.tool-card__duration {
  display: inline-flex;
  align-items: center;
  margin-left: 2px;
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
  font-weight: 500;
  user-select: none;
}

.tool-card--error .tool-card__duration {
  color: var(--color-tool-error);
}
</style>
