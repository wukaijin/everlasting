// Tests for `MessageList.vue` — BUGLIST CH8-2a (2026-08-29).
//
// Coverage:
//   1. Baseline: user scrolls up (>80px from bottom) → the
//      scroll-to-bottom button appears (pre-existing behavior,
//      guards the fixture).
//   2. A pending interaction appearing for the CURRENT session
//      (null → some) force-scrolls to bottom (scrollTop pinned to
//      scrollHeight) and hides the button — even though the user
//      was reading history.
//   3. some → some (pending replaced / session-switch between two
//      pending sessions) does NOT re-trigger the forced scroll.
//   4. Clearing the pending (some → null) does NOT force anything.
//
// jsdom has no layout: scrollHeight / clientHeight are
// defineProperty'd onto the <ul> per test. The scroll watcher only
// reads them inside isNearBottom, so static values are enough.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";
import { nextTick } from "vue";

const invokeMock = vi.fn(async (): Promise<unknown> => null);

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...(args as Parameters<typeof invokeMock>)),
    listen: async () => () => {},
  },
}));

import MessageList from "./MessageList.vue";
import { useChatStore } from "../../stores/chat";
import { useStreamControllerStore } from "../../stores/streamController";
import { useQuestionCardsStore } from "../../stores/questionCards";
import type { PendingInteraction } from "../../stores/questionCards.types";

function makePending(sessionId: string, toolUseId: string): PendingInteraction {
  return {
    kind: "question",
    payload: {
      session_id: sessionId,
      tool_use_id: toolUseId,
      ts: Date.now(),
      questions: [
        {
          question: "继续吗?",
          options: [{ label: "继续" }, { label: "停止" }],
          multi_select: false,
        },
      ],
    },
  };
}

/** Pin fake layout onto the rendered <ul> and simulate a scrolled-up
 *  position. Returns the element. */
function scrollUp(w: ReturnType<typeof mount>): HTMLElement {
  const el = w.get("ul.messages").element as HTMLElement;
  Object.defineProperty(el, "scrollHeight", { value: 1000, configurable: true });
  Object.defineProperty(el, "clientHeight", { value: 400, configurable: true });
  el.scrollTop = 100; // 1000 - 100 - 400 = 500 > 80 → not near bottom
  el.dispatchEvent(new Event("scroll"));
  return el;
}

/** Mount + let onMounted's stickToBottomUntilStable rAF loop exit
 *  (quietMs=150 → ~200ms real time). Interacting before it exits
 *  means the loop keeps re-pinning scrollTop to scrollHeight and
 *  races the test's own scroll positioning. */
async function mountList() {
  const w = mount(MessageList, {
    attachTo: document.body,
    // TransitionGroup must stay REAL: the scroll logic grabs the
    // rendered <ul> through the component instance's $el (setListEl).
    // VTU stubs transition-group by default, which would break that.
    global: {
      stubs: { MessageItem: true, Icon: true, "transition-group": false },
    },
  });
  await new Promise((r) => setTimeout(r, 300));
  return w;
}

describe("MessageList — pending-interaction force scroll (CH8-2a)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    document.body.innerHTML = "";
  });

  function seedSessionWithMessages(sessionId: string) {
    const store = useChatStore();
    store.sessions = [
      {
        id: sessionId,
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
        session_type: "chat",
        busy: false,
      } as never,
    ];
    store.currentSessionId = sessionId;
    // `store.messages` is a computed over the controller's LRU map —
    // seed the map, not the computed.
    useStreamControllerStore().messagesBySession.set(sessionId, [
      { id: "a1", role: "assistant", content: "历史消息" } as never,
    ]);
    return store;
  }

  it("baseline: scrolled-up user sees the scroll-to-bottom button", async () => {
    seedSessionWithMessages("s1");
    const w = await mountList();
    await nextTick();

    const el = scrollUp(w);
    await nextTick();
    expect(w.find(".scroll-to-bottom").exists()).toBe(true);
    // Pre-condition for the next test: nothing pinned us back.
    expect(el.scrollTop).toBe(100);
    w.unmount();
  });

  it("pending interaction appearing (null→some) force-scrolls to bottom", async () => {
    seedSessionWithMessages("s1");
    const w = await mountList();
    await nextTick();

    const el = scrollUp(w);
    await nextTick();
    expect(w.find(".scroll-to-bottom").exists()).toBe(true);

    // A blocking question registers for the current session.
    useQuestionCardsStore().addPending("s1", makePending("s1", "toolu-1"));
    await flushPromises();

    // scrollTop pinned to scrollHeight + button hidden.
    expect(el.scrollTop).toBe(1000);
    expect(w.find(".scroll-to-bottom").exists()).toBe(false);
    w.unmount();
  });

  it("some→some (pending replaced) does not re-trigger the forced scroll", async () => {
    seedSessionWithMessages("s1");
    useQuestionCardsStore().addPending("s1", makePending("s1", "toolu-1"));
    const w = await mountList();
    await nextTick();

    // User scrolls up AFTER the pending exists.
    const el = scrollUp(w);
    await nextTick();
    expect(w.find(".scroll-to-bottom").exists()).toBe(true);

    // A new pending object (backend overwrite semantics) — identity
    // changes but it is NOT a null→some transition: no forced scroll.
    useQuestionCardsStore().addPending("s1", makePending("s1", "toolu-2"));
    await flushPromises();

    expect(el.scrollTop).toBe(100);
    expect(w.find(".scroll-to-bottom").exists()).toBe(true);
    w.unmount();
  });

  it("some→null (pending resolved) does not force anything", async () => {
    seedSessionWithMessages("s1");
    useQuestionCardsStore().addPending("s1", makePending("s1", "toolu-1"));
    const w = await mountList();
    await nextTick();

    const el = scrollUp(w);
    await nextTick();
    expect(w.find(".scroll-to-bottom").exists()).toBe(true);

    useQuestionCardsStore().removePending("s1");
    await flushPromises();

    expect(el.scrollTop).toBe(100);
    expect(w.find(".scroll-to-bottom").exists()).toBe(true);
    w.unmount();
  });
});
