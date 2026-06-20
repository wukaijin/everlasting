// Tests for `ToolCallCard.vue` inline approval state (2026-06-16).
//
// Replaces the deleted `PermissionModal.test.ts`. Covers the inline
// approval UI that renders on the tool_use card the backend is
// asking permission for:
//   1. No approval UI when there's no pending ask.
//   2. No approval UI when the pending ask's toolUseId ≠ call.id.
//   3. Approval UI (4 actions) renders when toolUseId matches.
//   4. 仅一次 / 始终允许 / 拒绝 fire the right respond() IPC.
//   5. 拒绝并说明 opens a textarea + submits the feedback as the
//      deny reason.
//   6. Approval UI hides once a result arrives.
//
// Uses real Pinia stores (setActivePinia) + mocked Tauri IPC. The
// card is shallow-mounted so child components (Icon / DiffView) don't
// pull in their own deps.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import ToolCallCard from "./ToolCallCard.vue";
import {
  usePermissionsStore,
  type PermissionAsk,
} from "../../stores/permissions";
import { useChatStore, type ToolCallInfo } from "../../stores/chat";
import { useSubagentRunsStore } from "../../stores/subagentRuns";

function makeCall(overrides: Partial<ToolCallInfo> = {}): ToolCallInfo {
  return {
    id: "tu-1",
    name: "shell",
    input: { command: "rm -rf /tmp/x" },
    ...overrides,
  };
}

function makeAsk(overrides: Partial<PermissionAsk> = {}): PermissionAsk {
  return {
    rid: "rid-1",
    sessionId: "sess-1",
    toolUseId: "tu-1",
    toolName: "shell",
    toolInput: { command: "rm -rf /tmp/x" },
    risk: "high",
    ...overrides,
  };
}

describe("ToolCallCard inline approval", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(true);
  });

  function mountCard(call: ToolCallInfo = makeCall()) {
    return mount(ToolCallCard, { props: { call }, shallow: true });
  }

  /** Helper: put the current session into sess-1 + arm a pending ask. */
  function armPending(askOverrides: Partial<PermissionAsk> = {}) {
    const chat = useChatStore();
    const perm = usePermissionsStore();
    chat.currentSessionId = "sess-1";
    perm.setPending(makeAsk(askOverrides));
    return { chat, perm };
  }

  it("does NOT render approval UI when there is no pending ask", () => {
    const chat = useChatStore();
    chat.currentSessionId = "sess-1";
    const w = mountCard();
    expect(w.find(".tool-card__approval").exists()).toBe(false);
  });

  it("does NOT render approval when the pending toolUseId ≠ call.id", () => {
    armPending({ toolUseId: "some-other-tu" });
    const w = mountCard(); // call.id = "tu-1"
    expect(w.find(".tool-card__approval").exists()).toBe(false);
  });

  it("renders the 4 approval actions when toolUseId matches", () => {
    armPending();
    const w = mountCard();
    expect(w.find(".tool-card__approval").exists()).toBe(true);
    const text = w.text();
    expect(text).toContain("仅一次");
    expect(text).toContain("始终允许");
    expect(text).toContain("拒绝");
    expect(text).toContain("拒绝并说明");
    // risk label rendered in Chinese.
    expect(text).toContain("高");
  });

  it("clicking 仅一次 fires respond(allow_once)", async () => {
    armPending();
    const w = mountCard();
    await w.get(".tool-card__approval-btn--once").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-1",
      decision: "allow_once",
      reason: undefined,
    });
  });

  it("clicking 始终允许 fires respond(allow_always)", async () => {
    armPending();
    const w = mountCard();
    await w.get(".tool-card__approval-btn--always").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-1",
      decision: "allow_always",
      reason: undefined,
    });
  });

  it("clicking 拒绝 fires respond(deny) with no reason", async () => {
    armPending();
    const w = mountCard();
    // First --deny button is 拒绝, second is 拒绝并说明.
    await w.findAll(".tool-card__approval-btn--deny")[0].trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-1",
      decision: "deny",
      reason: undefined,
    });
  });

  it("拒绝并说明 opens a textarea + submits feedback as the deny reason", async () => {
    armPending();
    const w = mountCard();
    // No textarea before opening.
    expect(w.find(".tool-card__approval-textarea").exists()).toBe(false);
    // Open the feedback form (second --deny button).
    await w.findAll(".tool-card__approval-btn--deny")[1].trigger("click");
    expect(w.find(".tool-card__approval-textarea").exists()).toBe(true);
    // Type feedback + submit.
    await w.get("textarea").setValue("用 git clean 代替");
    await w
      .get(".tool-card__approval-feedback-actions .tool-card__approval-btn--deny")
      .trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-1",
      decision: "deny",
      reason: "用 git clean 代替",
    });
  });

  it("hides the approval UI once a result arrives", async () => {
    armPending();
    const w = mountCard();
    expect(w.find(".tool-card__approval").exists()).toBe(true);
    await w.setProps({
      result: {
        toolUseId: "tu-1",
        content: "ok",
        isError: false,
        durationMs: 42,
      },
    });
    expect(w.find(".tool-card__approval").exists()).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// B6 PR3 (2026-06-20): dispatch_subagent special branch.
// Clicking the whole card calls `subagentRuns.openDrawer(runId)` instead of
// expanding an inline transcript (the <SubagentDrawer> handles rendering).
// ---------------------------------------------------------------------------

describe("ToolCallCard dispatch_subagent branch", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(null);
  });

  function makeDispatchCall(overrides: Partial<ToolCallInfo> = {}): ToolCallInfo {
    return {
      id: "tooluse-dispatch",
      name: "dispatch_subagent",
      input: { subagent: "researcher", task: "find files" },
      ...overrides,
    };
  }

  function mountDispatchCard(call: ToolCallInfo = makeDispatchCall()) {
    const chat = useChatStore();
    chat.currentSessionId = "sess-1";
    return mount(ToolCallCard, { props: { call }, shallow: true });
  }

  it("renders the subagent preview row when the tool is dispatch_subagent", () => {
    const w = mountDispatchCard();
    expect(w.find(".tool-card--subagent").exists()).toBe(true);
    expect(w.find(".tool-card__subagent-preview").exists()).toBe(true);
  });

  it("shows the worker subagent name from the tool_use input when no summary cached", () => {
    const w = mountDispatchCard();
    expect(w.find(".tool-card__subagent-name").text()).toContain("researcher");
  });

  it("hides the default input <details> when dispatch_subagent", () => {
    const w = mountDispatchCard();
    // The input/output <details> are suppressed for dispatch_subagent.
    expect(w.find(".tool-card__details").exists()).toBe(false);
  });

  it("clicking the card triggers store.openDrawer when the summary is cached", async () => {
    const chat = useChatStore();
    const subagentRuns = useSubagentRunsStore();
    chat.currentSessionId = "sess-1";
    // Seed the list cache so getSummaryByToolUseId resolves.
    subagentRuns.runSummaryBySession.set("sess-1", [
      {
        id: "run-99",
        parentSessionId: "sess-1",
        parentRequestId: "parent-rid-sub-tooluse-dispatch",
        subagentName: "researcher",
        status: "completed",
        startedAt: "2026-06-20T10:00:00Z",
        finishedAt: "2026-06-20T10:00:30Z",
        tokenUsageJson: null,
        summary: "found 2 files",
      },
    ]);
    // Mock fetchRun so openDrawer doesn't actually try to call IPC.
    invokeMock.mockResolvedValueOnce({
      id: "run-99",
      parentSessionId: "sess-1",
      parentRequestId: "parent-rid-sub-tooluse-dispatch",
      subagentName: "researcher",
      status: "completed",
      startedAt: "2026-06-20T10:00:00Z",
      finishedAt: "2026-06-20T10:00:30Z",
      tokenUsageJson: null,
      summary: "found 2 files",
      transcriptJson: null,
      transcriptTruncated: 0,
      createdAt: "2026-06-20T10:00:00Z",
    });

    const w = mountDispatchCard();
    await w.trigger("click");
    await flushPromises();
    expect(subagentRuns.openRunId).toBe("run-99");
  });

  it("clicking with no summary cached shows waiting state and falls back after timeout", async () => {
    const subagentRuns = useSubagentRunsStore();
    const w = mountDispatchCard();
    expect(w.find(".tool-card--subagent-waiting").exists()).toBe(false);
    await w.trigger("click");
    // The click sets workerWaiting=true synchronously and fires the
    // first fetchForSession. invokeMock returns null (no row), so the
    // polling loop kicks in. flushPromises resolves microtasks only
    // (not setTimeout), so the loop stays in flight — openRunId stays
    // null and the waiting class is applied.
    await flushPromises();
    expect(subagentRuns.openRunId).toBeNull();
    expect(w.find(".tool-card--subagent-waiting").exists()).toBe(true);
    expect(w.find(".tool-card__subagent-summary").text()).toContain(
      "等待 worker 注册",
    );
  });

  // B6 PR3b (2026-06-20): click-time race fix. The store's
  // subagent:event listener is what makes this work end-to-end —
  // when the first event fires, the listener eager-fetches
  // list_subagent_runs_by_session, warming the cache. The
  // openSubagentDrawer retry loop picks that up. This test
  // simulates the race resolution by having the 3rd fetchForSession
  // call (from the polling loop's first tick) return the populated
  // list, mimicking the eager-fetch's effect.
  it("retry loop opens the drawer once the cache warms during the 1.5s window", async () => {
    vi.useFakeTimers();
    const subagentRuns = useSubagentRunsStore();
    const chat = useChatStore();
    chat.currentSessionId = "sess-1";
    invokeMock.mockReset();
    // Mount-time watch fetchForSession (empty: race lost).
    invokeMock.mockResolvedValueOnce([]);
    // Click handler's first fetchForSession (empty: still no row).
    invokeMock.mockResolvedValueOnce([]);
    // Polling tick 1's fetchForSession (POPULATED: simulates the
    // store's eager-fetch listener firing for the first event).
    invokeMock.mockResolvedValueOnce([
      {
        id: "run-warm",
        parentSessionId: "sess-1",
        parentRequestId: "parent-rid-sub-tooluse-dispatch",
        subagentName: "researcher",
        status: "running",
        startedAt: "2026-06-20T10:00:00Z",
        finishedAt: null,
        tokenUsageJson: null,
        summary: null,
      },
    ]);
    // openDrawer's fetchRun (after the polling tick finds the summary).
    invokeMock.mockResolvedValueOnce({
      id: "run-warm",
      parentSessionId: "sess-1",
      parentRequestId: "parent-rid-sub-tooluse-dispatch",
      subagentName: "researcher",
      status: "running",
      startedAt: "2026-06-20T10:00:00Z",
      finishedAt: null,
      tokenUsageJson: null,
      summary: null,
      transcriptJson: null,
      transcriptTruncated: 0,
      createdAt: "2026-06-20T10:00:00Z",
    });

    const w = mountDispatchCard();
    // Click — first fetchForSession returns empty, enters polling loop.
    await w.trigger("click");
    await flushPromises();
    expect(subagentRuns.openRunId).toBeNull();
    expect(w.find(".tool-card--subagent-waiting").exists()).toBe(true);

    // Advance past the first 300ms polling tick + drain all
    // microtasks the callback generates. advanceTimersByTimeAsync
    // is the async-aware variant — needed because the setTimeout
    // callback contains awaits that generate microtasks.
    await vi.advanceTimersByTimeAsync(300);
    await flushPromises();
    expect(subagentRuns.openRunId).toBe("run-warm");
    expect(w.find(".tool-card--subagent-waiting").exists()).toBe(false);
    vi.useRealTimers();
  });
});
