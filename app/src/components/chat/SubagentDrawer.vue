<script setup lang="ts">
// SubagentDrawer — right-side drawer for worker subagent transcripts.
//
// 5-segment grouped view (B6 redesign, 2026-06-21):
//   <SubagentDrawerHeader>      ← status badge + name + duration + banner
//   <DrawerPromptCard>          ← run.task (parent LLM prompt, 120-char truncate)
//   <SubagentDrawerErrorCard>   ← R25: status=error detailed card (PR6, split 2026-06-23)
//   <DrawerSection type="thinking">  ← collapsed by default
//   <DrawerSection type="tools">     ← expanded by default
//   <DrawerSection type="reply">     ← expanded by default
//
// Data source: `store.liveSections.get(openRunId)` — the accumulator's
// `TranscriptSection[]` output. The previous flat `transcript` +
// `showChatEvents` toggle + filter row are GONE (PRD R9).
//
// reka-ui note: 2.9.9 has no `Sheet` primitive; we compose the drawer
// from `Dialog*` + sidebar CSS. See `.trellis/spec/frontend/reka-ui-usage.md`.

import { computed, nextTick, onUnmounted, reactive, ref, watch } from "vue";
import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from "reka-ui";
import Icon from "../Icon.vue";
import { useSubagentRunsStore, coerceStatus } from "../../stores/subagentRuns";
import { parseTranscriptJson } from "../../stores/runAccumulator";
import type {
  SubagentStatus,
  TranscriptSection,
  ThinkingSection,
} from "../../stores/subagentRuns.types";
import { useChatStore } from "../../stores/chat";
import { usePermissionsStore } from "../../stores/permissions";
import type {
  PermissionAsk,
  Risk,
} from "../../stores/permissions";
import {
  pairSections,
  type SectionToolEntry,
} from "../../utils/transcriptPairing";
import { truncate } from "../../utils/useTruncate";
import { renderMarkdown } from "../../utils/markdown";
import DrawerSection from "./DrawerSection.vue";
import DrawerPromptCard from "./DrawerPromptCard.vue";
import DrawerThinkingBlock from "./DrawerThinkingBlock.vue";
import DrawerToolCallCard from "./DrawerToolCallCard.vue";
import DrawerPermissionAskCard from "./DrawerPermissionAskCard.vue";
import SubagentDrawerHeader from "./SubagentDrawerHeader.vue";
import SubagentDrawerErrorCard from "./SubagentDrawerErrorCard.vue";
import MarkdownDetailModal from "../common/MarkdownDetailModal.vue";

const store = useSubagentRunsStore();
const chatStore = useChatStore();
const permissionsStore = usePermissionsStore();

/** FT-F-001 stage 2 (2026-06-20): repo root for the historical-mode
 *  PermissionAskBody path badge. Q2 decision — we assume the worker
 *  runs under the same project root as the parent session's cwd (the
 *  common case). Edge case: a worker running in a different cwd will
 *  show an inaccurate 仓库内 / 仓库外 badge; accepted as an edge-case
 *  tradeoff for the simpler drawer API (no per-worker cwd tracking). */
const repoRoot = computed<string>(() => chatStore.currentCwd);

/** FT-F-001 stage 2 (2026-06-20): synthesize a `PermissionAsk` from a
 *  drawer transcript section's `payload_json`. The body component takes
 *  the typed `PermissionAsk` shape (camelCase), so we map field-by-field.
 *
 *  PR2 RULE-FrontSubagent-003 (2026-06-22): the payload also carries
 *  an optional `workerRunId` — set when the backend emits a LIVE
 *  worker ask. Historical transcript entries (RULE-A-016 collapse path
 *  pre-PR1) do NOT carry it. The `interactive` reconciliation below
 *  uses `getPendingByRid` (the live permissions store) instead, so the
 *  field is purely informational.
 *
 *  Cross-layer drift note (2026-06-20 check phase): the Rust
 *  `PermissionAskPayload` carries `#[serde(rename_all = "camelCase")]`
 *  (see `app/src-tauri/src/agent/permissions/mod.rs:406`), so the
 *  stored `payload_json` actually has camelCase keys. We read BOTH
 *  spellings defensively (camelCase first per production reality,
 *  snake_case as fallback). */
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
    workerRunId:
      typeof p.workerRunId === "string"
        ? p.workerRunId
        : typeof p.worker_run_id === "string"
          ? p.worker_run_id
          : undefined,
  };
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
  // Session 60 R2 (2026-06-21): `incomplete` is the `max_turns`
  // soft-cap terminal state (worker hit the turn limit mid-task
  // and exited with a partial summary). Mirrors the backend
  // `INCOMPLETE_MARKER` ([未完成]) text prefix; visually reuses
  // `--color-tool-shell` (amber) as the warning tint — design
  // tokens intentionally has no `--color-tool-warn` (see
  // `design-tokens.md` "Don't add a new `--color-*` token for a
  // one-off use"), and `--color-tool-shell` already carries the
  // "extra caution" connotation (per the re-grill 2026-06-13 PR2
  // precedent for the in-repo/out-of-repo badges).
  incomplete: { label: "未完成", color: "var(--color-tool-shell)" },
};

// ---------------------------------------------------------------------------
// B6 redesign PR5 (2026-06-21): section-based data source.
// ---------------------------------------------------------------------------

/** Section list for the drawer body. Reads `store.liveSections`
 *  (the PR2 accumulator's `TranscriptSection[]` output). Empty
 *  during the brief window between `openDrawer` and `fetchRun`
 *  resolving — the drawer shows the "Worker is starting..."
 *  empty state in that window. */
const sections = computed<TranscriptSection[]>(() => {
  const rid = store.openRunId;
  if (!rid) return [];
  return store.liveSections.get(rid) ?? [];
});

/** Thinking-segment entries (one per Anthropic thinking block). */
const thinkingSections = computed<ThinkingSection[]>(() =>
  sections.value.filter(
    (s): s is ThinkingSection => s.kind === "Thinking",
  ),
);

/** B6 PR3 redesign pending-timeout tracking. Same non-reactive Map
 *  pattern as the previous flat-list implementation — keys are
 *  `tool_use_id` strings, values are wall-clock `received_at` ms.
 *  Cleared on drawer close (the next open starts fresh). The 100ms
 *  `nowTick` ticker drives the age-out across re-invocations. */
const pendingFirstSeenAt = reactive(new Map<string, number>());

/** Tools segment entries: `pairSections` output (paired / pending /
 *  permission_ask). Recomputed on every `nowTick` tick (100ms) so
 *  pending calls naturally age out without a new section arriving.
 *  The `nowTick.value` reference is load-bearing — Vue's reactivity
 *  only re-runs a computed when one of its tracked reactive deps
 *  changes; `pairSections`'s `now` argument is a plain number. */
const toolEntries = computed<SectionToolEntry[]>(
  // `nowTick.value` is the load-bearing dep — the computed would
  // otherwise only re-evaluate on `sections` changes.
  () => pairSections(sections.value, nowTick.value, pendingFirstSeenAt),
);

/** Reply segment text.
 *
 *  - Live phase (worker running): the accumulator's `TextSection`
 *    carries whatever the LLM has streamed so far (the live text
 *    segment may span multiple Text sections — we concatenate them
 *    in arrival order).
 *  - Finished phase (`subagent:finished` → fetchRun →
 *    rebuildFromCache): the accumulator's `FinalTextSection`
 *    carries the authoritative `run.finalText` (PR1 column, with
 *    the `[status: ...]\n` prefix already stripped). Prefer this
 *    over any live Text when both are present (the rebuild drops
 *    live text per PR2's `rebuildFromCache`).
 *  - Empty string when no text section exists yet (the reply
 *    segment shows the empty-state placeholder). */
const replyText = computed<string>(() => {
  let live = "";
  let final: string | null = null;
  for (const s of sections.value) {
    if (s.kind === "Text") {
      live += s.text;
    } else if (s.kind === "FinalText") {
      // FinalText wins — once the worker finishes, the
      // accumulator appends a single FinalText section with the
      // authoritative text. Take it and stop reading Text
      // sections (they were live-only).
      final = s.text;
      break;
    }
  }
  return final ?? live;
});

/** Reply-segment truncation budget per PRD R13. */
const REPLY_MAX_CHARS = 280;

const replyPreview = computed<string>(() =>
  replyText.value.length === 0
    ? ""
    : truncate(replyText.value, REPLY_MAX_CHARS),
);

const replyIsTruncated = computed<boolean>(
  () => replyText.value.length > replyPreview.value.length,
);

const replyPreviewHtml = computed<string>(() =>
  renderMarkdown(replyPreview.value),
);

const replyModalOpen = ref<boolean>(false);

/** Whether the transcript was truncated at the 4 MiB cap on the
 *  backend. Drives the "原 transcript 已截断" banner. */
const truncated = computed<boolean>(() => {
  if (!run.value) return false;
  return (run.value.transcriptTruncated ?? 0) !== 0;
});

/** Empty state: worker just started, no transcript sections yet.
 *  The DrawerPromptCard renders independently (reads `run.task`),
 *  so the user sees the prompt + "Worker is starting..." side-by-side
 *  during the brief window between dispatch and the first
 *  accumulator publish. Once sections arrive, the thinking / tools /
 *  reply segments replace the placeholder.
 *
 *  PR6 (2026-06-21): terminal `cancelled` / `error` states with no
 *  transcript sections still need the Reply / Error card surfaces
 *  (cancelled chip + error card), so `isEmpty` returns `false` for
 *  those states even when `sections.value` is empty. Session 60 R2
 *  added `incomplete` to the terminal set — a `max_turns` soft-cap
 *  exit with a partial transcript still needs the "未完成" chip /
 *  Reply segment to render rather than the "Worker is starting..."
 *  placeholder. The "Worker is starting..." placeholder is for
 *  `running` only — a terminal state with no sections means the
 *  worker died (or was capped) before producing anything, not that
 *  it's about to start. */
const isEmpty = computed<boolean>(() => {
  if (sections.value.length > 0) return false;
  // Cancelled / error / incomplete: render the empty segment
  // shells so the per-state chip (R23 / R25) + the Reply segment
  // are visible. `incomplete` joined the set in Session 60 R2.
  return (
    status.value !== "cancelled" &&
    status.value !== "error" &&
    status.value !== "incomplete"
  );
});

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

/** Terminal duration (frozen once the worker finishes). `null` when
 *  the run is still in flight OR the row hasn't loaded yet. Drives
 *  the per-segment "✓ Completed · X.Xs" chip via DrawerSection's
 *  `finalDurationMs` prop. */
const terminalDurMs = computed<number | null>(() => {
  if (!run.value?.startedAt || !run.value?.finishedAt) return null;
  const startedMs = new Date(run.value.startedAt).getTime();
  const finishedMs = new Date(run.value.finishedAt).getTime();
  if (!Number.isFinite(startedMs) || !Number.isFinite(finishedMs)) return null;
  return Math.max(0, finishedMs - startedMs);
});

/** Status pill text — appends a duration suffix per state:
 *    - running → "running 8.2s" (live, updates every 100ms)
 *    - completed → "done in 12.4s" (terminal, computed once)
 *    - error → "failed at 4.2s" (terminal, wall-clock at error)
 *    - cancelled → "stopped at turn N" (terminal, turn-based when
 *      turn_count present; legacy NULL degrades to "stopped at X.Xs"
 *      wall-clock) — RULE-FrontSubagent-004 (2026-06-22)
 *    - incomplete → "incomplete at turn N" (terminal, turn-based
 *      when turn_count present; legacy NULL degrades to
 *      "incomplete at X.Xs" wall-clock) — RULE-FrontSubagent-004
 *  Falls back to the plain label if the row hasn't loaded yet.
 *
 *  B1 (2026-06-20): the error / cancelled branches previously used
 *  `elapsedMs` (nowTick - startedMs) for the suffix, which kept
 *  ticking after the run finished. If the drawer stayed open after
 *  the worker failed (e.g. user reading the transcript), the badge
 *  drifted from "failed at 11.7s" (real wall-clock) to "failed at
 *  14281.9s" (4 hours). Fix: use `finishedAt - startedAt` for all
 *  terminal states (same formula as `completed`), giving a frozen
 *  duration that doesn't change while the drawer is open.
 *
 *  2026-06-22 (RULE-FrontSubagent-004): cancelled + incomplete now
 *  prefer the turn-based suffix ("at turn N") when `turnCount` is
 *  non-null. PRD R23 字面是 "at turn N" — this lands the turn-count
 *  data PR2 of this task added to `subagent_runs`. Pre-PR2 legacy
 *  rows (turnCount null) degrade to the wall-clock suffix for
 *  backward compat. `completed` is UNCHANGED (still wall-clock):
 *  user-confirmed scope; completed runs measure latency, not turn
 *  progress (a 1-turn completed run that took 30s is "done in 30s",
 *  not "done at turn 1"). `error` is also UNCHANGED — error exits
 *  have less stable turn semantics (the error may fire mid-turn
 *  before the per-turn Done increments the counter). */
const statusDisplay = computed<{ label: string; color: string; suffix: string }>(() => {
  const meta = STATUS_META[status.value];
  if (!run.value?.startedAt) {
    return { label: meta.label, color: meta.color, suffix: "" };
  }
  if (status.value === "running") {
    return { label: meta.label, color: meta.color, suffix: ` ${(elapsedMs.value / 1000).toFixed(1)}s` };
  }
  if (status.value === "completed" && terminalDurMs.value !== null) {
    return { label: meta.label, color: meta.color, suffix: ` ${(terminalDurMs.value / 1000).toFixed(1)}s` };
  }
  if (status.value === "error") {
    const suffix = terminalDurMs.value !== null ? ` at ${(terminalDurMs.value / 1000).toFixed(1)}s` : "";
    return { label: "failed", color: meta.color, suffix };
  }
  if (status.value === "cancelled") {
    // Prefer turn-based suffix when turnCount is present (PR2 R4);
    // null (legacy pre-PR2 rows) degrades to wall-clock.
    if (run.value.turnCount !== null && run.value.turnCount !== undefined) {
      return { label: meta.label, color: meta.color, suffix: ` at turn ${run.value.turnCount}` };
    }
    const suffix = terminalDurMs.value !== null ? ` at ${(terminalDurMs.value / 1000).toFixed(1)}s` : "";
    return { label: meta.label, color: meta.color, suffix };
  }
  if (status.value === "incomplete") {
    // Symmetric with cancelled: prefer turn-based suffix when
    // turnCount present (the natural unit for max_turns — "ran
    // out of budget at turn 200" reads better than "at 142.7s");
    // null degrades to wall-clock. Label uses the meta label
    // ("未完成") rather than the english "failed" / "stopped"
    // form to match the chip text the user reads in STATUS_META.
    if (run.value.turnCount !== null && run.value.turnCount !== undefined) {
      return { label: meta.label, color: meta.color, suffix: ` at turn ${run.value.turnCount}` };
    }
    const suffix = terminalDurMs.value !== null ? ` at ${(terminalDurMs.value / 1000).toFixed(1)}s` : "";
    return { label: meta.label, color: meta.color, suffix };
  }
  return { label: meta.label, color: meta.color, suffix: "" };
});

/** Failure-reason banner shown in the header for terminal error /
 *  cancelled runs (2026-06-20, FT-F-005). Returns `null` for
 *  `running` / `completed` states (no banner) or when the row
 *  hasn't loaded yet. Session 60 R2 added `incomplete` to the
 *  warning set — `max_turns` soft-cap is non-fatal (the worker
 *  returned a partial summary), but the user deserves a banner
 *  explaining why the output looks truncated. */
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
  if (status.value === "incomplete") {
    return { kind: "warning", text: `Worker hit max_turns limit${statusDisplay.value.suffix}` };
  }
  return null;
});

/** Whether the worker is still running (drives the live indicators
 *  on each DrawerSection + autoFollow reactivity). */
const isRunning = computed<boolean>(() => status.value === "running");

/** PR6 (2026-06-21) R25: error terminal state — extract the worker's
 *  error message for the ❌ error card rendered below DrawerPromptCard.
 *
 *  WHY THIS IS A SEPARATE COMPUTED (not read from `sections`):
 *  the accumulator's `routeChatEvent` switch DROPs the `error` inner
 *  kind (`case "error": return;` — terminal signal, no text contribution).
 *  So the error message is NOT in the `sections` / `replyText` stream.
 *  We re-parse `run.transcriptJson` directly to find it.
 *
 *  Extraction priority (4-level fallback chain — see R25 in
 *  `.trellis/spec/frontend/chat.md`):
 *    1. transcriptJson reverse-scan for the last chat_event with
 *       inner kind="error" → payload_json.message
 *    2. run.finalText (backend's `format_final_text` carries the
 *       worker_text verbatim when no explicit error event exists)
 *    3. run.summary (last-resort)
 *    4. canned "(no error text captured)"
 *  Returns `null` when `status !== "error"`. */
const errorMessage = computed<string | null>(() => {
  if (status.value !== "error" || !run.value) return null;

  // Level 1: scan transcriptJson from the end for the last error event.
  const raw = run.value.transcriptJson;
  if (raw) {
    try {
      const entries = parseTranscriptJson(raw);
      for (let i = entries.length - 1; i >= 0; i -= 1) {
        const e = entries[i];
        if (e.kind !== "chat_event") continue;
        const inner = e.payload_json.kind;
        if (inner !== "error") continue;
        const msg = e.payload_json.message;
        if (typeof msg === "string" && msg.length > 0) return msg;
      }
    } catch {
      // parseTranscriptJson already swallows JSON errors internally
      // and returns []; the try/catch here is belt-and-braces in case
      // a future refactor introduces a throwing code path.
    }
  }

  // Level 2: finalText (backend's format_final_text output for
  // status=error carries the worker_text).
  if (run.value.finalText && run.value.finalText.length > 0) {
    return run.value.finalText;
  }

  // Level 3: summary (also used by the header banner).
  if (run.value.summary && run.value.summary.length > 0) {
    return run.value.summary;
  }

  // Level 4: canned fallback.
  return "(no error text captured)";
});

/** PR6 (2026-06-21) R23 (downgraded): cancelled terminal state — the
 *  Reply segment replaces its body with a `⊘ Cancelled · at X.Xs`
 *  chip. PRD R23 originally specified "at turn N" but the
 *  `subagent_runs` schema has no turn column pre-PR2; the downgrade
 *  uses the wall-clock terminal duration (`terminalDurMs`) instead.
 *
 *  Returns the formatted duration suffix (e.g. "at 5.3s") or `null`
 *  when the run is not cancelled or the duration is unavailable. */
const cancelledSuffix = computed<string | null>(() => {
  if (status.value !== "cancelled") return null;
  if (terminalDurMs.value === null) return null;
  return `at ${(terminalDurMs.value / 1000).toFixed(1)}s`;
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
      // 100ms cadence drives BOTH the header duration counter AND
      // the section-level pairing timeout flush.
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
 *  on proximity to the bottom. */
function onBodyScroll(e: Event): void {
  const el = e.target as HTMLElement;
  const atBottom =
    el.scrollHeight - el.scrollTop - el.clientHeight < SCROLL_BOTTOM_THRESHOLD_PX;
  autoFollow.value = atBottom;
  if (atBottom) {
    newCount.value = 0;
  }
}

/** Watch the rendered tool-entry count (the most dynamic segment
 *  during a worker run). When a new entry arrives: if auto-follow
 *  is on, scroll to bottom; otherwise increment the newCount badge. */
watch(
  () => toolEntries.value.length + thinkingSections.value.length + (replyText.value.length > 0 ? 1 : 0),
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
 *  "↓ N new" floating button. */
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

/** Whether to render the header jump-to-latest button. Mirrors the
 *  pre-redesign gate: visible only when autoFollow is off AND there
 *  is something to scroll to (sections non-empty). */
const showJumpLatest = computed<boolean>(
  () => !autoFollow.value && sections.value.length > 0,
);

/** PR2 RULE-FrontSubagent-003 (2026-06-22): reconciliation helper for
 *  the drawer's PermissionAsk cards. For each transcript ask entry,
 *  we check whether the same `rid` is live-pending in the permissions
 *  store. If so, the card renders in interactive mode (Allow / Deny
 *  buttons); otherwise it renders in historical mode (static).
 *
 *  Why rid-based reconciliation: the PR1 backend persists each live
 *  ask to BOTH the worker's transcript AND the permissions store
 *  with the SAME rid — they're the same ask, two surfaces. When the
 *  user responds, the store clears the live entry; the transcript
 *  entry stays as a historical record. So "live = getPendingByRid(rid)"
 *  naturally flips from `true` to `false` the moment the user acts.
 *
 *  Returns `false` for empty / missing rids (defensive against
 *  malformed payload_json — historical entries pre-PR2 may lack
 *  rids entirely). */
function isPermissionAskLive(rid: string): boolean {
  if (!rid) return false;
  return permissionsStore.getPendingByRid(rid) !== undefined;
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

          <!-- Header: status + name + timestamps + summary.
               Split out into <SubagentDrawerHeader> (2026-06-23,
               PRD 06-23-06-23-split-subagent-drawer). Header is a
               pure-presentation child — it receives the run row,
               the typed status, the pre-computed statusDisplay /
               bannerText, and the `truncated` flag. No store reads,
               no ticker, no scroll orchestration. The `↗ jump to
               latest` button moved out of the header to the body
               top (its visible + click handler depend on body's
               autoFollow / newCount / bodyEl state). -->
          <SubagentDrawerHeader
            :run="run"
            :status="status"
            :status-display="statusDisplay"
            :banner-text="bannerText"
            :truncated="truncated"
          />

          <!-- Body: 5-segment grouped view -->
          <div
            ref="bodyEl"
            class="subagent-drawer__body"
            @scroll="onBodyScroll"
          >
            <button
              v-if="showJumpLatest"
              class="subagent-drawer__jump-latest"
              type="button"
              :title="newCount > 0 ? `跳到最新 (${newCount} 条新事件)` : '跳到最新'"
              aria-label="Jump to latest"
              @click="jumpToLatest"
            >
              <Icon name="arrow-down" :size="14" />
            </button>
            <div class="subagent-drawer__segments">
              <!-- Prompt card (always-expanded, hidden when task is null).
                   Rendered OUTSIDE the isEmpty gate so a freshly-dispatched
                   worker (sections still empty) surfaces its prompt
                   immediately rather than flashing "Worker is
                   starting..." with the prompt hidden. -->
              <DrawerPromptCard :task="run?.task ?? null" />

              <!-- PR6 R25: error terminal state — detailed error card
                   below the prompt. The header banner (FT-F-005)
                   shows an 80-char summary line; this card shows the
                   full error message (errorMessage computed falls
                   back through transcriptJson → finalText → summary →
                   canned). Hidden unless status === 'error'. Split
                   into <SubagentDrawerErrorCard> (2026-06-23, PRD
                   06-23-06-23-split-subagent-drawer). -->
              <SubagentDrawerErrorCard
                v-if="status === 'error' && errorMessage !== null"
                :error-message="errorMessage"
              />

              <div v-if="isEmpty" class="subagent-drawer__empty">
                Worker is starting...
              </div>
              <template v-else>
                <!-- Thinking segment (collapsed by default per PRD R16). -->
                <DrawerSection
                  v-if="thinkingSections.length > 0"
                  type="thinking"
                  :entry-count="thinkingSections.reduce((acc, s) => acc + s.chars, 0)"
                  :live="isRunning"
                  :elapsed-ms="elapsedMs"
                  :final-duration-ms="!isRunning ? terminalDurMs ?? undefined : undefined"
                  :default-open="false"
                >
                  <DrawerThinkingBlock
                    v-for="(s, i) in thinkingSections"
                    :key="`thinking-${i}`"
                    :section="s"
                    :show-streaming-hint="isRunning ? undefined : false"
                  />
                </DrawerSection>

                <!-- Tools segment (expanded by default). -->
                <DrawerSection
                  v-if="toolEntries.length > 0"
                  type="tools"
                  :entry-count="toolEntries.length"
                  :live="isRunning"
                  :elapsed-ms="elapsedMs"
                  :final-duration-ms="!isRunning ? terminalDurMs ?? undefined : undefined"
                >
                  <template v-for="(e, i) in toolEntries" :key="`tool-${i}`">
                    <DrawerToolCallCard
                      v-if="e.kind === 'paired'"
                      :call="e.call"
                      :result="e.result"
                    />
                    <DrawerToolCallCard
                      v-else-if="e.kind === 'pending_call'"
                      :call="e.call"
                    />
                    <DrawerPermissionAskCard
                      v-else
                      :ask="synthesizeAsk(e.payload_json)"
                      :repo-root="repoRoot"
                      :interactive="isPermissionAskLive(String(e.payload_json.rid ?? ''))"
                      :outcome="e.outcome"
                    />
                  </template>
                </DrawerSection>

                <!-- Reply segment (expanded by default).
                     PR6 R23 (downgraded): cancelled state shows a
                     `⊘ Cancelled · at X.Xs` chip at the top of the
                     reply body. If worker replyText also exists
                     (worker produced output before being stopped),
                     the reply body renders BELOW the chip so the user
                     can still inspect the partial output. If
                     replyText is empty, only the chip renders. -->
                <DrawerSection
                  v-if="replyText.length > 0 || isRunning || status === 'cancelled'"
                  type="reply"
                  :entry-count="replyText.length > 0 || status === 'cancelled' ? 1 : 0"
                  :live="isRunning"
                  :elapsed-ms="elapsedMs"
                  :final-duration-ms="!isRunning ? terminalDurMs ?? undefined : undefined"
                >
                  <!-- PR6 R23: cancelled chip (replaces empty reply body). -->
                  <div
                    v-if="status === 'cancelled' && cancelledSuffix !== null"
                    class="subagent-drawer__reply-cancelled"
                    role="status"
                  >
                    <Icon name="x" :size="12" />
                    <span>⊘ Cancelled · {{ cancelledSuffix }}</span>
                  </div>
                  <div v-if="replyText.length > 0" class="subagent-drawer__reply-body">
                    <div class="subagent-drawer__reply-markdown" v-html="replyPreviewHtml" />
                    <button
                      v-if="replyIsTruncated"
                      type="button"
                      class="subagent-drawer__reply-view-full"
                      @click="replyModalOpen = true"
                    >
                      View full →
                    </button>
                  </div>
                  <div
                    v-else-if="status !== 'cancelled'"
                    class="subagent-drawer__reply-empty"
                  >
                    Worker has not produced a reply yet.
                  </div>
                  <MarkdownDetailModal
                    v-model:open="replyModalOpen"
                    title="Final Reply"
                    :markdown="replyText"
                    source="reply"
                  />
                </DrawerSection>
              </template>
            </div>
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
   <Teleport>, which preserves the parent chain for styling). */
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

.subagent-drawer__body {
  flex: 1;
  overflow-y: auto;
  padding: 8px 12px;
}

.subagent-drawer__empty {
  padding: 24px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: 12px;
}

.subagent-drawer__segments {
  display: flex;
  flex-direction: column;
  gap: 0;
}

/* Reply segment body — markdown + "View full →" affordance. Mirrors
   DrawerPromptCard's preview layout but with reply-specific
   truncation budget (280 chars per PRD R13). */
.subagent-drawer__reply-body {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.subagent-drawer__reply-markdown {
  font-size: 13px;
  line-height: 1.55;
  color: var(--color-text-primary);
  max-height: 320px;
  overflow-y: auto;
}

.subagent-drawer__reply-markdown :deep(p) {
  margin: 0 0 8px 0;
}

.subagent-drawer__reply-markdown :deep(p:last-child) {
  margin-bottom: 0;
}

.subagent-drawer__reply-markdown :deep(code) {
  font-family: var(--font-mono);
  font-size: 12px;
  background: var(--color-bg-elevated);
  padding: 1px 4px;
  border-radius: 3px;
}

.subagent-drawer__reply-markdown :deep(pre) {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  padding: 10px 12px;
  margin: 8px 0;
  overflow-x: auto;
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.45;
}

.subagent-drawer__reply-markdown :deep(pre code) {
  background: transparent;
  padding: 0;
}

.subagent-drawer__reply-view-full {
  align-self: flex-start;
  background: transparent;
  border: 0;
  color: var(--color-accent);
  cursor: pointer;
  font: inherit;
  font-family: var(--font-sans);
  font-size: 11px;
  padding: 2px 0;
}

.subagent-drawer__reply-view-full:hover {
  text-decoration: underline;
}

.subagent-drawer__reply-empty {
  font-size: 12px;
  color: var(--color-text-muted);
  font-style: italic;
}

/* PR6 R23 (downgraded): cancelled-state chip inside the Reply segment.
   Amber-tinted to match the header banner--warning color (the worker
   was stopped by the user, not by an error). Hidden when the run is
   not cancelled — the v-if in the template handles the gate. */
.subagent-drawer__reply-cancelled {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  border-radius: 4px;
  background: color-mix(in srgb, var(--color-tool-shell) 10%, transparent);
  color: var(--color-tool-shell);
  font-family: var(--font-sans);
  font-size: 12px;
  font-weight: 600;
  align-self: flex-start;
  margin-bottom: 6px;
}

.subagent-drawer__reply-cancelled svg {
  flex-shrink: 0;
}
</style>
