// D2 (08-17-cross-session-search) — `openSessionInProject`
// behavior lock (review P2 方案甲):
//   - same-project target → plain switchSession (no project churn);
//   - cross-project target → switchProject THEN an EXPLICIT awaited
//     loadSessions THEN switchSession (racing the chat.ts watcher
//     would blank currentCwd + miswrite lastSession);
//   - writeLastSession always records under the TARGET project.
//
// Mocks sit at the ctx boundary (controller.ensureLoaded /
// projectsStore.switchProject) and the transport module — NOT on
// the returned actions object: `openSessionInProject` closes over
// the INTERNAL loadSessions/switchSession bindings, so spying on
// the returned-object properties would never intercept them.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { ref } from "vue";

vi.mock("../transport", () => ({
  transport: { invoke: vi.fn() },
}));

import { transport } from "../transport";
import { createSessionActions } from "./chatSessionActions";

const invokeMock = vi.mocked(transport.invoke);
import type { SessionActionsContext } from "./chatSessionActions";
import type { SessionSummary } from "./chat.types";

function makeCtx(currentProjectId: string | null) {
  const sessions = ref<SessionSummary[]>([]);
  const currentSessionId = ref<string | null>(null);
  const currentCwd = ref("");
  const sessionLoading = ref(false);
  const writeLastSession = vi.fn();
  const order: string[] = [];
  const ensureLoaded = vi.fn(async () => {
    order.push("switchSession");
  });
  const switchProject = vi.fn(async () => {
    order.push("switchProject");
    // Emulate the real store flipping currentProjectId so the
    // subsequent switchSession's writeLastSession sees the target.
    projectsStore.currentProjectId = flippedTo ?? projectsStore.currentProjectId;
  });
  const projectsStore: { currentProjectId: string | null; switchProject: ReturnType<typeof vi.fn> } = {
    currentProjectId,
    switchProject,
  };
  let flippedTo: string | null = null;

  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "list_sessions") {
      order.push("loadSessions");
      return [];
    }
    return null;
  });

  const ctx = {
    sessions,
    currentSessionId,
    currentCwd,
    sessionLoading,
    diffCache: ref(new Map()),
    isCurrentSessionStreaming: ref(false),
    controller: { ensureLoaded },
    projectsStore,
    configStore: { writeLastSession },
    cancel: vi.fn(),
  } as unknown as SessionActionsContext;

  return {
    ctx,
    order,
    writeLastSession,
    ensureLoaded,
    switchProject,
    flipTo(projectId: string) {
      flippedTo = projectId;
    },
  };
}

beforeEach(() => {
  invokeMock.mockReset();
});

describe("openSessionInProject", () => {
  it("same-project target degrades to plain switchSession", async () => {
    const h = makeCtx("pa");
    const actions = createSessionActions(h.ctx);

    await actions.openSessionInProject("pa", "s1");

    expect(h.switchProject).not.toHaveBeenCalled();
    expect(h.order).toEqual(["switchSession"]);
    expect(invokeMock).not.toHaveBeenCalledWith("list_sessions", expect.anything());
  });

  it("cross-project target switches project, awaits sessions, then switches session", async () => {
    const h = makeCtx("pa");
    h.flipTo("pb");
    const actions = createSessionActions(h.ctx);

    await actions.openSessionInProject("pb", "s-in-b");

    expect(h.switchProject).toHaveBeenCalledWith("pb");
    expect(h.order).toEqual(["switchProject", "loadSessions", "switchSession"]);
    expect(invokeMock).toHaveBeenCalledWith("list_sessions", { projectId: "pb" });
    expect(h.ensureLoaded).toHaveBeenCalledWith("s-in-b");
  });

  it("cross-project writeLastSession records under the TARGET project", async () => {
    const h = makeCtx("pa");
    h.flipTo("pb");
    const actions = createSessionActions(h.ctx);

    await actions.openSessionInProject("pb", "s-in-b");

    expect(h.writeLastSession).toHaveBeenCalledTimes(1);
    expect(h.writeLastSession).toHaveBeenCalledWith("pb", "s-in-b");
  });
});
