// Behavior lock for `clearSessionMessages` — BUGLIST CH5-2
// (2026-08-29 GUI full-test): /clear used to leave the chat store's
// cumulative token/latency maps untouched, so the footer showed
// "累计 4.5s" while the latency popover simultaneously reported
// "本次 session 还没有 LLM 耗时数据". The action must now drop the
// session's usage stats (ctx.resetUsageStats) right after evicting
// the controller buffer — matching what a page reload (DB reseed on
// empty rows) would show.
//
// Mocks sit at the ctx boundary (controller / resetUsageStats /
// projectsStore / configStore) and the transport module, mirroring
// createNewSession.test.ts. Pinia is active because the action
// touches useChecklistStore().

import { describe, it, expect, vi, beforeEach } from "vitest";
import { ref } from "vue";
import { createPinia, setActivePinia } from "pinia";

vi.mock("../transport", () => ({
  transport: { invoke: vi.fn() },
}));

import { transport } from "../transport";
import { createSessionActions } from "./chatSessionActions";
import type { SessionActionsContext } from "./chatSessionActions";

const invokeMock = vi.mocked(transport.invoke);

function makeCtx(currentSessionId: string | null, streaming = false) {
  const order: string[] = [];
  const ensureLoaded = vi.fn(async () => {
    order.push("ensureLoaded");
  });
  const evict = vi.fn(() => {
    order.push("evict");
  });
  const resetUsageStats = vi.fn(() => {
    order.push("resetUsageStats");
  });
  const cancel = vi.fn(async () => {
    order.push("cancel");
  });
  const diffCacheMap = new Map<string, unknown>([["s-1", { fake: true }]]);

  const ctx = {
    sessions: ref([]),
    currentSessionId: ref<string | null>(currentSessionId),
    currentCwd: ref(""),
    sessionLoading: ref(false),
    diffCache: ref(diffCacheMap),
    isCurrentSessionStreaming: ref(streaming),
    controller: { ensureLoaded, evict },
    projectsStore: {},
    configStore: {},
    cancel,
    resetUsageStats,
  } as unknown as SessionActionsContext;

  return { ctx, order, ensureLoaded, evict, resetUsageStats, cancel, diffCacheMap };
}

beforeEach(() => {
  invokeMock.mockReset();
  setActivePinia(createPinia());
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "clear_session_messages") {
      return null;
    }
    return null;
  });
});

describe("clearSessionMessages — usage stats reset (BUGLIST CH5-2)", () => {
  it("清库后调用 resetUsageStats(sessionId) 丢弃累计统计", async () => {
    const h = makeCtx("s-1");
    const actions = createSessionActions(h.ctx);

    await actions.clearSessionMessages("s-1");

    expect(invokeMock).toHaveBeenCalledWith("clear_session_messages", {
      sessionId: "s-1",
    });
    expect(h.resetUsageStats).toHaveBeenCalledTimes(1);
    expect(h.resetUsageStats).toHaveBeenCalledWith("s-1");
    // The diff cache entry for the cleared session is dropped too.
    expect(h.diffCacheMap.has("s-1")).toBe(false);
    // Non-streaming path: no cancel.
    expect(h.cancel).not.toHaveBeenCalled();
  });

  it("流式中的当前会话:先 cancel 再清库(resetUsageStats 在 evict 后)", async () => {
    const h = makeCtx("s-1", true);
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "clear_session_messages") {
        h.order.push("clear_session_messages");
        return null;
      }
      return null;
    });
    const actions = createSessionActions(h.ctx);

    await actions.clearSessionMessages("s-1");

    expect(h.cancel).toHaveBeenCalledTimes(1);
    expect(h.order.indexOf("cancel")).toBeLessThan(
      h.order.indexOf("clear_session_messages"),
    );
    expect(h.order.indexOf("evict")).toBeLessThan(
      h.order.indexOf("resetUsageStats"),
    );
  });

  it("非当前会话:清库但不重播 ensureLoaded", async () => {
    const h = makeCtx("s-other");
    const actions = createSessionActions(h.ctx);

    await actions.clearSessionMessages("s-1");

    expect(h.resetUsageStats).toHaveBeenCalledWith("s-1");
    expect(h.ensureLoaded).not.toHaveBeenCalled();
  });
});
