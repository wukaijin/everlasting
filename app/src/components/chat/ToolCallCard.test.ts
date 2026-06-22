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
// FT-F-001 PR1 (2026-06-20): the inline approval block was
// extracted into `<PermissionAskBody>`. `mountCard` was changed
// from `shallow: true` to full mount because the approval
// selectors (`.tool-card__approval-btn--once` etc.) now live
// inside `PermissionAskBody`'s template — with `shallow: true`,
// child components are stubbed and the buttons never render. The
// outer `.tool-card__approval` wrapper class (added in
// `ToolCallCard.vue`) preserves the "approval UI present" lock
// for tests 1/2/3/6; the inner button-level selectors continue
// to assert the same IPC contract on tests 4/5.

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
import { useChatStore } from "../../stores/chat";
import type { ToolCallInfo } from "../../stores/chat.types";
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
    // FT-F-001 PR1: full mount (was `shallow: true` pre-extraction).
    // The approval UI now lives in `<PermissionAskBody>`, a child
    // component that needs full mount to render the buttons the
    // tests below assert on.
    return mount(ToolCallCard, { props: { call } });
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
    // FT-F-001 PR1: selector now points inside <PermissionAskBody>.
    await w.get(".permission-ask-body__btn--once").trigger("click");
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
    await w.get(".permission-ask-body__btn--always").trigger("click");
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
    await w.findAll(".permission-ask-body__btn--deny")[0].trigger("click");
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
    expect(w.find(".permission-ask-body__textarea").exists()).toBe(false);
    // Open the feedback form (second --deny button).
    await w.findAll(".permission-ask-body__btn--deny")[1].trigger("click");
    expect(w.find(".permission-ask-body__textarea").exists()).toBe(true);
    // Type feedback + submit.
    await w.get("textarea").setValue("用 git clean 代替");
    await w
      .get(".permission-ask-body__feedback-actions .permission-ask-body__btn--deny")
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
    // FT-F-001 PR1: outer .tool-card__approval wrapper preserves
    // the pre-extraction class — see ToolCallCard.vue.
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
        // B6 redesign PR1: nullable for legacy rows.
        task: null,
        finalText: null,
        // 2026-06-22 (RULE-FrontSubagent-004): nullable for legacy rows.
        turnCount: null,
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
        turnCount: null,
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

  // FT-F-002 (2026-06-21): 1.5s retry polling exhausts without the
  // cache warming → explicit "worker 未响应,点此重试" missed hint
  // (was: silent fallback to the default visual). Every
  // fetchForSession returns empty so the loop never resolves.
  // Fake-timer note (memory: subagentdrawer-banner-test-gotchas):
  // advanceTimersByTime advances setTimeout + Date.now() in
  // lockstep, so the `while (Date.now() - start < 1500)` guard
  // exits correctly per tick.
  it("shows 'worker 未响应' missed hint after 1.5s polling exhausts", async () => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]); // every fetchForSession misses

    const w = mountDispatchCard();
    await w.trigger("click");
    await flushPromises();
    expect(w.find(".tool-card--subagent-waiting").exists()).toBe(true);

    // Drive 5× 300ms ticks → loop exits (Date.now() - start >= 1500)
    // → workerMissed=true.
    for (let i = 0; i < 5; i++) {
      await vi.advanceTimersByTimeAsync(300);
      await flushPromises();
    }
    expect(w.find(".tool-card--subagent-waiting").exists()).toBe(false);
    expect(w.find(".tool-card__subagent-summary--missed").exists()).toBe(true);
    expect(w.find(".tool-card__subagent-summary--missed").text()).toContain(
      "worker 未响应",
    );
    vi.useRealTimers();
  });

  // FT-F-002 (2026-06-21): clicking the card again clears the
  // missed hint (openSubagentDrawer resets workerMissed at the top)
  // and re-enters the waiting/polling state for a fresh attempt.
  it("retry click clears the missed hint and re-enters polling", async () => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);

    const w = mountDispatchCard();
    await w.trigger("click");
    for (let i = 0; i < 5; i++) {
      await vi.advanceTimersByTimeAsync(300);
      await flushPromises();
    }
    expect(w.find(".tool-card__subagent-summary--missed").exists()).toBe(true);

    await w.trigger("click");
    await flushPromises();
    expect(w.find(".tool-card__subagent-summary--missed").exists()).toBe(false);
    expect(w.find(".tool-card--subagent-waiting").exists()).toBe(true);
    vi.useRealTimers();
  });

  // FT-F-003 (2026-06-20): `openSubagentDrawer`'s retry polling
  // loop uses `await new Promise(r => setTimeout(r, 300))` to pace
  // its 5 ticks — no timer id to clearTimeout. When the component
  // unmounts mid-poll (e.g. the user switches sessions during the
  // 1.5s window), the pending `await` resolves on an unmounted card
  // and the loop would otherwise continue calling `fetchForSession`
  // + writing `workerWaiting` / calling `openDrawer` on the dead
  // instance. This test asserts the unmounted-flag guard catches
  // that: after unmount, advancing the fake timer past the pending
  // 300ms tick does NOT trigger any further `fetchForSession` /
  // `openDrawer` calls, and no Vue warning is emitted.
  //
  // Note (memory: subagentdrawer-banner-test-gotchas.md): fake
  // timers also mock `Date.now()` — `vi.advanceTimersByTime` advances
  // both `setTimeout` AND `Date.now()` in lockstep, so the
  // `while (Date.now() - start < 1500)` condition moves forward
  // correctly per tick (no need for `vi.setSystemTime`).
  it("unmount during polling clears the loop (FT-F-003 unmounted guard)", async () => {
    vi.useFakeTimers();
    const subagentRuns = useSubagentRunsStore();
    const chat = useChatStore();
    chat.currentSessionId = "sess-1";
    invokeMock.mockReset();
    // Spy on the store methods so we can assert "not called after
    // unmount". fetchForSession / openDrawer are real (they route
    // through invokeMock); we spy without replacing the impl.
    const fetchSpy = vi.spyOn(subagentRuns, "fetchForSession");
    const openDrawerSpy = vi.spyOn(subagentRuns, "openDrawer");

    // Mount-time watch fetchForSession (empty).
    invokeMock.mockResolvedValue([]);

    const w = mountDispatchCard();
    // Baseline: mount-time eager fetch fired once (the immediate
    // watch in ToolCallCard.vue calls fetchForSession on mount).
    const baselineFetchCount = fetchSpy.mock.calls.length;

    // Click — enters openSubagentDrawer. immediate=undefined,
    // workerWaiting=true, first fetchForSession fires (the
    // afterRetry branch), returns [], while loop starts, first
    // `await setTimeout(300)` is pending.
    await w.trigger("click");
    await flushPromises();

    // Snapshot call counts right before unmount. The click path
    // fires 1 extra fetchForSession (the afterRetry branch); the
    // while loop's tick 1 has NOT fired yet (still pending in the
    // 300ms setTimeout).
    const fetchCountBeforeUnmount = fetchSpy.mock.calls.length;
    expect(fetchCountBeforeUnmount).toBeGreaterThan(baselineFetchCount);
    expect(openDrawerSpy).not.toHaveBeenCalled();

    // Unmount mid-poll — this is the race window FT-F-003 fixes.
    // The pending `await setTimeout(300)` is still in flight; the
    // onUnmounted hook sets `unmounted = true`.
    w.unmount();

    // Silence the Vue "unexpected mutation on unmounted component"
    // warning if Vue 3.5+ still emits one (it has gotten smarter
    // about this; capturing for AC6 evidence either way).
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

    // Advance the fake timer past the pending 300ms tick. With the
    // unmounted guard in place, the loop's first instruction after
    // the await is `if (unmounted) return;` — so the loop bails
    // WITHOUT calling fetchForSession again, WITHOUT writing
    // workerWaiting, WITHOUT calling openDrawer.
    await vi.advanceTimersByTimeAsync(300);
    await flushPromises();

    // Core FT-F-003 assertion: no further fetchForSession calls
    // after unmount. (The while loop tick 1 was the only thing
    // that would have fired one.)
    expect(fetchSpy.mock.calls.length).toBe(fetchCountBeforeUnmount);
    // openDrawer was never called (the cache never warmed before
    // unmount, so no drawer trigger either way — but lock it).
    expect(openDrawerSpy).not.toHaveBeenCalled();

    // AC6 (optional): future-proof Vue-warning lock. Verified
    // empirically (Vue 3.5.35, 2026-06-20): writing to a ref on
    // an unmounted component does NOT emit a console.warn in this
    // version — so this assertion trivially passes today. It is
    // retained as a regression lock: if a future Vue upgrade
    // re-introduces the warning (older 3.x versions did warn),
    // this test will fail and flag the need to revisit the guard.
    // The load-bearing assertion is the behavior check above
    // (`fetchSpy.mock.calls.length` + `openDrawerSpy`) — that one
    // genuinely fails when the guard is removed.
    expect(warnSpy).not.toHaveBeenCalled();

    warnSpy.mockRestore();
    vi.useRealTimers();
  });
});
