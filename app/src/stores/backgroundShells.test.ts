// Tests for `useBackgroundShellsStore` — the ActivityPanel's
// 「后台命令」 Pinia store (2026-09-02, task `09-02-chat-task-panel`).
//
// Coverage (design §4):
//   1. `background_shell:update` event routing: started upsert,
//      exited in-place flip, pruned drop-by-id.
//   2. `fetchForSession` authoritative replace (+ ordering).
//   3. `compareShells` pure comparator (running first, newest first).
//   4. `clearSession` (session delete wiring).
//   5. `ensureStarted` idempotence (one global listener).
//   6. `kill` invoke shape + error propagation.
//   7. `liveElapsedMs` monotonic+wall-offset contract (never
//      `Date.now() - startedAtMs`).
//
// Transport is mocked per `.trellis/spec/frontend/test-environment.md`
// §4 (canonical barrel mock, one `invokeMock` + captured listen
// handler).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

let capturedHandler: ((payload: unknown) => void) | null = null;
let capturedUnlisten: (() => void) | null = null;
let listenCallCount = 0;
/** When true, the next `listen` call rejects (ensureStarted failure-path
 *  tests); consumed once, reset by beforeEach. */
let listenFailNext = false;

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async (
      _event: string,
      handler: (payload: unknown) => void,
    ) => {
      if (listenFailNext) {
        listenFailNext = false;
        throw new Error("listen mount failed");
      }
      listenCallCount += 1;
      capturedHandler = handler;
      capturedUnlisten = vi.fn();
      return capturedUnlisten;
    },
  },
}));

import {
  useBackgroundShellsStore,
  compareShells,
  liveElapsedMs,
  type BackgroundShellSummary,
  type ShellEventPayload,
} from "./backgroundShells";

// -----------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------

export function makeShell(
  overrides: Partial<BackgroundShellSummary> = {},
): BackgroundShellSummary {
  return {
    shellSessionId: "bsh_a",
    sessionId: "sess-1",
    command: "echo hi",
    status: "running",
    startedAtMs: 1000,
    elapsedMs: 500,
    exitCode: null,
    stdoutPreview: null,
    stderrPreview: null,
    fullOutputPath: null,
    originToolUseId: null,
    ...overrides,
  };
}

function emit(payload: ShellEventPayload): void {
  expect(capturedHandler).not.toBeNull();
  capturedHandler!(payload);
}

/** Mount the listener (the panel's onMounted does this in the app)
 *  so `emit` has a handler to drive. */
async function mountListener(): Promise<void> {
  await useBackgroundShellsStore().ensureStarted();
}

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

describe("useBackgroundShellsStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    capturedHandler = null;
    capturedUnlisten = null;
    listenCallCount = 0;
    listenFailNext = false;
  });

  describe("event routing (handleEvent)", () => {
    it("started upserts a new running row", async () => {
      await mountListener();
      const store = useBackgroundShellsStore();
      emit({
        kind: "started",
        sessionId: "sess-1",
        shellSessionId: "bsh_1",
        shell: makeShell({ shellSessionId: "bsh_1" }),
      });
      const list = store.shellsBySession.get("sess-1");
      expect(list).toHaveLength(1);
      expect(list![0].shellSessionId).toBe("bsh_1");
      expect(list![0].status).toBe("running");
    });

    it("exited flips an existing row in place to its terminal summary", async () => {
      await mountListener();
      const store = useBackgroundShellsStore();
      emit({
        kind: "started",
        sessionId: "sess-1",
        shellSessionId: "bsh_1",
        shell: makeShell({ shellSessionId: "bsh_1" }),
      });
      emit({
        kind: "exited",
        sessionId: "sess-1",
        shellSessionId: "bsh_1",
        shell: makeShell({
          shellSessionId: "bsh_1",
          status: "completed",
          elapsedMs: 4200,
          exitCode: 0,
          stdoutPreview: "hi",
        }),
      });
      const list = store.shellsBySession.get("sess-1");
      expect(list).toHaveLength(1);
      expect(list![0].status).toBe("completed");
      expect(list![0].exitCode).toBe(0);
      expect(list![0].stdoutPreview).toBe("hi");
    });

    it("pruned drops the row by id (payload carries ids only)", async () => {
      await mountListener();
      const store = useBackgroundShellsStore();
      emit({
        kind: "started",
        sessionId: "sess-1",
        shellSessionId: "bsh_1",
        shell: makeShell({
          shellSessionId: "bsh_1",
          status: "completed",
          exitCode: 0,
        }),
      });
      emit({
        kind: "pruned",
        sessionId: "sess-1",
        shellSessionId: "bsh_1",
        shell: null,
      });
      expect(store.shellsBySession.get("sess-1")).toHaveLength(0);
    });

    it("started/exited without a summary payload is a no-op (defensive)", async () => {
      await mountListener();
      const store = useBackgroundShellsStore();
      emit({
        kind: "exited",
        sessionId: "sess-1",
        shellSessionId: "bsh_ghost",
        shell: null,
      });
      expect(store.shellsBySession.has("sess-1")).toBe(false);
    });
  });

  describe("fetchForSession", () => {
    it("invokes list_background_shells with camelCase args and replaces the list", async () => {
      invokeMock.mockResolvedValueOnce([
        makeShell({ shellSessionId: "bsh_1", status: "completed", exitCode: 0 }),
      ]);
      const store = useBackgroundShellsStore();
      await store.fetchForSession("sess-1");
      expect(invokeMock).toHaveBeenCalledWith("list_background_shells", {
        sessionId: "sess-1",
      });
      expect(store.shellsBySession.get("sess-1")).toHaveLength(1);
    });

    it("replace is last-write-wins: rows only present in the old list are dropped", async () => {
      await mountListener();
      const store = useBackgroundShellsStore();
      emit({
        kind: "started",
        sessionId: "sess-1",
        shellSessionId: "bsh_live",
        shell: makeShell({ shellSessionId: "bsh_live" }),
      });
      invokeMock.mockResolvedValueOnce([
        makeShell({
          shellSessionId: "bsh_1",
          status: "completed",
          exitCode: 0,
        }),
      ]);
      await store.fetchForSession("sess-1");
      const ids = store.shellsBySession.get("sess-1")!.map((s) => s.shellSessionId);
      expect(ids).toEqual(["bsh_1"]);
    });

    it("fetch replace re-stamps running rows (no double-count of pre-fetch elapsed)", async () => {
      // Regression: a refetch answer carries the backend's FRESH
      // elapsed snapshot; keeping the original `started`-event stamp
      // would add the away-period a second time (session switch away
      // + back to a live shell doubled the displayed elapsed).
      await mountListener();
      const store = useBackgroundShellsStore();
      emit({
        kind: "started",
        sessionId: "sess-1",
        shellSessionId: "bsh_live",
        shell: makeShell({ shellSessionId: "bsh_live", elapsedMs: 500 }),
      });
      // "60s later": the refetch re-measures elapsed server-side.
      invokeMock.mockResolvedValueOnce([
        makeShell({ shellSessionId: "bsh_live", elapsedMs: 61_500 }),
      ]);
      await store.fetchForSession("sess-1");
      const now = Date.now() + 30_000; // 30s AFTER the fetch landed
      const live = store.elapsedOf(
        store.shellsBySession.get("sess-1")![0],
        now,
      );
      // 61_500 + ~30s (+ test-execution ms) — NOT 61_500 + 30s + 60s
      // (the pre-fix double count would land ≈151_500).
      expect(live).toBeGreaterThanOrEqual(61_500 + 30_000);
      expect(live).toBeLessThan(61_500 + 35_000);
    });

    it("failure is logged + swallowed (no throw)", async () => {
      invokeMock.mockRejectedValueOnce(new Error("boom"));
      const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
      const store = useBackgroundShellsStore();
      await expect(store.fetchForSession("sess-1")).resolves.toBeUndefined();
      expect(errSpy).toHaveBeenCalled();
      errSpy.mockRestore();
    });
  });

  describe("comparator (compareShells)", () => {
    it("running rows come first; terminal rows sort newest-start first", () => {
      const doneOld = makeShell({
        shellSessionId: "bsh_old",
        status: "completed",
        exitCode: 0,
        startedAtMs: 100,
      });
      const doneNew = makeShell({
        shellSessionId: "bsh_new",
        status: "killed",
        exitCode: -1,
        startedAtMs: 900,
      });
      const running = makeShell({ shellSessionId: "bsh_run", startedAtMs: 50 });
      const sorted = [doneOld, running, doneNew].sort(compareShells);
      expect(sorted.map((s) => s.shellSessionId)).toEqual([
        "bsh_run",
        "bsh_new",
        "bsh_old",
      ]);
    });
  });

  describe("kill + clearSession + lifecycle", () => {
    it("kill invokes kill_background_shell with camelCase args", async () => {
      invokeMock.mockResolvedValueOnce(null);
      const store = useBackgroundShellsStore();
      await store.kill("sess-1", "bsh_1");
      expect(invokeMock).toHaveBeenCalledWith("kill_background_shell", {
        sessionId: "sess-1",
        shellSessionId: "bsh_1",
      });
    });

    it("kill rethrows backend errors (caller toasts)", async () => {
      invokeMock.mockRejectedValueOnce(new Error("后台 shell 不存在"));
      const store = useBackgroundShellsStore();
      await expect(store.kill("sess-1", "bsh_x")).rejects.toThrow(
        "后台 shell 不存在",
      );
    });

    it("clearSession drops the session's rows", async () => {
      await mountListener();
      const store = useBackgroundShellsStore();
      emit({
        kind: "started",
        sessionId: "sess-1",
        shellSessionId: "bsh_1",
        shell: makeShell({ shellSessionId: "bsh_1" }),
      });
      store.clearSession("sess-1");
      expect(store.shellsBySession.has("sess-1")).toBe(false);
    });

    it("ensureStarted mounts the listener exactly once", async () => {
      const store = useBackgroundShellsStore();
      await store.ensureStarted();
      await store.ensureStarted();
      expect(listenCallCount).toBe(1);
      store.stop();
      await store.ensureStarted();
      expect(listenCallCount).toBe(2);
      expect(capturedUnlisten).not.toBeNull();
    });

    it("ensureStarted resets the guard on a failed mount so a later call retries", async () => {
      const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
      listenFailNext = true;
      const store = useBackgroundShellsStore();
      // Failure is swallowed (panel mounts with `void ensureStarted()`),
      // logged, and the guard resets so the NEXT mount retries.
      await expect(store.ensureStarted()).resolves.toBeUndefined();
      expect(listenCallCount).toBe(0);
      await store.ensureStarted();
      expect(listenCallCount).toBe(1);
      expect(capturedHandler).not.toBeNull();
      errSpy.mockRestore();
    });
  });

  describe("liveElapsedMs offset contract", () => {
    it("running rows add the wall delta since receipt — never Date.now() - startedAtMs", () => {
      const running = makeShell({
        status: "running",
        startedAtMs: 42, // monotonic — tiny value, NOT an epoch
        elapsedMs: 500,
      });
      const receivedAt = 1_000_000; // wall clock at receipt
      const now = 1_004_000; // 4s later
      expect(liveElapsedMs(running, new Map([["bsh_a", receivedAt]]), now)).toBe(
        4500,
      );
    });

    it("terminal rows return the final duration unchanged", () => {
      const done = makeShell({ status: "completed", exitCode: 0, elapsedMs: 700 });
      expect(liveElapsedMs(done, new Map(), 999_999_999)).toBe(700);
    });

    it("running rows without a receive stamp return the snapshot", () => {
      const running = makeShell({ status: "running", elapsedMs: 300 });
      expect(liveElapsedMs(running, new Map(), 1_000_000)).toBe(300);
    });
  });
});
