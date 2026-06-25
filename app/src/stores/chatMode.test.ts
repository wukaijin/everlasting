// Tests for `useChatStore.requestSetMode` / `confirmYolo` /
// `cancelYolo` — the store-managed Mode orchestrator that both
// the `ModeSelect` popover and the `Shift+Tab` cycle in
// `ChatInput` route through.
//
// The contract under test (PR2 ACs §"前端 4 条" + impl plan):
//   1. Calling `requestSetMode` for a non-Yolo mode fires the
//      `set_session_mode` IPC with the right args and updates
//      the local SessionSummary optimistically.
//   2. Calling `requestSetMode` for Yolo does NOT fire IPC; it
//      flips `pendingYoloConfirm` to true so the modal mounts.
//   3. `confirmYolo` fires the IPC with mode="yolo" and closes
//      the modal.
//   4. `cancelYolo` only flips `pendingYoloConfirm` to false
//      and does NOT touch IPC.
//   5. `requestSetMode` is a no-op (no IPC) when the session is
//      already in the target mode.
//   6. Mode changes pass through unconditionally — including
//      while the session is streaming. The turn-boundary
//      semantics ("applies on the next turn") live in
//      `chat_loop.rs:396`, not here; the UI surface a toast
//      hint while streaming. See `ModeSelect.vue` for the
//      toast contract.
//
// Tauri IPC is mocked via `vi.mock("@tauri-apps/api/core")` so
// these tests run under vitest's jsdom env without Tauri.

import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import { setActivePinia, createPinia } from "pinia";

// Mock the Tauri invoke channel. The factory returns the
// default `vi.fn`; each test resets and re-stubs as needed.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

// Import AFTER the mock so the chat store picks up the mocked
// `@tauri-apps/api/core` module.
import { useChatStore } from "./chat";

describe("useChatStore — requestSetMode / confirmYolo / cancelYolo (PR2 B7)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  afterEach(() => {
    invokeMock.mockReset();
  });

  /** Convenience: seed the chat store with a single fake session
   *  in the requested mode and return the session id. */
  function seedSession(opts: {
    id: string;
    mode?: "edit" | "plan" | "yolo" | "background";
  }): string {
    const store = useChatStore();
    store.sessions = [
      {
        id: opts.id,
        title: "t",
        updated_at: "",
        preview: "",
        project_id: "p1",
        current_cwd: "/tmp",
        worktree_path: null,
        worktree_state: "none",
        last_worktree_path: null,
        model_id: null,
        input_tokens_total: null,
        output_tokens_total: null,
        cache_creation_total: null,
        cache_read_total: null,
        last_context_input_tokens: null,
        last_input_tokens: null,
        last_output_tokens: null,
        last_cache_creation: null,
        last_cache_read: null,
        color_tag: null,
        mode: opts.mode ?? "edit",
      },
    ];
    store.currentSessionId = opts.id;
    return opts.id;
  }

  it("fires set_session_mode IPC for a non-Yolo mode", async () => {
    const sid = seedSession({ id: "s1", mode: "edit" });
    invokeMock.mockResolvedValue({});

    const store = useChatStore();
    const result = await store.requestSetMode(sid, "plan");

    expect(result).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("set_session_mode", {
      sessionId: sid,
      mode: "plan",
    });
    // Optimistic local update.
    expect(store.sessions[0].mode).toBe("plan");
  });

  it("does NOT fire IPC when the target mode matches current", async () => {
    const sid = seedSession({ id: "s1", mode: "plan" });
    invokeMock.mockResolvedValue({});

    const store = useChatStore();
    const result = await store.requestSetMode(sid, "plan");

    expect(result).toBe(true);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("does NOT fire IPC when target is Yolo — flips pendingYoloConfirm instead", async () => {
    const sid = seedSession({ id: "s1", mode: "edit" });
    invokeMock.mockResolvedValue({});

    const store = useChatStore();
    const result = await store.requestSetMode(sid, "yolo");

    expect(result).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(store.pendingYoloConfirm).toBe(true);
    // Local session row should NOT have flipped yet — the modal
    // is still pending.
    expect(store.sessions[0].mode).toBe("edit");
  });

  it("confirmYolo fires the IPC with mode=yolo and clears pendingYoloConfirm", async () => {
    const sid = seedSession({ id: "s1", mode: "edit" });
    invokeMock.mockResolvedValue({});

    const store = useChatStore();
    await store.requestSetMode(sid, "yolo");
    expect(store.pendingYoloConfirm).toBe(true);

    await store.confirmYolo();

    expect(invokeMock).toHaveBeenCalledWith("set_session_mode", {
      sessionId: sid,
      mode: "yolo",
    });
    expect(store.pendingYoloConfirm).toBe(false);
    expect(store.sessions[0].mode).toBe("yolo");
  });

  it("cancelYolo clears pendingYoloConfirm without touching IPC", async () => {
    const sid = seedSession({ id: "s1", mode: "edit" });
    invokeMock.mockResolvedValue({});

    const store = useChatStore();
    await store.requestSetMode(sid, "yolo");
    expect(store.pendingYoloConfirm).toBe(true);

    store.cancelYolo();

    expect(store.pendingYoloConfirm).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(store.sessions[0].mode).toBe("edit");
  });

  it("returns false and skips IPC when no sessionId is passed", async () => {
    const store = useChatStore();
    store.sessions = [
      {
        id: "s1",
        title: "t",
        updated_at: "",
        preview: "",
        project_id: "p1",
        current_cwd: "/tmp",
        worktree_path: null,
        worktree_state: "none",
        last_worktree_path: null,
        model_id: null,
        input_tokens_total: null,
        output_tokens_total: null,
        cache_creation_total: null,
        cache_read_total: null,
        last_context_input_tokens: null,
        last_input_tokens: null,
        last_output_tokens: null,
        last_cache_creation: null,
        last_cache_read: null,
        color_tag: null,
        mode: "edit",
      },
    ];
    invokeMock.mockResolvedValue({});

    const result = await store.requestSetMode("", "plan");
    expect(result).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("confirmYolo is a no-op when no session is active", async () => {
    const store = useChatStore();
    store.currentSessionId = null;
    store.pendingYoloConfirm = true;
    await store.confirmYolo();
    expect(invokeMock).not.toHaveBeenCalled();
    // pendingYoloConfirm is unconditionally reset at the top of
    // confirmYolo so the modal always closes.
    expect(store.pendingYoloConfirm).toBe(false);
  });
});