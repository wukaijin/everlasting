// Tests for `MemorySlotToggles.vue` — 4 槽位记忆植入开关
// (2026-09-10 hard switch PR2,任务 09-10-memory-everlasting-md-hard-switch)。
//
// 契约:
//   1. scope="user" / "project" 各渲染 2 个 role="switch" 行,aria-checked
//      反映 config store 当前值(fail-open 缺省 true)。
//   2. 点击 → `set_app_config_flag` 携带正确的 key(`memory_*_enabled`)
//      与取反值,成功后 store ref 更新。
//   3. 写入失败 → toast 报错,store 值保持原状(GeneralTab 同款策略)。
//
// transport 全量 mock(GeneralTab.test.ts 同款);projects store 只保留
// showToast。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

const showToastMock = vi.fn();
vi.mock("../../stores/projects", () => ({
  useProjectsStore: () => ({ showToast: showToastMock }),
}));

import MemorySlotToggles from "./MemorySlotToggles.vue";
import { useConfigStore } from "../../stores/config";

async function mountToggles(scope: "user" | "project") {
  const w = mount(MemorySlotToggles, {
    props: { scope },
    global: { plugins: [createPinia()] },
  });
  await flushPromises();
  return w;
}

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(null);
  showToastMock.mockClear();
});

describe("MemorySlotToggles — user scope", () => {
  it("renders 2 switches, default on (fail-open)", async () => {
    const w = await mountToggles("user");
    const switches = w.findAll("button[role='switch']");
    expect(switches).toHaveLength(2);
    expect(switches[0]!.attributes("aria-checked")).toBe("true");
    expect(switches[1]!.attributes("aria-checked")).toBe("true");
    w.unmount();
  });

  it("click writes memory_user_everlasting_enabled=false and updates the store", async () => {
    const w = await mountToggles("user");
    await w.findAll("button[role='switch']")[0]!.trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("set_app_config_flag", {
      key: "memory_user_everlasting_enabled",
      value: false,
    });
    const config = useConfigStore();
    expect(config.memoryUserEverlastingEnabled).toBe(false);
    w.unmount();
  });
});

describe("MemorySlotToggles — project scope", () => {
  it("click writes memory_project_agents_enabled=false and updates the store", async () => {
    const w = await mountToggles("project");
    await w.findAll("button[role='switch']")[1]!.trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("set_app_config_flag", {
      key: "memory_project_agents_enabled",
      value: false,
    });
    const config = useConfigStore();
    expect(config.memoryProjectAgentsEnabled).toBe(false);
    w.unmount();
  });
});

describe("MemorySlotToggles — failure keeps the previous value", () => {
  it("toast on write error, store value unchanged", async () => {
    invokeMock.mockRejectedValue(new Error("daemon offline"));
    const w = await mountToggles("user");
    await w.findAll("button[role='switch']")[0]!.trigger("click");
    await flushPromises();

    expect(showToastMock).toHaveBeenCalledTimes(1);
    const config = useConfigStore();
    expect(config.memoryUserEverlastingEnabled).toBe(true);
    w.unmount();
  });
});
