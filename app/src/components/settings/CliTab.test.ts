// CliTab — Settings「CLI (evl)」分类组件测试。
//
// 契约:
//   1. 进入 tab 自动 detect_evl;两行状态(Node / evl)按 wire 形态渲染。
//   2. 按钮条件:notInstalled + node.ok → 「安装 evl CLI」;managed 且
//      version ≠ bundled → 「更新到内置版本 X」;external / 已最新 →
//      不出安装按钮(外部不覆盖语义)。
//   3. 安装流:install_evl → 返回 payload 直接替换本地状态 → toast;
//      失败 → toast 带 extractErrorMessage 文案。
//   4. managed 且 !onPath → PATH 提示行(role=alert)在场;external →
//      「不做覆盖」说明在场。
//
// transport / projects store mock(GroupChatPresetsTab.test.ts 同款)。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";

const invokeMock = vi.fn();

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

const showToastMock = vi.fn();
vi.mock("../../stores/projects", () => ({
  useProjectsStore: () => ({
    showToast: showToastMock,
  }),
}));

import CliTab from "./CliTab.vue";

/** detect_evl / install_evl 的 wire payload 工厂。 */
function payload(overrides: {
  node?: Partial<Payload["node"]>;
  evl?: Partial<Payload["evl"]>;
  bundledVersion?: string;
  localBinOnPath?: boolean;
} = {}): Payload {
  return {
    bundledVersion: overrides.bundledVersion ?? "0.1.0",
    node: { found: true, version: "v20.11.1", ok: true, reason: null, ...overrides.node },
    evl: {
      state: "notInstalled",
      path: null,
      version: null,
      onPath: false,
      ...overrides.evl,
    },
    localBinDir: "/home/u/.local/bin",
    localBinOnPath: overrides.localBinOnPath ?? true,
  };
}

interface Payload {
  bundledVersion: string;
  node: { found: boolean; version: string | null; ok: boolean; reason: string | null };
  evl: {
    state: "notInstalled" | "managed" | "external";
    path: string | null;
    version: string | null;
    onPath: boolean;
  };
  localBinDir: string;
  localBinOnPath: boolean;
}

async function mountTab(detectResult: Payload) {
  invokeMock.mockReset();
  invokeMock.mockResolvedValueOnce(detectResult);
  const wrapper = mount(CliTab);
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  showToastMock.mockReset();
});

describe("CliTab", () => {
  it("进入 tab 自动 detect_evl 并渲染两行状态", async () => {
    const wrapper = await mountTab(
      payload({
        evl: { state: "managed", path: "/home/u/.local/bin/evl", version: "0.1.0", onPath: true },
      }),
    );
    expect(invokeMock).toHaveBeenCalledWith("detect_evl");
    const text = wrapper.text();
    expect(text).toContain("Node");
    expect(text).toContain("v20.11.1");
    expect(text).toContain("evl CLI");
    expect(text).toContain("已安装 · 应用内置");
    expect(text).toContain("0.1.0");
    expect(text).toContain("/home/u/.local/bin/evl");
  });

  it("未安装 + Node 满足 → 「安装 evl CLI」按钮,点击走 install_evl 并 toast", async () => {
    const installed = payload({
      evl: { state: "managed", path: "/home/u/.local/bin/evl", version: "0.1.0", onPath: true },
    });
    invokeMock.mockReset();
    invokeMock
      .mockResolvedValueOnce(payload()) // detect
      .mockResolvedValueOnce(installed); // install
    const wrapper = mount(CliTab);
    await flushPromises();

    const btn = wrapper.get(".cli-tab__install");
    expect(btn.text()).toBe("安装 evl CLI");
    await btn.trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenNthCalledWith(2, "install_evl");
    expect(showToastMock).toHaveBeenCalledWith("evl CLI 安装完成", "info");
    // 安装返回的 payload 直接替换本地状态 → chip 翻为已安装。
    expect(wrapper.text()).toContain("已安装 · 应用内置");
  });

  it("Node 不满足 → 不出安装按钮 + Node 提示行", async () => {
    const wrapper = await mountTab(
      payload({
        node: { found: false, version: null, ok: false, reason: "宿主机未安装 Node(evl 需要 Node ≥ 20)" },
      }),
    );
    expect(wrapper.find(".cli-tab__install").exists()).toBe(false);
    expect(wrapper.get('[role="alert"]').text()).toContain("Node ≥ 20");
  });

  it("托管安装且版本落后 → 「更新到内置版本」按钮", async () => {
    const wrapper = await mountTab(
      payload({
        bundledVersion: "0.2.0",
        evl: { state: "managed", path: "/home/u/.local/bin/evl", version: "0.1.0", onPath: true },
      }),
    );
    const btn = wrapper.get(".cli-tab__install");
    expect(btn.text()).toContain("更新到内置版本 0.2.0");
  });

  it("托管安装且已是最新 → 无按钮,显示「已是最新」与移除指引", async () => {
    const wrapper = await mountTab(
      payload({
        evl: { state: "managed", path: "/home/u/.local/bin/evl", version: "0.1.0", onPath: true },
      }),
    );
    expect(wrapper.find(".cli-tab__install").exists()).toBe(false);
    const text = wrapper.text();
    expect(text).toContain("已是最新");
    expect(text).toContain("rm /home/u/.local/bin/evl");
  });

  it("托管安装但不在 PATH → role=alert 的 PATH 提示行", async () => {
    const wrapper = await mountTab(
      payload({
        localBinOnPath: false,
        evl: { state: "managed", path: "/home/u/.local/bin/evl", version: "0.1.0", onPath: false },
      }),
    );
    const alerts = wrapper.findAll('[role="alert"]');
    expect(alerts.some((a) => a.text().includes("PATH"))).toBe(true);
    expect(wrapper.text()).toContain("/home/u/.local/bin");
  });

  it("外部安装 → 不出安装按钮 + 「不做覆盖」说明", async () => {
    const wrapper = await mountTab(
      payload({
        evl: { state: "external", path: "/usr/local/bin/evl", version: "0.1.0", onPath: true },
      }),
    );
    expect(wrapper.find(".cli-tab__install").exists()).toBe(false);
    expect(wrapper.text()).toContain("不做覆盖");
    expect(wrapper.text()).toContain("/usr/local/bin/evl");
  });

  it("install 失败 → toast 错误,状态不翻", async () => {
    invokeMock.mockReset();
    invokeMock
      .mockResolvedValueOnce(payload())
      .mockRejectedValueOnce(new Error("宿主机 Node 不满足要求:版本过低"));
    const wrapper = mount(CliTab);
    await flushPromises();

    await wrapper.get(".cli-tab__install").trigger("click");
    await flushPromises();

    expect(showToastMock).toHaveBeenCalledWith(
      "安装失败:宿主机 Node 不满足要求:版本过低",
      "error",
    );
    expect(wrapper.text()).toContain("未安装");
  });

  it("detect 失败 → toast,不崩", async () => {
    invokeMock.mockReset();
    invokeMock.mockRejectedValueOnce(new Error("daemon 不可达"));
    const wrapper = mount(CliTab);
    await flushPromises();
    expect(showToastMock).toHaveBeenCalledWith("检测失败:daemon 不可达", "error");
    expect(wrapper.text()).toContain("暂无数据");
  });
});
