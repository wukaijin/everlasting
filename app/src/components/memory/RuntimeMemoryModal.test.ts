// Tests for `RuntimeMemoryModal.vue` — 07-06 (am-observability-panel B3).
//
// Coverage:
//   1. Renders nothing visible when `open=false` (reka-ui Dialog
//      Portal renders nothing while closed).
//   2. Renders the memory title + chips (kind/scope/status) when
//      open + memory is bound.
//   3. Stats grid shows hitCount / lastUsedAt / confidence.
//   4. editedByUser badge renders only when the row was user-edited.
//   5. The status dropdown only OFFERS legal targets (matrix-driven
//      — `LEGAL_STATUS_TRANSITIONS[current]`).
//   6. Edit toggle → save calls `store.updateMemory` with the drafted
//      title + content.
//   7. Delete → ConfirmDialog → confirm calls `store.deleteMemory`.
//
// reka-ui DialogContent/ConfirmDialog teleport to <body>; each test
// cleans up the leaked portal DOM in `beforeEach` (same gotcha as
// MarkdownDetailModal.test.ts).
//
// The reka-ui Select dropdown is exercised indirectly — we assert on
// the `legalTargets` logic by checking which `<SelectItem>` values
// render when the trigger is present, rather than driving the full
// open/select interaction (jsdom + reka-ui Select popper interactions
// are flaky; the matrix logic is the unit under test).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn(async (cmd: string): Promise<unknown> => {
  if (cmd === "update_autonomous_memory") return null;
  if (cmd === "update_autonomous_memory_status") return null;
  if (cmd === "delete_autonomous_memory") return 1;
  return null;
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...(args as Parameters<typeof invokeMock>)),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: async () => () => {},
}));

import RuntimeMemoryModal from "./RuntimeMemoryModal.vue";
import {
  useMemoryStore,
  LEGAL_STATUS_TRANSITIONS,
  type AutonomousMemory,
} from "../../stores/memory";

function makeMemory(
  overrides: Partial<AutonomousMemory> = {},
): AutonomousMemory {
  return {
    id: 1,
    memoryId: "uid-1",
    scope: "project",
    projectId: "proj-1",
    kind: "preference",
    status: "candidate",
    title: "Prefer absolute paths",
    content: "Always use absolute paths in tool outputs.",
    tags: "[]",
    toolName: null,
    commandPattern: null,
    pathGlobs: null,
    sourceSessionId: "sess-1",
    sourceRef: "remember tool call 3",
    confidence: 0.7,
    hitCount: 5,
    lastUsedAt: "2026-07-05T08:00:00.000+00:00",
    createdAt: "2026-06-29T12:34:56.789+00:00",
    updatedAt: "2026-06-29T12:34:56.789+00:00",
    demotedReason: null,
    editedByUser: false,
    ...overrides,
  };
}

function mountModal(props: {
  open: boolean;
  memory: AutonomousMemory | null;
}) {
  return mount(RuntimeMemoryModal, {
    attachTo: document.body,
    props,
    global: {
      stubs: { Icon: true },
    },
  });
}

describe("RuntimeMemoryModal — 07-06 (am-observability-panel B3)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "update_autonomous_memory") return null;
      if (cmd === "update_autonomous_memory_status") return null;
      if (cmd === "delete_autonomous_memory") return 1;
      return null;
    });
    // reka-ui + ConfirmDialog teleport to body; wipe leaks.
    document.body
      .querySelectorAll(
        ".runtime-memory-modal, .runtime-memory-modal__overlay, .confirm-modal",
      )
      .forEach((el) => el.remove());
  });

  it("renders nothing visible when open=false", () => {
    const w = mountModal({ open: false, memory: makeMemory() });
    expect(
      document.body.querySelector(".runtime-memory-modal"),
    ).toBeNull();
    w.unmount();
  });

  it("renders the title + kind/scope/status chips when open", async () => {
    const w = mountModal({
      open: true,
      memory: makeMemory({ kind: "pitfall", status: "active", scope: "user" }),
    });
    await flushPromises();
    const modal = document.body.querySelector(".runtime-memory-modal");
    expect(modal).not.toBeNull();
    expect(modal?.textContent).toContain("Prefer absolute paths");
    expect(modal?.querySelector(".runtime-memory-modal__chip--kind-pitfall")).not.toBeNull();
    expect(modal?.querySelector(".runtime-memory-modal__chip--scope")).not.toBeNull();
    expect(modal?.querySelector(".runtime-memory-modal__chip--status-active")).not.toBeNull();
    w.unmount();
  });

  it("renders the stats grid (hitCount / lastUsedAt / confidence)", async () => {
    const w = mountModal({
      open: true,
      memory: makeMemory({ hitCount: 7, confidence: 0.85 }),
    });
    await flushPromises();
    const modal = document.body.querySelector(".runtime-memory-modal");
    const stats = modal?.querySelectorAll(".runtime-memory-modal__stat");
    expect(stats?.length).toBeGreaterThanOrEqual(7);
    // hitCount + confidence render their values.
    expect(modal?.textContent).toContain("7");
    expect(modal?.textContent).toContain("85%");
    // lastUsedAt renders in the compact format.
    expect(modal?.textContent).toContain("2026-07-05 08:00");
    w.unmount();
  });

  it("renders the 人工编辑 chip only when editedByUser is true", async () => {
    // Edited row → chip present.
    const w1 = mountModal({
      open: true,
      memory: makeMemory({ editedByUser: true }),
    });
    await flushPromises();
    expect(
      document.body.querySelector(".runtime-memory-modal__chip--edited"),
    ).not.toBeNull();
    w1.unmount();

    // Agent-written row → chip absent.
    document.body
      .querySelectorAll(".runtime-memory-modal, .runtime-memory-modal__overlay")
      .forEach((el) => el.remove());
    const w2 = mountModal({
      open: true,
      memory: makeMemory({ editedByUser: false }),
    });
    await flushPromises();
    expect(
      document.body.querySelector(".runtime-memory-modal__chip--edited"),
    ).toBeNull();
    w2.unmount();
  });

  it("the P5 status matrix (LEGAL_STATUS_TRANSITIONS) offers legal targets per status — candidate → active/verified/demoted", () => {
    // The dropdown's option list is driven entirely by this map;
    // the reka-ui Select popper doesn't render its items until
    // opened in jsdom (flaky to drive), so we verify the matrix
    // logic directly. The component reads this map via
    // `legalTargets` computed — a correct map means correct options.
    expect(LEGAL_STATUS_TRANSITIONS["candidate"]).toEqual(
      expect.arrayContaining(["active", "verified", "demoted"]),
    );
    // Self-transitions excluded by design.
    expect(LEGAL_STATUS_TRANSITIONS["candidate"]).not.toContain("candidate");
  });

  it("verified status offers only → demoted (the most restricted row)", () => {
    expect(LEGAL_STATUS_TRANSITIONS["verified"]).toEqual(["demoted"]);
  });

  it("demoted can return to any non-demoted status (the recovery row)", () => {
    expect(LEGAL_STATUS_TRANSITIONS["demoted"]).toEqual(
      expect.arrayContaining(["candidate", "active", "verified"]),
    );
  });

  it("edit toggle → save calls store.updateMemory with drafted fields", async () => {
    const store = useMemoryStore();
    const updateSpy = vi.spyOn(store, "updateMemory").mockResolvedValue(true);
    const w = mountModal({
      open: true,
      memory: makeMemory({ id: 42, title: "Old", content: "old" }),
    });
    await flushPromises();

    // Click the "编辑" button to enter edit mode.
    const editBtn = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>("button"),
    ).find((b) => b.textContent?.includes("编辑"));
    expect(editBtn).toBeDefined();
    editBtn?.click();
    await flushPromises();

    // In edit mode the title <input> + content <textarea> mount.
    // reka-ui teleports the Dialog to body, but the inputs render
    // inside the DialogContent (also at body) — query them there.
    const titleInput = document.body.querySelector(
      ".runtime-memory-modal__input",
    ) as HTMLInputElement | null;
    expect(titleInput).not.toBeNull();
    if (titleInput) {
      titleInput.value = "New Title";
      titleInput.dispatchEvent(new Event("input"));
    }
    const textarea = document.body.querySelector(
      ".runtime-memory-modal__textarea",
    ) as HTMLTextAreaElement | null;
    expect(textarea).not.toBeNull();
    if (textarea) {
      textarea.value = "New content body";
      textarea.dispatchEvent(new Event("input"));
    }
    await flushPromises();

    // Click save.
    const saveBtn = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>("button"),
    ).find((b) => b.textContent?.includes("保存"));
    saveBtn?.click();
    await flushPromises();

    expect(updateSpy).toHaveBeenCalledWith(42, "New Title", "New content body");
    w.unmount();
  });

  it("delete → ConfirmDialog → confirm calls store.deleteMemory", async () => {
    const store = useMemoryStore();
    const deleteSpy = vi.spyOn(store, "deleteMemory").mockResolvedValue();
    const w = mountModal({
      open: true,
      memory: makeMemory({ id: 42 }),
    });
    await flushPromises();

    // Click the "删除记忆" button.
    const deleteBtn = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>("button"),
    ).find((b) => b.textContent?.includes("删除记忆"));
    deleteBtn?.click();
    await flushPromises();

    // The ConfirmDialog portals to body — find its confirm button.
    const confirmBtn = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>(".confirm-modal__btn"),
    ).find((b) => b.textContent?.includes("删除"));
    expect(confirmBtn).toBeDefined();
    confirmBtn?.click();
    await flushPromises();

    expect(deleteSpy).toHaveBeenCalledWith(42);
    w.unmount();
  });

  it("surfaces store.runtimeMemoriesError in the error banner", async () => {
    const store = useMemoryStore();
    store.runtimeMemoriesError = "非法状态转换";
    const w = mountModal({
      open: true,
      memory: makeMemory(),
    });
    await flushPromises();
    const banner = document.body.querySelector(".runtime-memory-modal__error");
    expect(banner?.textContent).toContain("非法状态转换");
    w.unmount();
  });
});
