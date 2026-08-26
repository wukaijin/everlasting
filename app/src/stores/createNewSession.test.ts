// F1 follow-up — `createNewSession` behavior lock:
//   - a fresh session is persisted as last-active IMMEDIATELY at
//     creation (before the ensureLoaded/loadSessions awaits) — this
//     path bypasses switchSession / onProjectChange, the other two
//     write points, so a create → send → quit round trip would
//     otherwise reopen the previous session on restart;
//   - no active project → throws and never writes.
//
// Mocks sit at the ctx boundary (controller.ensureLoaded /
// projectsStore) and the transport module, mirroring
// openSessionInProject.test.ts.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { ref } from "vue";

vi.mock("../transport", () => ({
  transport: { invoke: vi.fn() },
}));

import { transport } from "../transport";
import { createSessionActions } from "./chatSessionActions";
import type { SessionActionsContext } from "./chatSessionActions";

const invokeMock = vi.mocked(transport.invoke);

function makeCtx(currentProjectId: string | null) {
  const writeLastSession = vi.fn(() => {
    order.push("write");
  });
  const order: string[] = [];
  const ensureLoaded = vi.fn(async () => {
    order.push("ensureLoaded");
  });
  const projectsStore = {
    currentProjectId,
    projectById: vi.fn((id: string) => ({ id, path: `/code/${id}` })),
  };

  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "create_session") {
      order.push("create_session");
      return {
        id: "s-new",
        title: "",
        created_at: "",
        updated_at: "",
        model: "",
        project_id: currentProjectId,
        current_cwd: `/code/${currentProjectId}`,
      };
    }
    if (cmd === "list_sessions") {
      order.push("list_sessions");
      return [];
    }
    return null;
  });

  const ctx = {
    sessions: ref([]),
    currentSessionId: ref<string | null>(null),
    currentCwd: ref(""),
    sessionLoading: ref(false),
    diffCache: ref(new Map()),
    isCurrentSessionStreaming: ref(false),
    controller: { ensureLoaded },
    projectsStore,
    configStore: { writeLastSession },
    cancel: vi.fn(),
  } as unknown as SessionActionsContext;

  return { ctx, order, writeLastSession };
}

beforeEach(() => {
  invokeMock.mockReset();
});

describe("createNewSession last-session persistence", () => {
  it("persists the new id under the current project", async () => {
    const h = makeCtx("pa");
    const actions = createSessionActions(h.ctx);

    await actions.createNewSession();

    expect(h.writeLastSession).toHaveBeenCalledTimes(1);
    expect(h.writeLastSession).toHaveBeenCalledWith("pa", "s-new");
  });

  it("writes before the ensureLoaded / loadSessions awaits (crash-safe)", async () => {
    const h = makeCtx("pa");
    const actions = createSessionActions(h.ctx);

    await actions.createNewSession();

    expect(h.order).toEqual([
      "create_session",
      "write",
      "ensureLoaded",
      "list_sessions",
    ]);
  });

  it("no active project throws without writing", async () => {
    const h = makeCtx(null);
    const actions = createSessionActions(h.ctx);

    await expect(actions.createNewSession()).rejects.toThrow(
      "createNewSession: no current project",
    );
    expect(h.writeLastSession).not.toHaveBeenCalled();
  });
});
