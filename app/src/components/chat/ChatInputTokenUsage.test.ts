// ChatInputTokenUsage 测试 — 2026-08-21 迁移(原 QuotaChip.test.ts 并入)。
// 覆盖:chip 常驻态(上下文 % + 色档)/ 弹层打开渲染上下文进度条 +
// 上轮明细 + 缓存命中率 / 窗口聚合(总量 + 平均命中率 + provider 拆分 +
// top session)/ 额度占比条两态 / 设置行本地校验 / 空态(升级前未统计)。

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

import ChatInputTokenUsage from "./ChatInputTokenUsage.vue";
import { useQuotaStore, type UsageWindowReportWire } from "../../stores/quota";
import { useTraceStore } from "../../stores/traceStore";
import type { SessionTokenUsage } from "../../stores/chat.types";

/** 上轮快照:context_input = input + cc + cr = 122_300(归一化口径)。 */
const USAGE: SessionTokenUsage = {
  input_tokens: 10_000,
  output_tokens: 8_400,
  cache_creation_input_tokens: 14_100,
  cache_read_input_tokens: 98_200,
  context_input_tokens: 122_300,
};
// 122_300 / 200_000 → 61%(warn 档);命中率 = 98_200 / 122_300 → 80%。
const CONTEXT_WINDOW = 200_000;

function makeReport(overrides?: Partial<UsageWindowReportWire>): UsageWindowReportWire {
  return {
    windowHours: 5,
    limitTokens: null,
    providers: [
      {
        providerId: "prov-a",
        displayName: "Provider A",
        // 命中率 = 700K / (200K + 700K + 100K) = 70%。
        totals: {
          inputTokens: 200_000,
          outputTokens: 80_000,
          cacheReadInputTokens: 700_000,
          cacheCreationInputTokens: 100_000,
        },
        mainTotals: {
          inputTokens: 150_000,
          outputTokens: 60_000,
          cacheReadInputTokens: 0,
          cacheCreationInputTokens: 0,
        },
        workerTotals: {
          inputTokens: 50_000,
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
        windowMainInput: 150_000,
        windowWorkerInput: 50_000,
        lifetimeInput: 900_000,
        lifetimeOutput: 50_000,
      },
    ],
    ...overrides,
  };
}

async function mountPop(
  report: UsageWindowReportWire | null,
  usage: SessionTokenUsage | null = USAGE,
  seed?: (ts: ReturnType<typeof useTraceStore>) => void,
): Promise<VueWrapper> {
  setActivePinia(createPinia());
  if (seed) seed(useTraceStore());
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(report);
  const wrapper = mount(ChatInputTokenUsage, {
    attachTo: document.body,
    props: {
      tokenUsage: usage,
      contextWindow: CONTEXT_WINDOW,
      usageLevel: usage ? "warn" : null,
    },
  });
  await vi.waitFor(() => {
    expect(useQuotaStore().report).not.toBeNull();
  });
  await wrapper.vm.$nextTick();
  return wrapper;
}

async function openPopover(wrapper: VueWrapper): Promise<void> {
  await wrapper.find(".chat-input__token-usage").trigger("click");
  await wrapper.vm.$nextTick();
}

describe("ChatInputTokenUsage", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("chip renders the context footprint + pct; level class applied", async () => {
    const wrapper = await mountPop(makeReport());
    const chip = wrapper.find(".chat-input__token-usage");
    expect(chip.text()).toContain("122.3K");
    expect(chip.text()).toContain("61% / 200K");
    expect(chip.classes()).toContain("chat-input__token-usage--warn");
    expect(chip.attributes("aria-expanded")).toBe("false");
    wrapper.unmount();
  });

  it("popover shows the context progress bar + remaining + last-turn detail + hit rate", async () => {
    const wrapper = await mountPop(makeReport());
    await openPopover(wrapper);
    const pop = wrapper.find(".chat-input__token-popover");
    expect(pop.exists()).toBe(true);

    // 上下文占用段:61% 大号数字 + 进度条宽度 + 剩余(200K - 122.3K)。
    expect(pop.find(".chat-input__token-ctx-pct").text()).toBe("61%");
    const fill = pop.find(".chat-input__token-bar-fill");
    expect(fill.attributes("style")).toContain("61%");
    expect(fill.classes()).toContain("chat-input__token-bar-fill--warn");
    expect(pop.find(".chat-input__token-ctx-sub").text()).toContain("77.7K");

    // 上轮明细:四计数 + 命中率(98.2K / 122.3K = 80%)。
    const rows = pop.findAll(".chat-input__token-row");
    const rowsText = rows.map((r) => r.text());
    expect(rowsText.some((t) => t.includes("input") && t.includes("10K"))).toBe(true);
    expect(
      rowsText.some((t) => t.includes("cache_read") && t.includes("98.2K")),
    ).toBe(true);
    expect(
      rowsText.some((t) => t.includes("cache_creation") && t.includes("14.1K")),
    ).toBe(true);
    expect(rowsText.some((t) => t.includes("output") && t.includes("8.4K"))).toBe(true);
    expect(
      rowsText.some((t) => t.includes("缓存命中率") && t.includes("80%")),
    ).toBe(true);
    wrapper.unmount();
  });

  it("window section: totals, avg cache-hit rate, provider split, top session row", async () => {
    const wrapper = await mountPop(makeReport());
    await openPopover(wrapper);
    const pop = wrapper.find(".chat-input__token-popover");
    expect(pop.text()).toContain("5h 滚动窗口");
    // 窗口总量 = Σ in + out = 280K;平均命中率 = 700K / 1M = 70%。
    expect(pop.text()).toContain("280K");
    expect(pop.text()).toContain("70%");
    // provider 段 + 主/worker 拆分 + per-provider 命中率。
    expect(pop.text()).toContain("Provider A");
    expect(pop.text()).toContain("主 150K");
    expect(pop.text()).toContain("worker 50K");
    expect(pop.text()).toContain("命中 70%");
    // top session 行(可跳转入口)。
    const row = pop.find(".chat-input__token-session-row");
    expect(row.text()).toContain("session one");
    expect(row.text()).toContain("累计 900K");
    // 未设额度 → 无窗口占比条。
    expect(pop.findAll(".chat-input__token-bar")).toHaveLength(1);
    wrapper.unmount();
  });

  it("renders the quota limit bar + alert band at ≥75% when a limit is set", async () => {
    const wrapper = await mountPop(makeReport({ limitTokens: 350_000 }));
    await openPopover(wrapper);
    const pop = wrapper.find(".chat-input__token-popover");
    const bars = pop.findAll(".chat-input__token-bar");
    expect(bars).toHaveLength(2); // 上下文 + 窗口额度
    const quotaFill = bars[1].find(".chat-input__token-bar-fill");
    expect(quotaFill.attributes("style")).toContain("80%"); // 280K / 350K
    expect(quotaFill.classes()).toContain("chat-input__token-bar-fill--alert");
    wrapper.unmount();
  });

  it("null usage: chip renders —, popover shows the legacy empty state, no detail section", async () => {
    const wrapper = await mountPop(makeReport(), null);
    expect(wrapper.find(".chat-input__token-usage").text()).toBe("—");
    await openPopover(wrapper);
    const pop = wrapper.find(".chat-input__token-popover");
    expect(pop.text()).toContain("升级前未统计");
    // 上轮明细段整体不渲染(字段名 cache_read 只出现在该段;窗口段的
    // 命中率行是中文标签"平均缓存命中率")。
    expect(pop.text()).not.toContain("cache_read");
    // 窗口段仍可用(聚合与 session 快照无关)。
    expect(pop.text()).toContain("Provider A");
    wrapper.unmount();
  });

  it("composition: stacked bar + always-visible legend from the latest trace", async () => {
    const wrapper = await mountPop(makeReport(), USAGE, (ts) => {
      // ctx 100K;归因 tools 8K / system 12K / memory 5K / @文件 4K / 图片 0
      // → 消息残差 71K。满宽 = 100K / 200K = 50%。
      ts.currentSessionTraces.set(3, {
        id: 1,
        sessionId: "s1",
        seq: 3,
        createdAt: "",
        tokenUsage: {
          input_tokens: 1_000,
          output_tokens: 500,
          cache_creation_input_tokens: 200,
          cache_read_input_tokens: 800,
          context_input_tokens: 100_000,
        },
        toolsToken: 8_000,
        systemToken: 12_000,
        memoryToken: 5_000,
        atFilesToken: 4_000,
        imagesToken: 0,
      });
    });
    await openPopover(wrapper);
    const pop = wrapper.find(".chat-input__token-popover");
    // 堆叠条(而非单色 fill),总宽 = trace 实发 100K / 容量 200K = 50%。
    const stack = pop.find(".chat-input__token-bar-stack");
    expect(stack.exists()).toBe(true);
    expect(stack.attributes("style")).toContain("50%");
    expect(pop.findAll(".chat-input__token-bar-seg")).toHaveLength(5); // 图片 0 过滤
    // 图例常显:消息残差排首,归因段各自带 token + %。
    const texts = pop.findAll(".chat-input__token-legend-row").map((r) => r.text());
    expect(texts[0]).toContain("消息");
    expect(texts[0]).toContain("71K");
    expect(texts[0]).toContain("71%");
    expect(
      texts.some((t) => t.includes("system+技能") && t.includes("12K") && t.includes("12%")),
    ).toBe(true);
    expect(
      texts.some((t) => t.includes("tools[]") && t.includes("8K") && t.includes("8%")),
    ).toBe(true);
    expect(
      texts.some((t) => t.includes("memory") && t.includes("5K") && t.includes("5%")),
    ).toBe(true);
    expect(
      texts.some((t) => t.includes("@文件") && t.includes("4K") && t.includes("4%")),
    ).toBe(true);
    expect(pop.text()).toContain("system 含技能清单");
    // 单色退化条不应同时存在。
    expect(pop.find(".chat-input__token-bar-fill").exists()).toBe(false);
    wrapper.unmount();
  });

  it("composition: falls back to the plain level bar when slices are all missing", async () => {
    const wrapper = await mountPop(makeReport(), USAGE, (ts) => {
      // 旧行:有 usage 但切片全缺列。
      ts.currentSessionTraces.set(1, {
        id: 1,
        sessionId: "s1",
        seq: 1,
        createdAt: "",
        tokenUsage: {
          input_tokens: 1,
          output_tokens: 1,
          cache_creation_input_tokens: 1,
          cache_read_input_tokens: 1,
          context_input_tokens: 50_000,
        },
      });
    });
    await openPopover(wrapper);
    const pop = wrapper.find(".chat-input__token-popover");
    expect(pop.find(".chat-input__token-bar-stack").exists()).toBe(false);
    expect(pop.find(".chat-input__token-bar-fill").exists()).toBe(true);
    expect(pop.find(".chat-input__token-legend").exists()).toBe(false);
    wrapper.unmount();
  });

  it("settings row rejects a non-positive limit locally (no IPC)", async () => {
    const wrapper = await mountPop(makeReport());
    await openPopover(wrapper);
    const inputs = wrapper.findAll(".chat-input__token-input");
    await inputs[1].setValue("0");
    await wrapper.find(".chat-input__token-save").trigger("click");
    await wrapper.vm.$nextTick();
    expect(wrapper.find(".chat-input__token-error").text()).toContain("额度");
    // set_quota_settings 未被调用(本地校验拦截);两次 invoke 都是
    // mount / 弹层打开的 usage_window refresh。
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(invokeMock.mock.calls.every(([cmd]) => cmd === "usage_window")).toBe(true);
    wrapper.unmount();
  });
});
