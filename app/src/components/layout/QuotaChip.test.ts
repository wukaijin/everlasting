// 08-20-turn-usage-event-quota-view WP3 — QuotaChip 测试。
// 覆盖:chip 常驻态(总量缩写 + 额度条两态,AC5)/ 弹层打开拉取并渲染
// provider 段与 top session 行(AC6 入口)/ 设置行保存校验。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { mount, VueWrapper } from "@vue/test-utils";

const invokeMock = vi.fn();

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import QuotaChip from "./QuotaChip.vue";
import { useQuotaStore, type UsageWindowReportWire } from "../../stores/quota";

function makeReport(overrides?: Partial<UsageWindowReportWire>): UsageWindowReportWire {
  return {
    windowHours: 5,
    limitTokens: null,
    providers: [
      {
        providerId: "prov-a",
        displayName: "Provider A",
        totals: {
          inputTokens: 1_200_000,
          outputTokens: 80_000,
          cacheReadInputTokens: 0,
          cacheCreationInputTokens: 0,
        },
        mainTotals: {
          inputTokens: 900_000,
          outputTokens: 60_000,
          cacheReadInputTokens: 0,
          cacheCreationInputTokens: 0,
        },
        workerTotals: {
          inputTokens: 300_000,
          outputTokens: 20_000,
          cacheReadInputTokens: 0,
          cacheCreationInputTokens: 0,
        },
        hourly: [],
      },
    ],
    topSessions: [
      {
        sessionId: "s1",
        title: "session one",
        projectId: "p1",
        windowMainInput: 700_000,
        windowWorkerInput: 300_000,
        lifetimeInput: 900_000,
        lifetimeOutput: 50_000,
      },
    ],
    ...overrides,
  };
}

async function mountChip(report: UsageWindowReportWire | null): Promise<VueWrapper> {
  setActivePinia(createPinia());
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(report);
  const wrapper = mount(QuotaChip, { attachTo: document.body });
  await vi.waitFor(() => {
    expect(useQuotaStore().report).not.toBeNull();
  });
  await wrapper.vm.$nextTick();
  return wrapper;
}

describe("QuotaChip", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("renders the window label + abbreviated total; no bar without a limit", async () => {
    const wrapper = await mountChip(makeReport());
    const chip = wrapper.find(".quota-chip");
    expect(chip.text()).toContain("5h");
    expect(chip.text()).toContain("1.3M"); // 1.28M in + out 缩写
    expect(chip.find(".quota-chip__bar").exists()).toBe(false);
    wrapper.unmount();
  });

  it("renders the progress bar + danger level at ≥75% when a limit is set", async () => {
    const wrapper = await mountChip(makeReport({ limitTokens: 1_600_000 }));
    const chip = wrapper.find(".quota-chip");
    expect(chip.classes()).toContain("quota-chip--danger"); // 80% ≥ 75
    expect(chip.find(".quota-chip__bar-fill").attributes("style")).toContain("80%");
    wrapper.unmount();
  });

  it("opens the popover, renders provider split + top session row", async () => {
    const wrapper = await mountChip(makeReport());
    await wrapper.find(".quota-chip").trigger("click");
    await wrapper.vm.$nextTick();
    const pop = wrapper.find(".quota-pop");
    expect(pop.exists()).toBe(true);
    expect(pop.text()).toContain("Provider A");
    expect(pop.text()).toContain("主 900K"); // abbreviateTokens
    expect(pop.text()).toContain("worker 300K");
    const row = pop.find(".quota-pop__session-row");
    expect(row.text()).toContain("session one");
    expect(row.text()).toContain("累计 900K");
    wrapper.unmount();
  });

  it("settings row rejects a non-positive limit locally (no IPC)", async () => {
    const wrapper = await mountChip(makeReport());
    await wrapper.find(".quota-chip").trigger("click");
    await wrapper.vm.$nextTick();
    const inputs = wrapper.findAll(".quota-pop__input");
    await inputs[1].setValue("0");
    await wrapper.find(".quota-pop__save").trigger("click");
    await wrapper.vm.$nextTick();
    expect(wrapper.find(".quota-pop__error").text()).toContain("额度");
    // set_quota_settings 未被调用(本地校验拦截);两次 invoke 都是
    // mount / 弹层打开的 usage_window refresh。
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(invokeMock.mock.calls.every(([cmd]) => cmd === "usage_window")).toBe(true);
    wrapper.unmount();
  });
});
