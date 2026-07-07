// Tests for `PendingBadge.vue` — the top-bar global pending-interaction
// count indicator (B档 of 2026-07-08 `cross-session-pending-indicator`).
//
// Covers the two pieces of logic that aren't pure rendering:
//   1. `count` (computed over questionCards.pendingBySession.size)
//      drives v-if — badge hidden at 0.
//   2. click → switchSession to the most-recent (highest-ts) pending
//      session IN THE CURRENT project; cross-project targets (absent
//      from chatStore.sessions) are a no-op (Q4: same-project jump
//      only).
//
// `switchSession` is spied (not executed) so the test doesn't depend
// on its internal IPC `load_session` round-trip — we assert the call.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { mount, VueWrapper } from "@vue/test-utils";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import PendingBadge from "./PendingBadge.vue";
import { useQuestionCardsStore } from "../../stores/questionCards";
import { useChatStore } from "../../stores/chat";
import type { SessionSummary } from "../../stores/chat.types";

function makeSession(id: string, title: string): SessionSummary {
  return {
    id,
    title,
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
  };
}

describe("PendingBadge", () => {
  let wrapper: VueWrapper | null = null;

  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    wrapper = null;
  });

  function mountBadge() {
    wrapper = mount(PendingBadge);
    return wrapper;
  }

  it("count = 0 → 徽章不渲染", () => {
    const w = mountBadge();
    expect(w.find(".pending-badge").exists()).toBe(false);
  });

  it("有 pending → 渲染且显示计数 = pendingBySession.size", async () => {
    const qc = useQuestionCardsStore();
    qc.addPending("s2", {
      kind: "mode_change",
      payload: {
        session_id: "s2",
        tool_use_id: "t1",
        target_mode: "edit",
        current_mode: "plan",
        reason: undefined,
        ts: 100,
      },
    });
    const w = mountBadge();
    expect(w.find(".pending-badge").exists()).toBe(true);
    expect(w.find(".pending-badge__count").text()).toBe("1");
  });

  it("点击 → switchSession 到当前 project 内 ts 最大的 pending session", async () => {
    const chat = useChatStore();
    chat.sessions = [
      makeSession("s1", "一"),
      makeSession("s2", "二"),
      makeSession("s3", "三"),
    ];
    chat.currentSessionId = "s1";
    const switchSpy = vi
      .spyOn(chat, "switchSession")
      .mockResolvedValue(undefined);

    const qc = useQuestionCardsStore();
    // s2 ts=100, s3 ts=200 → 最近 = s3
    qc.addPending("s2", {
      kind: "mode_change",
      payload: {
        session_id: "s2",
        tool_use_id: "t1",
        target_mode: "edit",
        current_mode: "plan",
        reason: undefined,
        ts: 100,
      },
    });
    qc.addPending("s3", {
      kind: "question",
      payload: { session_id: "s3", tool_use_id: "t2", questions: [], ts: 200 },
    });

    const w = mountBadge();
    await w.find(".pending-badge").trigger("click");
    expect(switchSpy).toHaveBeenCalledWith("s3");
  });

  it("pending 全在跨 project session(不在 chatStore.sessions)→ 点击不切", async () => {
    const chat = useChatStore();
    chat.sessions = [makeSession("s1", "一")];
    chat.currentSessionId = "s1";
    const switchSpy = vi
      .spyOn(chat, "switchSession")
      .mockResolvedValue(undefined);

    const qc = useQuestionCardsStore();
    // sX 模拟另一 project 的 session(不在当前 sessions)
    qc.addPending("sX", {
      kind: "mode_change",
      payload: {
        session_id: "sX",
        tool_use_id: "t1",
        target_mode: "edit",
        current_mode: "plan",
        reason: undefined,
        ts: 100,
      },
    });

    const w = mountBadge();
    expect(w.find(".pending-badge").exists()).toBe(true); // count=1 仍显示
    await w.find(".pending-badge").trigger("click");
    expect(switchSpy).not.toHaveBeenCalled(); // 跨 project 不跳
  });
});
