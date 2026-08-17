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
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

vi.mock("../../transport", () => ({
  transport: { invoke: vi.fn() },
}));

import { transport } from "../../transport";
import SearchModal from "./SearchModal.vue";

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
});

afterEach(() => {
  document.body.innerHTML = "";
});
