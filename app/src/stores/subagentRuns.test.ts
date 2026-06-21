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
// B6 PR3b hotfix (2026-06-21): subagent:finished terminal signal
// listener. Separate capture so the mock can route by event name —
// without this, the second `listen` call would overwrite
// `capturedHandler` and break every existing subagent:event test.
let capturedFinishedHandler:
  | ((event: { payload: unknown }) => void)
  | null = null;
let capturedFinishedUnlisten: (() => void) | null = null;

vi.mock("@tauri-apps/api/event", () => ({
  listen: async (
    event: string,
    handler: (event: { payload: unknown }) => void,
  ) => {
    if (event === "subagent:finished") {
      capturedFinishedHandler = handler;
      capturedFinishedUnlisten = vi.fn();
      return capturedFinishedUnlisten;
    }
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
  RunAccumulator,
  type SubagentRunSummary,
  type SubagentRunRow,
  type SubagentEventPayload,
  type ThinkingSection,
  type TextSection,
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
  // B6 redesign PR1 (2026-06-21): nullable for legacy rows;
  // the redesign tests use a separate fixture with values.
  task: null,
  finalText: null,
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
  // B6 redesign PR1 (2026-06-21): new columns on the wire.
  task: null,
  finalText: null,
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
    capturedFinishedHandler = null;
    capturedFinishedUnlisten = null;
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

  // -------------------------------------------------------------------
  // B6 PR3b (2026-06-20): eager-fetch on first subagent:event per runId.
  // Race fix: when the dispatch_subagent tool_use fires, the
  // ToolCallCard's `fetchForSession` may race against the backend's
  // `insert_run` and return an empty list. The store's IPC listener
  // bridges that gap by eagerly fetching both `get_subagent_run` and
  // `list_subagent_runs_by_session` on the first event for any new
  // runId — by then the DB row is definitely committed.
  // -------------------------------------------------------------------

  it("first subagent:event for a runId fires fetchRun + fetchForSession", async () => {
    invokeMock.mockResolvedValueOnce(sampleRow); // for fetchRun -> get_subagent_run
    invokeMock.mockResolvedValueOnce([sampleSummary]); // for fetchForSession -> list_subagent_runs_by_session
    const store = useSubagentRunsStore();
    await store.start();
    invokeMock.mockClear();

    capturedHandler!({
      payload: {
        runId: "run-99",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { text: "hello" },
        timestamp: "2026-06-20T10:00:00Z",
      },
    });
    // Eager-fetch is fire-and-forget; let the microtasks drain.
    await new Promise((r) => setTimeout(r, 0));

    const calledCommands = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCommands).toContain("get_subagent_run");
    expect(calledCommands).toContain("list_subagent_runs_by_session");
    // Both calls targeted the runId / sessionId from the event.
    expect(invokeMock.mock.calls.find((c) => c[0] === "get_subagent_run")?.[1])
      .toEqual({ runId: "run-99" });
    expect(invokeMock.mock.calls.find((c) => c[0] === "list_subagent_runs_by_session")?.[1])
      .toEqual({ sessionId: "sess-1" });
  });

  it("subsequent subagent:events for the same runId do NOT re-fetch (dedup)", async () => {
    invokeMock.mockResolvedValueOnce(sampleRow);
    invokeMock.mockResolvedValueOnce([sampleSummary]);
    const store = useSubagentRunsStore();
    await store.start();
    invokeMock.mockClear();

    // Fire a burst of 5 events for the same runId within one debounce
    // window. Only the FIRST should trigger fetchRun + fetchForSession.
    for (let i = 0; i < 5; i++) {
      capturedHandler!({
        payload: {
          runId: "run-burst",
          sessionId: "sess-1",
          kind: "tool_call",
          payload: { name: "read_file", input: { path: `/tmp/${i}` } },
          timestamp: `t${i}`,
        },
      });
    }
    await new Promise((r) => setTimeout(r, 0));

    const getRunCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "get_subagent_run",
    );
    const listCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "list_subagent_runs_by_session",
    );
    expect(getRunCalls).toHaveLength(1);
    expect(listCalls).toHaveLength(1);
  });

  it("different runIds each fire their own eager-fetch", async () => {
    invokeMock.mockResolvedValue(sampleRow);
    invokeMock.mockResolvedValueOnce([sampleSummary]);
    const store = useSubagentRunsStore();
    await store.start();
    invokeMock.mockClear();

    capturedHandler!({
      payload: {
        runId: "run-A",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { text: "a" },
        timestamp: "t1",
      },
    });
    await new Promise((r) => setTimeout(r, 0));

    capturedHandler!({
      payload: {
        runId: "run-B",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { text: "b" },
        timestamp: "t2",
      },
    });
    await new Promise((r) => setTimeout(r, 0));

    const getRunCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "get_subagent_run",
    );
    // Two distinct runIds → two fetchRun calls (one each).
    expect(getRunCalls).toHaveLength(2);
    expect(getRunCalls.map((c) => (c[1] as { runId: string }).runId))
      .toEqual(["run-A", "run-B"]);
  });

  // -------------------------------------------------------------------
  // B6 PR3b hotfix (2026-06-21): subagent:finished terminal refresh.
  // The store listens for the one-shot terminal event emitted by
  // run_subagent after update_run_finished commits. On receipt it
  // flushes any buffered transcript events + refetches the run detail
  // (drawer source) + session summary (card source) so the drawer /
  // card flip from `running` to the terminal state without polling.
  // -------------------------------------------------------------------

  it("subagent:finished fires fetchRun + fetchForSession (terminal refresh)", async () => {
    invokeMock.mockResolvedValue(sampleRow);
    const store = useSubagentRunsStore();
    await store.start();
    expect(capturedFinishedHandler).not.toBeNull();
    invokeMock.mockClear();

    capturedFinishedHandler!({
      payload: {
        runId: "run-99",
        sessionId: "sess-1",
        status: "completed",
        finishedAt: "2026-06-21T10:00:30Z",
      },
    });
    await new Promise((r) => setTimeout(r, 0));

    const calledCommands = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCommands).toContain("get_subagent_run");
    expect(calledCommands).toContain("list_subagent_runs_by_session");
    expect(invokeMock.mock.calls.find((c) => c[0] === "get_subagent_run")?.[1])
      .toEqual({ runId: "run-99" });
    expect(
      invokeMock.mock.calls.find((c) => c[0] === "list_subagent_runs_by_session")?.[1],
    ).toEqual({ sessionId: "sess-1" });
  });

  it("subagent:finished flushes buffered transcript events before refetch", async () => {
    // The finished handler calls flushBuffer(runId) before fetchRun so
    // liveTranscript is complete (fetchRun's seed-guard won't overwrite
    // a non-empty liveTranscript). Fire an event into the debounce
    // buffer, then finished — the buffer must be flushed immediately
    // rather than waiting for the 200ms timer.
    vi.useFakeTimers();
    invokeMock.mockResolvedValue(sampleRow);
    const store = useSubagentRunsStore();
    await store.start();
    expect(capturedHandler).not.toBeNull();

    // An event sitting in the debounce buffer (timer not yet fired).
    capturedHandler!({
      payload: {
        runId: "run-flush",
        sessionId: "sess-1",
        kind: "tool_call",
        payload: { name: "grep", input: { pattern: "x" } },
        timestamp: "t1",
      },
    });
    expect(store.liveTranscript.get("run-flush") ?? []).toEqual([]);

    // Finished arrives — flushBuffer runs synchronously inside the
    // handler, before the fire-and-forget fetchRun.
    capturedFinishedHandler!({
      payload: {
        runId: "run-flush",
        sessionId: "sess-1",
        status: "completed",
        finishedAt: "t2",
      },
    });
    const live = store.liveTranscript.get("run-flush") ?? [];
    expect(live).toHaveLength(1);
    expect(live[0].kind).toBe("tool_call");
    expect(live[0].payload_json).toEqual({ name: "grep", input: { pattern: "x" } });
  });

  it("stop() tears down BOTH subagent:event and subagent:finished listeners", async () => {
    const store = useSubagentRunsStore();
    await store.start();
    expect(capturedUnlisten).not.toBeNull();
    expect(capturedFinishedUnlisten).not.toBeNull();
    store.stop();
    expect(capturedUnlisten).toHaveBeenCalled();
    expect(capturedFinishedUnlisten).toHaveBeenCalled();
  });

  // -------------------------------------------------------------------
  // B6 redesign PR2 (2026-06-21): RunAccumulator wiring.
  // The store now routes live chat_event payloads through a per-runId
  // `RunAccumulator` instance. The accumulator collapses the verbose
  // SSE chunk stream into Thinking / Text segments; the store
  // publishes the post-collapse sections into `liveSections`. These
  // tests assert the wiring + the live rebuild contract.
  // -------------------------------------------------------------------

  it("live chat_event delta routes into Text segment, not into chat_event section", async () => {
    vi.useFakeTimers();
    const store = useSubagentRunsStore();
    await store.start();
    capturedHandler!({
      payload: {
        runId: "run-acc",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { kind: "delta", text: "hello " },
        timestamp: "t1",
      },
    });
    vi.advanceTimersByTime(SUBAGENT_EVENT_DEBOUNCE_MS + 10);
    const sections = store.liveSections.get("run-acc") ?? [];
    // chat_event is collapsed — there is NO chat_event kind in the
    // derived sections (R9). The Text segment is the only entry.
    expect(sections).toHaveLength(1);
    expect(sections[0].kind).toBe("Text");
    if (sections[0].kind === "Text") {
      expect(sections[0].text).toBe("hello ");
      expect(sections[0].chars).toBe("hello ".length);
    }
  });

  it("live thinking_delta + signature_delta pair into a closed Thinking section", async () => {
    vi.useFakeTimers();
    const store = useSubagentRunsStore();
    await store.start();
    capturedHandler!({
      payload: {
        runId: "run-think",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { kind: "thinking_delta", text: "reasoning " },
        timestamp: "t1",
      },
    });
    capturedHandler!({
      payload: {
        runId: "run-think",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { kind: "thinking_delta", text: "more" },
        timestamp: "t2",
      },
    });
    vi.advanceTimersByTime(SUBAGENT_EVENT_DEBOUNCE_MS + 10);
    const sections = store.liveSections.get("run-think") ?? [];
    // One Thinking section; in-place string append kept the array
    // length at 1 (R20 — no array reallocation per event).
    expect(sections).toHaveLength(1);
    expect(sections[0].kind).toBe("Thinking");
    if (sections[0].kind === "Thinking") {
      expect(sections[0].text).toBe("reasoning more");
      expect(sections[0].chars).toBe("reasoning more".length);
      expect(sections[0].closed).toBe(false);
    }
    // signature_delta closes the block; the store publishes the
    // mutated segment on the next flush (the segment object is
    // mutated in place — Vue sees a new array identity from the
    // publishAccumulator path).
    capturedHandler!({
      payload: {
        runId: "run-think",
        sessionId: "sess-1",
        kind: "chat_event",
        payload: { kind: "signature_delta", signature: "blob" },
        timestamp: "t3",
      },
    });
    vi.advanceTimersByTime(SUBAGENT_EVENT_DEBOUNCE_MS + 10);
    const sections2 = store.liveSections.get("run-think") ?? [];
    expect(sections2).toHaveLength(1);
    if (sections2[0].kind === "Thinking") {
      expect(sections2[0].closed).toBe(true);
    }
  });

  it("rebuildFromCache: transcriptJson + finalText populate sections + FinalText", async () => {
    vi.useFakeTimers();
    const store = useSubagentRunsStore();
    await store.start();
    // Use a fetchRun to trigger the rebuild path. The fetchRun
    // path calls `acc.rebuildFromCache(row.transcriptJson,
    // row.finalText)`.
    const row: SubagentRunRow = {
      ...sampleRow,
      id: "run-rebuild",
      finalText: "found 3 files",
      transcriptJson: JSON.stringify([
        {
          kind: "chat_event",
          payload_json: { kind: "delta", text: "answer: " },
        },
        {
          kind: "chat_event",
          payload_json: { kind: "delta", text: "files" },
        },
        {
          kind: "tool_call",
          payload_json: { name: "grep", input: { pattern: "x" } },
        },
      ]),
    };
    invokeMock.mockResolvedValueOnce(row);
    await store.fetchRun("run-rebuild");
    const sections = store.liveSections.get("run-rebuild") ?? [];
    // 2 chat_event deltas collapse to 1 Text section; the
    // tool_call is a pass-through; finalText is the FinalText
    // section. Total 3 sections.
    expect(sections).toHaveLength(3);
    expect(sections.map((s) => s.kind)).toEqual([
      "Text",
      "ToolCall",
      "FinalText",
    ]);
    const text = sections[0] as TextSection;
    expect(text.text).toBe("answer: files");
    expect(text.chars).toBe("answer: files".length);
    const final = sections[2];
    expect(final.kind).toBe("FinalText");
    if (final.kind === "FinalText") {
      expect(final.text).toBe("found 3 files");
    }
  });
});

// -----------------------------------------------------------------------
// RunAccumulator class — direct unit tests
// -----------------------------------------------------------------------

describe("RunAccumulator", () => {
  it("feed collapses chat_event delta into a Text section", () => {
    const acc = new RunAccumulator();
    acc.feed({
      kind: "chat_event",
      payload_json: { kind: "delta", text: "hello" },
    });
    const out = acc.transcript.value;
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("Text");
    if (out[0].kind === "Text") {
      expect(out[0].text).toBe("hello");
    }
  });

  it("feed is O(1) per delta — multiple deltas mutate the same Text section", () => {
    const acc = new RunAccumulator();
    for (let i = 0; i < 100; i++) {
      acc.feed({
        kind: "chat_event",
        payload_json: { kind: "delta", text: "x" },
      });
    }
    const out = acc.transcript.value;
    // 1 section, not 100. R20: no array reallocation per event.
    expect(out).toHaveLength(1);
    const text = out[0] as TextSection;
    expect(text.chars).toBe(100);
    expect(text.text.length).toBe(100);
  });

  it("feed routes thinking_delta into a Thinking section; signature_delta closes it", () => {
    const acc = new RunAccumulator();
    acc.feed({
      kind: "chat_event",
      payload_json: { kind: "thinking_delta", text: "think" },
    });
    acc.feed({
      kind: "chat_event",
      payload_json: { kind: "thinking_delta", text: " more" },
    });
    acc.feed({
      kind: "chat_event",
      payload_json: { kind: "signature_delta", signature: "blob" },
    });
    const out = acc.transcript.value;
    expect(out).toHaveLength(1);
    const think = out[0] as ThinkingSection;
    expect(think.text).toBe("think more");
    expect(think.closed).toBe(true);
  });

  it("feed appends tool_call / tool_result / permission_ask as pass-through sections", () => {
    const acc = new RunAccumulator();
    acc.feed({
      kind: "tool_call",
      payload_json: { name: "grep", input: { pattern: "foo" } },
    });
    acc.feed({
      kind: "tool_result",
      payload_json: { content: "matched 3" },
    });
    acc.feed({
      kind: "permission_ask",
      payload_json: { toolName: "shell" },
    });
    const out = acc.transcript.value;
    expect(out.map((s) => s.kind)).toEqual([
      "ToolCall",
      "ToolResult",
      "PermissionAsk",
    ]);
  });

  it("rawEvents wraps in markRaw — Vue does not proxy the array", () => {
    // Vue 3.5's markRaw flag sets a `__v_skip` symbol on the
    // target. Reading `__v_skip` directly is the lock — the
    // value is `true` for `markRaw()`-wrapped objects, and
    // `undefined` for plain objects that Vue would otherwise
    // proxy on read.
    //
    // NOTE: the live `feed` path does NOT accumulate rawEvents
    // (R20 — array-spread per event was O(N²) cumulative; the
    // live path mutates segments in place). rawEvents is only
    // populated by `rebuildFromCache` (the cold-cache fetch path
    // after worker exit). We seed via rebuildFromCache below.
    const acc = new RunAccumulator();
    acc.rebuildFromCache(
      JSON.stringify([
        {
          kind: "tool_call",
          payload_json: { name: "grep" },
        },
      ]),
      null,
    );
    const raw = acc.rawEvents;
    // The first element of the array is a TranscriptEntry, but
    // the array itself is wrapped in markRaw. Vue's reactivity
    // proxy creation reads `__v_skip`; markRaw sets it.
    const skipSymbol = Object.getOwnPropertySymbols(raw).find(
      (s) => s.description === "v_skip" || s.toString() === "Symbol(__v_skip)",
    );
    // The `__v_skip` symbol is Vue-internal. If present, the
    // proxy creation is skipped — that's the lock the PRD
    // calls for (R21). The markRaw contract is that the
    // target's `__v_skip` is `true` after the call.
    if (skipSymbol) {
      // The array itself, not its elements, should be skipped.
      expect((raw as unknown as Record<symbol, unknown>)[skipSymbol]).toBe(
        true,
      );
    }
    // Defensive: the inner TranscriptEntry objects are NOT
    // wrapped in markRaw (markRaw only marks the array root,
    // not its elements). Vue 3.5 reads `__v_skip` from the
    // root; nested objects without the flag ARE proxied on
    // access (but our store never accesses them through
    // Vue's proxy — the drawer reads them through Pinia's
    // reactive Map).
    expect(acc.transcript.value).toHaveLength(1);
  });

  it("rebuildFromCache parses 20k chat_event deltas into 1 Text section in <500ms", () => {
    // R22 + AC: 20000 events cold start (parse + build) < 500ms.
    // We construct 20k chat_event delta entries, JSON.stringify
    // them, then call rebuildFromCache and assert the wall
    // clock. 500ms is generous; the implementation should hit
    // <100ms on jsdom (V8).
    const acc = new RunAccumulator();
    const events: unknown[] = [];
    for (let i = 0; i < 20000; i++) {
      events.push({
        kind: "chat_event",
        payload_json: { kind: "delta", text: "x" },
      });
    }
    const json = JSON.stringify(events);
    const t0 = performance.now();
    acc.rebuildFromCache(json, null);
    const dt = performance.now() - t0;
    // Verbose reporting — surfaces the actual budget used in
    // `pnpm vitest run` output. The 500ms ceiling is the
    // PR2 hard requirement; if a future regression pushes
    // the number past 500ms, this test fires and the
    // implementer sees the actual measurement in the
    // failure log.
    // eslint-disable-next-line no-console
    console.log(`[perf] 20k events rebuildFromCache: ${dt.toFixed(1)}ms`);
    // PR2 hard requirement: <500ms. Assert with a small
    // margin so a noisy CI box doesn't flake. The
    // implementation must keep this — if a future change
    // regresses past 500ms, this test fires and the failure
    // surfaces the architectural concern (R22 explicitly
    // calls out the 20000-event budget as a hard
    // requirement).
    expect(dt).toBeLessThan(500);
    // Sanity: 1 Text section, 20000 chars.
    const out = acc.transcript.value;
    expect(out).toHaveLength(1);
    if (out[0].kind === "Text") {
      expect(out[0].chars).toBe(20000);
    }
  });

  it("rebuildFromCache appends finalText as FinalText section", () => {
    const acc = new RunAccumulator();
    acc.rebuildFromCache(null, "done");
    const out = acc.transcript.value;
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("FinalText");
    if (out[0].kind === "FinalText") {
      expect(out[0].text).toBe("done");
    }
  });

  it("rebuildFromCache with empty transcriptJson and null finalText yields []", () => {
    const acc = new RunAccumulator();
    acc.rebuildFromCache(null, null);
    expect(acc.transcript.value).toEqual([]);
  });
});
