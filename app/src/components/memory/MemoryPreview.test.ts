// Tests for `MemoryPreview.vue` — P2 PR3 additions: the
// "自主记忆" / Runtime Memories section.
//
// Coverage:
//   1. Section renders the "自主记忆" header.
//   2. Empty state renders the placeholder when `runtimeMemories`
//      is empty and not loading.
//   3. Loading state renders the spinner placeholder when
//      `runtimeMemoriesLoading === true` and list is empty.
//   4. Error state renders the error banner when
//      `runtimeMemoriesError` is set.
//   5. The list renders a row per memory with title, kind badge,
//      content preview, and timestamp.
//   6. Per-row delete button → opens the ConfirmDialog.
//   7. ConfirmDialog confirm → invokes `store.deleteMemory` with
//      the row's auto-id.
//   8. ConfirmDialog cancel → does NOT invoke `store.deleteMemory`.
//   9. Refresh button → invokes `store.fetchMemories`.
//  10. The instruction-file section is unchanged (regression lock):
//      the panel still shows the `MemoryLayerItem` children when
//      `store.layers` is populated.
//
// Tauri IPC is mocked at the file level (jsdom cannot import
// `@tauri-apps/api/core` for real). The store is driven directly
// per-test via `storeToRefs(...).runtimeMemories.value = [...]`
// — Pinia setup stores do NOT support direct proxy property
// assignment for refs (only `.value` via the refs handle works).
//
// The component's `onMounted` calls `store.loadForProject(pid)`,
// which fetches BOTH the instruction-file layers AND the runtime
// memories. The mock must return a valid `[]` array (not `null`)
// for the layer fetch — assigning `null` to a `Ref<...[]>` makes
// the template's `.length` blow up. Tests that need a different
// layer shape use `vi.mocked(invoke).mockImplementation(...)`.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia, storeToRefs } from "pinia";

const invokeMock = vi.fn(async (cmd: string): Promise<unknown> => {
  if (cmd === "read_memory_layers") return [];
  if (cmd === "list_autonomous_memories") return [];
  if (cmd === "delete_autonomous_memory") return 1;
  return null;
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...(args as Parameters<typeof invokeMock>)),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import MemoryPreview from "./MemoryPreview.vue";
import { useMemoryStore, type AutonomousMemory, type MemoryLayerInfo } from "../../stores/memory";
import { useProjectsStore } from "../../stores/projects";

function makeMemory(overrides: Partial<AutonomousMemory> = {}): AutonomousMemory {
  return {
    id: 1,
    memoryId: "uid-1",
    scope: "project",
    projectId: "proj-1",
    kind: "preference",
    status: "candidate",
    title: "Prefer absolute paths",
    content: "Always use absolute paths in tool outputs and code examples.",
    tags: '["paths","preference"]',
    toolName: null,
    commandPattern: null,
    pathGlobs: null,
    sourceSessionId: "sess-1",
    sourceRef: "remember tool call 3",
    confidence: 0.5,
    hitCount: 0,
    lastUsedAt: null,
    createdAt: "2026-06-29T12:34:56.789+00:00",
    updatedAt: "2026-06-29T12:34:56.789+00:00",
    demotedReason: null,
    ...overrides,
  };
}

const SAMPLE_LAYER: MemoryLayerInfo = {
  kind: "project",
  source: "claude",
  path: "/home/x/code/everlasting/CLAUDE.md",
  tokens: 0,
  status: { kind: "missing" },
  char_count: 0,
};

function mountPreview(props: { projectId?: string | null; kind?: "user" | "project" | "all" } = {}) {
  return mount(MemoryPreview, {
    attachTo: document.body,
    props,
    global: {
      stubs: { Icon: true },
    },
  });
}

describe("MemoryPreview — runtime memories section (P2 PR3)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") return [];
      if (cmd === "delete_autonomous_memory") return 1;
      return null;
    });
    // The Projects store is read by the preview's
    // `effectiveProjectId` fallback. Set a default project so the
    // panel enters a usable state.
    const projects = useProjectsStore();
    projects.currentProjectId = "proj-1";
  });

  it("renders the 自主记忆 section header", () => {
    const w = mountPreview();
    const header = w.find(".memory-preview__runtime-title");
    expect(header.exists()).toBe(true);
    expect(header.text()).toBe("自主记忆");
    w.unmount();
  });

  it("renders the empty state when there are no runtime memories", async () => {
    const w = mountPreview();
    await flushPromises();
    const empty = w.findAll(".memory-preview__empty").map((n) => n.text());
    // The empty list shows the runtime-memories placeholder text.
    expect(empty.some((t) => t.includes("该项目暂无自主记忆"))).toBe(true);
    w.unmount();
  });

  it("renders the loading state when runtimeMemoriesLoading is true and the list is empty", async () => {
    // Force the store to be in the loading state. Easiest path:
    // spy on fetchMemories to flip the loading flag and never
    // resolve, then assert the spinner. (Calling
    // `runtimeMemoriesLoading.value = true` directly via
    // storeToRefs works for the initial state, but `onMounted`
    // immediately resets it to false via the fetch.)
    const store = useMemoryStore();
    const refs = storeToRefs(store);
    // Make the fetch hang so loading stays true.
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") {
        // Never resolve — keeps the loading flag true.
        await new Promise(() => {});
        return [];
      }
      return null;
    });

    const w = mountPreview();
    await flushPromises();
    // Manually set the flag in case the fetch resolved too fast
    // (defensive — the hanging mock should keep it true).
    refs.runtimeMemoriesLoading.value = true;
    await flushPromises();

    const loadingNodes = w.findAll(".memory-preview__loading");
    expect(loadingNodes.some((n) => n.text().includes("加载自主记忆中"))).toBe(true);
    w.unmount();
  });

  it("renders the error state when runtimeMemoriesError is set", async () => {
    // Force the fetch to fail.
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") {
        throw new Error("list_autonomous_memories: connection lost");
      }
      return null;
    });

    const w = mountPreview();
    await flushPromises();

    const errors = w.findAll(".memory-preview__error");
    expect(
      errors.some((n) =>
        n.text().includes("自主记忆暂不可用:Error: list_autonomous_memories: connection lost"),
      ),
    ).toBe(true);
    w.unmount();
  });

  it("renders one row per runtime memory with title, kind badge, content preview, and timestamp", async () => {
    // Make the fetch return the rows we want to render.
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") {
        return [
          makeMemory({ id: 11, title: "WSL cargo test pitfall", kind: "pitfall", content: "WSL cargo test fails on gdk-pixbuf not found.", tags: "[]" }),
          makeMemory({ id: 12, memoryId: "uid-2", title: "User-level preference", scope: "user", projectId: null, kind: "preference", content: "Likes concise replies.", tags: '["concise"]' }),
        ];
      }
      return null;
    });

    const w = mountPreview();
    await flushPromises();
    const rows = w.findAll(".runtime-memory");
    expect(rows).toHaveLength(2);

    // Row 1: title + kind badge + content preview + timestamp.
    expect(rows[0]?.text()).toContain("WSL cargo test pitfall");
    expect(rows[0]?.find(".runtime-memory__badge--kind-pitfall").exists()).toBe(true);
    expect(rows[0]?.text()).toContain("WSL cargo test fails on gdk-pixbuf not found.");
    // RFC 3339 → YYYY-MM-DD HH:MM
    expect(rows[0]?.text()).toContain("2026-06-29 12:34");

    // Row 2: user scope + preference kind + tag chip.
    expect(rows[1]?.text()).toContain("User-level preference");
    expect(rows[1]?.find(".runtime-memory__badge--scope-user").exists()).toBe(true);
    expect(rows[1]?.find(".runtime-memory__badge--kind-preference").exists()).toBe(true);
    expect(rows[1]?.text()).toContain("#concise");
    w.unmount();
  });

  it("truncates long content previews to 80 chars with ellipsis", async () => {
    const longContent = "x".repeat(120);
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") {
        return [makeMemory({ id: 11, content: longContent })];
      }
      return null;
    });

    const w = mountPreview();
    await flushPromises();
    const preview = w.find(".runtime-memory__content").text();
    expect(preview).toContain("…");
    // 80 'x' + the '…' suffix.
    expect(preview.length).toBeLessThanOrEqual(81);
    w.unmount();
  });

  it("delete button click opens the ConfirmDialog and confirm calls store.deleteMemory", async () => {
    const store = useMemoryStore();
    const deleteSpy = vi.spyOn(store, "deleteMemory");
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") {
        return [makeMemory({ id: 11, title: "Doomed memory" })];
      }
      return null;
    });

    const w = mountPreview();
    await flushPromises();
    const deleteBtn = w.find(".runtime-memory__delete");
    expect(deleteBtn.exists()).toBe(true);

    // Click → confirm dialog opens with the row's title.
    await deleteBtn.trigger("click");
    await flushPromises();
    const dialog = document.body.querySelector(".confirm-modal");
    expect(dialog).not.toBeNull();
    expect(dialog?.textContent).toContain("Doomed memory");

    // Confirm → invokes store.deleteMemory(11) + closes the dialog.
    const confirmBtn = Array.from(document.body.querySelectorAll(".confirm-modal__btn"))
      .find((b) => b.textContent?.includes("删除")) as HTMLButtonElement | undefined;
    expect(confirmBtn).toBeDefined();
    confirmBtn?.click();
    await flushPromises();
    expect(deleteSpy).toHaveBeenCalledWith(11);
    // Dialog closes.
    expect(document.body.querySelector(".confirm-modal")).toBeNull();
    w.unmount();
  });

  it("delete confirmation cancel does NOT invoke store.deleteMemory", async () => {
    const store = useMemoryStore();
    const deleteSpy = vi.spyOn(store, "deleteMemory");
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") {
        return [makeMemory({ id: 11 })];
      }
      return null;
    });

    const w = mountPreview();
    await flushPromises();
    const deleteBtn = w.find(".runtime-memory__delete");
    await deleteBtn.trigger("click");
    await flushPromises();

    const cancelBtn = Array.from(document.body.querySelectorAll(".confirm-modal__btn"))
      .find((b) => b.textContent?.includes("取消")) as HTMLButtonElement | undefined;
    expect(cancelBtn).toBeDefined();
    cancelBtn?.click();
    await flushPromises();

    expect(deleteSpy).not.toHaveBeenCalled();
    w.unmount();
  });

  it("refresh button invokes store.fetchMemories", async () => {
    const store = useMemoryStore();
    const fetchSpy = vi.spyOn(store, "fetchMemories");
    const w = mountPreview();
    await flushPromises();
    const refreshBtns = w.findAll(".memory-preview__refresh");
    // The runtime section's refresh is the one inside the
    // .memory-preview__runtime section.
    const runtimeSection = w.find(".memory-preview__runtime");
    const runtimeRefresh = runtimeSection.find(".memory-preview__refresh");
    expect(runtimeRefresh.exists()).toBe(true);
    await runtimeRefresh.trigger("click");
    expect(fetchSpy).toHaveBeenCalled();
    expect(refreshBtns.length).toBeGreaterThanOrEqual(1);
    w.unmount();
  });
});

describe("MemoryPreview — instruction-file section is unchanged (regression lock)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "read_memory_layers") return [];
      if (cmd === "list_autonomous_memories") return [];
      if (cmd === "delete_autonomous_memory") return 1;
      return null;
    });
    const projects = useProjectsStore();
    projects.currentProjectId = "proj-1";
  });

  it("still renders the instruction-file header and the 4-layer summary when no runtime memories exist", async () => {
    // Mock `read_memory_layers` to return 4 layers (all missing)
    // so the chip "0 loaded · 4 missing" is rendered.
    const store = useMemoryStore();
    const refs = storeToRefs(store);
    const fourMissingLayers: MemoryLayerInfo[] = [
      { ...SAMPLE_LAYER, kind: "user", source: "claude", path: "/home/x/.claude/CLAUDE.md" },
      { ...SAMPLE_LAYER, kind: "user", source: "agents", path: "/home/x/.config/everlasting/AGENTS.md" },
      { ...SAMPLE_LAYER, kind: "project", source: "claude" },
      { ...SAMPLE_LAYER, kind: "project", source: "agents" },
    ];
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "read_memory_layers") return fourMissingLayers;
      if (cmd === "list_autonomous_memories") return [];
      if (cmd === "delete_autonomous_memory") return 1;
      return null;
    });

    const w = mountPreview({ kind: "all" });
    await flushPromises();

    // The instruction-file section title is still there.
    const allTitles = w.findAll(".memory-preview__title").map((n) => n.text());
    expect(allTitles).toContain("指令文件");

    // The runtime section is still there.
    expect(w.find(".memory-preview__runtime-title").exists()).toBe(true);

    // The "0 loaded · 4 missing" chip is still there (4 layers
    // all missing → 0 loaded, 4 missing).
    expect(refs.layers.value).toHaveLength(4);
    expect(w.text()).toContain("0 loaded");
    expect(w.text()).toContain("4 missing");
    w.unmount();
  });
});

