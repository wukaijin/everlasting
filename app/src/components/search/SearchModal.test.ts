// D2 (08-17-cross-session-search) — SearchModal component tests.
//
// Coverage:
//   1. Debounced query → `search_messages` invoke with the wire
//      contract ({ query, projectId: null, limit: 50 }).
//   2. Results render: title section first, content hits grouped
//      project → session, "还有 N 条" folding.
//   3. Clicking a content hit switches to the preview state and
//      loads the target session via `load_session` (read-only
//      snapshot — never ensureLoaded/switchSession).
//
// reka-ui DialogPortal teleports to document.body, so DOM queries
// go against `document.body` (attachTo), not the wrapper.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { nextTick } from "vue";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

vi.mock("../../transport", () => ({
  transport: { invoke: vi.fn() },
}));

import { transport } from "../../transport";
import SearchModal from "./SearchModal.vue";
import { useChatStore } from "../../stores/chat";

const invokeMock = vi.mocked(transport.invoke);
import { useSearchModal } from "../../composables/useSearchModal";
import type { MessageSearchHit } from "../../stores/chat.types";

// jsdom lacks scrollIntoView (SearchPreviewBody calls it on reveal).
beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
});
beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
});

function contentHit(over: Partial<MessageSearchHit> = {}): MessageSearchHit {
  return {
    kind: "content",
    session_id: "s1",
    session_title: "Session One",
    project_id: "pa",
    project_name: "Project A",
    updated_at: "2026-08-17T00:00:00Z",
    seq: 3,
    role: "user",
    speaker: null,
    snippet: "…the matched 部分 text…",
    ...over,
  };
}

async function mountOpen() {
  const { open } = useSearchModal();
  open();
  const wrapper = mount(SearchModal, { attachTo: document.body });
  await flushPromises();
  return wrapper;
}

describe("SearchModal", () => {
  it("debounces the query and invokes search_messages with the wire contract", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValue([]);
    const wrapper = await mountOpen();
    const input = document.body.querySelector<HTMLInputElement>(".search-modal__input");
    expect(input).not.toBeNull();
    input!.value = "权限";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    expect(invokeMock).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(260);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("search_messages", {
      query: "权限",
      projectId: null,
      limit: 50,
    });
    vi.useRealTimers();
    wrapper.unmount();
  });

  it("Enter triggers the search immediately (no debounce wait) and lands a result-status line", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValue([contentHit()]);
    const wrapper = await mountOpen();
    const input = document.body.querySelector<HTMLInputElement>(".search-modal__input");
    input!.value = "权限";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    // Enter must bypass the 250ms debounce — fired with 0 advance.
    input!.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(document.body.textContent).toContain("找到 1 条命中");
    vi.useRealTimers();
    wrapper.unmount();
  });

  it("zero-hit search echoes the searched query (distinct from never-searched placeholder)", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValue([]);
    const wrapper = await mountOpen();
    const input = document.body.querySelector<HTMLInputElement>(".search-modal__input");
    input!.value = "量子纠缠";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(260);
    await flushPromises();
    const state = document.body.querySelector(".search-modal__state");
    expect(state?.textContent).toContain("量子纠缠");
    expect(state?.textContent).toContain("没有找到");
    vi.useRealTimers();
    wrapper.unmount();
  });

  it("typed-but-not-yet-searched shows an Enter hint, never an empty-echo with blank query", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValue([]);
    const wrapper = await mountOpen();
    const input = document.body.querySelector<HTMLInputElement>(".search-modal__input");
    // Type WITHOUT advancing the debounce timer — the 250ms gap used
    // to fall into the empty-result branch (`没有找到与 "" 匹配`).
    input!.value = "开始";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    await flushPromises();
    const body = document.body.textContent ?? "";
    expect(body).toContain("回车立即搜索");
    expect(body).not.toContain('""');
    vi.useRealTimers();
    wrapper.unmount();
  });

  it("clicking a project chip keeps the chip row alive (chips from unfiltered runs only)", async () => {
    vi.useFakeTimers();
    const paHit = contentHit();
    const pbHit = contentHit({ session_id: "s2", project_id: "pb", project_name: "Project B" });
    invokeMock.mockImplementation(async (_cmd: string, args?: Record<string, unknown>) =>
      (args?.projectId ?? null) === null ? [paHit, pbHit] : [pbHit],
    );
    const wrapper = await mountOpen();
    const input = document.body.querySelector<HTMLInputElement>(".search-modal__input");
    input!.value = "权限";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(260);
    await flushPromises();

    // Two projects in chips.
    let chips = [...document.body.querySelectorAll(".search-modal__chip")];
    expect(chips.length).toBe(3); // 全部 + pa + pb
    (chips.find((c) => c.textContent?.includes("Project B")) as HTMLButtonElement)!.click();
    await vi.advanceTimersByTimeAsync(0);
    await flushPromises();

    // Filtered search ran scoped…
    const scopedCall = invokeMock.mock.calls.find(
      (c) => (c[1] as Record<string, unknown>)?.projectId === "pb",
    );
    expect(scopedCall).toBeTruthy();
    // …and the chip row SURVIVED the filtered result set (previously
    // the row vanished because chips derived from filtered hits).
    chips = [...document.body.querySelectorAll(".search-modal__chip")];
    expect(chips.length).toBe(3);
    expect(chips[0]?.textContent).toContain("全部");
    vi.useRealTimers();
    wrapper.unmount();
  });

  it("renders title hits first and groups content hits project→session", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValue([
      contentHit(),
      contentHit({ session_id: "s1", seq: 7, snippet: "second hit in same session" }),
      contentHit({ session_id: "s2", project_id: "pb", project_name: "Project B" }),
      {
        kind: "title",
        session_id: "s9",
        session_title: "权限系统重构",
        project_id: "pa",
        project_name: "Project A",
        updated_at: "2026-08-16T00:00:00Z",
        seq: null,
        role: null,
        speaker: null,
        snippet: null,
      },
    ]);
    const wrapper = await mountOpen();
    const input = document.body.querySelector<HTMLInputElement>(".search-modal__input");
    input!.value = "权限";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(260);
    await flushPromises();

    const sections = [...document.body.querySelectorAll(".search-modal__section-title")];
    // First section: title hits.
    expect(sections[0]?.textContent).toContain("会话标题");
    // Project A group with session folding ("还有 1 条").
    const body = document.body.textContent ?? "";
    expect(body).toContain("Project A");
    expect(body).toContain("Project B");
    expect(body).toContain("还有 1 条");
    vi.useRealTimers();
    wrapper.unmount();
  });

  it("clicking a content hit opens the read-only preview (load_session, not switch)", async () => {
    vi.useFakeTimers();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "search_messages") return [contentHit()];
      if (cmd === "load_session") {
        return {
          session: { id: "s1" },
          messages: [
            {
              id: 1,
              session_id: "s1",
              role: "user",
              content: [{ type: "text", text: "hello 权限" }],
              text: "hello 权限",
              has_tool_calls: false,
              has_tool_results: false,
              created_at: "2026-08-17T00:00:00Z",
              seq: 3,
            },
          ],
        };
      }
      return null;
    });
    const wrapper = await mountOpen();
    const input = document.body.querySelector<HTMLInputElement>(".search-modal__input");
    input!.value = "权限";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(260);
    await flushPromises();

    const row = document.body.querySelector<HTMLButtonElement>(".search-modal__row--snippet");
    expect(row).not.toBeNull();
    row!.click();
    await flushPromises();

    // Preview header visible + load_session invoked read-only.
    expect(document.body.textContent).toContain("在主窗口打开");
    const calls = invokeMock.mock.calls.map((c) => c[0]);
    expect(calls).toContain("load_session");
    // data-seq anchor present on the rendered message.
    expect(document.body.querySelector('[data-seq="3"]')).not.toBeNull();
    vi.useRealTimers();
    wrapper.unmount();
  });

  // BUGLIST CH12-1a (2026-08-29): the session head inside a content-hit
  // group used to be a dead div that looked clickable (the GUI test
  // clicked it and reported "search does nothing"). It's a real button
  // now — same action as a title hit: open the session in the main window.
  it("clicking a session head opens that session in the main window (CH12-1a)", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValue([contentHit()]);
    const wrapper = await mountOpen();
    const input = document.body.querySelector<HTMLInputElement>(".search-modal__input");
    input!.value = "权限";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(260);
    await flushPromises();

    const chatStore = useChatStore();
    const openSpy = vi
      .spyOn(chatStore, "openSessionInProject")
      .mockResolvedValue(undefined);
    const head = document.body.querySelector<HTMLButtonElement>(
      ".search-modal__session-head",
    );
    expect(head).not.toBeNull();
    expect(head!.tagName).toBe("BUTTON");
    head!.click();
    await flushPromises();

    expect(openSpy).toHaveBeenCalledWith("pa", "s1");
    // No seq on a head click → no locate pass (no rAF wait needed).
    vi.useRealTimers();
    wrapper.unmount();
  });

  // BUGLIST CH12-1b: the preview's "在主窗口打开" used to drop the
  // seq — the main window opened the session but never positioned on
  // the hit message. It must hand the seq to the main-window list.
  //
  // Real timers: `locateMessage` waits nextTick + one real rAF before
  // querying, so fake timers would freeze the wait and leak the
  // pending continuation past unmount. The prefill open (no debounce)
  // makes the search synchronous without fake timers.
  it("preview '在主窗口打开' passes the seq through and scrolls to the hit message (CH12-1b)", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "search_messages") return [contentHit()];
      if (cmd === "load_session") {
        return {
          session: { id: "s1" },
          messages: [
            {
              id: 1,
              session_id: "s1",
              role: "user",
              content: [{ type: "text", text: "hello 权限" }],
              text: "hello 权限",
              has_tool_calls: false,
              has_tool_results: false,
              created_at: "2026-08-17T00:00:00Z",
              seq: 3,
            },
          ],
        };
      }
      return null;
    });
    const wrapper = await reopenWith({ query: "权限" });

    // Open the preview, then stub a main-window message list with the
    // data-seq hook MessageList stamps on MessageItem roots.
    document.body.querySelector<HTMLButtonElement>(".search-modal__row--snippet")!.click();
    await flushPromises();
    const list = document.createElement("div");
    list.className = "messages";
    const msgEl = document.createElement("div");
    msgEl.setAttribute("data-seq", "3");
    list.appendChild(msgEl);
    document.body.appendChild(list);
    const scrollSpy = vi.fn();
    msgEl.scrollIntoView = scrollSpy;

    const chatStore = useChatStore();
    const openSpy = vi
      .spyOn(chatStore, "openSessionInProject")
      .mockResolvedValue(undefined);

    document.body.querySelector<HTMLButtonElement>(".search-modal__open-btn")!.click();
    await flushPromises();
    // locateMessage waits nextTick + nextPaint (rAF races a 60ms
    // setTimeout fallback — jsdom's rAF clock is dead after this
    // file's earlier fake-timer cycles, so the fallback carries it).
    await new Promise((r) => setTimeout(r, 120));
    await flushPromises();

    expect(openSpy).toHaveBeenCalledWith("pa", "s1");
    expect(scrollSpy).toHaveBeenCalledWith({ block: "center", behavior: "smooth" });
    wrapper.unmount();
  });

  // -------------------------------------------------------------------
  // D2②+ (08-17-search-history-card): programmatic prefill open.
  // The open watcher only fires on a false→true transition, and the
  // module singleton may already be true from the tests above —
  // close + tick first so every prefill test starts from false.
  // -------------------------------------------------------------------

  async function reopenWith(prefill?: { query: string; projectId?: string | null }) {
    const { open, close } = useSearchModal();
    close();
    await nextTick();
    // Mount FIRST (mirrors production: AppShell keeps SearchModal
    // always-mounted, so the open watcher observes the transition).
    // Mounting after open() would miss the false→true change.
    const wrapper = mount(SearchModal, { attachTo: document.body });
    await flushPromises();
    invokeMock.mockClear();
    open(prefill);
    await flushPromises();
    await nextTick(); // bootingPrefill guard cleared
    return wrapper;
  }

  it("open({query}) prefills the query and searches immediately (no debounce)", async () => {
    invokeMock.mockResolvedValue([contentHit()]);
    const wrapper = await reopenWith({ query: "worktree" });

    // Fired once, synchronously with the open — no timer advance.
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("search_messages", {
      query: "worktree",
      projectId: null,
      limit: 50,
    });
    expect(document.body.textContent).toContain("找到 1 条命中");
    wrapper.unmount();
  });

  it("open({query, projectId}) arms the project filter without double-firing", async () => {
    invokeMock.mockResolvedValue([contentHit()]);
    const wrapper = await reopenWith({ query: "worktree", projectId: "pa" });

    // Exactly ONE search — the prefill's own run; arming
    // query/projectFilter must not trigger watcher-driven reruns.
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("search_messages", {
      query: "worktree",
      projectId: "pa",
      limit: 50,
    });
    wrapper.unmount();
  });

  it("a plain open() after a prefill open does NOT reuse the stale prefill", async () => {
    invokeMock.mockResolvedValue([]);
    const wrapperPrefill = await reopenWith({ query: "worktree" });
    wrapperPrefill.unmount();
    invokeMock.mockClear();

    // Second open: no args → blank state, no auto search.
    const wrapper = await reopenWith();
    expect(invokeMock).not.toHaveBeenCalled();
    const input = document.body.querySelector<HTMLInputElement>(".search-modal__input");
    expect(input?.value).toBe("");
    wrapper.unmount();
  });
});

afterEach(() => {
  document.body.innerHTML = "";
});
