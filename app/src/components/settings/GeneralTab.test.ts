// Tests for `GeneralTab.vue` — Settings「通用」分类的两个开关
// (settings-shell 重构,2026-08-29)。
//
// 契约:
//   1. 渲染两个 role="switch" 行:轮次完成通知 + 定时任务调度,
//      aria-checked 反映 config store 当前值。
//   2. 点击开关 → `set_app_config_flag` 携带正确的 key 与取反值,
//      成功后 store ref 更新(aria-checked 跟随)。
//   3. 写入失败 → toast 报错,store 值保持原状(不乐观提交)。
//   4. in-flight 期间开关禁点(防双击)。
//
// transport 全量 mock(SearchTab.test.ts 同款);projects store 只
// 保留 showToast。

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

import GeneralTab from "./GeneralTab.vue";
import { useConfigStore } from "../../stores/config";

async function mountTab() {
  const w = mount(GeneralTab, { global: { plugins: [createPinia()] } });
  await flushPromises();
  return w;
}

function switchAt(w: ReturnType<typeof mount>, idx: number) {
  return w.findAll("button[role='switch']")[idx]!;
}

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(null);
  showToastMock.mockClear();
});

describe("GeneralTab", () => {
  it("渲染两个开关,缺省 aria-checked=true(store fail-open 缺省开)", async () => {
    const w = await mountTab();
    const switches = w.findAll("button[role='switch']");
    expect(switches).toHaveLength(2);
    expect(switches[0]?.attributes("aria-checked")).toBe("true");
    expect(switches[1]?.attributes("aria-checked")).toBe("true");
    expect(switches[0]?.attributes("aria-label")).toBe("轮次完成通知");
    expect(switches[1]?.attributes("aria-label")).toBe("定时任务调度");
  });

  it("点击携带正确的 key + 取反值,成功后 store 更新", async () => {
    const w = await mountTab();
    const pinia = useConfigStore();

    await switchAt(w, 0).trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("set_app_config_flag", {
      key: "turn_complete_notify_enabled",
      value: false,
    });
    expect(pinia.turnCompleteNotify).toBe(false);
    expect(switchAt(w, 0).attributes("aria-checked")).toBe("false");

    await switchAt(w, 1).trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenLastCalledWith("set_app_config_flag", {
      key: "scheduled_tasks_enabled",
      value: false,
    });
    expect(pinia.scheduledTasksEnabled).toBe(false);
  });

  it("写入失败 → toast 报错,store 值保持原状", async () => {
    invokeMock.mockRejectedValueOnce(new Error("daemon unreachable"));
    const w = await mountTab();
    const pinia = useConfigStore();

    await switchAt(w, 0).trigger("click");
    await flushPromises();
    expect(showToastMock).toHaveBeenCalledTimes(1);
    expect(showToastMock.mock.calls[0]?.[0]).toContain("设置失败");
    expect(pinia.turnCompleteNotify).toBe(true);
    expect(switchAt(w, 0).attributes("aria-checked")).toBe("true");
  });

  it("in-flight 期间开关 disabled,重复点击是 no-op", async () => {
    let resolveWrite!: (v: unknown) => void;
    invokeMock.mockImplementation(
      () => new Promise((resolve) => {
        resolveWrite = resolve;
      }),
    );
    const w = await mountTab();

    await switchAt(w, 0).trigger("click");
    expect(switchAt(w, 0).attributes("disabled")).toBeDefined();

    // 第二次点击被 pending guard 拦下(invoke 仍只有一次在途调用)。
    await switchAt(w, 0).trigger("click");
    expect(invokeMock).toHaveBeenCalledTimes(1);

    resolveWrite(null);
    await flushPromises();
    expect(switchAt(w, 0).attributes("disabled")).toBeUndefined();
  });
});
