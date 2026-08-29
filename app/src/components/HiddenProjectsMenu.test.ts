// Tests for `HiddenProjectsMenu.vue` — BUGLIST CH3-2 (2026-08-29).
//
// Coverage: clicking a row's「重新打开」closes the dropdown immediately
// (multi-hidden-project case — the list would otherwise keep showing
// the just-restored row until manual dismissal), while the badge count
// keeps tracking the store. Single-row case: restoring the last hidden
// project removes the trigger entirely (pre-existing v-if, guarded
// here so the close logic can't regress it).
//
// reka DropdownMenuContent teleports to <body>: open via a real
// trigger click, query rows in document.body (SearchModal.test.ts
// attachTo-body precedent), wipe leaked portal DOM in `beforeEach`.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn(async (cmd: string): Promise<unknown> => {
  if (cmd === "list_projects") return [];
  if (cmd === "list_hidden_projects") return [];
  if (cmd === "unhide_project") return null;
  return null;
});

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...(args as Parameters<typeof invokeMock>)),
    listen: async () => () => {},
  },
}));

import HiddenProjectsMenu from "./HiddenProjectsMenu.vue";
import { useProjectsStore, type ProjectInfo } from "../stores/projects";

function makeProject(id: string, overrides: Partial<ProjectInfo> = {}): ProjectInfo {
  return {
    id,
    name: `proj-${id}`,
    path: `/tmp/${id}`,
    is_git_repo: false,
    git_branch: null,
    is_legacy: false,
    created_at: "",
    updated_at: "",
    hidden: true,
    metadata: null,
    ...overrides,
  };
}

async function openMenu(twoProjects: boolean) {
  // The component's onMounted `loadHiddenProjects()` re-reads the
  // backend and OVERWRITES any direct store seed — the mock must
  // answer with the initial hidden list (production shape).
  const initialHidden = twoProjects
    ? [makeProject("p1"), makeProject("p2")]
    : [makeProject("p1")];
  invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
    if (cmd === "list_projects") return [];
    if (cmd === "list_hidden_projects") return initialHidden;
    if (cmd === "unhide_project") return null;
    return null;
  });
  const store = useProjectsStore();
  const w = mount(HiddenProjectsMenu, {
    attachTo: document.body,
    global: { stubs: { Icon: true } },
  });
  await flushPromises();
  // Open the dropdown via the real trigger.
  const trigger = document.body.querySelector<HTMLButtonElement>(
    "[data-testid='hidden-projects-trigger']",
  );
  trigger?.click();
  await flushPromises();
  return { w, store };
}

function portalRows(): HTMLElement[] {
  return Array.from(
    document.body.querySelectorAll<HTMLElement>("[data-testid='hidden-projects-row']"),
  );
}

describe("HiddenProjectsMenu — auto-close after unhide (CH3-2)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [];
      if (cmd === "unhide_project") return null;
      return null;
    });
    document.body.innerHTML = "";
  });

  it("unhide with 2 hidden projects: menu closes, badge decrements", async () => {
    const { w, store } = await openMenu(true);
    expect(portalRows().length).toBe(2);

    // unhide_project succeeds → store reloads (list_hidden_projects
    // now returns only p2, mirroring the backend re-read).
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "list_projects") return [makeProject("p1", { hidden: false })];
      if (cmd === "list_hidden_projects") return [makeProject("p2")];
      if (cmd === "unhide_project") return null;
      return null;
    });

    const action = portalRows()[0].querySelector<HTMLButtonElement>(
      "[data-testid='hidden-projects-action']",
    );
    action?.click();
    await flushPromises();

    // Menu closed (portal content gone) + badge shows the reloaded
    // count (1).
    expect(document.body.querySelector(".hidden-menu__content")).toBeNull();
    expect(
      document.body.querySelector("[data-testid='hidden-projects-count']")?.textContent,
    ).toBe("1");
    expect(store.hiddenProjects.length).toBe(1);
    w.unmount();
  });

  it("unhide of the last hidden project: menu gone AND trigger unmounted", async () => {
    const { w, store } = await openMenu(false);
    expect(portalRows().length).toBe(1);

    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "list_projects") return [makeProject("p1", { hidden: false })];
      if (cmd === "list_hidden_projects") return [];
      if (cmd === "unhide_project") return null;
      return null;
    });

    portalRows()[0]
      .querySelector<HTMLButtonElement>("[data-testid='hidden-projects-action']")
      ?.click();
    await flushPromises();

    expect(document.body.querySelector(".hidden-menu__content")).toBeNull();
    expect(
      document.body.querySelector("[data-testid='hidden-projects-trigger']"),
    ).toBeNull();
    expect(store.hiddenProjects.length).toBe(0);
    w.unmount();
  });

  it("failed unhide (IPC rejects) keeps the menu open", async () => {
    const { w } = await openMenu(true);
    invokeMock.mockImplementation(async (cmd: string): Promise<unknown> => {
      if (cmd === "unhide_project") throw new Error("db locked");
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [makeProject("p1"), makeProject("p2")];
      return null;
    });

    portalRows()[0]
      .querySelector<HTMLButtonElement>("[data-testid='hidden-projects-action']")
      ?.click();
    await flushPromises();

    // No close on failure — the user can retry; error toast is the
    // store's contract.
    expect(document.body.querySelector(".hidden-menu__content")).not.toBeNull();
    w.unmount();
  });
});
