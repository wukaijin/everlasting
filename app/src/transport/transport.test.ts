// tauriTransport forwarding tests (远程访问 Phase 1).
//
// Locks two invariants:
//   1. `invoke` forwards cmd + args verbatim to `@tauri-apps/api/core`.
//   2. `listen` unwraps the Tauri `Event<T>` envelope → the handler
//      receives `payload` directly (NOT `{ event, id, payload }`).

import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
let capturedEvent: string | null = null;
let capturedHandler: ((e: { payload: unknown }) => void) | null = null;
const unlistenMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: async (event: string, handler: (e: { payload: unknown }) => void) => {
    capturedEvent = event;
    capturedHandler = handler;
    return unlistenMock;
  },
}));

import { tauriTransport } from "./tauri";

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue("ok");
  capturedEvent = null;
  capturedHandler = null;
  unlistenMock.mockReset();
});

describe("tauriTransport.invoke", () => {
  it("forwards cmd + args to @tauri-apps/api/core invoke", async () => {
    const result = await tauriTransport.invoke<string>("load_session", {
      sessionId: "s-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("load_session", {
      sessionId: "s-1",
    });
    expect(result).toBe("ok");
  });

  it("forwards a cmd with no args", async () => {
    await tauriTransport.invoke("get_home_dir");
    expect(invokeMock).toHaveBeenCalledWith("get_home_dir", undefined);
  });
});

describe("tauriTransport.listen", () => {
  it("unwraps Event<T> — handler receives payload directly", async () => {
    const received: unknown[] = [];
    const unlisten = await tauriTransport.listen<{ n: number }>(
      "chat-event",
      (payload) => received.push(payload),
    );

    expect(capturedEvent).toBe("chat-event");
    // Simulate Tauri delivering an Event<T> envelope.
    capturedHandler?.({ payload: { n: 42 } });

    expect(received).toEqual([{ n: 42 }]);
    // unlisten is the handle Tauri returned.
    expect(unlisten).toBe(unlistenMock);
  });
});
