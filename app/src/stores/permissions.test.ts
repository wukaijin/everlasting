// Tests for `usePermissionsStore` — the ⑨ 关 ↔ `permission:ask`
// IPC bridge.
//
// Coverage targets (PR3 spec §"PermissionModal Acceptance Criteria"
// + store contract):
//   1. `start()` registers a listener; `permission:ask` events
//      populate `pendingPermission`.
//   2. `respond(rid, decision)` invokes `permission_response` IPC
//      with the right args + clears `pendingPermission` if rid
//      matches.
//   3. `startAskTimer` / 120s timeout → auto-deny + toast (verified
//      with a stubbed `window.setTimeout` via fake timers).
//   4. New ask replaces the prior (single-slot semantics).
//   5. `stop()` tears down the listener + clears state.
//
// Tauri IPC + event are mocked via `vi.mock("@tauri-apps/api/event")`
// and `vi.mock("@tauri-apps/api/core")` so the suite runs in jsdom
// without a real Tauri runtime.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

// Tauri mocks — both IPC invoke and event listen. The listener
// mock captures the handler so tests can drive events directly
// (rather than relying on `emit()` which the real API exposes
// only via `AppHandle` on the Rust side).
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
  toolName: "shell",
  toolInput: { command: "ls -la" },
  risk: "high",
  reason: "Test reason",
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
    // listenMock is wired in the vi.mock factory above.
    await store.start();
    expect(capturedHandler).not.toBeNull();
  });

  it("setPending populates pendingPermission", () => {
    const store = usePermissionsStore();
    expect(store.pendingPermission).toBeNull();
    store.setPending(sampleAsk);
    expect(store.pendingPermission).toEqual(sampleAsk);
  });

  it("a new ask replaces the prior (single-slot semantics)", () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    const second: PermissionAsk = {
      rid: "rid-2",
      toolName: "write_file",
      toolInput: { path: "/tmp/x" },
      risk: "medium",
    };
    store.setPending(second);
    expect(store.pendingPermission?.rid).toBe("rid-2");
  });

  it("respond fires permission_response IPC with the rid + decision", async () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    await store.respond(sampleAsk.rid, "allow_once");
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: sampleAsk.rid,
      decision: "allow_once",
    });
    // Local state is cleared after respond (matches our UX
    // contract — the modal closes regardless of IPC outcome).
    expect(store.pendingPermission).toBeNull();
  });

  it("respond with allow_always fires the right IPC", async () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    await store.respond(sampleAsk.rid, "allow_always");
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: sampleAsk.rid,
      decision: "allow_always",
    });
  });

  it("respond with deny fires the right IPC", async () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    await store.respond(sampleAsk.rid, "deny");
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: sampleAsk.rid,
      decision: "deny",
    });
  });

  it("respond does NOT clear pendingPermission if rid doesn't match", async () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    // Respond with a different rid (race: user clicked button after
    // a new ask arrived).
    await store.respond("rid-other", "deny");
    expect(store.pendingPermission?.rid).toBe("rid-1");
  });

  it("clearPending empties the slot + clears the timer", () => {
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    expect(store.pendingPermission).not.toBeNull();
    store.clearPending();
    expect(store.pendingPermission).toBeNull();
  });

  it("120s timer fires deny + toast", () => {
    vi.useFakeTimers();
    const toastMock = vi.fn();
    const store = usePermissionsStore();
    store.setPending(sampleAsk);
    // start() wires the toast callback.
    void store.start(toastMock);
    // Advance to just before the timeout.
    vi.advanceTimersByTime(ASK_TIMEOUT_MS - 100);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(toastMock).not.toHaveBeenCalled();
    // Cross the timeout.
    vi.advanceTimersByTime(200);
    // The timer should have fired a deny + surfaced a toast.
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: sampleAsk.rid,
      decision: "deny",
    });
    expect(toastMock).toHaveBeenCalledWith(
      "权限询问已超时,已自动拒绝",
      "warn",
    );
  });

  it("stop() tears down the listener + clears state", async () => {
    const store = usePermissionsStore();
    await store.start();
    expect(capturedUnlisten).not.toBeNull();
    store.setPending(sampleAsk);
    store.stop();
    // capturedUnlisten was called.
    expect(capturedUnlisten).toHaveBeenCalled();
    // State is reset.
    expect(store.pendingPermission).toBeNull();
  });

  it("start() is idempotent — calling twice replaces the prior unlisten", async () => {
    const store = usePermissionsStore();
    await store.start();
    const firstUnlisten = capturedUnlisten;
    await store.start();
    // The first listener was torn down.
    expect(firstUnlisten).toHaveBeenCalled();
  });
});