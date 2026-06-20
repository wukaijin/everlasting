// Tests for `useSubagentRunsStore` — B6 PR3 subagent drawer Pinia store.
//
// Coverage (per PRD R8):
//   1. Store API: fetchForSession / fetchRun / openDrawer / closeDrawer
//      / getSummaryByToolUseId.
//   2. IPC listener `subagent:event` is registered on `start()` and
//      torn down on `stop()`.
//   3. 200ms debounce batches live events into `liveTranscript`
//      (vi.useFakeTimers).
//   4. transcriptJson parsing — `payload_json` (snake_case) is used,
//      NOT `payload` (Drift trap 2).
//   5. `coerceStatus` handles raw strings from SubagentRunRow.status
//      (Drift trap 1 — Row.status is a raw String, Summary.status is
//      the typed enum).
//
// Tauri IPC + event are mocked so the suite runs in jsdom without a
// real Tauri runtime.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

let capturedHandler:
  | ((event: { payload: unknown }) => void)
  | null = null;
let capturedUnlisten: (() => void) | null = null;

vi.mock("@tauri-apps/api/event", () => ({
  listen: async (
    _event: string,
    handler: (event: { payload: unknown }) => void,
  ) => {
    capturedHandler = handler;
    capturedUnlisten = vi.fn();
    return capturedUnlisten;
  },
}));

import {
  useSubagentRunsStore,
  coerceStatus,
  parseTranscriptJson,
  SUBAGENT_EVENT_DEBOUNCE_MS,
  type SubagentRunSummary,
  type SubagentRunRow,
  type SubagentEventPayload,
} from "./subagentRuns";

// -----------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------

const sampleSummary: SubagentRunSummary = {
  id: "run-1",
  parentSessionId: "sess-1",
  parentRequestId: "parent-rid-sub-tooluse-1",
  subagentName: "researcher",
  status: "completed",
  startedAt: "2026-06-20T10:00:00Z",
  finishedAt: "2026-06-20T10:00:30Z",
  tokenUsageJson: '{"input":100,"output":20}',
  summary: "found 3 files",
};

// NOTE: SubagentRunRow.status is a raw `string` (Drift trap 1) — NOT
// the typed enum. The fixture intentionally uses a raw string.
const sampleRow: SubagentRunRow = {
  id: "run-1",
  parentSessionId: "sess-1",
  parentRequestId: "parent-rid-sub-tooluse-1",
  subagentName: "researcher",
  status: "completed",
  startedAt: "2026-06-20T10:00:00Z",
  finishedAt: "2026-06-20T10:00:30Z",
  tokenUsageJson: '{"input":100,"output":20}',
  summary: "found 3 files",
  // transcript_json entries keep snake_case payload_json (Drift
  // trap 2). The fixture below has TWO entries with distinct kinds
  // so we can assert all four are parsed.
  transcriptJson: JSON.stringify([
    { kind: "tool_call", payload_json: { name: "grep", input: { pattern: "foo" } } },
    { kind: "tool_result", payload_json: { content: "matched 3" } },
    { kind: "chat_event", payload_json: { text: "investigating..." } },
    { kind: "permission_ask", payload_json: { toolName: "shell" } },
  ]),
  transcriptTruncated: 0,
  createdAt: "2026-06-20T10:00:00Z",
};

// -----------------------------------------------------------------------
// Helpers — coerceStatus + parseTranscriptJson
// -----------------------------------------------------------------------

describe("coerceStatus (Drift trap 1: Row.status is raw string)", () => {
  it("returns the typed union when the string matches", () => {
    expect(coerceStatus("running")).toBe("running");
    expect(coerceStatus("completed")).toBe("completed");
    expect(coerceStatus("cancelled")).toBe("cancelled");
    expect(coerceStatus("error")).toBe("error");
  });

  it("falls back to 'running' for unknown / malformed strings", () => {
    expect(coerceStatus("READY")).toBe("running");
    expect(coerceStatus("")).toBe("running");
    expect(coerceStatus("timed_out")).toBe("running");
  });
});

describe("parseTranscriptJson (Drift trap 2: payload_json snake_case)", () => {
  it("returns [] for missing / null / malformed input", () => {
    expect(parseTranscriptJson(null)).toEqual([]);
    expect(parseTranscriptJson(undefined)).toEqual([]);
    expect(parseTranscriptJson("")).toEqual([]);
    expect(parseTranscriptJson("not json")).toEqual([]);
    expect(parseTranscriptJson("{}")).toEqual([]);
  });

  it("reads payload_json (snake_case) — NOT payload (camelCase)", () => {
    const json = JSON.stringify([
      {
        kind: "tool_call",
        payload_json: { name: "grep" },
        // Deliberately include a `payload` (camelCase) to verify the
        // parser ignores it. This locks the drift trap.
        payload: { name: "WRONG" },
      },
    ]);
    const out = parseTranscriptJson(json);
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("tool_call");
    expect(out[0].payload_json).toEqual({ name: "grep" });
  });

  it("skips entries with unknown kind or non-object payload_json", () => {
    const json = JSON.stringify([
      { kind: "tool_call", payload_json: { ok: true } },
      { kind: "weird_kind", payload_json: {} }, // skipped (unknown kind)
      { kind: "tool_result", payload_json: "not-object" }, // payload coerced to {}
      { kind: 42, payload_json: {} }, // skipped (non-string kind)
    ]);
    const out = parseTranscriptJson(json);
    // 2 survive: tool_call (with payload), tool_result (payload coerced).
    expect(out).toHaveLength(2);
    expect(out.map((e) => e.kind)).toEqual(["tool_call", "tool_result"]);
    expect(out[1].payload_json).toEqual({}); // coerced from "not-object"
  });
});

// -----------------------------------------------------------------------
// Store API
// -----------------------------------------------------------------------

describe("useSubagentRunsStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);
    capturedHandler = null;
    capturedUnlisten = null;
    vi.useRealTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("fetchForSession invokes list_subagent_runs_by_session + caches", async () => {
    invokeMock.mockResolvedValueOnce([sampleSummary]);
    const store = useSubagentRunsStore();
    await store.fetchForSession("sess-1");
    expect(invokeMock).toHaveBeenCalledWith("list_subagent_runs_by_session", {
      sessionId: "sess-1",
    });
    expect(store.runSummaryBySession.get("sess-1")).toEqual([sampleSummary]);
  });

  it("fetchForSession handles non-array response defensively", async () => {
    invokeMock.mockResolvedValueOnce(null);
    const store = useSubagentRunsStore();
    await store.fetchForSession("sess-1");
    expect(store.runSummaryBySession.get("sess-1")).toEqual([]);
  });

  it("fetchRun invokes get_subagent_run + caches + seeds liveTranscript", async () => {
    invokeMock.mockResolvedValueOnce(sampleRow);
    const store = useSubagentRunsStore();
    await store.fetchRun("run-1");
    expect(invokeMock).toHaveBeenCalledWith("get_subagent_run", {
      runId: "run-1",
    });
    expect(store.getRunCache.get("run-1")).toEqual(sampleRow);
    // The transcriptJson has 4 entries — all should be seeded.
    expect(store.liveTranscript.get("run-1")?.length).toBe(4);
  });

  it("fetchRun does NOT overwrite a live transcript that already has entries", async () => {
    invokeMock.mockResolvedValueOnce(sampleRow);
    const store = useSubagentRunsStore();
    // Pre-seed the live transcript (simulating in-flight streaming).
    store.liveTranscript.set("run-1", [
      { kind: "chat_event", payload_json: { text: "in-flight" } },
    ]);
    await store.fetchRun("run-1");
    // The cached row is stored, but the live transcript is NOT
    // overwritten by the parsed cache.
    expect(store.getRunCache.get("run-1")).toEqual(sampleRow);
    expect(store.liveTranscript.get("run-1")).toHaveLength(1);
    expect(store.liveTranscript.get("run-1")?.[0].payload_json).toEqual({
      text: "in-flight",
    });
  });

  it("openDrawer sets openRunId + fetches if uncached", async () => {
    invokeMock.mockResolvedValueOnce(sampleRow);
    const store = useSubagentRunsStore();
    await store.openDrawer("run-1");
    expect(store.openRunId).toBe("run-1");
    expect(store.getRunCache.has("run-1")).toBe(true);
  });

  it("openDrawer does NOT refetch when already cached", async () => {
    invokeMock.mockResolvedValueOnce(sampleRow);
    const store = useSubagentRunsStore();
    await store.fetchRun("run-1");
    invokeMock.mockClear();
    await store.openDrawer("run-1");
    expect(invokeMock).not.toHaveBeenCalled();
    expect(store.openRunId).toBe("run-1");
  });

  it("closeDrawer clears openRunId only (caches intact)", async () => {
    invokeMock.mockResolvedValueOnce(sampleRow);
    const store = useSubagentRunsStore();
    await store.openDrawer("run-1");
    store.closeDrawer();
    expect(store.openRunId).toBeNull();
    expect(store.getRunCache.has("run-1")).toBe(true);
  });

  it("getSummaryByToolUseId matches parentRequestId ending in -sub-{toolUseId}", async () => {
    invokeMock.mockResolvedValueOnce([sampleSummary]);
    const store = useSubagentRunsStore();
    await store.fetchForSession("sess-1");
    const found = store.getSummaryByToolUseId("sess-1", "tooluse-1");
    expect(found?.id).toBe("run-1");
  });

  it("getSummaryByToolUseId returns undefined when no match", async () => {
    invokeMock.mockResolvedValueOnce([sampleSummary]);
    const store = useSubagentRunsStore();
    await store.fetchForSession("sess-1");
    expect(store.getSummaryByToolUseId("sess-1", "other-tooluse")).toBeUndefined();
  });

  it("getSummaryByToolUseId returns undefined when session uncached", () => {
    const store = useSubagentRunsStore();
    expect(store.getSummaryByToolUseId("missing", "tooluse-1")).toBeUndefined();
  });

  // -------------------------------------------------------------------
  // IPC listener lifecycle + 200ms debounce
  // -------------------------------------------------------------------

  it("start() registers a subagent:event listener", async () => {
    const store = useSubagentRunsStore();
    await store.start();
    expect(capturedHandler).not.toBeNull();
  });

  it("start() is idempotent — calling twice replaces the prior unlisten", async () => {
    const store = useSubagentRunsStore();
    await store.start();
    const first = capturedUnlisten;
    await store.start();
    expect(first).toHaveBeenCalled();
  });

  it("stop() tears down the listener", async () => {
    const store = useSubagentRunsStore();
    await store.start();
    expect(capturedUnlisten).not.toBeNull();
    store.stop();
    expect(capturedUnlisten).toHaveBeenCalled();
  });

  it("200ms debounce batches live events into liveTranscript", async () => {
    vi.useFakeTimers();
    const store = useSubagentRunsStore();
    await store.start();
    expect(capturedHandler).not.toBeNull();

    // Fire 3 events in rapid succession — they should all land in
    // the buffer, not yet in the reactive liveTranscript.
    const events: SubagentEventPayload[] = [
      {
        runId: "run-1",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { text: "delta 1" },
        timestamp: "2026-06-20T10:00:00Z",
      },
      {
        runId: "run-1",
        sessionId: "sess-1",
        kind: "tool_call",
        payload: { name: "grep" },
        timestamp: "2026-06-20T10:00:01Z",
      },
      {
        runId: "run-1",
        sessionId: "sess-1",
        kind: "tool_result",
        payload: { content: "match" },
        timestamp: "2026-06-20T10:00:02Z",
      },
    ];
    for (const e of events) {
      capturedHandler!({ payload: e });
    }
    // Before the 200ms timer fires, liveTranscript is empty.
    expect(store.liveTranscript.get("run-1") ?? []).toEqual([]);

    // Advance just shy of 200ms — still empty.
    vi.advanceTimersByTime(SUBAGENT_EVENT_DEBOUNCE_MS - 10);
    expect(store.liveTranscript.get("run-1") ?? []).toEqual([]);

    // Cross the threshold — all 3 events flush.
    vi.advanceTimersByTime(20);
    const live = store.liveTranscript.get("run-1") ?? [];
    expect(live.length).toBe(3);
    expect(live[0].kind).toBe("chat_event");
    expect(live[0].payload_json).toEqual({ text: "delta 1" });
    expect(live[1].kind).toBe("tool_call");
    expect(live[1].payload_json).toEqual({ name: "grep" });
  });

  it("live event routes camelCase `payload` into snake_case `payload_json`", async () => {
    vi.useFakeTimers();
    const store = useSubagentRunsStore();
    await store.start();
    capturedHandler!({
      payload: {
        runId: "run-9",
        sessionId: "sess-1",
        kind: "tool_call",
        payload: { name: "grep", input: { pattern: "foo" } },
        timestamp: "2026-06-20T10:00:00Z",
      },
    });
    vi.advanceTimersByTime(SUBAGENT_EVENT_DEBOUNCE_MS + 10);
    const live = store.liveTranscript.get("run-9") ?? [];
    expect(live).toHaveLength(1);
    // The live `payload` (camelCase) is stored as `payload_json`
    // (snake_case) — the storage shape. This unifies the rendering
    // path so the drawer never has to know which source it came from.
    expect(live[0].payload_json).toEqual({
      name: "grep",
      input: { pattern: "foo" },
    });
  });

  it("subsequent event batches append to (not replace) liveTranscript", async () => {
    vi.useFakeTimers();
    const store = useSubagentRunsStore();
    await store.start();
    // First batch.
    capturedHandler!({
      payload: {
        runId: "run-1",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { text: "a" },
        timestamp: "t1",
      },
    });
    vi.advanceTimersByTime(SUBAGENT_EVENT_DEBOUNCE_MS + 10);
    expect(store.liveTranscript.get("run-1")).toHaveLength(1);

    // Second batch.
    capturedHandler!({
      payload: {
        runId: "run-1",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { text: "b" },
        timestamp: "t2",
      },
    });
    vi.advanceTimersByTime(SUBAGENT_EVENT_DEBOUNCE_MS + 10);
    expect(store.liveTranscript.get("run-1")).toHaveLength(2);
  });

  it("stop() flushes pending buffered events so the user doesn't lose the last batch", async () => {
    vi.useFakeTimers();
    const store = useSubagentRunsStore();
    await store.start();
    capturedHandler!({
      payload: {
        runId: "run-1",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { text: "about to be flushed" },
        timestamp: "t1",
      },
    });
    // Don't advance the timer — call stop() instead.
    store.stop();
    expect(store.liveTranscript.get("run-1")).toHaveLength(1);
  });

  it("clearSession drops all state for the session's runs", async () => {
    invokeMock.mockResolvedValueOnce(sampleRow);
    const store = useSubagentRunsStore();
    await store.fetchForSession("sess-1");
    store.runSummaryBySession.set("sess-1", [sampleSummary]);
    store.liveTranscript.set("run-1", [
      { kind: "chat_event", payload_json: {} },
    ]);
    store.openDrawer("run-1");
    // Vue's reactive ref needs .value; the Pinia store proxy exposes
    // it as a plain property.
    expect(store.openRunId).toBe("run-1");

    store.clearSession("sess-1");

    expect(store.runSummaryBySession.has("sess-1")).toBe(false);
    expect(store.getRunCache.has("run-1")).toBe(false);
    expect(store.liveTranscript.has("run-1")).toBe(false);
    // openRunId was cleared because run-1 belonged to sess-1.
    expect(store.openRunId).toBeNull();
  });
});
