// Tests for the NET section of `ProjectSandboxTab.vue` —
// 09-21-sandbox-net-bindonly (R8 文案三硬要求 + R4 快照确认流)。
//
// 契约:
//   1. 网络档 radiogroup 三项:block(默认选中)/ bind_only /
//      allow_all(disabled 挂账)。
//   2. 文案三硬要求可断言:① 放行端口=数据可外发面(保密性不适用)
//      ② UDP/DNS 不受控 ③ 平台不支持 → 「本平台不生效」+ 禁写。
//   3. 快照确认流:端口输入 + 确认 → confirmNetSnapshot;建议列表
//      确认/拒绝动作;确认成功后档位显示 bind_only。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";

const showToastMock = vi.fn();
const setPolicyMock = vi.fn();
const projectByIdMock = vi.fn();
const getNetStateMock = vi.fn();
const setNetMock = vi.fn();
const confirmNetMock = vi.fn();
const rejectNetMock = vi.fn();
const loadProjectsMock = vi.fn();

vi.mock("../../stores/projects", () => ({
  useProjectsStore: () => ({
    showToast: showToastMock,
    setProjectSandboxPolicy: setPolicyMock,
    setProjectSandboxNet: setNetMock,
    getProjectNetState: getNetStateMock,
    confirmNetSnapshot: confirmNetMock,
    rejectNetProposal: rejectNetMock,
    loadProjects: loadProjectsMock,
    projectById: projectByIdMock,
  }),
}));

import ProjectSandboxTab from "./ProjectSandboxTab.vue";

function netState(overrides: Record<string, unknown> = {}) {
  return {
    tier: null,
    snapshots: [],
    proposals: [],
    bind_only_supported: true,
    ...overrides,
  };
}

function mountTab(projectId: string | null) {
  return mount(ProjectSandboxTab, { props: { projectId } });
}

beforeEach(() => {
  vi.clearAllMocks();
  projectByIdMock.mockReturnValue({
    id: "p1",
    sandbox_policy: "readwrite",
    path: "/proj/root",
  });
  getNetStateMock.mockResolvedValue(netState());
});

describe("ProjectSandboxTab net section", () => {
  it("网络档三项:block 默认选中,allow_all 恒禁用(挂账)", async () => {
    const w = mountTab("p1");
    await flushPromises();
    const radios = w.findAll("input[name='project-sandbox-net']");
    expect(radios).toHaveLength(3);
    expect((radios[0]!.element as HTMLInputElement).value).toBe("block");
    expect((radios[0]!.element as HTMLInputElement).checked).toBe(true);
    const allowAll = radios[2]!.element as HTMLInputElement;
    expect(allowAll.value).toBe("allow_all");
    expect(allowAll.disabled).toBe(true);
    expect(w.text()).toContain("挂账");
  });

  it("文案硬要求①②:保密性不适用 + UDP/DNS 不受控", async () => {
    const w = mountTab("p1");
    await flushPromises();
    const desc = w.find("[data-testid='net-bindonly-desc']").text();
    expect(desc).toContain("保密性保护不适用");
    expect(desc).toContain("UDP/DNS");
    expect(desc).toContain("不受控");
  });

  it("文案硬要求③:平台不支持 → 「本平台不生效」横幅 + bind_only 禁写", async () => {
    getNetStateMock.mockResolvedValue(
      netState({ bind_only_supported: false }),
    );
    const w = mountTab("p1");
    await flushPromises();
    expect(w.find("[data-testid='net-unsupported']").text()).toContain(
      "本平台不生效",
    );
    const bind = w
      .findAll("input[name='project-sandbox-net']")[1]!
      .element as HTMLInputElement;
    expect(bind.disabled).toBe(true);
    // 快照确认按钮同样禁写。
    expect(
      (w.find("[data-testid='net-confirm']").element as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });

  it("快照确认流:输入端口确认 → confirmNetSnapshot + 档位变 bind_only", async () => {
    getNetStateMock.mockResolvedValueOnce(netState());
    confirmNetMock.mockResolvedValue(
      netState({ tier: "bind_only:3000,3001", snapshots: [{ worktree_key: "/proj/root", ports: "3000,3001", confirmed_by: "operator", confirmed_at: 1 }] }),
    );
    const w = mountTab("p1");
    await flushPromises();
    await w.find("[data-testid='net-ports-input']").setValue("3000,3001");
    await w.find("[data-testid='net-confirm']").trigger("click");
    await flushPromises();
    expect(confirmNetMock).toHaveBeenCalledWith(
      "p1",
      "/proj/root",
      [3000, 3001],
    );
    const bind = w
      .findAll("input[name='project-sandbox-net']")[1]!
      .element as HTMLInputElement;
    expect(bind.checked).toBe(true);
    expect(w.find("[data-testid='net-snapshot-list']").text()).toContain(
      "3000,3001",
    );
  });

  it("端口建议:确认/拒绝动作触发对应通道(两轮独立挂载)", async () => {
    const withProposal = () =>
      netState({
        proposals: [
          {
            project_id: "p1",
            worktree_key: "/proj/root",
            ports: "5173",
            source: "llm",
            status: "pending",
            proposed_at: 1,
          },
        ],
      });
    confirmNetMock.mockResolvedValue(netState({ tier: "bind_only:5173" }));
    rejectNetMock.mockResolvedValue(netState());

    // 接受:确认动作走 confirmNetSnapshot(建议端口 + 建议 worktree)。
    getNetStateMock.mockResolvedValue(withProposal());
    let w = mountTab("p1");
    await flushPromises();
    expect(w.find("[data-testid='net-proposal-list']").text()).toContain(
      "5173",
    );
    await w.find("[data-testid='net-accept-proposal']").trigger("click");
    await flushPromises();
    expect(confirmNetMock).toHaveBeenCalledWith("p1", "/proj/root", [5173]);

    // 拒绝:独立一轮(接受后建议行已从列表消失,引用会分离)。
    getNetStateMock.mockResolvedValue(withProposal());
    w = mountTab("p1");
    await flushPromises();
    await w.find("[data-testid='net-reject-proposal']").trigger("click");
    await flushPromises();
    expect(rejectNetMock).toHaveBeenCalledWith("p1", "/proj/root");
  });

  it("非法端口输入 → toast,不触发确认", async () => {
    const w = mountTab("p1");
    await flushPromises();
    await w.find("[data-testid='net-ports-input']").setValue("0,99999,abc");
    await w.find("[data-testid='net-confirm']").trigger("click");
    await flushPromises();
    expect(confirmNetMock).not.toHaveBeenCalled();
    expect(showToastMock).toHaveBeenCalled();
  });

  it("切回 block → setProjectSandboxNet('block')", async () => {
    getNetStateMock.mockResolvedValue(netState({ tier: "bind_only:3000" }));
    const w = mountTab("p1");
    await flushPromises();
    await w.findAll("input[name='project-sandbox-net']")[0]!.setValue();
    await flushPromises();
    expect(setNetMock).toHaveBeenCalledWith("p1", "block");
  });
});
