// Tests for `useProjectsStore` — focused on the "添加项目" flow
// (RULE-FrontProj-001 fix: "关闭项目后无法重新打开(create_project
// already exists)").
//
// 2026-09-03 (`09-03-dirbrowser-desktop-unify`): the native picker
// path is gone — `openDirBrowser()` (all modes) opens the
// DirBrowserModal and `addProjectByPath` is the single registration
// entry. Coverage targets:
//
//   1. openDirBrowser() → flips `dirBrowserOpen`, fires ZERO IPC
//      (in particular no native pick command — the chain is dead).
//   2. addProjectByPath() hit on a visible project path → focus
//      existing, do NOT call create_project, do NOT call
//      unhide_project.
//   3. addProjectByPath() hit on a hidden project path → call
//      unhide_project (NOT create_project), toast "已重新打开",
//      return the now-visible row.
//   4. addProjectByPath() on a brand-new path → call create_project.
//   5. Empty-path guard → warn toast, no IPC.
//
// Tauri IPC is mocked so the suite runs in jsdom without a real
// Tauri runtime.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useProjectsStore, type ProjectInfo } from "./projects";

function makeProject(overrides: Partial<ProjectInfo> = {}): ProjectInfo {
  return {
    id: "proj-1",
    name: "Everlasting",
    path: "/usr/local/code/github/everlasting",
    is_git_repo: true,
    git_branch: "main",
    is_legacy: false,
    created_at: "2026-06-23T00:00:00Z",
    updated_at: "2026-06-23T00:00:00Z",
    hidden: false,
    metadata: null,
    ...overrides,
  };
}

const VISIBLE_PROJECT = makeProject({
  id: "vis-1",
  name: "Visible",
  path: "/path/visible",
  hidden: false,
});

const HIDDEN_PROJECT = makeProject({
  id: "hid-1",
  name: "Hidden",
  path: "/path/hidden",
  hidden: true,
  is_git_repo: false,
  git_branch: null,
});

const FRESH_PROJECT = makeProject({
  id: "fresh-1",
  name: "Fresh",
  path: "/path/fresh",
  hidden: false,
});

// ---------------------------------------------------------------------------
// openDirBrowser — unified entry (2026-09-03): flips the modal flag,
// zero backend traffic. The registration path (create/unhide) only
// runs when the modal calls addProjectByPath with a chosen path.
// ---------------------------------------------------------------------------
describe("useProjectsStore — openDirBrowser (unified entry)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [];
      return null;
    });
  });

  it("翻 dirBrowserOpen,零 IPC(native pick 链已下线)", async () => {
    const store = useProjectsStore();
    await store.loadProjects();
    expect(store.dirBrowserOpen).toBe(false);

    invokeMock.mockClear();
    store.openDirBrowser();

    expect(store.dirBrowserOpen).toBe(true);
    expect(invokeMock.mock.calls).toHaveLength(0);
  });

  it("closeDirBrowser:关 dirBrowserOpen", () => {
    const store = useProjectsStore();
    store.dirBrowserOpen = true;

    store.closeDirBrowser();

    expect(store.dirBrowserOpen).toBe(false);
  });
});

describe("useProjectsStore — addProjectByPath (RULE-FrontProj-001)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    // Default: list_* IPCs return empty so the store starts in a
    // known state. Per-test cases override.
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [];
      return null;
    });
  });

  it("命中 visible 项目路径:不调 IPC,直接 focus + 提示「项目已存在」", async () => {
    const store = useProjectsStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [VISIBLE_PROJECT];
      if (cmd === "list_hidden_projects") return [];
      return null;
    });
    await store.loadProjects();
    invokeMock.mockClear();

    const result = await store.addProjectByPath(VISIBLE_PROJECT.path);

    expect(result?.id).toBe(VISIBLE_PROJECT.id);
    expect(store.currentProjectId).toBe(VISIBLE_PROJECT.id);
    // The unhide / create IPCs must NOT be called for the visible
    // path; the modal flag closes.
    const calledCmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCmds).not.toContain("unhide_project");
    expect(calledCmds).not.toContain("create_project");
    expect(store.dirBrowserOpen).toBe(false);
  });

  it("命中 hidden 项目路径:调 unhide_project,不调 create_project,toast 成功", async () => {
    const store = useProjectsStore();
    // Simulate state mutation: once `unhide_project` IPC fires,
    // the row moves from `hidden` to `visible`.
    const visibleNow = [VISIBLE_PROJECT];
    const hiddenNow = [HIDDEN_PROJECT];
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return visibleNow;
      if (cmd === "list_hidden_projects") return hiddenNow;
      if (cmd === "unhide_project") {
        // Move the hidden row into the visible list.
        const idx = visibleNow.findIndex((p) => p.id === HIDDEN_PROJECT.id);
        if (idx === -1) {
          visibleNow.push({ ...HIDDEN_PROJECT, hidden: false });
        }
        const hidIdx = hiddenNow.findIndex((p) => p.id === HIDDEN_PROJECT.id);
        if (hidIdx !== -1) hiddenNow.splice(hidIdx, 1);
        return null;
      }
      return null;
    });
    await store.loadProjects();
    await store.loadHiddenProjects();
    invokeMock.mockClear();

    const result = await store.addProjectByPath(HIDDEN_PROJECT.path);

    expect(result?.id).toBe(HIDDEN_PROJECT.id);
    expect(store.currentProjectId).toBe(HIDDEN_PROJECT.id);
    const calledCmds = invokeMock.mock.calls.map((c) => c[0]);
    // Core fix: create_project must NEVER be called when the picked
    // path matches a hidden project.
    expect(calledCmds).not.toContain("create_project");
    // unhide_project MUST be called.
    expect(calledCmds).toContain("unhide_project");
  });

  it("全新路径:调 create_project,不走 unhide", async () => {
    const store = useProjectsStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [VISIBLE_PROJECT];
      if (cmd === "list_hidden_projects") return [HIDDEN_PROJECT];
      if (cmd === "create_project") return FRESH_PROJECT;
      return null;
    });
    await store.loadProjects();
    await store.loadHiddenProjects();
    invokeMock.mockClear();

    const result = await store.addProjectByPath("/path/brand-new");

    expect(result?.id).toBe(FRESH_PROJECT.id);
    expect(store.currentProjectId).toBe(FRESH_PROJECT.id);
    const calledCmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCmds).toContain("create_project");
    expect(calledCmds).not.toContain("unhide_project");
    expect(store.dirBrowserOpen).toBe(false);
  });

  it("空路径 → toast warn,不调 create_project,模态框不关", async () => {
    const store = useProjectsStore();
    await store.loadProjects();
    store.dirBrowserOpen = true;

    const result = await store.addProjectByPath("   ");

    expect(result).toBeNull();
    expect(store.toast?.kind).toBe("warn");
    // Empty path never reaches the backend; the modal stays open
    // for the user to pick a real one.
    expect(store.dirBrowserOpen).toBe(true);
    const calledCmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCmds).not.toContain("create_project");
  });

  it("create_project 失败 → toast error,return null", async () => {
    const store = useProjectsStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [];
      if (cmd === "create_project") throw new Error("UNIQUE constraint");
      return null;
    });
    await store.loadProjects();
    await store.loadHiddenProjects();

    const result = await store.addProjectByPath("/path/brand-new");

    expect(result).toBeNull();
    expect(store.currentProjectId).toBeNull();
    expect(store.toast?.kind).toBe("error");
  });

  it("hiddenProjects.value 空时应先 loadHiddenProjects 再判断 (lazy load 兜底)", async () => {
    const store = useProjectsStore();
    // Initial state: loadProjects returns visible, hiddenProjects not
    // yet loaded. The chosen path matches a hidden project — the
    // store should load hidden first and then auto-unhide.
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [HIDDEN_PROJECT];
      if (cmd === "unhide_project") return null;
      return null;
    });
    await store.loadProjects();
    invokeMock.mockClear();
    // NB: NOT calling loadHiddenProjects() here — registerPickedPath
    // must do it itself when hiddenProjects.value is empty.
    expect(store.hiddenProjects.length).toBe(0);

    const result = await store.addProjectByPath(HIDDEN_PROJECT.path);

    expect(result?.id).toBe(HIDDEN_PROJECT.id);
    const calledCmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCmds).toContain("list_hidden_projects");
    expect(calledCmds).not.toContain("create_project");
  });
});

describe("useProjectsStore — unhideProject return value", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("IPC 成功:返回 true + 自动 focus", async () => {
    const store = useProjectsStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [HIDDEN_PROJECT];
      if (cmd === "list_hidden_projects") return [];
      if (cmd === "unhide_project") return null;
      return null;
    });
    await store.loadProjects();

    const ok = await store.unhideProject(HIDDEN_PROJECT.id);
    expect(ok).toBe(true);
    expect(store.currentProjectId).toBe(HIDDEN_PROJECT.id);
  });

  it("IPC 失败:返回 false + 不改 currentProjectId", async () => {
    const store = useProjectsStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [];
      if (cmd === "unhide_project") {
        throw new Error("backend gone");
      }
      return null;
    });
    await store.loadProjects();

    const ok = await store.unhideProject(HIDDEN_PROJECT.id);
    expect(ok).toBe(false);
    expect(store.currentProjectId).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// BUGLIST CH3-1 (2026-08-29 GUI full-test): `hideProject` used to refresh
// only `list_projects`, leaving `hiddenProjects` stale until the next page
// reload. With another hidden project already present at startup, the
// stale list never contained the just-hidden row, so「已隐藏项目」didn't
// show it and re-adding its path fell through to `create_project` →
// UNIQUE "already exists" toast. Lock: hide refreshes the hidden list
// immediately, and the re-add path recovers via unhide (no create).
// ---------------------------------------------------------------------------
describe("useProjectsStore — hideProject refreshes hidden list (CH3-1)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("hide 后 store.hiddenProjects 立即含该项目(重调 list_hidden_projects)", async () => {
    const store = useProjectsStore();
    // Stale-list scenario: ANOTHER hidden project already exists at
    // startup, so the lazy `length === 0` reload in registerPickedPath
    // would never have fired — only hideProject's own refresh helps.
    let visibleNow = [VISIBLE_PROJECT];
    let hiddenNow = [HIDDEN_PROJECT];
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return visibleNow;
      if (cmd === "list_hidden_projects") return hiddenNow;
      if (cmd === "hide_project") {
        visibleNow = [];
        hiddenNow = [HIDDEN_PROJECT, { ...VISIBLE_PROJECT, hidden: true }];
        return null;
      }
      return null;
    });
    await store.loadProjects();
    await store.loadHiddenProjects();
    expect(store.hiddenProjects.map((p) => p.id)).toEqual([HIDDEN_PROJECT.id]);

    await store.hideProject(VISIBLE_PROJECT.id);

    expect(store.hiddenProjects.map((p) => p.id)).toContain(VISIBLE_PROJECT.id);
  });

  it("hide 后立即重加同路径:走 unhide 恢复,不触发 create_project", async () => {
    const store = useProjectsStore();
    let visibleNow = [VISIBLE_PROJECT];
    let hiddenNow = [HIDDEN_PROJECT];
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return visibleNow;
      if (cmd === "list_hidden_projects") return hiddenNow;
      if (cmd === "hide_project") {
        visibleNow = [];
        hiddenNow = [HIDDEN_PROJECT, { ...VISIBLE_PROJECT, hidden: true }];
        return null;
      }
      if (cmd === "unhide_project") {
        hiddenNow = [HIDDEN_PROJECT];
        visibleNow = [VISIBLE_PROJECT];
        return null;
      }
      return null;
    });
    await store.loadProjects();
    await store.loadHiddenProjects();

    await store.hideProject(VISIBLE_PROJECT.id);
    const result = await store.addProjectByPath(VISIBLE_PROJECT.path);

    expect(result?.id).toBe(VISIBLE_PROJECT.id);
    expect(store.currentProjectId).toBe(VISIBLE_PROJECT.id);
    const calledCmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCmds).toContain("unhide_project");
    expect(calledCmds).not.toContain("create_project");
  });
});
