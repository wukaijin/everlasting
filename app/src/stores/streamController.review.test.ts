// Integration tests for the C2 write_file → reviewStateStore
// routing inside `streamController.handleToolCall`
// (2026-07-26 review visualization view).
//
// Locks the design.md §2 contract:
//   - `payload.name === "write_file"` with a path matching
//     `matchesReviewStatePath(path, currentSlug)` →
//     `useReviewStateStore.handleReviewStateWritten` is called.
//   - Non-matching paths, non-write_file tools, and writes when
//     no slug is active are no-ops (zero impact on non-review
//     sessions).
//
// The top-level `vi.mock("../transport")` delegates `invoke` to
// `invokeMock` so per-test `.mockResolvedValue` setups take
// effect for BOTH the streamController's own invokes AND the
// reviewStateStore's `get_review_state` calls (same module
// instance, same `transport` object).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../transport", () => ({
  transport: {
    // Delegate to the outer mock so per-test resolved-value
    // setups apply uniformly.
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: vi.fn(async () => () => {}),
  },
}));

import { useStreamControllerStore } from "./streamController";
import { useReviewStateStore } from "./reviewState";
import { rehydrateMessages } from "./streamRehydrate";

const SID = "review-sess";
const RID = "rid-review";
const SLUG = "demo-task";

interface ReqState {
  requestId: string;
  sessionId: string;
  projectId: string | null;
  userMsgId: string;
  assistantMsgId: string;
  history: unknown[];
  sendAt: number;
  firstDeltaAt: number | null;
  toolStartedAt: Map<string, number>;
  currentTurnIndex: number;
  latencyByTurn: Map<string, unknown>;
}

function seedRequest(stream: ReturnType<typeof useStreamControllerStore>): void {
  const req: ReqState = {
    requestId: RID,
    sessionId: SID,
    projectId: null,
    userMsgId: "u1",
    assistantMsgId: "a1",
    history: [],
    sendAt: 0,
    firstDeltaAt: null,
    toolStartedAt: new Map(),
    currentTurnIndex: -1,
    latencyByTurn: new Map(),
  };
  (
    stream as unknown as { activeRequests: Map<string, ReqState> }
  ).activeRequests.set(RID, req);
  stream.putMessages(
    SID,
    // A user message + an empty assistant message so handleToolCall
    // has a `last.role === "assistant"` to push onto.
    rehydrateMessages([
      {
        id: 0,
        session_id: SID,
        role: "user",
        content: [{ type: "text", text: "go" }],
        text: "go",
        has_tool_calls: false,
        has_tool_results: false,
        created_at: "",
        seq: 0,
        ttfb_ms: null,
        gen_ms: null,
        total_ms: null,
        thinking_ms: null,
      },
      {
        id: 1,
        session_id: SID,
        role: "assistant",
        content: [{ type: "text", text: "" }],
        text: "",
        has_tool_calls: false,
        has_tool_results: false,
        created_at: "",
        seq: 1,
        ttfb_ms: null,
        gen_ms: null,
        total_ms: null,
        thinking_ms: null,
      },
    ]),
    false,
  );
}

function fireToolCall(
  stream: ReturnType<typeof useStreamControllerStore>,
  name: string,
  input: Record<string, unknown>,
): void {
  (
    stream as unknown as {
      handleToolCall: (p: {
        request_id: string;
        id: string;
        name: string;
        input: unknown;
      }) => void;
    }
  ).handleToolCall({ request_id: RID, id: "call_1", name, input });
}

describe("streamController handleToolCall write_file → reviewStateStore routing", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    // Default: get_review_state returns Missing (harmless).
    invokeMock.mockResolvedValue({ kind: "missing" });
  });

  it("routes a write_file hitting the current task's review-state.json", async () => {
    vi.useFakeTimers();
    try {
      const stream = useStreamControllerStore();
      seedRequest(stream);
      const reviewStore = useReviewStateStore();
      await reviewStore.start(SLUG, "/proj");

      const callsBefore = invokeMock.mock.calls.length;
      fireToolCall(stream, "write_file", {
        path: "/proj/.everlasting/tasks/demo-task/review-state.json",
        content: "{}",
      });
      await vi.advanceTimersByTimeAsync(500);

      // One extra refresh from the routed write.
      expect(invokeMock.mock.calls.length).toBe(callsBefore + 1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("routes a write_file with a bare-basename relative path (fallback match)", async () => {
    vi.useFakeTimers();
    try {
      const stream = useStreamControllerStore();
      seedRequest(stream);
      const reviewStore = useReviewStateStore();
      await reviewStore.start(SLUG, "/proj");

      const callsBefore = invokeMock.mock.calls.length;
      fireToolCall(stream, "write_file", { path: "review-state.json", content: "{}" });
      await vi.advanceTimersByTimeAsync(500);
      expect(invokeMock.mock.calls.length).toBe(callsBefore + 1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does NOT route a write_file for a different task's path", async () => {
    vi.useFakeTimers();
    try {
      const stream = useStreamControllerStore();
      seedRequest(stream);
      const reviewStore = useReviewStateStore();
      await reviewStore.start(SLUG, "/proj");

      const callsBefore = invokeMock.mock.calls.length;
      fireToolCall(stream, "write_file", {
        path: "/proj/.everlasting/tasks/other-task/review-state.json",
        content: "{}",
      });
      await vi.advanceTimersByTimeAsync(500);
      expect(invokeMock.mock.calls.length).toBe(callsBefore);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does NOT route a write_file when no slug is active (non-review session)", async () => {
    vi.useFakeTimers();
    try {
      const stream = useStreamControllerStore();
      seedRequest(stream);
      // Note: no reviewStore.start — currentSlug is null (this is
      // a dev session that happens to write a file named
      // review-state.json, which would be a coincidence).

      fireToolCall(stream, "write_file", {
        path: "/proj/.everlasting/tasks/demo-task/review-state.json",
        content: "{}",
      });
      await vi.advanceTimersByTimeAsync(500);
      expect(invokeMock).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("does NOT route non-write_file tools (e.g. shell, edit_file)", async () => {
    vi.useFakeTimers();
    try {
      const stream = useStreamControllerStore();
      seedRequest(stream);
      const reviewStore = useReviewStateStore();
      await reviewStore.start(SLUG, "/proj");

      const callsBefore = invokeMock.mock.calls.length;
      fireToolCall(stream, "shell", { command: "ls" });
      fireToolCall(stream, "edit_file", {
        path: "/proj/.everlasting/tasks/demo-task/review-state.json",
      });
      await vi.advanceTimersByTimeAsync(500);
      expect(invokeMock.mock.calls.length).toBe(callsBefore);
    } finally {
      vi.useRealTimers();
    }
  });

  it("still records the tool_use on the assistant message (no regression to tool tracking)", () => {
    const stream = useStreamControllerStore();
    seedRequest(stream);

    fireToolCall(stream, "write_file", {
      path: "/proj/.everlasting/tasks/demo-task/review-state.json",
      content: "{}",
    });

    const msgs = (
      stream as unknown as {
        messagesBySession: Map<string, { toolCalls?: { name: string }[] }[]>;
      }
    ).messagesBySession.get(SID);
    const last = msgs![msgs!.length - 1];
    expect(last.toolCalls?.some((tc) => tc.name === "write_file")).toBe(true);
  });
});
