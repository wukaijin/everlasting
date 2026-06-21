// Tests for `WorkerAskBanner.vue` — the compact pill rendered above
// the chat panel when one or more worker asks are pending for the
// current session.
//
// PR2 RULE-FrontSubagent-003 (2026-06-22). Coverage:
//   1. Hidden when no worker asks are pending (`count === 0`).
//   2. Shows count text when one or more worker asks are live.
//   3. Click invokes `subagentRuns.openDrawer(runId)` for the most
//      recent pending worker run.
//   4. Banner reflects the CURRENT session's asks only (cross-session
//      isolation: switching `currentSessionId` changes the count).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import WorkerAskBanner from "./WorkerAskBanner.vue";
import { usePermissionsStore, type PermissionAsk } from "../../stores/permissions";
import { useChatStore } from "../../stores/chat";
import { useSubagentRunsStore } from "../../stores/subagentRuns";

const workerAskSess1: PermissionAsk = {
  rid: "rid-w1",
  sessionId: "sess-1",
  toolUseId: "tu-w1",
  toolName: "shell",
  toolInput: { command: "ls" },
  risk: "high",
  workerRunId: "run-1",
};

const workerAskSess2: PermissionAsk = {
  rid: "rid-w2",
  sessionId: "sess-2",
  toolUseId: "tu-w2",
  toolName: "write_file",
  toolInput: { path: "/tmp/x" },
  risk: "medium",
  workerRunId: "run-2",
};

function mountBanner() {
  return mount(WorkerAskBanner, {
    global: {
      // No store stubs — the banner reads 3 real stores; stubbing
      // them would defeat the test's purpose.
    },
  });
}

describe("WorkerAskBanner", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("is hidden when no worker asks are pending", () => {
    const chat = useChatStore();
    chat.currentSessionId = "sess-1";
    const w = mountBanner();
    // v-if="count > 0" — the button doesn't render at all.
    expect(w.find(".worker-ask-banner").exists()).toBe(false);
    w.unmount();
  });

  it("shows count text when a worker ask is live for the current session", () => {
    const chat = useChatStore();
    const permissions = usePermissionsStore();
    chat.currentSessionId = "sess-1";
    permissions.setPending(workerAskSess1);

    const w = mountBanner();
    const el = w.find(".worker-ask-banner");
    expect(el.exists()).toBe(true);
    expect(el.text()).toContain("1 个 worker 待审批");
    w.unmount();
  });

  it("shows updated count when multiple worker asks are live", () => {
    const chat = useChatStore();
    const permissions = usePermissionsStore();
    chat.currentSessionId = "sess-1";
    permissions.setPending(workerAskSess1);
    permissions.setPending({
      ...workerAskSess1,
      rid: "rid-w1b",
      workerRunId: "run-1b",
    });

    const w = mountBanner();
    expect(w.find(".worker-ask-banner").text()).toContain("2 个 worker 待审批");
    w.unmount();
  });

  it("click invokes openDrawer for the most recent pending worker run", async () => {
    const chat = useChatStore();
    const permissions = usePermissionsStore();
    const subagentRuns = useSubagentRunsStore();
    chat.currentSessionId = "sess-1";
    permissions.setPending(workerAskSess1);

    // Stub openDrawer so we don't need the async fetchRun IPC path
    // (which is mocked to return null but still async).
    const openSpy = vi.spyOn(subagentRuns, "openDrawer").mockResolvedValue();

    const w = mountBanner();
    await w.find(".worker-ask-banner").trigger("click");
    await flushPromises();
    expect(openSpy).toHaveBeenCalledWith("run-1");
    w.unmount();
  });

  it("reflects only the CURRENT session's asks (cross-session isolation)", async () => {
    const chat = useChatStore();
    const permissions = usePermissionsStore();
    permissions.setPending(workerAskSess1); // sess-1
    permissions.setPending(workerAskSess2); // sess-2

    // sess-1 is current → banner shows sess-1's count only.
    chat.currentSessionId = "sess-1";
    let w = mountBanner();
    expect(w.find(".worker-ask-banner").text()).toContain("1 个 worker 待审批");
    w.unmount();

    // Switch to sess-2 → banner updates reactively.
    chat.currentSessionId = "sess-2";
    w = mountBanner();
    expect(w.find(".worker-ask-banner").text()).toContain("1 个 worker 待审批");
    // Sanity: the runId is sess-2's, not sess-1's.
    await w.find(".worker-ask-banner").trigger("click");
    w.unmount();
  });

  it("hides again when the worker ask is resolved (respond clears the slot)", async () => {
    const chat = useChatStore();
    const permissions = usePermissionsStore();
    chat.currentSessionId = "sess-1";
    permissions.setPending(workerAskSess1);

    const w = mountBanner();
    expect(w.find(".worker-ask-banner").exists()).toBe(true);

    await permissions.respond("rid-w1", "deny");
    await flushPromises();
    // Banner disappears now that count is 0.
    expect(w.find(".worker-ask-banner").exists()).toBe(false);
    w.unmount();
  });
});
