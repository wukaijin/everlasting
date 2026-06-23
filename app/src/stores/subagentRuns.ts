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
// File layout (post 06-23-06-23-split-subagent-runs): this module was
// split into 3 files — types live in `./subagentRuns.types`, the
// `RunAccumulator` class + its parsers (`parseTranscriptJson`,
// `buildSectionsFromRaw`, the chat_event field coercers) live in
// `./runAccumulator`. This file keeps the Pinia store + the
// `coerceStatus` wire→enum helper. Dependency direction (one-way):
// subagentRuns.ts → runAccumulator.ts → subagentRuns.types.ts.
//
// 2026-06-21 redesign (PR2 of the subagent-drawer refactor): the
// store also runs a per-runId `RunAccumulator` (in `./runAccumulator`)
// that collapses the raw `chat_event` SSE chunk stream into
// `Thinking | Text` segments
// (so the drawer can render the worker's intermediate state the
// same way the main chat panel does, without exposing 6963
// meaningless chat_event rows). The accumulator lives in
// `liveSections: Map<runId, TranscriptSection[]>` (R22); the raw
// `liveTranscript` is preserved unchanged for the pairing layer
// (call+result merge) that powers the drawer's per-tool cards.
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
//   3. The raw `chat_event` payload carries a NESTED `ChatEvent`
//      whose `kind` discriminates between `delta` / `thinking_delta`
//      / `signature_delta` / `redacted_thinking_delta` / `done` /
//      `error` / `start` / `tool_call` / `tool_result`. The
//      accumulator dispatches on the INNER kind to route to the
//      Thinking vs. Text segment. See `RunAccumulator` (in
//      `./runAccumulator`) for the discriminator rules.

import { defineStore } from "pinia";
import { reactive, computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  SUBAGENT_EVENT_DEBOUNCE_MS,
  type SubagentEventPayload,
  type SubagentFinishedPayload,
  type SubagentRunRow,
  type SubagentRunSummary,
  type SubagentStatus,
  type TranscriptEntry,
  type TranscriptSection,
} from "./subagentRuns.types";
import { RunAccumulator, parseTranscriptJson } from "./runAccumulator";

// -----------------------------------------------------------------------
// Helpers — cross-layer drift trap fixes
// -----------------------------------------------------------------------

/** Coerce a raw status string (from `SubagentRunRow.status` or a
 *  malformed `SubagentRunSummary.status`) into the typed union.
 *  Unknown strings fall back to `"running"` (matches the Rust
 *  `SubagentStatusDb::from_str_opt` lenient-parse default). The
 *  5-variant union (incl. `"incomplete"`) mirrors the backend
 *  `SubagentStatusDb` enum (Session 60 R2, 2026-06-21); missing
 *  it here previously caused incomplete runs to render as
 *  "运行中" forever (RULE-FrontSubagent-005). */
export function coerceStatus(raw: string): SubagentStatus {
  if (
    raw === "running" ||
    raw === "completed" ||
    raw === "cancelled" ||
    raw === "error" ||
    raw === "incomplete"
  ) {
    return raw;
  }
  return "running";
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

  /** B6 redesign PR2 (2026-06-21): per-runId `RunAccumulator`
   *  instance. Created lazily on first event / rebuild. The
   *  accumulator's internal `transcript: shallowRef` is what the
   *  store copies into `liveSections` on debounce flush. The
   *  Map is non-reactive (plain `Map`) — the per-runId
   *  `liveSections` Map is the reactive surface the drawer
   *  subscribes to. */
  const accumulators = new Map<string, RunAccumulator>();

  /** B6 redesign PR2: per-runId derived `TranscriptSection[]`,
   *  the drawer's 5-segment grouped view source. Mirrors
   *  `liveTranscript` but each entry is a post-accumulator
   *  section (Thinking / Text / FinalText / ToolCall /
   *  ToolResult / PermissionAsk) — the chat_event SSE chunk
   *  stream is collapsed into Thinking / Text segments rather
   *  than exposed verbatim. The drawer reads this Map first
   *  when the worker is running; falls back to a
   *  `buildSectionsFromRaw(parseTranscriptJson(transcriptJson))`
   *  computation when no live sections exist yet. */
  const liveSections = reactive(new Map<string, TranscriptSection[]>());

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

  /** Tauri `listen` unlisten handle for `subagent:finished`
   *  (B6 PR3b hotfix, 2026-06-21). Separate from `unlisten` so the
   *  two channels' lifecycles stay independent and `stop()` tears
   *  both down. */
  let unlistenFinished: UnlistenFn | null = null;

  /** B6 PR3b (2026-06-20): dedup Set for the eager-fetch path in the
   *  `subagent:event` listener. A burst of events for the same runId
   *  (e.g. a tool_call arriving milliseconds apart from a tool_result)
   *  must NOT fire `fetchRun` / `fetchForSession` more than once —
   *  the IPC roundtrip is cheap but the cost adds up on a busy worker.
   *  Non-reactive (plain Set) so per-event checks don't trigger Vue
   *  effect re-evaluation. Lives for the lifetime of the store (not
   *  cleared on `stop()` — the cache survives component unmount so the
   *  dedup should too). Bounded by the number of distinct runIds seen
   *  in this app session; not a memory concern for realistic usage. */
  const eagerFetchedRunIds = new Set<string>();

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
   *  (that would erase in-flight streaming progress).
   *
   *  B6 redesign PR2 (2026-06-21): also rebuilds the per-runId
   *  accumulator from `row.transcriptJson` + `row.finalText` and
   *  publishes the derived `TranscriptSection[]` to `liveSections`
   *  (R22 — worker finished → fetchRun → rebuildFromCache replaces
   *  the in-memory transcript with the authoritative DB-cached
   *  version). If the worker is still streaming (live transcript
   *  non-empty), the rebuild is skipped to avoid losing in-flight
   *  progress — the live path keeps owning the transcript until
   *  `subagent:finished` triggers a fresh fetchRun (which sees
   *  the now-terminal state and rebuilds). */
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
      // Rebuild the accumulator's derived sections. If the live
      // transcript is non-empty (worker still streaming), this
      // call's sections are dropped by the "first publish wins"
      // rule in `routeEvent`'s `feed` path. The post-`finished`
      // path flushes the live buffer first (subagent:finished
      // handler) and then calls fetchRun — at which point the
      // live transcript is non-empty, but the
      // `subagent:finished` handler's flushBuffer call has
      // cleared the live buffer and the new rebuild IS the
      // authoritative source.
      const acc = accumulators.get(runId) ?? new RunAccumulator();
      acc.rebuildFromCache(row.transcriptJson, row.finalText ?? null);
      accumulators.set(runId, acc);
      // Publish the rebuilt sections to `liveSections`. The
      // shallowRef inside the accumulator now points to the
      // fresh post-rebuild array; we copy that identity into
      // the reactive Map.
      publishAccumulator(runId);
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
   *  switches).
   *
   *  B6 redesign PR2 (2026-06-21): the entry is ALSO fed to the
   *  per-runId `RunAccumulator` (R8). The accumulator's live
   *  `transcript` shallowRef carries the post-collapse sections;
   *  the store copies it into `liveSections` on debounce flush. The
   *  raw `TranscriptEntry[]` is still preserved in `liveTranscript`
   *  for the pairing layer (call+result merge). */
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
    // Feed the accumulator live (R20: O(1) per event, in-place
    // mutation of the active segment). The accumulator's
    // `transcript` shallowRef is read on flush and copied to
    // `liveSections` (the reactive surface the drawer subscribes
    // to). We do NOT read the shallowRef on every event — the
    // debounce flush batches the writes to one Map.set per 200ms
    // (matches the previous 200ms cadence; R22 "live phase does
    // NOT run full accumulator").
    const acc = accumulators.get(event.runId) ?? new RunAccumulator();
    acc.feed(entry);
    accumulators.set(event.runId, acc);
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

  /** Commit the buffer for a runId into the reactive
   *  `liveTranscript`. Clears the buffer slot. Also copies the
   *  accumulator's current `transcript: shallowRef` into
   *  `liveSections` so the drawer's 5-segment view picks up the
   *  post-collapse sections. B6 redesign PR2. */
  function flushBuffer(runId: string): void {
    const buffered = liveTranscriptBuffer.get(runId);
    if (!buffered || buffered.length === 0) {
      liveTranscriptBuffer.delete(runId);
      // The accumulator may have mutated even with no buffered
      // events (e.g. eager-fetch path that doesn't feed events).
      // Still publish the current `transcript` value to keep
      // the drawer's `liveSections` in sync.
      publishAccumulator(runId);
      return;
    }
    const existing = liveTranscript.get(runId) ?? [];
    // Concat defensively — we never mutate the existing reactive
    // array in place (Vue's reactivity tracks `.set` on the Map,
    // not array push on a cached reference).
    liveTranscript.set(runId, [...existing, ...buffered]);
    liveTranscriptBuffer.delete(runId);
    publishAccumulator(runId);
  }

  /** Copy the per-runId accumulator's `transcript: shallowRef`
   *  into the reactive `liveSections` Map. Called on every
   *  debounce flush + on `rebuildFromCache`. The Map is the
   *  reactive surface the drawer subscribes to; the shallowRef
   *  itself is not. R20 / R22. */
  function publishAccumulator(runId: string): void {
    const acc = accumulators.get(runId);
    if (!acc) return;
    const sections = acc.transcript.value;
    // Defensive: skip the publish if the section list is
    // identical to the prior publish (avoids spurious
    // re-renders when no events have been fed since the
    // last flush). The shallowRef .value identity is the
    // ground truth — the accumulator replaces .value on
    // every appendSection, so identity is the cheapest
    // check. We do NOT deep-compare.
    const prior = liveSections.get(runId);
    if (prior === sections) return;
    liveSections.set(runId, sections);
  }

  /** Mount the `subagent:event` listener. Idempotent — calling twice
   *  replaces the prior unlisten. Mirrors permissions.ts `start()`.
   *
   *  B6 PR3b (2026-06-20): the listener ALSO fires an eager-fetch
   *  on the first event for any new runId. This fixes the
   *  dispatch_subagent card race — see PR3b PRD §"Root cause".
   *  Without this, the ToolCallCard's `getSummaryByToolUseId`
   *  lookup may stay empty for the entire worker lifetime if the
   *  initial `fetchForSession` IPC roundtrip races against the
   *  backend's `insert_run`. By the time the first `subagent:event`
   *  arrives, `insert_run` has definitely committed (the sink is
   *  constructed AFTER the row insert), so the eager-fetch is
   *  guaranteed to see the row. */
  async function start(): Promise<void> {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    unlisten = await listen<SubagentEventPayload>("subagent:event", (event) => {
      const e = event.payload;
      routeEvent(e);
      // Eager-fetch: warm the run-detail cache + session-summary
      // cache the first time we see a runId. Dedup'd by the
      // `eagerFetchedRunIds` Set so burst events don't re-fetch.
      // `fetchRun` and `fetchForSession` are fire-and-forget here —
      // they're independent of the routeEvent debounce path, and a
      // failure to warm the cache just falls back to the existing
      // ToolCallCard click-time retry (which polls fetchForSession
      // for up to 1.5s before giving up).
      if (!eagerFetchedRunIds.has(e.runId)) {
        eagerFetchedRunIds.add(e.runId);
        void fetchRun(e.runId);
        void fetchForSession(e.sessionId);
      }
    });
    // Bug2 fix (2026-06-21): listen for the one-shot terminal signal
    // emitted by `run_subagent` after `update_run_finished` commits.
    // On receipt, flush any buffered transcript events for the run
    // (so `liveTranscript` is complete before `fetchRun`'s seed-guard
    // checks it) then refetch the run detail (drawer source:
    // terminal status + finishedAt + full transcript) + session
    // summary (card source: status). This flips the drawer / card
    // from `running` to the terminal state without polling. NOT
    // dedup'd by `eagerFetchedRunIds` — the terminal signal is
    // one-shot by definition.
    if (unlistenFinished) {
      unlistenFinished();
      unlistenFinished = null;
    }
    unlistenFinished = await listen<SubagentFinishedPayload>(
      "subagent:finished",
      (event) => {
        const f = event.payload;
        flushBuffer(f.runId);
        void fetchRun(f.runId);
        void fetchForSession(f.sessionId);
      },
    );
  }

  /** Tear down the listener + flush all pending buffers + clear
   *  timers. Does NOT clear the caches (the drawer may need them
   *  again on reopen). */
  function stop(): void {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    if (unlistenFinished) {
      unlistenFinished();
      unlistenFinished = null;
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
      // B6 redesign PR2: also drop the per-runId accumulator
      // + the derived `liveSections` entry. The accumulator
      // is plain JS (no reactive resources), so just `delete`
      // on the Map. `liveSections` IS reactive — the delete
      // triggers a Map-level re-render.
      accumulators.delete(s.id);
      liveSections.delete(s.id);
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
    // B6 redesign PR2: derived sections (chat_event stream
    // collapsed into Thinking/Text segments). The drawer's
    // 5-segment grouped view reads this; falls back to
    // `buildSectionsFromRaw(parseTranscriptJson(...))` for
    // the cold-cache path. Kept as a separate Map from
    // `liveTranscript` so the pairing layer (which needs
    // the raw entries) and the segmented view (which needs
    // the collapsed sections) don't fight over the same
    // data.
    liveSections,
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
