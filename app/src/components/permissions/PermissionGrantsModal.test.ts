// Tests for `PermissionGrantsModal.vue` — BUGLIST CH7-4 (2026-08-29).
//
// Coverage:
//   1. Renders nothing visible when `open=false` (reka Dialog Portal
//      closed).
//   2. Lists rows when open + loaded (kind badge / tool / value).
//   3. 撤销 click STAGES the confirm (dialog appears with the row's
//      kind/tool/value) but does NOT call `revoke_tool_permission` yet.
//   4. Confirm in the dialog fires the revoke IPC; cancel does not
//      (and leaves the list untouched).
//
// Mirror of RuntimeMemoryModal.test.ts: reka DialogContent +
// ConfirmDialog teleport to <body>, so every test queries the body
// and `beforeEach` wipes leaked portal DOM.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn(async (cmd: string): Promise<unknown> => {
  if (cmd === "list_session_tool_permissions") return [];
  if (cmd === "revoke_tool_permission") return 1;
  return null;
});

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...(args as Parameters<typeof invokeMock>)),
    listen: async () => () => {},
  },
}));

import PermissionGrantsModal from "./PermissionGrantsModal.vue";
import { useChatStore } from "../../stores/chat";
import {
  usePermissionGrantsStore,
  matchKindLabel,
  type PermissionGrantRow,
} from "../../stores/permissionGrants";

function makeRow(overrides: Partial<PermissionGrantRow> = {}): PermissionGrantRow {
  return {
    sessionId: "s1",
    toolName: "shell",
    matchKind: "prefix",
    matchValue: "git status",
    grantedAt: "2026-08-29 10:00:00",
    ...overrides,
  };
}

/** Seed the chat store so `boundSessionId` resolves and the modal
 *  loads rows on open. */
function seedChat(sessionId: string | null) {
  const chat = useChatStore();
  chat.currentSessionId = sessionId;
  if (sessionId) {
    chat.sessions.push({
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
    } as never);
  }
}

function mountModal(open: boolean) {
  const w = mount(PermissionGrantsModal, {
    attachTo: document.body,
    props: { open },
    global: { stubs: { Icon: true } },
  });
  return w;
}

/** The modal loads on the open transition (`watch(open)`), so tests
 *  mount closed and then flip the prop. */
async function openModal() {
  const w = mountModal(false);
  await w.setProps({ open: true });
  await flushPromises();
  return w;
}

describe("PermissionGrantsModal — revoke confirmation (CH7-4)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "list_session_tool_permissions") return [];
      if (cmd === "revoke_tool_permission") return 1;
      return null;
    });
    // reka DialogContent + ConfirmDialog teleport to body; wipe leaks.
    document.body
      .querySelectorAll(".grant-modal, .grant-modal__overlay, .confirm-modal")
      .forEach((el) => el.remove());
  });

  it("renders nothing visible when open=false", () => {
    seedChat("s1");
    const w = mountModal(false);
    expect(document.body.querySelector(".grant-modal")).toBeNull();
    w.unmount();
  });

  it("lists loaded rows with kind label + tool + match value", async () => {
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "list_session_tool_permissions")
        return [
          makeRow(),
          makeRow({ toolName: "write_file", matchKind: "path", matchValue: "src/*" }),
        ];
      return null;
    });
    seedChat("s1");
    const w = await openModal();

    const modal = document.body.querySelector(".grant-modal");
    expect(modal).not.toBeNull();
    const items = modal?.querySelectorAll(".grant-item");
    expect(items?.length).toBe(2);
    expect(modal?.textContent).toContain("前缀");
    expect(modal?.textContent).toContain("git status");
    expect(modal?.textContent).toContain("路径");
    w.unmount();
  });

  it("撤销 click stages the confirm dialog but does NOT revoke yet", async () => {
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "list_session_tool_permissions") return [makeRow()];
      return null;
    });
    seedChat("s1");
    const w = await openModal();
    expect(invokeMock).not.toHaveBeenCalledWith("revoke_tool_permission", expect.anything());

    // Click the row's 撤销 button.
    const revokeBtn = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>("button"),
    ).find((b) => b.textContent?.includes("撤销") && b.classList.contains("grant-item__revoke"));
    expect(revokeBtn).toBeDefined();
    revokeBtn!.click();
    await flushPromises();

    // Confirm dialog visible with the row summary; no IPC yet.
    const confirm = document.body.querySelector(".confirm-modal");
    expect(confirm).not.toBeNull();
    expect(confirm?.textContent).toContain("撤销此放行?");
    expect(confirm?.textContent).toContain("前缀");
    expect(confirm?.textContent).toContain("shell");
    expect(confirm?.textContent).toContain("git status");
    expect(invokeMock).not.toHaveBeenCalledWith("revoke_tool_permission", expect.anything());
    w.unmount();
  });

  it("confirm fires revoke_tool_permission and drops the row", async () => {
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "list_session_tool_permissions") return [makeRow()];
      if (cmd === "revoke_tool_permission") return 1;
      return null;
    });
    seedChat("s1");
    const w = await openModal();

    const revokeBtn = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>("button"),
    ).find((b) => b.textContent?.includes("撤销") && b.classList.contains("grant-item__revoke"));
    revokeBtn!.click();
    await flushPromises();

    const confirmBtn = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>(".confirm-modal__btn"),
    ).find((b) => b.textContent?.trim() === "撤销");
    expect(confirmBtn).toBeDefined();
    confirmBtn!.click();
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("revoke_tool_permission", {
      sessionId: "s1",
      toolName: "shell",
      matchKind: "prefix",
      matchValue: "git status",
    });
    // Row removed locally + dialog closed.
    expect(usePermissionGrantsStore().grants.length).toBe(0);
    expect(document.body.querySelector(".confirm-modal")).toBeNull();
    w.unmount();
  });

  it("cancel closes the confirm with zero side effects", async () => {
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "list_session_tool_permissions") return [makeRow()];
      return null;
    });
    seedChat("s1");
    const w = await openModal();

    const revokeBtn = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>("button"),
    ).find((b) => b.textContent?.includes("撤销") && b.classList.contains("grant-item__revoke"));
    revokeBtn!.click();
    await flushPromises();

    const cancelBtn = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>(".confirm-modal__btn"),
    ).find((b) => b.textContent?.trim() === "取消");
    expect(cancelBtn).toBeDefined();
    cancelBtn!.click();
    await flushPromises();

    expect(invokeMock).not.toHaveBeenCalledWith("revoke_tool_permission", expect.anything());
    expect(usePermissionGrantsStore().grants.length).toBe(1);
    expect(document.body.querySelector(".confirm-modal")).toBeNull();
    w.unmount();
  });

  it("matchKindLabel covers all three kinds (shared with the item badge)", () => {
    expect(matchKindLabel("tool")).toBe("整工具");
    expect(matchKindLabel("prefix")).toBe("前缀");
    expect(matchKindLabel("path")).toBe("路径");
  });
});
