// GCE M4b (09-07-gce-m4b-discussion-search) — DiscussionLibraryModal
// component tests.
//
// Coverage:
//   1. Open (no query) → full-browse via `list_group_chat_sessions`
//      (empty-keyword mode is the panel's default — R2).
//   2. Debounced query → `search_group_chat_discussions` with the wire
//      contract ({ query, projectId, stopReason }).
//   3. Hit rows render title / task_name badge / participants /
//      stop-reason / summary preview; classic browse groups by bucket.
//   4. Clicking a row closes + `openSessionInProject` (project-aware
//      switch — the "打开会话" action).
//
// reka-ui DialogPortal teleports to document.body, so DOM queries go
// against `document.body` (attachTo), same as SearchModal.test.ts.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

vi.mock("../../transport", () => ({
  transport: { invoke: vi.fn() },
}));

import { transport } from "../../transport";
import DiscussionLibraryModal from "./DiscussionLibraryModal.vue";
import { useChatStore } from "../../stores/chat";
import type { GroupChatSessionHit } from "../../stores/chat.types";

const invokeMock = vi.mocked(transport.invoke);
import { useDiscussionLibrary } from "../../composables/useDiscussionLibrary";

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
});

function hit(over: Partial<GroupChatSessionHit> = {}): GroupChatSessionHit {
  return {
    session_id: "gc1",
    project_id: "pa",
    title: "权限系统重构讨论",
    task_name: "每周审议",
    participants: ["Alice", "Bob"],
    stop_reason: "group_chat_end",
    discussion_summary: "结论:采用 Rust 重写,保留事件溯源。",
    created_at: "2026-09-05T10:00:00+00:00",
    updated_at: "2026-09-05T10:00:00+00:00",
    total_tokens: null,
    ...over,
  };
}

async function mountOpen() {
  const { open } = useDiscussionLibrary();
  open();
  const wrapper = mount(DiscussionLibraryModal, { attachTo: document.body });
  await flushPromises();
  return wrapper;
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("DiscussionLibraryModal", () => {
  it("open with no query triggers the full browse (list_group_chat_sessions)", async () => {
    invokeMock.mockResolvedValue([hit()]);
    const wrapper = await mountOpen();
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("list_group_chat_sessions", {
      projectId: null,
      stopReason: null,
    });
    expect(document.body.textContent).toContain("共 1 场历史审议");
    expect(document.body.textContent).toContain("权限系统重构讨论");
    wrapper.unmount();
  });

  it("debounces the query and invokes search_group_chat_discussions with the wire contract", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValue([hit()]);
    const wrapper = await mountOpen();
    await vi.advanceTimersByTimeAsync(0);
    await flushPromises();
    invokeMock.mockClear(); // drop the initial browse call

    const input = document.body.querySelector<HTMLInputElement>(".discussion-lib__input");
    expect(input).not.toBeNull();
    input!.value = "权限";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    expect(invokeMock).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(260);
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("search_group_chat_discussions", {
      query: "权限",
      projectId: null,
      stopReason: null,
    });
    vi.useRealTimers();
    wrapper.unmount();
  });

  it("renders task badge / participants / stop badge / summary preview on a hit row", async () => {
    invokeMock.mockResolvedValue([hit()]);
    const wrapper = await mountOpen();
    await flushPromises();
    const body = document.body.textContent ?? "";
    expect(body).toContain("每周审议"); // 定时场任务名徽章
    expect(body).toContain("Alice、Bob"); // 参与人
    expect(body).toContain("正常收官"); // stop_reason label
    expect(body).toContain("采用 Rust 重写"); // summary 预览
    wrapper.unmount();
  });

  it("search with no hits echoes the searched query (distinct from browse-empty)", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValue([]);
    const wrapper = await mountOpen();
    await vi.advanceTimersByTimeAsync(0);
    await flushPromises();
    invokeMock.mockClear();

    const input = document.body.querySelector<HTMLInputElement>(".discussion-lib__input");
    input!.value = "量子纠缠";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(260);
    await flushPromises();
    const body = document.body.textContent ?? "";
    expect(body).toContain("量子纠缠");
    expect(body).toContain("没有找到");
    vi.useRealTimers();
    wrapper.unmount();
  });

  it("clicking a row opens the session via openSessionInProject (project-aware)", async () => {
    invokeMock.mockResolvedValue([hit()]);
    const wrapper = await mountOpen();
    await flushPromises();

    const chat = useChatStore();
    const openSpy = vi.spyOn(chat, "openSessionInProject").mockResolvedValue();
    const row = [...document.body.querySelectorAll(".discussion-lib__row")].find((r) =>
      r.textContent?.includes("权限系统重构讨论"),
    ) as HTMLButtonElement;
    expect(row).toBeTruthy();
    row.click();
    await flushPromises();

    // Modal closed (single-instance dialog unmounts on close).
    expect(useDiscussionLibrary().discussionLibraryOpen.value).toBe(false);
    expect(openSpy).toHaveBeenCalledWith("pa", "gc1");
    wrapper.unmount();
  });

  it("non-scheduled sessions (no task_name) still render without the badge", async () => {
    invokeMock.mockResolvedValue([
      hit({
        session_id: "gc2",
        title: "临时发起的讨论",
        task_name: null,
        participants: [],
        stop_reason: null,
        discussion_summary: null,
      }),
    ]);
    const wrapper = await mountOpen();
    await flushPromises();
    const body = document.body.textContent ?? "";
    expect(body).toContain("临时发起的讨论");
    expect(body).toContain("尚未生成总结");
    wrapper.unmount();
  });

  // gce-m4c(09-08):每场累计 token 消耗列(万单位;无数据 = 「—」)。
  it("renders the per-discussion token total in 万 units when present", async () => {
    invokeMock.mockResolvedValue([hit({ total_tokens: 265000 })]);
    const wrapper = await mountOpen();
    await flushPromises();
    const tokens = document.body.querySelector(".discussion-lib__row-tokens");
    expect(tokens).not.toBeNull();
    expect(tokens?.textContent).toContain("26.5万");
    wrapper.unmount();
  });

  it("renders '—' for the token total when the hit carries null (no usage rows)", async () => {
    invokeMock.mockResolvedValue([hit({ total_tokens: null })]);
    const wrapper = await mountOpen();
    await flushPromises();
    const tokens = document.body.querySelector(".discussion-lib__row-tokens");
    expect(tokens?.textContent).toContain("—");
    wrapper.unmount();
  });
});
