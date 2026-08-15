// E2 (harness trace pipeline, 2026-07-14) — tests for the trace
// store. Locks the live + 回看 unification + the cleanup path.
//
// Coverage:
//   1. `applyEvent` — live path. 3 new ChatEvent variants each
//      upsert into the right `TurnTrace` sub-object (compaction /
//      loopHint / breadcrumb). Same `seq` re-applied accumulates
//      without overwriting other dimensions.
//   2. `loadHistory` — 回看 path. Both `turn_trace` rows AND
//      `session_audit_events` rows are fetched in parallel,
//      parsed, and grouped by `turnSeq`. `turnSeq === null` rows
//      land in the virtual UNGROUPED_SEQ bucket.
//   3. `clearSessionTrace` — invokes the IPC, refreshes the
//      local state via `loadHistory`. Failure surfaces on the
//      `error` ref and rethrows.
//   4. `resetForNewSession` — clears the in-memory Map and
//      reloads from DB. Session-switch invariant: a different
//      session's traces do NOT bleed into the new session's
//      timeline.
//   5. `setPanelOpen` / `togglePanel` — UI gate.
//
// Tauri IPC is mocked so the suite runs in jsdom. The mocks
// follow the same file-level `vi.mock` pattern as
// `app/src/stores/memory.test.ts`.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));


import { useTraceStore, UNGROUPED_SEQ } from "./traceStore";
import type { TurnTraceRow } from "../types/turnTrace";
import type { AuditEventRow } from "../utils/audit";

function makeTurnTraceRow(
  seq: number,
  overrides: Partial<TurnTraceRow> = {},
): TurnTraceRow {
  return {
    id: seq,
    sessionId: "sess-1",
    seq,
    tokenUsageJson: null,
    compactionJson: null,
    loopHintJson: null,
    breadcrumbJson: null,
    toolsToken: null,
    memoryToken: null,
    createdAt: "2026-07-14 12:00:00",
    ...overrides,
  };
}

function makeAuditRow(
  id: number,
  kind: string,
  turnSeq: number | null,
  payloadJson: string | null = null,
  ts: string = "2026-07-14 12:00:01",
): AuditEventRow {
  return {
    id,
    sessionId: "sess-1",
    ts,
    kind,
    payloadJson,
    turnSeq,
  };
}

describe("useTraceStore — applyEvent (live path)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("upserts a context_compacted event into the compaction field", () => {
    const store = useTraceStore();
    store.currentSessionId = "sess-1";
    store.applyEvent({
      kind: "context_compacted",
      request_id: "rid-1",
      seq: 1,
      tokens_before: 5000,
      tokens_after: 3000,
      dropped_count: 5,
      degradation: "none",
    });
    const t = store.currentSessionTraces.get(1);
    expect(t).toBeDefined();
    expect(t?.compaction).toEqual({
      tokens_before: 5000,
      tokens_after: 3000,
      dropped_count: 5,
      degradation: "none",
    });
  });

  it("upserts a loop_hint event into the loopHint field", () => {
    const store = useTraceStore();
    store.currentSessionId = "sess-1";
    store.applyEvent({
      kind: "loop_hint",
      request_id: "rid-1",
      seq: 1,
      hit_count: 2,
      verdict_kind: "soft",
    });
    const t = store.currentSessionTraces.get(1);
    expect(t?.loopHint).toEqual({
      hit_count: 2,
      verdict_kind: "soft",
    });
  });

  it("upserts a workflow_breadcrumb event with null task_slug", () => {
    const store = useTraceStore();
    store.currentSessionId = "sess-1";
    store.applyEvent({
      kind: "workflow_breadcrumb",
      request_id: "rid-1",
      seq: 1,
      task_slug: null,
      status: null,
      breadcrumb_text: "<workflow-task-meta>bootstrap</workflow-task-meta>",
    });
    const t = store.currentSessionTraces.get(1);
    expect(t?.breadcrumb).toEqual({
      task_slug: null,
      status: null,
      breadcrumb_text: "<workflow-task-meta>bootstrap</workflow-task-meta>",
    });
  });

  it("accumulates dimensions on the same seq without overwriting", () => {
    // The 3 dimensions live in different sub-objects; the
    // store's `apply*` helpers each read the existing entry
    // and spread, so a second event for the same seq
    // accumulates. Locks the UPSERT-shaped data path.
    const store = useTraceStore();
    store.currentSessionId = "sess-1";
    store.applyEvent({
      kind: "context_compacted",
      request_id: "rid-1",
      seq: 1,
      tokens_before: 5000,
      tokens_after: 3000,
      dropped_count: 5,
      degradation: "none",
    });
    store.applyEvent({
      kind: "loop_hint",
      request_id: "rid-1",
      seq: 1,
      hit_count: 2,
      verdict_kind: "soft",
    });
    const t = store.currentSessionTraces.get(1);
    expect(t?.compaction).toBeDefined();
    expect(t?.loopHint).toBeDefined();
    // Neither field is nulled by the second write.
    expect(t?.compaction?.dropped_count).toBe(5);
    expect(t?.loopHint?.hit_count).toBe(2);
  });
});

describe("useTraceStore — loadHistory (回看 path)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("parses turn_trace rows into typed TurnTrace entries (camelCase JSON)", () => {
    const store = useTraceStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_turn_traces") {
        return [
          makeTurnTraceRow(1, {
            tokenUsageJson: JSON.stringify({
              input_tokens: 100,
              output_tokens: 50,
              cache_creation_input_tokens: 10,
              cache_read_input_tokens: 20,
              context_input_tokens: 130,
            }),
            compactionJson: JSON.stringify({
              tokens_before: 5000,
              tokens_after: 3000,
              dropped_count: 5,
              degradation: "none",
            }),
          }),
          makeTurnTraceRow(2, {
            loopHintJson: JSON.stringify({
              hit_count: 1,
              verdict_kind: "soft",
            }),
          }),
        ];
      }
      if (cmd === "list_session_audit_events") return [];
      return null;
    });
    return store.loadHistory("sess-1").then(() => {
      const traces = store.tracesForCurrentSession();
      expect(traces).toHaveLength(2);
      expect(traces[0].seq).toBe(1);
      expect(traces[0].tokenUsage?.input_tokens).toBe(100);
      expect(traces[0].compaction?.dropped_count).toBe(5);
      expect(traces[1].seq).toBe(2);
      expect(traces[1].loopHint?.hit_count).toBe(1);
    });
  });

  it("groups audit events by turnSeq (matches existing TurnTrace entries)", () => {
    const store = useTraceStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_turn_traces") {
        return [makeTurnTraceRow(1)];
      }
      if (cmd === "list_session_audit_events") {
        return [
          makeAuditRow(1, "tool_executed", 1, JSON.stringify({
            tool_name: "shell",
            duration_ms: 123,
            exit_code: 0,
          })),
          makeAuditRow(2, "tool_executed", 1, JSON.stringify({
            tool_name: "read_file",
            duration_ms: 50,
            exit_code: 0,
          })),
        ];
      }
      return null;
    });
    return store.loadHistory("sess-1").then(() => {
      const t = store.currentSessionTraces.get(1);
      expect(t?.auditEvents).toHaveLength(2);
      expect(t?.auditEvents?.[0].kind).toBe("tool_executed");
    });
  });

  it("routes audit rows with turnSeq=null into the UNGROUPED_SEQ bucket", () => {
    const store = useTraceStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_turn_traces") return [makeTurnTraceRow(1)];
      if (cmd === "list_session_audit_events") {
        return [
          makeAuditRow(1, "tool_allowed", 1),
          // pre-v7 historical row + IPC-handler audit write —
          // turnSeq is null. Must land in UNGROUPED_SEQ, not
          // pollute the seq=1 turn's auditEvents.
          makeAuditRow(2, "mode_changed", null, JSON.stringify({
            prev_mode: "edit",
            new_mode: "plan",
          })),
        ];
      }
      return null;
    });
    return store.loadHistory("sess-1").then(() => {
      const seq1 = store.currentSessionTraces.get(1);
      const ungrouped = store.currentSessionTraces.get(UNGROUPED_SEQ);
      expect(seq1?.auditEvents).toHaveLength(1);
      expect(ungrouped?.auditEvents).toHaveLength(1);
      expect(ungrouped?.auditEvents?.[0].kind).toBe("mode_changed");
      expect(store.ungroupedAuditEvents()).toHaveLength(1);
    });
  });

  it("synthesizes a stub for audit-only turns (no turn_trace row)", () => {
    // A session that pre-dates v7 might have audit rows
    // with turnSeq=5 but no matching `turn_trace` row (e.g.
    // the row was never written). The store synthesizes a
    // stub so the audit row still surfaces on the timeline.
    const store = useTraceStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_turn_traces") return [];
      if (cmd === "list_session_audit_events") {
        return [makeAuditRow(1, "tool_executed", 5)];
      }
      return null;
    });
    return store.loadHistory("sess-1").then(() => {
      const t = store.currentSessionTraces.get(5);
      expect(t).toBeDefined();
      expect(t?.seq).toBe(5);
      expect(t?.auditEvents).toHaveLength(1);
    });
  });

  it("session switch wipes the prior session's traces", async () => {
    const store = useTraceStore();
    const queriedSessions: string[] = [];
    invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "list_turn_traces") {
        const a = args as { sessionId?: string };
        queriedSessions.push(a?.sessionId ?? "");
        // Return a row tagged with the queried session id so
        // the test can assert the post-switch state is keyed
        // by the NEW session, not the old one.
        return [makeTurnTraceRow(1, { sessionId: a?.sessionId ?? "unknown" })];
      }
      if (cmd === "list_session_audit_events") return [];
      return null;
    });
    await store.loadHistory("sess-A");
    expect(queriedSessions).toEqual(["sess-A"]);
    await store.loadHistory("sess-B");
    expect(queriedSessions).toEqual(["sess-A", "sess-B"]);
    // After the session switch, the timeline should show
    // sess-B's row (NOT sess-A's). The previous session's
    // entry was wiped by loadHistory's `currentSessionTraces.clear()`.
    const t = store.currentSessionTraces.get(1);
    expect(t).toBeDefined();
    expect(t?.sessionId).toBe("sess-B");
    // And the panel-bound session is updated.
    expect(store.currentSessionId).toBe("sess-B");
  });
});

describe("useTraceStore — clearSessionTrace", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("invokes clear_session_trace IPC then refreshes loadHistory", async () => {
    const store = useTraceStore();
    const calls: string[] = [];
    invokeMock.mockImplementation(async (cmd: string) => {
      calls.push(cmd);
      if (cmd === "list_turn_traces") return [];
      if (cmd === "list_session_audit_events") return [];
      return null;
    });
    await store.loadHistory("sess-1");
    calls.length = 0; // reset after initial load
    await store.clearSessionTrace("sess-1");
    // The IPC ran, then the post-clear refresh fired both
    // list_turn_traces + list_session_audit_events.
    expect(calls[0]).toBe("clear_session_trace");
    expect(calls).toContain("list_turn_traces");
    expect(calls).toContain("list_session_audit_events");
  });

  it("surfaces IPC failures on the error ref and rethrows", async () => {
    const store = useTraceStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "clear_session_trace") {
        throw new Error("DB locked");
      }
      return null;
    });
    await expect(store.clearSessionTrace("sess-1")).rejects.toThrow("DB locked");
    expect(store.error).toBe("DB locked");
  });
});

describe("useTraceStore — panel UI gate", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("panelOpen defaults to false (collapsed)", () => {
    const store = useTraceStore();
    expect(store.panelOpen).toBe(false);
  });

  it("setPanelOpen + togglePanel flip the gate", () => {
    const store = useTraceStore();
    store.setPanelOpen(true);
    expect(store.panelOpen).toBe(true);
    store.togglePanel();
    expect(store.panelOpen).toBe(false);
    store.togglePanel();
    expect(store.panelOpen).toBe(true);
  });
});
