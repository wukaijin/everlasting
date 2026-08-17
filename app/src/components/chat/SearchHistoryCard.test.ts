// D2②+ (08-17-search-history-card) — SearchHistoryCard tests.
//
// Coverage:
//   1. pending / error states (no requery when result missing or
//      is_error).
//   2. hits state: requery fired with the tool call's own params
//      ({query, projectId, limit}), top-3 rows + CTA with the
//      total count; "本会话" marker for current-session hits.
//   3. scope mapping: `current_project` → projectsStore
//      .currentProjectId; default/`all` → null.
//   4. empty state (requery resolved with 0 hits).
//   5. degrade state (requery rejected → renders the tool_result
//      raw text instead of a dead spinner).
//   6. CTA opens the search modal with a prefill (query + scoped
//      projectId).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

vi.mock("../../transport", () => ({
  transport: { invoke: vi.fn() },
}));

import { transport } from "../../transport";
import SearchHistoryCard from "./SearchHistoryCard.vue";
import { useSearchModal } from "../../composables/useSearchModal";
import { useProjectsStore } from "../../stores/projects";
import { useChatStore } from "../../stores/chat";
import type { MessageSearchHit, ToolResultInfo } from "../../stores/chat.types";

const invokeMock = vi.mocked(transport.invoke);

function hit(over: Partial<MessageSearchHit> = {}): MessageSearchHit {
  return {
    kind: "content",
    session_id: "s-other",
    session_title: "another session",
    project_id: "p1",
    project_name: "proj-a",
    updated_at: "2026-08-15T10:00:00Z",
    seq: 3,
    role: "assistant",
    speaker: null,
    snippet: "we solved it via lazy auto-attach",
    ...over,
  };
}

function result(over: Partial<ToolResultInfo> = {}): ToolResultInfo {
  return {
    toolUseId: "tu-1",
    content: 'Found 1 hits for "worktree" (scope: all projects):\n1. [2026-08-15] proj-a / another session · #3 assistant: …',
    isError: false,
    ...over,
  };
}

function mountCard(input: Record<string, unknown>, res?: ToolResultInfo | null) {
  return mount(SearchHistoryCard, {
    props: {
      call: { id: "tu-1", name: "search_history", input },
      result: res ?? null,
    },
  });
}

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
  // Generic default: store machinery (loadSessions etc. — the chat
  // store reacts to projectsStore.currentProjectId below) gets []
  // so nothing crashes; per-test overrides below swap in the
  // search_messages payload.
  invokeMock.mockResolvedValue([]);
  const chatStore = useChatStore();
  chatStore.currentSessionId = "sess-cur";
});

/** Seed the active project ONLY where a test needs it: setting
 * currentProjectId fires the chat store's onProjectChange →
 * loadSessions (mocked []) which RESETS currentSessionId to null —
 * seeding it globally would race away the 本会话 fixture. */
function seedActiveProject(): void {
  const projectsStore = useProjectsStore();
  projectsStore.currentProjectId = "p1";
}

/** Only the card's own requery calls (store machinery excluded). */
function searchCalls(): Array<[string, Record<string, unknown>]> {
  return invokeMock.mock.calls.filter((c) => c[0] === "search_messages") as Array<
    [string, Record<string, unknown>]
  >;
}

// ---------------------------------------------------------------------
// 1. pending / error
// ---------------------------------------------------------------------

describe("SearchHistoryCard — pending / error", () => {
  it("renders pending state and does NOT requery before the result arrives", async () => {
    const wrapper = mountCard({ query: "worktree" }, null);
    await flushPromises();
    expect(wrapper.text()).toContain("正在检索历史");
    expect(searchCalls()).toHaveLength(0);
  });

  it("renders the backend error text when result.is_error, without requery", async () => {
    const wrapper = mountCard(
      { query: "  " },
      result({ isError: true, content: "`query` must not be empty" }),
    );
    await flushPromises();
    expect(wrapper.text()).toContain("query");
    expect(searchCalls()).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------
// 2. hits state
// ---------------------------------------------------------------------

describe("SearchHistoryCard — hits", () => {
  it("requeries with the call's own params and renders top-3 rows + CTA", async () => {
    const fourHits = [
      hit({ session_id: "a", seq: 1, snippet: "worktree fix one" }),
      hit({ session_id: "b", seq: 2, snippet: "worktree fix two" }),
      hit({ session_id: "c", seq: 3, snippet: "worktree fix three" }),
      hit({ session_id: "d", seq: 4, snippet: "worktree fix four" }),
    ];
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "search_messages" ? fourHits : [],
    );
    const wrapper = mountCard({ query: "worktree", limit: 10 }, result());
    await flushPromises();

    expect(searchCalls()).toHaveLength(1);
    expect(searchCalls()[0][1]).toEqual({
      query: "worktree",
      projectId: null,
      limit: 10,
    });
    // Only the top 3 rows render.
    expect(wrapper.findAll(".shcard__row")).toHaveLength(3);
    // CTA carries the FULL count.
    expect(wrapper.text()).toContain("共 4 条命中");
    // Snippet query highlighting present (mark wraps the query).
    expect(wrapper.find("mark").text()).toBe("worktree");
  });

  it("marks current-session hits with 本会话", async () => {
    const two = [hit({ session_id: "sess-cur" }), hit({ session_id: "s-other" })];
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "search_messages" ? two : [],
    );
    const wrapper = mountCard({ query: "worktree" }, result());
    await flushPromises();
    const rows = wrapper.findAll(".shcard__row");
    expect(rows[0].text()).toContain("本会话");
    expect(rows[1].text()).not.toContain("本会话");
  });

  it("title hits render with the [标题] kind marker", async () => {
    const titleHit = hit({
      kind: "title",
      seq: null,
      role: null,
      snippet: null,
      session_title: "权限讨论",
    });
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "search_messages" ? [titleHit] : [],
    );
    const wrapper = mountCard({ query: "权限" }, result());
    await flushPromises();
    expect(wrapper.text()).toContain("[标题]");
    expect(wrapper.text()).toContain("权限讨论");
  });
});

// ---------------------------------------------------------------------
// 3. scope mapping
// ---------------------------------------------------------------------

describe("SearchHistoryCard — scope mapping", () => {
  it("scope=current_project maps to the active project id", async () => {
    invokeMock.mockResolvedValue([]);
    seedActiveProject();
    mountCard({ query: "worktree", scope: "current_project" }, result());
    await flushPromises();
    expect(searchCalls()[0]?.[1]).toEqual({
      query: "worktree",
      projectId: "p1",
      limit: 20,
    });
  });

  it("default limit is 20 when the call omitted it", async () => {
    invokeMock.mockResolvedValue([]);
    mountCard({ query: "worktree" }, result());
    await flushPromises();
    expect(searchCalls()[0]?.[1]).toMatchObject({ limit: 20 });
  });
});

// ---------------------------------------------------------------------
// 4. empty / 5. degrade
// ---------------------------------------------------------------------

describe("SearchHistoryCard — empty / degrade", () => {
  it("renders the empty state echoing the query", async () => {
    invokeMock.mockResolvedValue([]);
    const wrapper = mountCard({ query: "zzz-no-hit" }, result());
    await flushPromises();
    expect(wrapper.text()).toContain("没有找到与 “zzz-no-hit” 匹配");
  });

  it("degrades to the raw tool_result text when the requery fails", async () => {
    invokeMock.mockRejectedValue(new Error("net down"));
    const wrapper = mountCard({ query: "worktree" }, result());
    await flushPromises();
    expect(wrapper.text()).toContain("工具原始输出");
    expect(wrapper.find(".shcard__raw").text()).toContain("Found 1 hits");
  });
});

// ---------------------------------------------------------------------
// 6. CTA → modal prefill
// ---------------------------------------------------------------------

describe("SearchHistoryCard — CTA opens the modal with prefill", () => {
  it("opens with query + null projectId for scope all", async () => {
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "search_messages" ? [hit()] : [],
    );
    const wrapper = mountCard({ query: "worktree" }, result());
    await flushPromises();
    await wrapper.find(".shcard__cta").trigger("click");
    const { searchModalOpen, pendingPrefill } = useSearchModal();
    expect(searchModalOpen.value).toBe(true);
    expect(pendingPrefill.value).toEqual({ query: "worktree", projectId: null });
  });

  it("opens with the scoped projectId for scope=current_project", async () => {
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "search_messages" ? [hit()] : [],
    );
    seedActiveProject();
    const wrapper = mountCard(
      { query: "worktree", scope: "current_project" },
      result(),
    );
    await flushPromises();
    await wrapper.find(".shcard__cta").trigger("click");
    const { pendingPrefill } = useSearchModal();
    expect(pendingPrefill.value).toEqual({ query: "worktree", projectId: "p1" });
  });
});
