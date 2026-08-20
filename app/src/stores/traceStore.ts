// E2 (harness trace pipeline, 2026-07-14) — useTraceStore.
//
// The trace store is the frontend-side reactive wrapper for the
// per-turn trace viewer (child-2). It owns:
//
//   1. `currentSessionTraces: Map<seq, TurnTrace>` — the live +
//      回看 unified in-memory state. Keyed by the per-session
//      turn seq. Both the streamController's live event handler
//      (F2) and `loadHistory` (F3) write into this Map.
//
//   2. `panelOpen: boolean` — the right-drawer visibility gate.
//      Wired to AppHeader's trace toggle (F5) and the TracePanel
//      component's close button.
//
//   3. `loading` / `error` — `loadHistory` is the single IPC
//      surface; the renderer's loading skeleton reads `loading`,
//      and IPC failures set `error` for an inline error chip.
//
// API surface:
//
//   - `applyEvent(event)` — live path entry. The
//     streamController dispatches the 3 new ChatEvent variants
//     here. Internal switch: context_compacted → upsert
//     `compaction`; loop_hint → upsert `loopHint`;
//     workflow_breadcrumb → upsert `breadcrumb`.
//
//   - `loadHistory(sessionId)` — 回看 path. Fetches
//     `list_turn_traces` + `list_session_audit_events`, parses
//     the rows, and groups audit rows by `turnSeq` onto the
//     matching `TurnTrace.auditEvents`. A `turnSeq === null` row
//     (pre-v7 historical) lands in a virtual "ungrouped" bucket
//     rendered as a footer block on the timeline.
//
//   - `clearSessionTrace(sessionId)` — IPC for the "清理" button.
//     Calls `clear_session_trace` then `loadHistory(sessionId)`
//     to refresh. The TracePanel gates this behind ConfirmDialog.
//
//   - `resetForNewSession(sessionId)` — fired on session switch
//     / `startRequest`. Clears `currentSessionTraces` and runs
//     `loadHistory(sessionId)` for the new session. Mirrors the
//     `useMemoryStore().clearRecallHits(sid)` pattern in
//     `startRequest` (F2 in streamController).
//
//   - `setPanelOpen(open)` / `togglePanel()` — UI gate.

import { defineStore } from "pinia";
import { reactive, ref } from "vue";
import { transport } from "../transport";
import { extractErrorMessage } from "../utils/useErrorBus";
import type { AuditEventRow } from "../utils/audit";
import {
  type TurnTrace,
  type TurnTraceRow,
  type ContextCompactedEvent,
  type LoopHintEvent,
  type WorkflowBreadcrumbEvent,
  type BudgetTrimEvent,
  type TurnUsageEvent,
  parseTurnTraceRow,
} from "../types/turnTrace";

/** Virtual seq used for audit events that have no `turnSeq`
 *  (pre-v7 historical rows, or IPC-handler audits). Rendered
 *  as a footer block under the timeline ("无 turn 上下文的
 *  审计事件"). Using `Number.MAX_SAFE_INTEGER` (instead of a
 *  string sentinel) keeps the Map's key type uniform. */
const UNGROUPED_SEQ = Number.MAX_SAFE_INTEGER;

export const useTraceStore = defineStore("trace", () => {
  // -----------------------------------------------------------------------
  // State
  // -----------------------------------------------------------------------

  /** Per-session turn traces. Outer Map is `reactive` so .set /
   *  .delete trigger UI updates. Inner TurnTrace objects are
   *  plain objects (mutated by the store actions, not by the
   *  component) — Vue's reactive(Map) tracks .get/.has for
   *  template-side reads; per-field updates on a TurnTrace
   *  require either replacing the object via `.set(key, {...})`
   *  (used in `applyEvent` for simplicity) or wrapping the
   *  TurnTrace in `reactive()` (NOT done here — see the
   *  `recallHitsBySession` pattern note below). The current
   *  per-turn writes always go through `applyEvent`, which
   *  uses the `.set` path, so the lack of inner reactive
   *  wrapping is invisible to the renderer.
   *
   *  Why a top-level `reactive(new Map())` and not `ref(new
   *  Map())`: mirrors the `recallHitsBySession` / `liveTranscript`
   *  pattern in memory / subagentRuns. The trade-off is that
   *  `store.currentSessionTraces` is a reactive proxy of the
   *  Map, and any code that mutates the Map from inside a
   *  computed getter would trigger the recursion guard (see
   *  `state-management.md` RULE on computed-must-not-mutate).
   *  The component here reads via `store.getTracesForSession`
   *  (a plain function, no computed), so the constraint is
   *  satisfied. */
  const currentSessionTraces = reactive(new Map<number, TurnTrace>());

  /** The sessionId these traces are scoped to. Set on
   *  `loadHistory` / `resetForNewSession`; used to decide
   *  whether incoming `applyEvent` events belong to the
   *  active session (defensive — a stale event from a
   *  non-current session would otherwise pollute the timeline
   *  of a different session). */
  const currentSessionId = ref<string | null>(null);

  /** Drawer visibility. AppHeader toggle writes `true`;
   *  TracePanel close button writes `false`. */
  const panelOpen = ref<boolean>(false);

  /** `loadHistory` busy state. The renderer's loading
   *  skeleton reads this. */
  const loading = ref<boolean>(false);

  /** `loadHistory` error message (or `null`). On failure
   *  the renderer shows an inline error chip with a "重试"
   *  button that re-invokes `loadHistory(lastSessionId)`. */
  const error = ref<string | null>(null);

  // -----------------------------------------------------------------------
  // Live path — `applyEvent`
  // -----------------------------------------------------------------------

  /** Apply a live trace ChatEvent to the in-memory Map. The
   *  streamController dispatches here from the 3 new event
   *  cases in `handleChatEvent`. The function is a pure
   *  upsert — same `seq` may be re-applied safely (e.g. a
   *  `context_compacted` lands first, then a `loop_hint` for
   *  the same turn, then a `workflow_breadcrumb`); the
   *  Map entry accumulates dimensions without overwriting
   *  unrelated fields.
   *
   *  Defensive: the function only mutates the Map entry whose
   *  key matches `event.seq`. If the current session differs
   *  from `event.request_id`'s session (we don't have session
   *  on the event payload — it lives on the `activeRequests`
   *  entry in streamController, not on `ChatEvent`), the
   *  store accepts the write; the upstream
   *  `activeRequests.get(request_id)` filter in the controller
   *  is the authoritative gate. */
  function applyContextCompacted(event: ContextCompactedEvent): void {
    const existing = currentSessionTraces.get(event.seq) ?? {
      id: 0,
      sessionId: currentSessionId.value ?? "",
      seq: event.seq,
      createdAt: new Date().toISOString(),
    };
    const next: TurnTrace = {
      ...existing,
      compaction: {
        tokens_before: event.tokens_before,
        tokens_after: event.tokens_after,
        dropped_count: event.dropped_count,
        degradation: event.degradation,
        // 08-18 PR2:压缩路径徽标数据(live 路径;summary_usage 只在
        // DB 回看路径的 compaction_json 里)。
        method: event.method ?? "none",
      },
    };
    currentSessionTraces.set(event.seq, next);
  }

  function applyLoopHint(event: LoopHintEvent): void {
    const existing = currentSessionTraces.get(event.seq) ?? {
      id: 0,
      sessionId: currentSessionId.value ?? "",
      seq: event.seq,
      createdAt: new Date().toISOString(),
    };
    const next: TurnTrace = {
      ...existing,
      loopHint: {
        hit_count: event.hit_count,
        verdict_kind: event.verdict_kind,
      },
    };
    currentSessionTraces.set(event.seq, next);
  }

  function applyWorkflowBreadcrumb(event: WorkflowBreadcrumbEvent): void {
    const existing = currentSessionTraces.get(event.seq) ?? {
      id: 0,
      sessionId: currentSessionId.value ?? "",
      seq: event.seq,
      createdAt: new Date().toISOString(),
    };
    const next: TurnTrace = {
      ...existing,
      breadcrumb: {
        task_slug: event.task_slug,
        status: event.status,
        breadcrumb_text: event.breadcrumb_text,
      },
    };
    currentSessionTraces.set(event.seq, next);
  }

  // unified-context-budget WP2 (2026-08-19): 关卡⑤裁剪观察(live)。
  function applyBudgetTrim(event: BudgetTrimEvent): void {
    const existing = currentSessionTraces.get(event.seq) ?? {
      id: 0,
      sessionId: currentSessionId.value ?? "",
      seq: event.seq,
      createdAt: new Date().toISOString(),
    };
    const next: TurnTrace = {
      ...existing,
      budgetTrim: {
        freed_tokens: event.freed_tokens,
        post_total: event.post_total,
        window: event.window,
      },
    };
    currentSessionTraces.set(event.seq, next);
  }

  // 08-20-turn-usage-event-quota-view WP1: per-turn token 观察(live)。
  // 切片 null → undefined 归一化(TurnTrace 的字段语义是
  // "undefined = never written",与 parseTurnTraceRow 的回看路径一致),
  // compaction/loopHint/breadcrumb 等已落维度经 spread 保留(merge)。
  function applyTurnUsage(event: TurnUsageEvent): void {
    const existing = currentSessionTraces.get(event.seq) ?? {
      id: 0,
      sessionId: currentSessionId.value ?? "",
      seq: event.seq,
      createdAt: new Date().toISOString(),
    };
    const next: TurnTrace = {
      ...existing,
      tokenUsage: event.usage,
      toolsToken: event.tools_token ?? undefined,
      memoryToken: event.memory_token ?? undefined,
      imagesToken: event.images_token ?? undefined,
      atFilesToken: event.at_files_token ?? undefined,
      systemToken: event.system_token ?? undefined,
      contextWindow: event.context_window,
    };
    currentSessionTraces.set(event.seq, next);
  }

  /** Public dispatcher — used by streamController. Returns
   *  `true` if the event was handled, `false` if the kind is
   *  not a trace event (defensive — the streamController
   *  already filters to the 3 known kinds before calling). */
  function applyEvent(
    event:
      | ContextCompactedEvent
      | LoopHintEvent
      | WorkflowBreadcrumbEvent
      | BudgetTrimEvent
      | TurnUsageEvent,
  ): void {
    switch (event.kind) {
      case "context_compacted":
        applyContextCompacted(event);
        break;
      case "loop_hint":
        applyLoopHint(event);
        break;
      case "workflow_breadcrumb":
        applyWorkflowBreadcrumb(event);
        break;
      case "budget_trim":
        applyBudgetTrim(event);
        break;
      case "turn_usage":
        applyTurnUsage(event);
        break;
    }
  }

  // -----------------------------------------------------------------------
  // 回看 path — `loadHistory`
  // -----------------------------------------------------------------------

  /** Load the per-session trace history. Replaces the entire
   *  `currentSessionTraces` Map (the previous session's
   *  entries are dropped — the timeline is per-session, not
   *  cross-session). Fetches BOTH the `turn_trace` table AND
   *  the audit events in parallel (the two IPCs are
   *  independent; a single `Promise.all` saves a round trip).
   *
   *  Audit events are grouped by `turnSeq`. A row with
   *  `turnSeq === null` (pre-v7 historical) lands in the
   *  virtual UNGROUPED_SEQ bucket — the renderer's
   *  TurnTimeline can render that bucket as a footer
   *  ("无 turn 上下文的审计事件") or skip it. MVP scope per
   *  PRD: render the ungrouped bucket as the last timeline
   *  card with a "未关联 turn" label, so the user sees the
   *  historical rows without losing them.
   *
   *  Failure policy: on error, sets `error` and leaves the
   *  Map at the previous value (defensive — the user can
   *  still see the previous session's traces; the new
   *  session's load was a no-op). The renderer's "重试"
   *  button calls `loadHistory(currentSessionId)` again. */
  async function loadHistory(sessionId: string): Promise<void> {
    loading.value = true;
    error.value = null;
    currentSessionId.value = sessionId;
    try {
      const [turnRows, auditRows] = await Promise.all([
        transport.invoke<TurnTraceRow[]>("list_turn_traces", { sessionId }),
        transport.invoke<AuditEventRow[]>("list_session_audit_events", {
          sessionId,
        }),
      ]);
      // Wipe and rebuild — a session switch is a hard
      // boundary, not a merge.
      currentSessionTraces.clear();
      // 1) Build the per-seq TurnTrace entries.
      for (const row of turnRows) {
        currentSessionTraces.set(row.seq, parseTurnTraceRow(row));
      }
      // 2) Group audit rows by turnSeq. Rows whose turnSeq
      //    is `null` go to UNGROUPED_SEQ.
      for (const audit of auditRows) {
        const key =
          audit.turnSeq === null || audit.turnSeq === undefined
            ? UNGROUPED_SEQ
            : audit.turnSeq;
        const existing = currentSessionTraces.get(key);
        if (existing) {
          // Common case: the turn has both a trace row and
          // some audit rows. Append to its auditEvents.
          const events = existing.auditEvents ?? [];
          events.push(audit);
          currentSessionTraces.set(key, {
            ...existing,
            auditEvents: events,
          });
        } else {
          // Audit-only turn (no `turn_trace` row — e.g. a
          // session that pre-dates the v7 migration and
          // only has audit data, or a turn whose
          // `persist_turn` succeeded but whose
          // `token_usage` UPSERT failed and the rest of
          // the trace dimensions were never written).
          // Synthesize a stub so the audit rows still
          // surface on the timeline.
          currentSessionTraces.set(key, {
            id: 0,
            sessionId,
            seq: key,
            createdAt: audit.ts,
            auditEvents: [audit],
          });
        }
      }
    } catch (e) {
      error.value =
        e instanceof Error ? e.message : extractErrorMessage(e);
    } finally {
      loading.value = false;
    }
  }

  /** Clear the in-memory state. Called from
   *  `resetForNewSession` before the new session's history
   *  lands. Exposed for tests (the `currentSessionId.value
   *  = null` part is the production path; the action is a
   *  convenience). */
  function clearTraces(): void {
    currentSessionTraces.clear();
    currentSessionId.value = null;
  }

  /** Switch to a new session. Wipes the timeline and reloads
   *  from DB. Called from:
   *  - `streamController.startRequest` (F2 — start a fresh
   *    user turn; the new turn's events will land in the
   *    Map once the live `applyEvent` calls arrive).
   *  - The session-switch watcher (if any; MVP doesn't
   *    wire one — the user opens the trace panel after
   *    switching, which triggers `loadHistory` via
   *    `TracePanel`'s mount-time watcher). */
  async function resetForNewSession(sessionId: string): Promise<void> {
    clearTraces();
    await loadHistory(sessionId);
  }

  // -----------------------------------------------------------------------
  // Cleanup path — `clearSessionTrace`
  // -----------------------------------------------------------------------

  /** Delete all `turn_trace` rows for `sessionId` (the trace
   *  viewer's "清理" button). The matching audit rows are
   *  NOT deleted — the trace viewer's cleanup is a partial
   *  reset (the user can keep the audit log if they want).
   *  The audit `turn_seq` column on rows that referenced
   *  the deleted trace rows stays as-is (just dangles);
   *  the audit log itself remains intact. */
  async function clearSessionTrace(sessionId: string): Promise<void> {
    try {
      await transport.invoke<void>("clear_session_trace", { sessionId });
    } catch (e) {
      const msg =
        e instanceof Error ? e.message : extractErrorMessage(e);
      error.value = msg;
      throw e;
    }
    // Refresh — drop the in-memory trace rows (audit rows
    // stay, they were not deleted). The simplest
    // implementation: re-run `loadHistory`, which will pull
    // the now-empty `turn_trace` set and re-attach the audit
    // rows to a fresh Map. Defensive against a stale
    // local copy.
    await loadHistory(sessionId);
  }

  // -----------------------------------------------------------------------
  // UI gate — `panelOpen`
  // -----------------------------------------------------------------------

  function setPanelOpen(open: boolean): void {
    panelOpen.value = open;
  }

  function togglePanel(): void {
    panelOpen.value = !panelOpen.value;
  }

  // -----------------------------------------------------------------------
  // Getters
  // -----------------------------------------------------------------------

  /** Sorted timeline view of the current session's traces.
   *  Used by `<TurnTimeline>` to render the seq-ASC list. The
   *  sort materializes a fresh array on each call (small N
   *  per session — max ~200 turns — so the cost is
   *  negligible). For long sessions the underlying Map's
   *  insertion order matches the seq ASC order (the
   *  `list_turn_traces` SQL returns `ORDER BY seq ASC` and
   *  the live path uses `applyEvent` which sets keys in seq
   *  ASC order in practice). */
  function tracesForCurrentSession(): TurnTrace[] {
    const out: TurnTrace[] = [];
    for (const t of currentSessionTraces.values()) {
      if (t.seq === UNGROUPED_SEQ) continue;
      out.push(t);
    }
    out.sort((a, b) => a.seq - b.seq);
    return out;
  }

  /** Audit rows with no `turnSeq` (the virtual ungrouped
   *  bucket). Rendered as a footer block by `<TurnTimeline>`. */
  function ungroupedAuditEvents(): AuditEventRow[] {
    const stub = currentSessionTraces.get(UNGROUPED_SEQ);
    return stub?.auditEvents ?? [];
  }

  // -----------------------------------------------------------------------
  // Re-export the audit store so callers can use it from the same
  // import path (used by `TracePanel` for the audit modal link).
  // -----------------------------------------------------------------------
  // (E2 trace check 2026-07-14: removed the unused `useAudit()` re-export
  // — it had zero call sites and only served as a stale alias for
  // `useAuditStore`. Callers can import `useAuditStore` directly from
  // `./audit` if needed.)

  return {
    // state
    currentSessionTraces,
    currentSessionId,
    panelOpen,
    loading,
    error,
    // actions
    applyEvent,
    loadHistory,
    clearTraces,
    resetForNewSession,
    clearSessionTrace,
    setPanelOpen,
    togglePanel,
    // getters
    tracesForCurrentSession,
    ungroupedAuditEvents,
  };
});

export { UNGROUPED_SEQ };
