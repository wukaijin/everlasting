// 08-20-turn-usage-event-quota-view WP3 — useQuotaStore 测试。
// 覆盖:refresh 解析 + chipTotal/chipPct 派生(AC5 两态)/ setSettings
// 调 IPC 后刷新 / 失败写 error 不抛。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useQuotaStore, type UsageWindowReportWire } from "./quota";

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
        hourly: [{ hour: "2026-08-20T10:00:00", inputTokens: 500_000, outputTokens: 10_000 }],
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

describe("useQuotaStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("refresh() parses the wire report; chipTotal sums input+output across providers", async () => {
    invokeMock.mockResolvedValueOnce(makeReport());
    const store = useQuotaStore();
    await store.refresh();
    expect(invokeMock).toHaveBeenCalledWith("usage_window", { providerId: null });
    expect(store.error).toBeNull();
    // 1.2M in + 80k out = 1.28M。
    expect(store.chipTotal).toBe(1_280_000);
    expect(store.chipPct).toBeNull();
  });

  it("chipPct is null without a limit and clamped 0..1 with one (AC5 两态)", async () => {
    invokeMock.mockResolvedValueOnce(makeReport({ limitTokens: 2_000_000 }));
    const store = useQuotaStore();
    await store.refresh();
    expect(store.chipPct).toBeCloseTo(0.64);

    invokeMock.mockResolvedValueOnce(makeReport({ limitTokens: 100_000 }));
    await store.refresh();
    // over-limit clamp 到 1。
    expect(store.chipPct).toBe(1);
  });

  it("setSettings() invokes the IPC then refreshes", async () => {
    invokeMock
      .mockResolvedValueOnce(undefined) // set_quota_settings
      .mockResolvedValueOnce(makeReport({ windowHours: 3, limitTokens: 500_000 })); // refresh
    const store = useQuotaStore();
    await store.setSettings(3, 500_000);
    expect(invokeMock).toHaveBeenNthCalledWith(1, "set_quota_settings", {
      windowHours: 3,
      limitTokens: 500_000,
    });
    expect(store.report?.windowHours).toBe(3);
    expect(store.report?.limitTokens).toBe(500_000);
  });

  it("refresh failure surfaces on error and keeps the last report", async () => {
    invokeMock.mockResolvedValueOnce(makeReport());
    const store = useQuotaStore();
    await store.refresh();
    invokeMock.mockRejectedValueOnce(new Error("daemon down"));
    await store.refresh(); // 不抛
    expect(store.error).toContain("daemon down");
    // 上一份 report 保留。
    expect(store.report?.windowHours).toBe(5);
  });
});
