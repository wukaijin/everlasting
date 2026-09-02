// Tests for `DirBrowserModal.vue` — 2026-09-02 browser-mode 目录浏览
// 模态框(pick_project_dir 的 web degrade,替代原内联手输路径框)。
//
// Coverage:
//   1. open → get_home_dir 冷启动 + browse_dir(home) 拉首屏列表。
//   2. 单击目录行 → browse_dir(该目录);「..」/「上一步」 → browse_dir(父目录)。
//   3. 路径输入 + 前往 → browse_dir(输入值);「显示隐藏目录」toggle →
//      browse_dir 以 showHidden=true 重取当前目录。
//   4. 「选择此目录」 → store.addProjectByPath(当前目录)全链
//      (create_project + 关 dirBrowserOpen)。
//   5. browse_dir 失败 → 行内错误,「选择此目录」禁用。
//
// Mirror of AuditLogModal.test.ts: reka DialogContent teleports to
// <body>, so every test queries the body and `beforeEach` wipes
// leaked portal DOM. The real projects store runs against the mocked
// transport (store-coupled modal test precedent).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();
invokeMock.mockImplementation(async (): Promise<unknown> => null);

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...(args as Parameters<typeof invokeMock>)),
    listen: async () => () => {},
  },
}));

import DirBrowserModal from "./DirBrowserModal.vue";
import { useProjectsStore } from "../stores/projects";

const HOME = "/home/tester";

/** browse_dir 的 wire 应答。 */
function dirPayload(over: Partial<{
  path: string;
  parent: string | null;
  entries: Array<{ name: string; path: string }>;
}> = {}) {
  return {
    path: HOME,
    parent: "/home",
    entries: [
      { name: "alpha", path: `${HOME}/alpha` },
      { name: "beta", path: `${HOME}/beta` },
    ],
    ...over,
  };
}

/** 找到 teleport 到 body 里的元素(portal 泄漏时兜底查组件根)。 */
function findInBody(wrapper: ReturnType<typeof mount>, selector: string): Element[] {
  const all = [...document.body.querySelectorAll(selector)];
  if (all.length > 0) return all;
  return [...wrapper.element.querySelectorAll(selector)];
}

async function openModal() {
  const wrapper = mount(DirBrowserModal, {
    props: { open: false },
    attachTo: document.body,
  });
  await wrapper.setProps({ open: true });
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  document.body.innerHTML = "";
  setActivePinia(createPinia());
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "get_home_dir") return HOME;
    if (cmd === "browse_dir") return dirPayload();
    if (cmd === "list_projects") return [];
    if (cmd === "list_hidden_projects") return [];
    return null;
  });
});

describe("DirBrowserModal", () => {
  it("open 后从 home 冷启动: get_home_dir + browse_dir(home),渲染目录行", async () => {
    const wrapper = await openModal();

    const cmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(cmds[0]).toBe("get_home_dir");
    expect(cmds[1]).toBe("browse_dir");
    expect(invokeMock.mock.calls[1][1]).toEqual({
      path: HOME,
      showHidden: false,
    });

    const rows = findInBody(wrapper, ".dir-browser__row");
    const names = rows.map((r) => r.textContent?.trim());
    expect(names).toContain("..");
    expect(names).toContain("alpha");
    expect(names).toContain("beta");

    wrapper.unmount();
  });

  it("get_home_dir 拿不到时退化到 /,不阻塞浏览", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "get_home_dir") throw new Error("boom");
      if (cmd === "browse_dir") return dirPayload({ path: "/", parent: null });
      return null;
    });
    const wrapper = await openModal();

    expect(invokeMock.mock.calls[1][1]).toEqual({
      path: "/",
      showHidden: false,
    });
    // 根目录无 parent:无「..」行、「上一步」禁用
    expect(findInBody(wrapper, ".dir-browser__row--up").length).toBe(0);
    const back = findInBody(wrapper, ".dir-browser__back")[0] as HTMLButtonElement;
    expect(back.disabled).toBe(true);

    wrapper.unmount();
  });

  it("单击目录行 → 进入该目录;「..」行 → 父目录", async () => {
    const wrapper = await openModal();
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "browse_dir") {
        return dirPayload({
          path: `${HOME}/alpha`,
          parent: HOME,
          entries: [],
        });
      }
      return null;
    });

    const rows = findInBody(wrapper, ".dir-browser__row");
    const alphaRow = rows.find((r) => r.textContent?.includes("alpha"))!;
    (alphaRow as HTMLButtonElement).click();
    await flushPromises();

    expect(invokeMock.mock.calls[0]).toEqual([
      "browse_dir",
      { path: `${HOME}/alpha`, showHidden: false },
    ]);

    // 回上级:点「..」行
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "browse_dir") return dirPayload();
      return null;
    });
    (findInBody(wrapper, ".dir-browser__row--up")[0] as HTMLButtonElement).click();
    await flushPromises();
    expect(invokeMock.mock.calls[0][1]).toEqual({
      path: HOME,
      showHidden: false,
    });

    wrapper.unmount();
  });

  it("路径输入 + 前往 → browse_dir(输入值)", async () => {
    const wrapper = await openModal();
    invokeMock.mockClear();

    const input = findInBody(wrapper, ".dir-browser__path")[0] as HTMLInputElement;
    input.value = "/opt/projects";
    input.dispatchEvent(new Event("input"));
    await flushPromises();
    (findInBody(wrapper, ".dir-browser__go")[0] as HTMLButtonElement).click();
    await flushPromises();

    expect(invokeMock.mock.calls[0]).toEqual([
      "browse_dir",
      { path: "/opt/projects", showHidden: false },
    ]);

    wrapper.unmount();
  });

  it("「显示隐藏目录」toggle → 以 showHidden=true 重取当前目录,再点切回", async () => {
    const wrapper = await openModal();
    invokeMock.mockClear();

    (findInBody(wrapper, ".dir-browser__hidden")[0] as HTMLButtonElement).click();
    await flushPromises();
    expect(invokeMock.mock.calls[0][1]).toEqual({
      path: HOME,
      showHidden: true,
    });

    (findInBody(wrapper, ".dir-browser__hidden")[0] as HTMLButtonElement).click();
    await flushPromises();
    expect(invokeMock.mock.calls[1][1]).toEqual({
      path: HOME,
      showHidden: false,
    });

    wrapper.unmount();
  });

  it("「选择此目录」→ addProjectByPath 全链: create_project + 关 dirBrowserOpen", async () => {
    const store = useProjectsStore();
    const created = {
      id: "proj-new",
      name: "alpha",
      path: `${HOME}/alpha`,
      is_git_repo: false,
      git_branch: null,
      is_legacy: false,
      created_at: "",
      updated_at: "",
      hidden: false,
      metadata: null,
    };
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "get_home_dir") return HOME;
      if (cmd === "browse_dir") {
        return dirPayload({
          path: `${HOME}/alpha`,
          parent: HOME,
          entries: [],
        });
      }
      if (cmd === "list_projects") return [];
      if (cmd === "list_hidden_projects") return [];
      if (cmd === "create_project") return created;
      return null;
    });
    store.dirBrowserOpen = true;
    const wrapper = await openModal();

    (findInBody(wrapper, ".dir-browser__choose")[0] as HTMLButtonElement).click();
    await flushPromises();

    const cmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(cmds).toContain("create_project");
    expect(store.currentProjectId).toBe("proj-new");
    expect(store.dirBrowserOpen).toBe(false);

    wrapper.unmount();
  });

  it("browse_dir 失败 → 行内错误提示,「选择此目录」/「上一步」禁用", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "get_home_dir") return HOME;
      if (cmd === "browse_dir") throw new Error("路径不存在或不可访问");
      return null;
    });
    const wrapper = await openModal();

    const err = findInBody(wrapper, ".dir-browser__error");
    expect(err.length).toBe(1);
    expect(err[0].textContent).toContain("路径不存在或不可访问");

    const choose = findInBody(wrapper, ".dir-browser__choose")[0] as HTMLButtonElement;
    const back = findInBody(wrapper, ".dir-browser__back")[0] as HTMLButtonElement;
    expect(choose.disabled).toBe(true);
    expect(back.disabled).toBe(true);

    wrapper.unmount();
  });
});
