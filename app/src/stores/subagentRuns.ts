// useSubagentRunsStore — Pinia store for B6 PR3 subagent drawer.
//
// Backend contract (post PR2 hotfix + PR3a, see
// `.trellis/tasks/06-20-b6-pr3-frontend-expand/wire-shape-contract.md`):
//
//   1. Two Tauri commands:
//      - `list_subagent_runs_by_session(sessionId) → SubagentRunSummary[]`
//        (no transcript_json — light list for ToolCallCard lookup).
//      - `get_subagent_run(runId) → SubagentRunRow | null`
//        (full row incl. transcriptJson for drawer render).
//
//   2. One IPC event stream: `subagent:event` — emitted live by the
//      worker's `SubagentBufferSink` while the worker runs. Payload
//      shape `{ runId, sessionId, kind, payload, timestamp }`. The
//      drawer reads these in real-time (debounced 200ms) so the user
//      sees worker progress before the run completes.
//
// Data sources for the drawer's transcript list (R6 priority):
//
//     store.liveTranscript.get(openRunId)              // live stream
//       ?? parse(store.getRunCache.get(openRunId)?.transcriptJson)  // DB cache
//       ?? []
//
// ⚠️ Cross-layer drift traps (see wire-shape-contract.md):
//   1. `SubagentRunRow.status` is a raw `string` on the wire but
//      `SubagentRunSummary.status` is a typed enum
//      `"running" | "completed" | "cancelled" | "error"`. We coerce
//      both into a single TS union (`SubagentStatus`) via
//      `coerceStatus` for display.
//   2. `TranscriptEntry` (from transcriptJson, the DB storage shape)
//      uses snake_case `payload_json` because the Rust struct has NO
//      `rename_all`. The live `subagent:event` IPC payload wraps the
//      body as camelCase `payload`. NEVER conflate them — when
//      parsing transcriptJson use `payload_json`; when handling the
//      live stream use `payload`.

import { defineStore } from "pinia";
import { reactive, computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// -----------------------------------------------------------------------
// Types — mirror wire-shape-contract.md verbatim
// -----------------------------------------------------------------------

/** Worker run status. Mirrors `SubagentStatusDb`
 *  `#[serde(rename_all = "lowercase")]`. */
export type SubagentStatus = "running" | "completed" | "cancelled" | "error";

/** `TranscriptKind` — mirrors the Rust enum's
 *  `#[serde(rename_all = "snake_case")]` wire values. Used both as
 *  the `kind` field on `SubagentEventPayload` (live stream) AND as
 *  the `kind` field on `TranscriptEntry` (transcriptJson DB storage
 *  shape). */
export type TranscriptKind =
  | "chat_event"
  | "tool_call"
  | "tool_result"
  | "permission_ask";

/** `list_subagent_runs_by_session` array element. The Rust struct
 *  carries `#[serde(rename_all = "camelCase")]`; `status` is a
 *  typed enum on this shape. */
export interface SubagentRunSummary {
  id: string;
  parentSessionId: string;
  parentRequestId: string;
  subagentName: string;
  status: SubagentStatus;
  startedAt: string;
  finishedAt: string | null;
  tokenUsageJson: string | null;
  summary: string | null;
}

/** `get_subagent_run` return. The Rust struct carries
 *  `#[serde(rename_all = "camelCase")]`.
 *  ⚠️ Drift trap 1: `status` is a raw `String` on the wire (NOT the
 *  typed enum like on SubagentRunSummary). Coerce via `coerceStatus`
 *  before comparing to the union type. */
export interface SubagentRunRow {
  id: string;
  parentSessionId: string;
  parentRequestId: string;
  subagentName: string;
  status: string;
  startedAt: string;
  finishedAt: string | null;
  tokenUsageJson: string | null;
  summary: string | null;
  transcriptJson: string | null;
  transcriptTruncated: number;
  createdAt: string;
}

/** Live `subagent:event` IPC payload. camelCase via the Rust
 *  `build_subagent_event_payload`. `payload` is the wrapped entry
 *  body (camelCase on the wire). */
export interface SubagentEventPayload {
  runId: string;
  sessionId: string;
  kind: TranscriptKind;
  payload: Record<string, unknown>;
  timestamp: string;
}

/** Transcript entry as stored in `transcriptJson` (the DB storage
 *  shape). ⚠️ Drift trap 2: the Rust struct has NO `rename_all`, so
 *  the field is `payload_json` (snake_case) — distinct from the live
 *  `subagent:event` payload's `payload` (camelCase). */
export interface TranscriptEntry {
  kind: TranscriptKind;
  payload_json: Record<string, unknown>;
}

/** Debounce window for batching live events into the reactive
 *  `liveTranscript`. Self-implemented (no lodash) per PRD decision
 *  #8. A 200ms cadence keeps the drawer lively without re-rendering
 *  on every SSE delta. */
export const SUBAGENT_EVENT_DEBOUNCE_MS = 200;

// -----------------------------------------------------------------------
// Helpers — cross-layer drift trap fixes
// -----------------------------------------------------------------------

/** Coerce a raw status string (from `SubagentRunRow.status` or a
 *  malformed `SubagentRunSummary.status`) into the typed union.
 *  Unknown strings fall back to `"running"` (matches the Rust
 *  `SubagentStatusDb::from_str_opt` lenient-parse default). */
export function coerceStatus(raw: string): SubagentStatus {
  if (
    raw === "running" ||
    raw === "completed" ||
    raw === "cancelled" ||
    raw === "error"
  ) {
    return raw;
  }
  return "running";
}

/** Parse `transcriptJson` (DB storage shape) into `TranscriptEntry[]`.
 *  Defensive: a missing or malformed JSON string yields `[]`. Uses
 *  `payload_json` (snake_case) — see Drift trap 2. Exported for the
 *  drawer + the vitest. */
export function parseTranscriptJson(
  raw: string | null | undefined,
): TranscriptEntry[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const out: TranscriptEntry[] = [];
  for (const entry of parsed) {
    if (!entry || typeof entry !== "object") continue;
    const e = entry as { kind?: unknown; payload_json?: unknown };
    if (typeof e.kind !== "string") continue;
    // Lenient kind coercion — mirrors `TranscriptKind` wire values.
    if (
      e.kind !== "chat_event" &&
      e.kind !== "tool_call" &&
      e.kind !== "tool_result" &&
      e.kind !== "permission_ask"
    ) {
      continue;
    }
    const payloadJson =
      e.payload_json && typeof e.payload_json === "object"
        ? (e.payload_json as Record<string, unknown>)
        : {};
    out.push({ kind: e.kind, payload_json: payloadJson });
  }
  return out;
}

// -----------------------------------------------------------------------
// Store
// -----------------------------------------------------------------------

export const useSubagentRunsStore = defineStore("subagentRuns", () => {
  // -----------------------------------------------------------------------
  // Reactive state
  // -----------------------------------------------------------------------

  /** List cache: per-session summaries. Keyed by `sessionId`; the
   *  value is the array returned by `list_subagent_runs_by_session`.
   *  The ToolCallCard looks up its worker via
   *  `getSummaryByToolUseId`. */
  const runSummaryBySession = reactive(
    new Map<string, SubagentRunSummary[]>(),
  );

  /** Detail cache: per-runId full row. Written by `fetchRun`. */
  const getRunCache = reactive(new Map<string, SubagentRunRow>());

  /** Live transcript: per-runId transcript entries streamed in from
   *  `subagent:event` (debounced). Drawer reads this first when the
   *  worker is still running. On worker completion the backend
   *  persists the full transcript; the drawer falls back to the
   *  cache after `fetchRun` resolves. */
  const liveTranscript = reactive(new Map<string, TranscriptEntry[]>());

  /** Drawer open state. `null` = closed; a runId = open at that run.
   *  Drawer binds `open = computed(() => openRunId !== null)`. Single
   *  drawer at a time — opening run B closes run A (no nesting per
   *  PRD Out of Scope). */
  const openRunId = ref<string | null>(null);

  // -----------------------------------------------------------------------
  // Non-reactive debounce buffer
  // -----------------------------------------------------------------------

  /** Stage buffer for the 200ms debounce. Non-reactive (a plain
   *  `Map`) so SSE deltas don't trigger per-event re-renders. The
   *  debounce timer flushes this into `liveTranscript` (the reactive
   *  mirror) every `SUBAGENT_EVENT_DEBOUNCE_MS`. */
  const liveTranscriptBuffer = new Map<string, TranscriptEntry[]>();

  /** Pending debounce timer per runId. Cleared on flush. */
  const debounceTimers = new Map<string, ReturnType<typeof setTimeout>>();

  /** Tauri `listen` unlisten handle for `subagent:event`. Set by
   *  `start()`, torn down by `stop()`. */
  let unlisten: UnlistenFn | null = null;

  // -----------------------------------------------------------------------
  // API
  // -----------------------------------------------------------------------

  /** Load all worker summaries for a session. Replaces the cached
   *  array. Failure is logged + swallowed (the caller can show a
   *  toast if it cares; the store doesn't own toasts). */
  async function fetchForSession(sessionId: string): Promise<void> {
    try {
      const rows = await invoke<SubagentRunSummary[]>(
        "list_subagent_runs_by_session",
        { sessionId },
      );
      runSummaryBySession.set(sessionId, Array.isArray(rows) ? rows : []);
    } catch (e) {
      console.error("useSubagentRunsStore.fetchForSession failed:", e);
    }
  }

  /** Load the full row for a run (incl. transcriptJson). Writes
   *  `getRunCache` AND parses the transcript into `liveTranscript`
   *  so the drawer can fall back to the cached transcript when no
   *  live events have arrived yet (e.g. opening a completed worker).
   *  Does NOT overwrite a live transcript that already has entries
   *  (that would erase in-flight streaming progress). */
  async function fetchRun(runId: string): Promise<void> {
    try {
      const row = await invoke<SubagentRunRow | null>("get_subagent_run", {
        runId,
      });
      if (!row) return;
      getRunCache.set(runId, row);
      // Only seed liveTranscript if it's empty — once live events
      // start streaming we let them own the transcript.
      if (
        !liveTranscript.has(runId) ||
        (liveTranscript.get(runId)?.length ?? 0) === 0
      ) {
        const parsed = parseTranscriptJson(row.transcriptJson);
        if (parsed.length > 0) {
          liveTranscript.set(runId, parsed);
        }
      }
    } catch (e) {
      console.error("useSubagentRunsStore.fetchRun failed:", e);
    }
  }

  /** Open the drawer for a worker run. Sets `openRunId` + fetches
   *  the row if it isn't cached yet. */
  async function openDrawer(runId: string): Promise<void> {
    openRunId.value = runId;
    if (!getRunCache.has(runId)) {
      await fetchRun(runId);
    }
  }

  /** Close the drawer. Clears `openRunId` only — leaves caches
   *  intact so reopening is instant. */
  function closeDrawer(): void {
    openRunId.value = null;
  }

  /** Find the summary for a worker run by the dispatch_subagent
   *  tool_use's `id`. The backend formats the worker's rid as
   *  `"{parent_rid}-sub-{tool_use_id}"` (see
   *  `chat_loop.rs::run_subagent`), so we match summaries whose
   *  `parentRequestId` ends with `"-sub-" + toolUseId`. Returns
   *  `undefined` when no worker for this tool_use has been
   *  dispatched yet (e.g. the lookup ran before `fetchForSession`
   *  resolved). */
  function getSummaryByToolUseId(
    sessionId: string,
    toolUseId: string,
  ): SubagentRunSummary | undefined {
    const list = runSummaryBySession.get(sessionId);
    if (!list) return undefined;
    const suffix = `-sub-${toolUseId}`;
    return list.find((s) => s.parentRequestId.endsWith(suffix));
  }

  // -----------------------------------------------------------------------
  // IPC listener — `subagent:event` + 200ms debounce
  // -----------------------------------------------------------------------

  /** Route a live event into the debounce buffer + schedule a flush.
   *  The buffer is per-runId so multiple concurrent workers don't
   *  interleave (the drawer only shows one at a time, but the buffer
   *  still needs to preserve per-run ordering for when the user
   *  switches). */
  function routeEvent(event: SubagentEventPayload): void {
    // Convert the live payload's camelCase `payload` into the DB
    // storage shape `payload_json` so the drawer has a single
    // TranscriptEntry type to render. (See Drift trap 2 — the live
    // stream wraps the body as `payload`, transcriptJson stores it
    // as `payload_json`. We unify on the storage shape internally.)
    const entry: TranscriptEntry = {
      kind: event.kind,
      payload_json: event.payload,
    };
    const existing = liveTranscriptBuffer.get(event.runId) ?? [];
    existing.push(entry);
    liveTranscriptBuffer.set(event.runId, existing);
    scheduleFlush(event.runId);
  }

  /** Arm (or re-arm) the debounce timer for a runId. Self-implemented
   *  setTimeout — no lodash dependency (PRD decision #8). */
  function scheduleFlush(runId: string): void {
    const prev = debounceTimers.get(runId);
    if (prev !== undefined) {
      clearTimeout(prev);
    }
    const t = setTimeout(() => {
      debounceTimers.delete(runId);
      flushBuffer(runId);
    }, SUBAGENT_EVENT_DEBOUNCE_MS);
    debounceTimers.set(runId, t);
  }

  /** Commit the buffer for a runId into the reactive `liveTranscript`.
   *  Clears the buffer slot. */
  function flushBuffer(runId: string): void {
    const buffered = liveTranscriptBuffer.get(runId);
    if (!buffered || buffered.length === 0) {
      liveTranscriptBuffer.delete(runId);
      return;
    }
    const existing = liveTranscript.get(runId) ?? [];
    // Concat defensively — we never mutate the existing reactive
    // array in place (Vue's reactivity tracks `.set` on the Map,
    // not array push on a cached reference).
    liveTranscript.set(runId, [...existing, ...buffered]);
    liveTranscriptBuffer.delete(runId);
  }

  /** Mount the `subagent:event` listener. Idempotent — calling twice
   *  replaces the prior unlisten. Mirrors permissions.ts `start()`. */
  async function start(): Promise<void> {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    unlisten = await listen<SubagentEventPayload>("subagent:event", (event) => {
      routeEvent(event.payload);
    });
  }

  /** Tear down the listener + flush all pending buffers + clear
   *  timers. Does NOT clear the caches (the drawer may need them
   *  again on reopen). */
  function stop(): void {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    // Flush any pending buffered events so the user doesn't lose
    // the last batch when the component unmounts.
    for (const runId of [...debounceTimers.keys()]) {
      const t = debounceTimers.get(runId);
      if (t !== undefined) clearTimeout(t);
      debounceTimers.delete(runId);
      flushBuffer(runId);
    }
  }

  /** Drop all state for a session (e.g. on `deleteSession`). */
  function clearSession(sessionId: string): void {
    const list = runSummaryBySession.get(sessionId) ?? [];
    for (const s of list) {
      getRunCache.delete(s.id);
      liveTranscript.delete(s.id);
      liveTranscriptBuffer.delete(s.id);
      const t = debounceTimers.get(s.id);
      if (t !== undefined) {
        clearTimeout(t);
        debounceTimers.delete(s.id);
      }
    }
    runSummaryBySession.delete(sessionId);
    if (openRunId.value && list.some((s) => s.id === openRunId.value)) {
      openRunId.value = null;
    }
  }

  // -----------------------------------------------------------------------
  // Drawer-derived getters
  // -----------------------------------------------------------------------

  /** The currently open run's full row (from cache), or `undefined`.
   *  Drawer reads this for the header (status + summary + timestamps). */
  const openRun = computed<SubagentRunRow | undefined>(() => {
    const rid = openRunId.value;
    if (!rid) return undefined;
    return getRunCache.get(rid);
  });

  return {
    // reactive state
    runSummaryBySession,
    getRunCache,
    liveTranscript,
    openRunId,
    openRun,
    // actions
    fetchForSession,
    fetchRun,
    openDrawer,
    closeDrawer,
    getSummaryByToolUseId,
    clearSession,
    // lifecycle
    start,
    stop,
  };
});
