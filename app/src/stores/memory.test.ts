// Tests for `useMemoryStore` — P2 PR3 additions: `fetchMemories` +
// `deleteMemory` for runtime autonomous memories.
//
// Coverage:
//   1. `fetchMemories` happy path — invokes `list_autonomous_memories`
//      and populates `runtimeMemories`.
//   2. `fetchMemories` error path — populates `runtimeMemoriesError`
//      and leaves the previous list intact (defensive).
//   3. `fetchMemories` with no project set — short-circuits without
//      calling IPC and clears the list (defensive).
//   4. `deleteMemory` happy path — resolves the row's `memoryId`
//      (UUID v7) from the auto-id, invokes `delete_autonomous_memory`,
//      and optimistically removes the row.
//   5. `deleteMemory` error path — populates `runtimeMemoriesError`
//      and leaves the list unchanged.
//   6. `deleteMemory` with unknown id — no-op (defensive).
//   7. `loadForProject` also triggers the runtime-memories fetch
//      in parallel (P2 PR3 contract: the panel's mount / project
//      switch should populate BOTH sections in one tick).
//   8. `loadForProject` failure on the instruction-file side does
//      not block the runtime-memories fetch (and vice versa).
//
// Tauri IPC is mocked so the suite runs in jsdom without a real
// Tauri runtime. The mocks follow the pattern in
// `app/src/stores/projects.test.ts` (file-level vi.mock).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));


import { useMemoryStore, type AutonomousMemory, type RecallHit } from "./memory";

function makeMemory(overrides: Partial<AutonomousMemory> = {}): AutonomousMemory {
  return {
    id: 1,
    memoryId: "0190f6e7-2a8b-7e8e-b123-456789abcdef",
    scope: "project",
    projectId: "proj-1",
    kind: "preference",
    status: "candidate",
    title: "Prefer absolute paths in code blocks",
    content: "Always use absolute paths in tool outputs and code examples.",
    tags: '["paths","preference"]',
    toolName: null,
    commandPattern: null,
    pathGlobs: null,
    sourceSessionId: "sess-1",
    sourceRef: "remember tool call 3",
    confidence: 0.5,
    hitCount: 0,
    lastUsedAt: null,
    createdAt: "2026-06-29T12:34:56.789+00:00",
    updatedAt: "2026-06-29T12:34:56.789+00:00",
    demotedReason: null,
    editedByUser: false,
    ...overrides,
  };
}

const USER_MEMORY = makeMemory({
  id: 10,
  memoryId: "uid-1",
  scope: "user",
  projectId: null,
  title: "User-level preference",
  content: "Likes concise replies.",
  tags: "[]",
});

const PROJECT_MEMORY_A = makeMemory({
  id: 11,
  memoryId: "pid-a-1",
  scope: "project",
  projectId: "proj-1",
  title: "Project A pitfall",
  content: "WSL cargo test fails on gdk-pixbuf not found.",
  kind: "pitfall",
  toolName: "shell",
  commandPattern: "cargo test",
  pathGlobs: null,
});

const PROJECT_MEMORY_B = makeMemory({
  id: 12,
  memoryId: "pid-b-1",
  scope: "project",
  projectId: "proj-2",
  title: "Project B fact",
  content: "DB schema v3 was a one-off.",
});

describe("useMemoryStore — fetchMemories (P2 PR3)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    // Default: instruction-file IPC returns a 2-layer set (User +
    // Project for proj-1) so the store enters a usable state without
    // surprising tests. Per-test cases override.
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "read_memory_layers") {
        return [
          {
            kind: "user",
            source: "claude",
            path: "/home/x/.claude/CLAUDE.md",
            tokens: 0,
            status: { kind: "missing" },
            char_count: 0,
          },
        ];
      }
      if (cmd === "list_autonomous_memories") {
        return [USER_MEMORY, PROJECT_MEMORY_A];
      }
      if (cmd === "delete_autonomous_memory") return 1;
      return null;
    });
  });

  it("happy path: invokes list_autonomous_memories and populates runtimeMemories", async () => {
    const store = useMemoryStore();
    // First seed a project so the fetch has a projectId to send.
    await store.loadForProject("proj-1");

    // Reset call history so we can assert the runtime-memory IPC
    // call was issued by `fetchMemories` specifically (not just by
    // `loadForProject`).
    const callsBefore = invokeMock.mock.calls.length;
    invokeMock.mockClear();

    // Replace the IPC stub to return a known list.
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_autonomous_memories") return [USER_MEMORY, PROJECT_MEMORY_A];
      return null;
    });

    await store.fetchMemories();

    expect(store.runtimeMemories).toHaveLength(2);
    expect(store.runtimeMemories[0]?.memoryId).toBe("uid-1");
    expect(store.runtimeMemories[1]?.memoryId).toBe("pid-a-1");
    expect(store.runtimeMemoriesLoading).toBe(false);
    expect(store.runtimeMemoriesError).toBeNull();

    // The IPC call must have used camelCase `projectId` (matches
    // the Rust Tauri 2 convention; snake_case would silently miss
    // the param).
    const listCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "list_autonomous_memories",
    );
    expect(listCalls.length).toBe(1);
    expect(listCalls[0]?.[1]).toEqual({ projectId: "proj-1" });

    // sanity: loadForProject fired at least one runtime-memory
    // call too (the parallel-fetch contract below is verified in
    // a dedicated test).
    expect(callsBefore).toBeGreaterThan(0);
  });

  it("error path: populates runtimeMemoriesError and leaves the previous list intact", async () => {
    const store = useMemoryStore();
    await store.loadForProject("proj-1");
    // Pre-populate the list with a successful fetch.
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_autonomous_memories") return [USER_MEMORY];
      return null;
    });
    await store.fetchMemories();
    expect(store.runtimeMemories).toHaveLength(1);
    const before = store.runtimeMemories[0]?.memoryId;

    // Now fail the next fetch.
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_autonomous_memories") {
        throw new Error("list_autonomous_memories: query failed: connection lost");
      }
      return null;
    });
    await store.fetchMemories();

    expect(store.runtimeMemoriesError).toBe(
      "list_autonomous_memories: query failed: connection lost",
    );
    // Defensive: the previous list is NOT cleared on failure.
    expect(store.runtimeMemories).toHaveLength(1);
    expect(store.runtimeMemories[0]?.memoryId).toBe(before);
    expect(store.runtimeMemoriesLoading).toBe(false);
  });

  it("with no project: short-circuits without calling IPC and clears the list", async () => {
    const store = useMemoryStore();
    // Don't call loadForProject — lastProjectId stays null.
    invokeMock.mockClear();

    await store.fetchMemories();

    expect(store.runtimeMemories).toEqual([]);
    expect(store.runtimeMemoriesError).toBeNull();
    // No IPC call should have been made.
    expect(invokeMock).not.toHaveBeenCalled();
  });
});

describe("useMemoryStore — deleteMemory (P2 PR3)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") {
        return [USER_MEMORY, PROJECT_MEMORY_A];
      }
      if (cmd === "delete_autonomous_memory") return 1;
      return null;
    });
  });

  it("happy path: resolves memoryId from auto-id, invokes IPC, optimistically removes the row", async () => {
    const store = useMemoryStore();
    await store.loadForProject("proj-1");
    expect(store.runtimeMemories).toHaveLength(2);

    invokeMock.mockClear();
    await store.deleteMemory(10); // auto-id 10 → memoryId "uid-1"

    // IPC: must have called delete_autonomous_memory with the
    // UUID v7 (NOT the auto-id).
    const deleteCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "delete_autonomous_memory",
    );
    expect(deleteCalls).toHaveLength(1);
    expect(deleteCalls[0]?.[1]).toEqual({ memoryId: "uid-1" });

    // Optimistic remove.
    expect(store.runtimeMemories).toHaveLength(1);
    expect(store.runtimeMemories[0]?.memoryId).toBe("pid-a-1");
    expect(store.runtimeMemoriesError).toBeNull();
  });

  it("error path: populates runtimeMemoriesError and leaves the list unchanged", async () => {
    const store = useMemoryStore();
    await store.loadForProject("proj-1");
    expect(store.runtimeMemories).toHaveLength(2);

    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "delete_autonomous_memory") {
        throw new Error("delete_autonomous_memory: delete failed: db busy");
      }
      return null;
    });

    await store.deleteMemory(10);

    expect(store.runtimeMemoriesError).toBe(
      "delete_autonomous_memory: delete failed: db busy",
    );
    // The failed row is still in the list.
    expect(store.runtimeMemories).toHaveLength(2);
    expect(store.runtimeMemories.find((m) => m.id === 10)).toBeDefined();
  });

  it("unknown id: no-op (defensive — race against concurrent delete)", async () => {
    const store = useMemoryStore();
    await store.loadForProject("proj-1");
    invokeMock.mockClear();

    // id 999 is not in the list.
    await store.deleteMemory(999);

    // No IPC call should have been made (we never resolved a row).
    expect(invokeMock).not.toHaveBeenCalled();
    // The list is unchanged.
    expect(store.runtimeMemories).toHaveLength(2);
  });
});

describe("useMemoryStore — loadForProject / refresh fire both fetches", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") return [USER_MEMORY];
      if (cmd === "delete_autonomous_memory") return 1;
      return null;
    });
  });

  it("loadForProject fires BOTH read_memory_layers and list_autonomous_memories", async () => {
    const store = useMemoryStore();
    await store.loadForProject("proj-1");

    const cmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(cmds).toContain("read_memory_layers");
    expect(cmds).toContain("list_autonomous_memories");
    // The runtime-memory state must be populated.
    expect(store.runtimeMemories).toHaveLength(1);
    expect(store.runtimeMemories[0]?.memoryId).toBe("uid-1");
  });

  it("refresh fires BOTH IPCs against the last loaded project", async () => {
    const store = useMemoryStore();
    await store.loadForProject("proj-1");
    invokeMock.mockClear();
    await store.refresh();

    const cmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(cmds).toContain("read_memory_layers");
    expect(cmds).toContain("list_autonomous_memories");
  });

  it("loadForProject: instruction-file failure does NOT block the runtime-memory fetch", async () => {
    const store = useMemoryStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "read_memory_layers") {
        throw new Error("read_memory_layers: project 'proj-1' not found");
      }
      if (cmd === "list_autonomous_memories") return [USER_MEMORY];
      return null;
    });

    await store.loadForProject("proj-1");

    // Instruction-file error was captured.
    expect(store.error).toContain("not found");
    // The runtime-memory fetch is independent — it must still have
    // populated the list.
    expect(store.runtimeMemories).toHaveLength(1);
    expect(store.runtimeMemoriesError).toBeNull();
  });

  it("loadForProject: runtime-memory failure does NOT block the instruction-file fetch", async () => {
    const store = useMemoryStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") {
        throw new Error("list_autonomous_memories: query failed");
      }
      return null;
    });

    await store.loadForProject("proj-1");

    expect(store.error).toBeNull();
    expect(store.runtimeMemoriesError).toContain("query failed");
    // The instruction-file state populated normally.
    expect(store.layers).toEqual([]);
  });
});

describe("useMemoryStore — fetchLayers clears the runtime list on project change", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") return [USER_MEMORY];
      return null;
    });
  });

  it("switching projects clears runtimeMemories (no leak between projects)", async () => {
    const store = useMemoryStore();
    await store.loadForProject("proj-1");
    expect(store.runtimeMemories).toHaveLength(1);

    // Switch to proj-2 — the fetchLayers call clears the list
    // BEFORE the new fetch lands. (The new fetch lands almost
    // instantly in the test, so we sample the state mid-flight
    // by inspecting the post-load state: it should reflect the
    // project-2 IPC stub.)
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") return [PROJECT_MEMORY_B];
      return null;
    });
    await store.loadForProject("proj-2");

    // The list now reflects proj-2's IPC response (not proj-1's
    // stale USER_MEMORY). This proves the clear-on-switch contract.
    expect(store.runtimeMemories).toHaveLength(1);
    expect(store.runtimeMemories[0]?.memoryId).toBe("pid-b-1");
  });
});

// 07-06 (am-observability-panel B1): status transitions + edits +
// recall-hit accumulation. Mirrors the optimistic + rollback pattern
// of the existing `deleteMemory` tests — IPC failure must leave the
// row (or recall slice) at its pre-patch state and surface the error
// in `runtimeMemoriesError`.
describe("useMemoryStore — 07-06 status / edit / recall", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  function makeHit(overrides: Partial<RecallHit> = {}): RecallHit {
    return {
      memory_id: 1,
      title: "Pitfall hit",
      kind: "pitfall",
      source: "pitfall",
      ...overrides,
    };
  }

  // -----------------------------------------------------------------
  // updateMemoryStatus
  // -----------------------------------------------------------------

  it("updateMemoryStatus patches the row optimistically and clears error on success", async () => {
    const store = useMemoryStore();
    // Seed a candidate row directly via the runtime list.
    store.runtimeMemories = [makeMemory({ id: 5, status: "candidate" })];
    invokeMock.mockResolvedValue(undefined);
    const ok = await store.updateMemoryStatus(5, "active");
    expect(ok).toBe(true);
    expect(store.runtimeMemories[0]?.status).toBe("active");
    expect(store.runtimeMemoriesError).toBeNull();
    expect(invokeMock).toHaveBeenCalledWith("update_autonomous_memory_status", {
      memoryId: store.runtimeMemories[0]?.memoryId,
      newStatus: "active",
      demotedReason: null,
    });
  });

  it("updateMemoryStatus → demoted carries the demoteReason", async () => {
    const store = useMemoryStore();
    store.runtimeMemories = [makeMemory({ id: 5, status: "active" })];
    invokeMock.mockResolvedValue(undefined);
    await store.updateMemoryStatus(5, "demoted", "stale advice");
    expect(store.runtimeMemories[0]?.status).toBe("demoted");
    expect(store.runtimeMemories[0]?.demotedReason).toBe("stale advice");
    expect(invokeMock).toHaveBeenCalledWith("update_autonomous_memory_status", {
      memoryId: store.runtimeMemories[0]?.memoryId,
      newStatus: "demoted",
      demotedReason: "stale advice",
    });
  });

  it("updateMemoryStatus rolls back on IPC failure (illegal transition / network)", async () => {
    const store = useMemoryStore();
    store.runtimeMemories = [
      makeMemory({ id: 5, status: "active", demotedReason: null }),
    ];
    invokeMock.mockRejectedValue(new Error("illegal transition"));
    const ok = await store.updateMemoryStatus(5, "candidate");
    expect(ok).toBe(false);
    // Rolled back to pre-patch status.
    expect(store.runtimeMemories[0]?.status).toBe("active");
    expect(store.runtimeMemoriesError).toContain("illegal transition");
  });

  it("updateMemoryStatus on an unknown id is a no-op (returns false)", async () => {
    const store = useMemoryStore();
    store.runtimeMemories = [makeMemory({ id: 5 })];
    const ok = await store.updateMemoryStatus(999, "active");
    expect(ok).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  // -----------------------------------------------------------------
  // updateMemory
  // -----------------------------------------------------------------

  it("updateMemory patches title/content/editedByUser optimistically and applies the IPC response on success", async () => {
    const store = useMemoryStore();
    store.runtimeMemories = [
      makeMemory({ id: 5, title: "Old", content: "old body", editedByUser: false }),
    ];
    // The IPC returns the authoritative post-update row.
    const authoritative = makeMemory({
      id: 5,
      title: "New",
      content: "new body",
      editedByUser: true,
      updatedAt: "2026-07-06T10:00:00.000+00:00",
    });
    invokeMock.mockResolvedValue(authoritative);
    const ok = await store.updateMemory(5, "New", "new body");
    expect(ok).toBe(true);
    expect(store.runtimeMemories[0]?.title).toBe("New");
    expect(store.runtimeMemories[0]?.content).toBe("new body");
    expect(store.runtimeMemories[0]?.editedByUser).toBe(true);
    // The server's `updated_at` wins over the optimistic placeholder.
    expect(store.runtimeMemories[0]?.updatedAt).toBe(
      "2026-07-06T10:00:00.000+00:00",
    );
    expect(store.runtimeMemoriesError).toBeNull();
    expect(invokeMock).toHaveBeenCalledWith("update_autonomous_memory", {
      memoryId: store.runtimeMemories[0]?.memoryId,
      title: "New",
      content: "new body",
    });
  });

  it("updateMemory rolls back on IPC failure (sensitive content rejected)", async () => {
    const store = useMemoryStore();
    const original = makeMemory({
      id: 5,
      title: "Old",
      content: "old body",
      editedByUser: false,
    });
    store.runtimeMemories = [{ ...original }];
    invokeMock.mockRejectedValue(new Error("sensitive pattern detected"));
    const ok = await store.updateMemory(5, "New", "SECRET_KEY=...");
    expect(ok).toBe(false);
    // Fully rolled back to the snapshot.
    expect(store.runtimeMemories[0]?.title).toBe("Old");
    expect(store.runtimeMemories[0]?.content).toBe("old body");
    expect(store.runtimeMemories[0]?.editedByUser).toBe(false);
    expect(store.runtimeMemoriesError).toContain("sensitive pattern");
  });

  // -----------------------------------------------------------------
  // recall-hit accumulation
  // -----------------------------------------------------------------

  it("pushRecallHits accumulates per-session (append, not replace)", () => {
    const store = useMemoryStore();
    store.pushRecallHits("sess-1", [makeHit({ memory_id: 1 })]);
    store.pushRecallHits("sess-1", [makeHit({ memory_id: 2 })]);
    expect(store.recallHitsForSession("sess-1")).toHaveLength(2);
  });

  it("pushRecallHits isolates sessions (sess-1 ≠ sess-2)", () => {
    const store = useMemoryStore();
    store.pushRecallHits("sess-1", [makeHit({ memory_id: 1 })]);
    store.pushRecallHits("sess-2", [makeHit({ memory_id: 2 })]);
    expect(store.recallHitsForSession("sess-1")).toHaveLength(1);
    expect(store.recallHitsForSession("sess-2")).toHaveLength(1);
  });

  it("pushRecallHits ignores empty / malformed payloads (defensive)", () => {
    const store = useMemoryStore();
    store.pushRecallHits("sess-1", []);
    // @ts-expect-error — defensive against a future wire change
    store.pushRecallHits("sess-1", undefined);
    expect(store.recallHitsForSession("sess-1")).toHaveLength(0);
  });

  it("clearRecallHits clears the session's slice", () => {
    const store = useMemoryStore();
    store.pushRecallHits("sess-1", [makeHit({ memory_id: 1 })]);
    store.clearRecallHits("sess-1");
    expect(store.recallHitsForSession("sess-1")).toHaveLength(0);
  });

  it("recallHitsForSession is a pure read (returns [] for null / unknown)", () => {
    const store = useMemoryStore();
    expect(store.recallHitsForSession(null)).toEqual([]);
    expect(store.recallHitsForSession("never-seen")).toEqual([]);
  });
});
