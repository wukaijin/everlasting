// Tests for `ProjectSandboxTab.vue` — Settings「项目」scope →
// 项目沙盒三态选择(P3c, task 09-01-a2-p3c-sandbox-ux)。
//
// 契约:
//   1. 有项目时渲染 3 个 radio(放行 / 读写默认 / 只读),当前档位
//      来自 `projectById(id).sandbox_policy`(缺字段 → readwrite)。
//   2. 点选其它档 → `update_project_sandbox_policy` 携带 id + policy,
//      成功后本地选中跟随。
//   3. 写入失败 → toast,选中保持原状。
//   4. projectId 为 null → 空态文案,无 radiogroup。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";

const showToastMock = vi.fn();
const setPolicyMock = vi.fn();
const projectByIdMock = vi.fn();

vi.mock("../../stores/projects", () => ({
  useProjectsStore: () => ({
    showToast: showToastMock,
    setProjectSandboxPolicy: setPolicyMock,
    projectById: projectByIdMock,
  }),
}));

import ProjectSandboxTab from "./ProjectSandboxTab.vue";

function mountTab(projectId: string | null) {
  return mount(ProjectSandboxTab, { props: { projectId } });
}

beforeEach(() => {
  vi.clearAllMocks();
  projectByIdMock.mockReturnValue({
    id: "p1",
    sandbox_policy: "readwrite",
  });
  setPolicyMock.mockResolvedValue(undefined);
});

describe("ProjectSandboxTab", () => {
  it("渲染 radiogroup + 三个档位;当前档 = store 值", async () => {
    const w = mountTab("p1");
    await flushPromises();
    const group = w.find("[role='radiogroup']");
    expect(group.exists()).toBe(true);
    const radios = w.findAll("input[type='radio']");
    expect(radios).toHaveLength(3);
    expect((radios[0]!.element as HTMLInputElement).value).toBe("off");
    expect((radios[1]!.element as HTMLInputElement).value).toBe("readwrite");
    expect((radios[2]!.element as HTMLInputElement).value).toBe("readonly");
    expect((radios[1]!.element as HTMLInputElement).checked).toBe(true);
  });

  it("sandbox_policy 缺省(旧 daemon)→ 选中 readwrite", async () => {
    projectByIdMock.mockReturnValue({ id: "p1" });
    const w = mountTab("p1");
    await flushPromises();
    const checked = w.findAll("input[type='radio']")[1]!
      .element as HTMLInputElement;
    expect(checked.checked).toBe(true);
  });

  it("点选其它档 → invoke 携带 id+policy,成功后选中跟随", async () => {
    const w = mountTab("p1");
    await flushPromises();
    await w.findAll("input[type='radio']")[0]!.setValue();
    await flushPromises();
    expect(setPolicyMock).toHaveBeenCalledWith("p1", "off");
    const checked = w.findAll("input[type='radio']")[0]!
      .element as HTMLInputElement;
    expect(checked.checked).toBe(true);
  });

  it("写入失败 → toast,选中保持原状", async () => {
    setPolicyMock.mockRejectedValue(new Error("daemon unreachable"));
    const w = mountTab("p1");
    await flushPromises();
    await w.findAll("input[type='radio']")[2]!.setValue();
    await flushPromises();
    expect(showToastMock).toHaveBeenCalledTimes(1);
    expect(showToastMock.mock.calls[0]?.[0]).toContain("设置失败");
    const checked = w.findAll("input[type='radio']")[1]!
      .element as HTMLInputElement;
    expect(checked.checked).toBe(true);
  });

  it("projectId=null → 空态,无 radiogroup", () => {
    const w = mountTab(null);
    expect(w.find("[role='radiogroup']").exists()).toBe(false);
    expect(w.text()).toContain("没有可选项目");
  });
});
