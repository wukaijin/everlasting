// Tests for `usePermissionsStore` — the ⑨ 关 ↔ `permission:ask`
// IPC bridge.
//
// 2026-06-16 (inline approval card): the store now routes pending
// asks PER SESSION. Coverage targets:
//   1. `start()` registers a listener; `permission:ask` routes into
//      `pendingBySession` by `sessionId`.
//   2. Same-session replace (serial agent loop → one slot/session).
//   3. **Multi-session coexistence** — the core bug fix: asks in
//      different sessions no longer overwrite each other.
//   4. `respond(rid, decision, reason?)` invokes `permission_response`
//      with the right args + clears ONLY the matching rid's pending
//      (does NOT touch another session's pending).
//   5. deny forwards the "拒绝并说明" feedback reason.
//   6. Per-rid 120s timer → auto-deny + toast.
//   7. `stop()` tears down + clears all.
//
// Tauri IPC + event are mocked so the suite runs in jsdom without a
// real Tauri runtime.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();
const listenMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

let capturedHandler: ((event: { payload: unknown }) => void) | null = null;
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
  usePermissionsStore,
  ASK_TIMEOUT_MS,
  type PermissionAsk,
} from "./permissions";

const sampleAsk: PermissionAsk = {
  rid: "rid-1",
  sessionId: "sess-1",
  toolUseId: "tooluse-1",
  toolName: "shell",
  toolInput: { command: "ls -la" },
  risk: "high",
  reason: "Test reason",
};

/** A second ask in a DIFFERENT session — the multi-concurrency
 *  fixture. */
const otherSessionAsk: PermissionAsk = {
  rid: "rid-9",
  sessionId: "sess-2",
  toolUseId: "tooluse-9",
  toolName: "write_file",
  toolInput: { path: "/tmp/x" },
  risk: "medium",
};

describe("usePermissionsStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(true);
    listenMock.mockReset();
    capturedHandler = null;
    capturedUnlisten = null;
    vi.useRealTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("start() registers a permission:ask listener", async () => {
    const store = usePermissionsStore();
    await store.start();
    expect(capturedHandler).not.toBeNull();
  });

  it("setPending routes the ask by sessionId", () => {
    const store = usePermissionsStore();
    expect(store.getPending("sess-1")).toBeUndefined();
    store.setPending(sampleAsk);
    expect(store.getPending("sess-1")).toEqual(sampleAsk);
    expect(store.hasPending("sess-1")).toBe(true);
  });

  it("a new ask in the SAME session replaces the prior", () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    const next: PermissionAsk = {
      rid: "rid-2",
      sessionId: "sess-1",
      toolUseId: "tooluse-2",
      toolName: "write_file",
      toolInput: { path: "/tmp/x" },
      risk: "medium",
    };
    store.setPending(next);
    expect(store.getPending("sess-1")?.rid).toBe("rid-2");
    expect(store.pendingSessionIds).toEqual(["sess-1"]);
  });

  it("asks in DIFFERENT sessions coexist (multi-session concurrency)", () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk); // sess-1
    store.setPending(otherSessionAsk); // sess-2
    // Both survive — no silent overwrite (the old single-slot bug).
    expect(store.getPending("sess-1")?.rid).toBe("rid-1");
    expect(store.getPending("sess-2")?.rid).toBe("rid-9");
    expect(store.pendingSessionIds).toHaveLength(2);
    expect(store.pendingSessionIds.sort()).toEqual(["sess-1", "sess-2"]);
  });

  it("respond allow_once fires IPC + clears the matching session pending", async () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    await store.respond(sampleAsk.rid, "allow_once");
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: sampleAsk.rid,
      decision: "allow_once",
      reason: undefined,
    });
    expect(store.getPending("sess-1")).toBeUndefined();
  });

  it("respond deny forwards the 拒绝并说明 feedback reason", async () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    await store.respond(sampleAsk.rid, "deny", "use git clean instead");
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: sampleAsk.rid,
      decision: "deny",
      reason: "use git clean instead",
    });
  });

  it("respond deny without feedback sends reason: undefined", async () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    await store.respond(sampleAsk.rid, "deny");
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: sampleAsk.rid,
      decision: "deny",
      reason: undefined,
    });
  });

  it("respond on one session does NOT clear another session's pending", async () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk); // sess-1
    store.setPending(otherSessionAsk); // sess-2
    await store.respond("rid-1", "allow_once");
    expect(store.getPending("sess-1")).toBeUndefined();
    // sess-2 untouched — the old store would have lost this.
    expect(store.getPending("sess-2")?.rid).toBe("rid-9");
  });

  it("clearPending(sessionId) empties that session + its timer", () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    expect(store.hasPending("sess-1")).toBe(true);
    store.clearPending("sess-1");
    expect(store.hasPending("sess-1")).toBe(false);
  });

  it("120s timer fires deny + toast for the rid", () => {
    vi.useFakeTimers();
    const toastMock = vi.fn();
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    void store.start(toastMock);
    vi.advanceTimersByTime(ASK_TIMEOUT_MS - 100);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(toastMock).not.toHaveBeenCalled();
    vi.advanceTimersByTime(200);
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: sampleAsk.rid,
      decision: "deny",
      reason: undefined,
    });
    expect(toastMock).toHaveBeenCalledWith(
      "权限询问已超时,已自动拒绝",
      "warn",
    );
  });

  it("stop() tears down the listener + clears all pending", async () => {
    const store = usePermissionsStore();
    await store.start();
    expect(capturedUnlisten).not.toBeNull();
    store.setPending(sampleAsk);
    store.setPending(otherSessionAsk);
    store.stop();
    expect(capturedUnlisten).toHaveBeenCalled();
    expect(store.pendingSessionIds).toEqual([]);
  });

  it("start() is idempotent — calling twice replaces the prior unlisten", async () => {
    const store = usePermissionsStore();
    await store.start();
    const firstUnlisten = capturedUnlisten;
    await store.start();
    expect(firstUnlisten).toHaveBeenCalled();
  });
});
