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
  it("渲染四个开关:前三缺省 aria-checked=true(fail-open),askNoTimeout 缺省 false(fail-closed)", async () => {
    const w = await mountTab();
    const switches = w.findAll("button[role='switch']");
    expect(switches).toHaveLength(4);
    expect(switches[0]?.attributes("aria-checked")).toBe("true");
    expect(switches[1]?.attributes("aria-checked")).toBe("true");
    expect(switches[2]?.attributes("aria-checked")).toBe("true");
    expect(switches[3]?.attributes("aria-checked")).toBe("false");
    expect(switches[0]?.attributes("aria-label")).toBe("轮次完成通知");
    expect(switches[1]?.attributes("aria-label")).toBe("定时任务调度");
    expect(switches[2]?.attributes("aria-label")).toBe("命令沙盒");
    expect(switches[3]?.attributes("aria-label")).toBe("等待确认不超时");
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

  it("askNoTimeout 点击:缺省 false → 写 true,key 为 ask_no_timeout,store + aria 跟随", async () => {
    const w = await mountTab();
    const pinia = useConfigStore();

    await switchAt(w, 3).trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenLastCalledWith("set_app_config_flag", {
      key: "ask_no_timeout",
      value: true,
    });
    expect(pinia.askNoTimeout).toBe(true);
    expect(switchAt(w, 3).attributes("aria-checked")).toBe("true");
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

// ---------------------------------------------------------------------------
// P3b(2026-08-31, task 08-31-a2-p3b-sandbox-executor):沙盒开关 +
// 能力徽标 + 额外可写目录列表编辑(set_app_config_list 写通道)。
// ---------------------------------------------------------------------------

describe("GeneralTab — P3b sandbox", () => {
  it("渲染第三个开关(命令沙盒),缺省开;能力徽标 null 时不显示", async () => {
    const w = await mountTab();
    const switches = w.findAll("button[role='switch']");
    expect(switches).toHaveLength(4);
    expect(switches[2]?.attributes("aria-label")).toBe("命令沙盒");
    expect(switches[2]?.attributes("aria-checked")).toBe("true");
    expect(w.find(".general-tab__cap").exists()).toBe(false);
  });

  it("沙盒开关点击 → set_app_config_flag(sandbox_enabled) + store 更新", async () => {
    const w = await mountTab();
    const pinia = useConfigStore();

    await switchAt(w, 2).trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenLastCalledWith("set_app_config_flag", {
      key: "sandbox_enabled",
      value: false,
    });
    expect(pinia.sandboxEnabled).toBe(false);
    expect(switchAt(w, 2).attributes("aria-checked")).toBe("false");
  });

  it("capability=true → 「沙盒生效」徽标;false → 「已回退」徽标", async () => {
    // 在 mount 之后再取 store(与组件同一 pinia 实例)并翻转探测值,
    // 徽标随响应式更新。
    const w = await mountTab();
    const pinia = useConfigStore();

    pinia.sandboxCapability = true;
    await flushPromises();
    expect(w.find(".general-tab__cap").text()).toContain("沙盒生效");

    pinia.sandboxCapability = false;
    await flushPromises();
    expect(w.find(".general-tab__cap").text()).toContain("已回退");
  });

  it("store.load() 拉取 get_app_config 的 P3b 三字段(additive,旧 daemon 缺省不炸)", async () => {
    const pinia = useConfigStore();
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_app_config") {
        return Promise.resolve({
          turnCompleteNotifyEnabled: true,
          scheduledTasksEnabled: true,
          sandboxEnabled: false,
          sandboxExtraWritable: ["/root/.cargo", "/opt/cache"],
          sandboxExtraWritableRaw: ["/opt/cache"],
          sandboxCapability: false,
        });
      }
      return Promise.resolve(null);
    });
    await pinia.load();
    await flushPromises();
    expect(pinia.sandboxEnabled).toBe(false);
    expect(pinia.sandboxExtraWritable).toEqual(["/root/.cargo", "/opt/cache"]);
    expect(pinia.sandboxExtraWritableRaw).toEqual(["/opt/cache"]);
    expect(pinia.sandboxCapability).toBe(false);
    // askNoTimeout 未回字段 → 缺省 false(additive + fail-closed)。
    expect(pinia.askNoTimeout).toBe(false);
  });

  it("额外可写目录(RULE-SBX-002):编辑 raw 清单;~/.cargo 默认项为固定 chip 不可删", async () => {
    const w = await mountTab();
    const pinia = useConfigStore();

    // 默认 chip 恒在(无移除按钮)。
    const items = w.findAll(".general-tab__extra-item");
    expect(items).toHaveLength(1);
    expect(items[0]!.text()).toContain("~/.cargo");
    expect(items[0]!.find(".general-tab__extra-remove").exists()).toBe(false);

    const input = w.find(".general-tab__extra-input");
    await input.setValue("/opt/build-cache");
    await w.find(".general-tab__extra-addbtn").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenLastCalledWith("set_app_config_list", {
      key: "sandbox_extra_writable",
      value: ["/opt/build-cache"],
    });
    // raw ref 更新;生效清单(展示层)不动。
    expect(pinia.sandboxExtraWritableRaw).toEqual(["/opt/build-cache"]);
    expect(pinia.sandboxExtraWritable).toEqual([]);

    await w.findAll(".general-tab__extra-remove")[0]!.trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenLastCalledWith("set_app_config_list", {
      key: "sandbox_extra_writable",
      value: [],
    });
    expect(pinia.sandboxExtraWritableRaw).toEqual([]);
    // 回归锚(移除后复活):raw 为空后,列表只剩默认 chip。
    expect(w.findAll(".general-tab__extra-item")).toHaveLength(1);
    expect(w.findAll(".general-tab__extra-item")[0]!.text()).toContain("~/.cargo");
  });

  it("列表写入失败 → toast,raw 值保持原状(不乐观提交)", async () => {
    const w = await mountTab();
    const pinia = useConfigStore();

    const input = w.find(".general-tab__extra-input");
    await input.setValue("/failing");
    invokeMock.mockRejectedValueOnce(new Error("daemon unreachable"));
    await w.find(".general-tab__extra-addbtn").trigger("click");
    await flushPromises();
    expect(showToastMock).toHaveBeenCalledTimes(1);
    expect(showToastMock.mock.calls[0]?.[0]).toContain("设置失败");
    expect(pinia.sandboxExtraWritableRaw).toEqual([]);
    expect(w.findAll(".general-tab__extra-item")).toHaveLength(1);
  });
});
