// Tests for `useProjectsStore` — focused on the addProject path
// (RULE-FrontProj-001 fix: "关闭项目后无法重新打开(create_project
// already exists)").
//
// Coverage targets:
//   1. addProject() hit on a visible project path → focus existing,
//      do NOT call create_project IPC, do NOT call unhide_project.
//   2. addProject() hit on a hidden project path → call unhide_project
//      (NOT create_project), toast "已重新打开", return the now-visible
//      row.
//   3. addProject() on a brand-new path → call create_project.
//   4. addProject() with user cancelling the dialog (picked === null)
//      → no IPC calls, return null.
//   5. addProject() with the dialog failing (invoke throws) → no
//      create_project call, surface error toast, return null.
//
// Tauri IPC is mocked so the suite runs in jsdom without a real
// Tauri runtime.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: async () => () => {},
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

describe("useProjectsStore — addProject (RULE-FrontProj-001)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    // Default: list_* IPCs return empty so the store starts in a
    // known state. Per-test cases override.
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [];
      if (cmd === "pick_project_dir") return null;
      return null;
    });
  });

  it("命中 visible 项目路径:不调 IPC,直接 focus + 提示「项目已存在」", async () => {
    const store = useProjectsStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [VISIBLE_PROJECT];
      if (cmd === "list_hidden_projects") return [];
      if (cmd === "pick_project_dir") return VISIBLE_PROJECT.path;
      return null;
    });
    await store.loadProjects();

    const result = await store.addProject();

    expect(result?.id).toBe(VISIBLE_PROJECT.id);
    expect(store.currentProjectId).toBe(VISIBLE_PROJECT.id);
    // The unhide / create IPCs must NOT be called for the visible
    // path; the only IPC call after `loadProjects` is `pick_project_dir`.
    const calledCmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCmds).toContain("pick_project_dir");
    expect(calledCmds).not.toContain("unhide_project");
    expect(calledCmds).not.toContain("create_project");
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
      if (cmd === "pick_project_dir") return HIDDEN_PROJECT.path;
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

    const result = await store.addProject();

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
      if (cmd === "pick_project_dir") return "/path/brand-new";
      if (cmd === "create_project") return FRESH_PROJECT;
      return null;
    });
    await store.loadProjects();
    await store.loadHiddenProjects();

    const result = await store.addProject();

    expect(result?.id).toBe(FRESH_PROJECT.id);
    expect(store.currentProjectId).toBe(FRESH_PROJECT.id);
    const calledCmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCmds).toContain("create_project");
    expect(calledCmds).not.toContain("unhide_project");
  });

  it("用户取消 dialog (picked === null):不调任何 IPC,return null", async () => {
    const store = useProjectsStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [];
      if (cmd === "pick_project_dir") return null;
      return null;
    });
    await store.loadProjects();

    const result = await store.addProject();

    expect(result).toBeNull();
    expect(store.currentProjectId).toBeNull();
    const calledCmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCmds).not.toContain("create_project");
    expect(calledCmds).not.toContain("unhide_project");
  });

  it("dialog 失败 (invoke throws):toast 错误，不调 create_project,return null", async () => {
    const store = useProjectsStore();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [];
      if (cmd === "pick_project_dir") {
        throw new Error("dialog channel closed");
      }
      return null;
    });
    await store.loadProjects();

    const result = await store.addProject();

    expect(result).toBeNull();
    expect(store.currentProjectId).toBeNull();
    const calledCmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(calledCmds).not.toContain("create_project");
    expect(calledCmds).not.toContain("unhide_project");
  });

  it("hiddenProjects.value 空时 addProject 应先 loadHiddenProjects 再判断 (lazy load 兜底)", async () => {
    const store = useProjectsStore();
    // Initial state: loadProjects returns visible, hiddenProjects not
    // yet loaded. User adds a path that matches a hidden project — the
    // store should load hidden first and then auto-unhide.
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [HIDDEN_PROJECT];
      if (cmd === "pick_project_dir") return HIDDEN_PROJECT.path;
      if (cmd === "unhide_project") return null;
      return null;
    });
    await store.loadProjects();
    // NB: NOT calling loadHiddenProjects() here — addProject must do
    // it itself when hiddenProjects.value is empty.
    expect(store.hiddenProjects.length).toBe(0);

    const result = await store.addProject();

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