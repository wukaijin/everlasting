// Tests for `useChatStore.send` — the Phase 4 / D9-Q4 preemptive
// interrupt fix (group-chat "human preemption").
//
// Contract under test:
//   1. Group chat (`session_type === "group_chat"`) while streaming:
//      `send` is a NO-OP on the old early-return guard and instead
//      preempts — it fires `cancel_chat` (cancel the in-flight turn)
//      and then continues the normal send path (`chat` IPC fires).
//      This is the "human preemption" semantics: cancel → human
//      message lands → host re-enters turn-taking.
//   2. Ordinary chat (`session_type === "chat"`) while streaming:
//      the original "can't interject while streaming" guard stays —
//      `send` is a no-op, no IPC at all.
//   3. Either session type while NOT streaming: normal send path
//      (`chat` IPC fires, no cancel).
//
// Streaming state is driven by seeding `controller.activeRequests`
// directly (the same reactive source `isCurrentSessionStreaming`
// reads off), so we don't have to spin up a full agent loop. Tauri
// IPC is mocked via `../transport` so these tests run under jsdom.

import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import { setActivePinia, createPinia } from "pinia";

// Single shared invoke mock. Each test asserts call order / args
// against this. `load_session` / `get_pending_interaction` return
// null (empty history); `chat` / `cancel_chat` return null (Tauri
// unit). `list_sessions` returns [] so the project-change watcher's
// `for (const s of sessions.value)` doesn't throw on `null`.
// The invoke mock is typed loosely (`any`) so it can stand in for
// the variadic real `transport.invoke` without fighting `vi.fn`'s
// tuple-typed rest parameters. Call assertions use the same string
// command-name + args shape the production code passes.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const invokeMock: any = vi.fn();
invokeMock.mockImplementation(async (cmd: string) => {
  if (cmd === "list_sessions") return [];
  return null;
});
vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useChatStore } from "./chat";
import { useStreamControllerStore } from "./streamController";
import { useProjectsStore } from "./projects";
import type { SessionSummary } from "./chat.types";

/** A minimal but well-typed SessionSummary seed. Only the fields
 *  `send` reads are varied per-test (`session_type`); the rest are
 *  inert defaults. */
function seedSession(id: string, sessionType: SessionSummary["session_type"]): SessionSummary {
  return {
    id,
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
    workflow_enabled: false,
    plugin_name: "dev",
    session_type: sessionType,
    metadata: null,
  };
}

/** Mark a session as "currently streaming" by seeding the stream
 *  controller's `activeRequests` with one in-flight request — the
 *  exact source `isCurrentSessionStreaming` reads off. Returns the
 *  requestId so a test can assert it matches the `cancel_chat` arg. */
function seedStreamingRequest(
  controller: ReturnType<typeof useStreamControllerStore>,
  sessionId: string,
  requestId: string,
): void {
  controller.activeRequests.set(requestId, {
    requestId,
    sessionId,
    projectId: "p1",
    userMsgId: "u-existing",
    assistantMsgId: "a-existing",
    // 08-04 群聊逐轮流式: plain chat — first `done` finalizes (existing).
    groupChat: false,
    groupChatStarted: false,
    pendingSpeaker: null,
    history: [],
    sendAt: Date.now(),
    firstDeltaAt: null,
    toolStartedAt: new Map(),
    currentTurnIndex: -1,
    latencyByTurn: new Map(),
    pendingTimelineText: null,
    activeThinkingIdx: null,
  });
}

/** Names of the IPC commands `send`'s normal path touches, in order.
 *  `cancel_chat` is the preemptive cancel. We filter out the
 *  follow-up reads (`load_session`, `get_pending_interaction`,
 *  `record_tool_duration`, `update_message_latency`) so the call
 *  order assertions stay focused on the cancel/chat lifecycle. */
function lifecycleCalls(): string[] {
  return (invokeMock.mock.calls as Array<[string, unknown]>)
    .map((c) => c[0])
    .filter(
      (cmd: string) =>
        cmd === "cancel_chat" ||
        cmd === "chat",
    );
}

describe("useChatStore.send — Phase 4 D9-Q4 human preemption", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    // Restore the default implementation (kept here so `mockReset`
    // in afterEach / a stray `mockResolvedValue` in a test doesn't
    // leave `list_sessions` returning null and breaking the
    // project-change watcher). Tests that need to assert on a
    // specific command still can — the implementation just defaults.
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "list_sessions" ? [] : null,
    );
  });

  afterEach(() => {
    invokeMock.mockReset();
  });

  /** Set up a project + a single seeded session so `send` finds an
   *  active session. The project-change watcher in `chat.ts` is
   *  async (`onProjectChange` → `loadSessions` → `list_sessions`),
   *  and its tail sets `currentSessionId = null` when the list is
   *  empty — so we MUST await that tail settle before stamping our
   *  own `sessions` / `currentSessionId`, or the watcher clobbers
   *  them on the next microtask. */
  async function setupProjectAndSession(
    sessionId: string,
    sessionType: SessionSummary["session_type"],
  ): Promise<void> {
    const projects = useProjectsStore();
    projects.currentProjectId = "p1";
    const store = useChatStore();
    // Let the async `onProjectChange` watcher settle (it sets
    // sessions=[] and currentSessionId=null).
    await Promise.resolve();
    await Promise.resolve();
    store.sessions = [seedSession(sessionId, sessionType)];
    store.currentSessionId = sessionId;
  }

  it("group_chat + streaming: preempts — cancel_chat THEN chat", async () => {
    await setupProjectAndSession("s1", "group_chat");
    const controller = useStreamControllerStore();
    const store = useChatStore();

    const rid = "req-in-flight";
    seedStreamingRequest(controller, "s1", rid);
    // Sanity: the guard's streaming predicate sees the session.
    expect(store.isCurrentSessionStreaming).toBe(true);

    await store.send("let me jump in");

    const calls = lifecycleCalls();
    expect(calls).toEqual(["cancel_chat", "chat"]);
    // The cancel must target the in-flight request.
    expect(invokeMock).toHaveBeenCalledWith("cancel_chat", { requestId: rid });
  });

  it("ordinary chat + streaming: queued send goes through (F1 message queue)", async () => {
    // F1 消息队列 (2026-08-25): 经典 session 流式期间发送不再丢弃 ——
    // 走后端入队路径(仍发 `chat`,不打断在途请求)。原"no-op 守卫"
    // 契约由本用例取代;排队徽标/续轮注入的渲染契约见
    // messageQueueStream.test.ts。
    await setupProjectAndSession("s1", "chat");
    const controller = useStreamControllerStore();
    const store = useChatStore();

    seedStreamingRequest(controller, "s1", "req-in-flight");
    expect(store.isCurrentSessionStreaming).toBe(true);

    await store.send("queued while busy");

    // 不取消在途请求(排队 ≠ 打断),但 chat IPC 必须发生。
    expect(lifecycleCalls()).toEqual(["chat"]);
    expect(invokeMock).not.toHaveBeenCalledWith(
      "cancel_chat",
      expect.anything(),
    );
  });

  it("group_chat + NOT streaming: normal send, no cancel", async () => {
    await setupProjectAndSession("s1", "group_chat");
    const store = useChatStore();

    expect(store.isCurrentSessionStreaming).toBe(false);

    await store.send("first message");

    const calls = lifecycleCalls();
    expect(calls).toEqual(["chat"]);
    expect(invokeMock).not.toHaveBeenCalledWith(
      "cancel_chat",
      expect.anything(),
    );
  });

  it("empty input is rejected even for group_chat", async () => {
    await setupProjectAndSession("s1", "group_chat");
    const store = useChatStore();

    await store.send("   ");

    expect(lifecycleCalls()).toEqual([]);
  });
});
