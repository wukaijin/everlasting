// Tests for `useReviewStateStore` (C2 review visualization view).
//
// Coverage (design §6 + §2):
//   1. `matchesReviewStatePath`: absolute path / relative with
//      `/tasks/<slug>/` / bare basename fallback / misses.
//   2. `applyPayload` three-state branching (State/Missing/Invalid).
//   3. `handleReviewStateWritten`: slug gate (non-current slug
//      ignored) + debounce (multiple writes coalesce to one
//      refresh).
//   4. `refresh`: invokes `get_review_state` with the right args;
//      network failure surfaces as `error.kind: "network"`.
//   5. `start` / `stop` lifecycle: stop clears state + cancels
//      pending debounce.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

// Mock the transport — `get_review_state` returns whatever the
// test sets up via `mockInvokeImpl`. Defaults to `Missing` so a
// stray invoke doesn't crash unrelated tests.
const invokeMock = vi.fn();
vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: vi.fn(async () => () => {}),
  },
}));

import {
  useReviewStateStore,
  matchesReviewStatePath,
} from "./reviewState";
import type {
  ReviewState,
  ReviewStatePayload,
} from "../types/review-state";

const SLUG = "demo-task";

function sampleState(): ReviewState {
  return {
    schema_version: "1.0",
    task_id: SLUG,
    current_round: 1,
    rounds: [
      {
        round: 1,
        dimensions: ["清晰度"],
        models_present: ["model-a"],
        models: {
          "model-a": {
            model_display: "Model A",
            run_id: "run-1",
            status: "completed",
            verdict: "revise",
            findings: [
              {
                finding_id: "f1",
                dimension: "清晰度",
                severity: "high",
                issue: "unclear",
                source_run_id: "run-1",
              },
            ],
          },
        },
      },
    ],
  };
}

describe("matchesReviewStatePath", () => {
  it("matches an absolute path ending in /tasks/<slug>/review-state.json", () => {
    expect(
      matchesReviewStatePath(
        "/home/u/proj/.everlasting/tasks/demo-task/review-state.json",
        SLUG,
      ),
    ).toBe(true);
  });

  it("matches a relative path with ./ prefix", () => {
    expect(
      matchesReviewStatePath(
        "./.everlasting/tasks/demo-task/review-state.json",
        SLUG,
      ),
    ).toBe(true);
  });

  it("matches a path that contains /tasks/<slug>/ but doesn't end with it", () => {
    // Sibling-relative write: the agent's cwd is one level up.
    expect(
      matchesReviewStatePath(
        "tasks/demo-task/review-state.json",
        SLUG,
      ),
    ).toBe(true);
  });

  it("matches a bare basename when there is no separator (relative fallback)", () => {
    expect(matchesReviewStatePath("review-state.json", SLUG)).toBe(true);
  });

  it("matches a Windows-style path (backslashes)", () => {
    expect(
      matchesReviewStatePath(
        "C:\\proj\\.everlasting\\tasks\\demo-task\\review-state.json",
        SLUG,
      ),
    ).toBe(true);
  });

  it("rejects a different file name", () => {
    expect(
      matchesReviewStatePath(
        "/home/u/proj/.everlasting/tasks/demo-task/task.json",
        SLUG,
      ),
    ).toBe(false);
  });

  it("rejects a path for a different slug", () => {
    // basename matches but the slug in the path is different AND
    // the path has a separator → the bare-basename fallback does
    // NOT apply (the path is unambiguously for another task).
    expect(
      matchesReviewStatePath(
        "/home/u/proj/.everlasting/tasks/other-task/review-state.json",
        SLUG,
      ),
    ).toBe(false);
  });

  it("rejects empty inputs", () => {
    expect(matchesReviewStatePath("", SLUG)).toBe(false);
    expect(matchesReviewStatePath("review-state.json", "")).toBe(false);
  });
});

describe("useReviewStateStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("start sets currentSlug + applies the payload via refresh", async () => {
    const payload: ReviewStatePayload = {
      kind: "state",
      state: sampleState(),
    };
    invokeMock.mockResolvedValue(payload);

    const store = useReviewStateStore();
    await store.start(SLUG, "/proj");

    expect(invokeMock).toHaveBeenCalledWith("get_review_state", {
      projectPath: "/proj",
      taskSlug: SLUG,
    });
    expect(store.state).toEqual(payload.state);
    expect(store.error).toBeNull();
    expect(store.currentSlugForRouting).toBe(SLUG);
  });

  it("refresh applies Missing by clearing state without surfacing an error", async () => {
    // Seed a State first so we can confirm Missing clears it.
    invokeMock.mockResolvedValue({
      kind: "state",
      state: sampleState(),
    } satisfies ReviewStatePayload);

    const store = useReviewStateStore();
    await store.start(SLUG, "/proj");
    expect(store.state).not.toBeNull();

    // Now switch to Missing — state should clear, but NO error
    // surfaces (missing → hide panel, not error state).
    invokeMock.mockResolvedValue({ kind: "missing" } satisfies ReviewStatePayload);
    await store.refresh(SLUG, "/proj");
    expect(store.state).toBeNull();
    expect(store.error).toBeNull(); // missing → hide, not error
  });

  it("refresh applies Invalid by clearing state + surfacing error", async () => {
    invokeMock.mockResolvedValue({
      kind: "invalid",
      detail: "parse error: ...",
    } satisfies ReviewStatePayload);

    const store = useReviewStateStore();
    await store.refresh(SLUG, "/proj");

    expect(store.state).toBeNull();
    expect(store.error).toEqual({
      kind: "invalid",
      detail: "parse error: ...",
    });
  });

  it("refresh surfaces a transport rejection as a network error", async () => {
    invokeMock.mockRejectedValue(new Error("daemon down"));

    const store = useReviewStateStore();
    await store.refresh(SLUG, "/proj");

    expect(store.error?.kind).toBe("network");
    expect(store.error?.detail).toContain("daemon down");
    expect(store.state).toBeNull();
  });

  it("stop clears state + cancels any pending debounced refresh", async () => {
    vi.useFakeTimers();
    try {
      invokeMock.mockResolvedValue({ kind: "state", state: sampleState() } satisfies ReviewStatePayload);
      const store = useReviewStateStore();
      await store.start(SLUG, "/proj");
      expect(store.state).not.toBeNull();

      // Arm a debounced refresh, then stop before it fires.
      store.handleReviewStateWritten("sess-1", SLUG);
      store.stop();

      // Flush any pending timers — refresh should NOT have run
      // (currentSlug was cleared by stop).
      const callsBefore = invokeMock.mock.calls.length;
      await vi.advanceTimersByTimeAsync(500);
      expect(invokeMock.mock.calls.length).toBe(callsBefore);

      expect(store.state).toBeNull();
      expect(store.currentSlugForRouting).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("handleReviewStateWritten slug-gates: non-current slug is ignored", async () => {
    vi.useFakeTimers();
    try {
      invokeMock.mockResolvedValue({ kind: "state", state: sampleState() } satisfies ReviewStatePayload);
      const store = useReviewStateStore();
      await store.start(SLUG, "/proj");

      const callsBefore = invokeMock.mock.calls.length;
      store.handleReviewStateWritten("sess-1", "other-task");
      await vi.advanceTimersByTimeAsync(500);
      expect(invokeMock.mock.calls.length).toBe(callsBefore);
    } finally {
      vi.useRealTimers();
    }
  });

  it("handleReviewStateWritten is a no-op when no slug is active", async () => {
    vi.useFakeTimers();
    try {
      invokeMock.mockResolvedValue({ kind: "state", state: sampleState() } satisfies ReviewStatePayload);
      const store = useReviewStateStore();
      // Note: no start() — currentSlug is null.

      store.handleReviewStateWritten("sess-1", SLUG);
      await vi.advanceTimersByTimeAsync(500);
      expect(invokeMock).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("handleReviewStateWritten debounces: multiple writes coalesce to one refresh", async () => {
    vi.useFakeTimers();
    try {
      invokeMock.mockResolvedValue({ kind: "state", state: sampleState() } satisfies ReviewStatePayload);
      const store = useReviewStateStore();
      await store.start(SLUG, "/proj");

      const callsBefore = invokeMock.mock.calls.length;
      // Fire 5 rapid writes; only one refresh should land.
      store.handleReviewStateWritten("sess-1", SLUG);
      store.handleReviewStateWritten("sess-1", SLUG);
      store.handleReviewStateWritten("sess-1", SLUG);
      store.handleReviewStateWritten("sess-1", SLUG);
      store.handleReviewStateWritten("sess-1", SLUG);
      await vi.advanceTimersByTimeAsync(500);

      expect(invokeMock.mock.calls.length).toBe(callsBefore + 1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("handleReviewStateWritten fires exactly one refresh 200ms after the last write", async () => {
    vi.useFakeTimers();
    try {
      invokeMock.mockResolvedValue({ kind: "state", state: sampleState() } satisfies ReviewStatePayload);
      const store = useReviewStateStore();
      await store.start(SLUG, "/proj");

      const callsBefore = invokeMock.mock.calls.length;
      store.handleReviewStateWritten("sess-1", SLUG);

      // 100ms in — not yet fired (debounce window 200ms).
      await vi.advanceTimersByTimeAsync(100);
      expect(invokeMock.mock.calls.length).toBe(callsBefore);

      // Cross the 200ms boundary.
      await vi.advanceTimersByTimeAsync(150);
      expect(invokeMock.mock.calls.length).toBe(callsBefore + 1);
    } finally {
      vi.useRealTimers();
    }
  });
});
