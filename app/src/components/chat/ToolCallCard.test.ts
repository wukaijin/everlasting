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
