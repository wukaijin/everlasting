// Tests for `scheduledTaskFormat.ts` — F2 定时任务的人话格式化纯函数。
// Wire 契约锁:weekday 三字母小写(chrono Weekday serde 形)、档位 kind
// 三值、未知/损坏 spec 降级「未知档位」不抛。

import { describe, it, expect } from "vitest";
import {
  describeSchedule,
  formatFireTime,
  formatLocalYMDHm,
  summarizePrompt,
  weekdayLabel,
  WEEKDAY_OPTIONS,
  INTERVAL_UNITS,
  splitEveryMin,
  describeRunCount,
  describeEndDate,
  completedByRunLimit,
  completedByEndDate,
  completedByOnce,
  displayNextFireAt,
} from "./scheduledTaskFormat";

describe("describeSchedule", () => {
  it("daily / interval / weekly 三档人话", () => {
    expect(describeSchedule({ kind: "daily", at: "09:00" })).toBe("每天 09:00");
    expect(describeSchedule({ kind: "interval", every_min: 30 })).toBe(
      "每 30 分钟",
    );
    expect(
      describeSchedule({ kind: "weekly", weekday: "mon", at: "08:30" }),
    ).toBe("每周一 08:30");
  });

  it("F2b 三新档人话:hourly / weekdays / monthly", () => {
    expect(describeSchedule({ kind: "hourly", minute: 30 })).toBe("每小时 30 分");
    expect(describeSchedule({ kind: "hourly", minute: 5 })).toBe("每小时 05 分");
    expect(describeSchedule({ kind: "weekdays", at: "09:00" })).toBe(
      "每工作日 09:00",
    );
    expect(describeSchedule({ kind: "monthly", day: 15, at: "09:00" })).toBe(
      "每月 15 号 09:00",
    );
  });

  it("once 档人话:单次 + 本地 YYYY-MM-DD HH:mm(CH11-1)", () => {
    const d = new Date(2026, 7, 30, 9, 5, 0);
    expect(describeSchedule({ kind: "once", at_ms: d.getTime() })).toBe(
      "单次 2026-08-30 09:05",
    );
  });

  it("interval 单位人话(F2b):整除取大单位,n=1 去数量", () => {
    expect(describeSchedule({ kind: "interval", every_min: 1440 })).toBe("每天");
    expect(describeSchedule({ kind: "interval", every_min: 120 })).toBe(
      "每 2 小时",
    );
    expect(describeSchedule({ kind: "interval", every_min: 10080 })).toBe("每周");
    expect(describeSchedule({ kind: "interval", every_min: 90 })).toBe(
      "每 90 分钟",
    );
  });

  it("null / 损坏档位降级为「未知档位」,不抛", () => {
    expect(describeSchedule(null)).toBe("未知档位");
    // @ts-expect-error 模拟存量脏数据(未知 kind)
    expect(describeSchedule({ kind: "cron", expr: "* * * * *" })).toBe(
      "未知档位",
    );
  });

  it("weekday 未知值原样透出(防御)", () => {
    expect(weekdayLabel("funday")).toBe("funday");
  });

  it("周几选项覆盖 mon..sun 七值", () => {
    expect(WEEKDAY_OPTIONS.map((w) => w.value)).toEqual([
      "mon",
      "tue",
      "wed",
      "thu",
      "fri",
      "sat",
      "sun",
    ]);
  });
});

describe("F2b 固定频率单位换算", () => {
  it("INTERVAL_UNITS 覆盖 分钟/小时/天/周 且系数正确", () => {
    expect(INTERVAL_UNITS.map((u) => [u.value, u.minutes])).toEqual([
      ["minute", 1],
      ["hour", 60],
      ["day", 1440],
      ["week", 10080],
    ]);
  });

  it("splitEveryMin:能整除的最大单位,否则原值分钟", () => {
    expect(splitEveryMin(1440)).toMatchObject({ n: 1, unit: "day" });
    expect(splitEveryMin(10080)).toMatchObject({ n: 1, unit: "week" });
    expect(splitEveryMin(20160)).toMatchObject({ n: 2, unit: "week" });
    expect(splitEveryMin(60)).toMatchObject({ n: 1, unit: "hour" });
    expect(splitEveryMin(90)).toMatchObject({ n: 90, unit: "minute" });
    expect(splitEveryMin(45)).toMatchObject({ n: 45, unit: "minute" });
  });
});

describe("F2b 结束条件格式化", () => {
  it("describeRunCount:上限设置时 N/M,否则 N", () => {
    expect(describeRunCount(3, 5)).toBe("3/5 次");
    expect(describeRunCount(3, null)).toBe("3 次");
    expect(describeRunCount(0, undefined)).toBe("0 次");
  });

  it("describeEndDate:本地日期粒度;缺失/非法为空串", () => {
    const d = new Date(2026, 11, 31, 23, 59, 59, 999);
    expect(describeEndDate(d.getTime())).toBe("至 2026-12-31");
    expect(describeEndDate(null)).toBe("");
    expect(describeEndDate(NaN)).toBe("");
  });

  it("completedByRunLimit:停用 + 计数达限 才算完成", () => {
    expect(
      completedByRunLimit({ enabled: false, run_count: 3, max_runs: 3 }),
    ).toBe(true);
    expect(
      completedByRunLimit({ enabled: false, run_count: 2, max_runs: 3 }),
    ).toBe(false);
    expect(
      completedByRunLimit({ enabled: false, run_count: 9, max_runs: null }),
    ).toBe(false);
    expect(
      completedByRunLimit({ enabled: true, run_count: 3, max_runs: 3 }),
    ).toBe(false);
  });

  it("completedByEndDate:停用 + ends_at 已过 才算结束", () => {
    expect(
      completedByEndDate({ enabled: false, ends_at: Date.now() - 1 }),
    ).toBe(true);
    expect(
      completedByEndDate({ enabled: false, ends_at: Date.now() + 60_000 }),
    ).toBe(false);
    expect(completedByEndDate({ enabled: false, ends_at: null })).toBe(false);
    expect(
      completedByEndDate({ enabled: true, ends_at: Date.now() - 1 }),
    ).toBe(false);
  });

  it("completedByOnce:停用的 once 档,已消费或已过期才算完成(CH11-1)", () => {
    const past = { kind: "once", at_ms: Date.now() - 60_000 } as const;
    const future = { kind: "once", at_ms: Date.now() + 60_000 } as const;
    // 已 fire 过(run_count ≥ 1)→ 完成,与 at_ms 新旧无关。
    expect(
      completedByOnce({ enabled: false, run_count: 1, schedule: past }),
    ).toBe(true);
    // 过期未 fire(如停用跨过时刻后重启用)→ 完成。
    expect(
      completedByOnce({ enabled: false, run_count: 0, schedule: past }),
    ).toBe(true);
    // 未到点 / 仍启用 / 非 once 档 → 不是完成态。
    expect(
      completedByOnce({ enabled: false, run_count: 0, schedule: future }),
    ).toBe(false);
    expect(
      completedByOnce({ enabled: true, run_count: 0, schedule: past }),
    ).toBe(false);
    expect(
      completedByOnce({
        enabled: false,
        run_count: 3,
        schedule: { kind: "daily", at: "09:00" },
      }),
    ).toBe(false);
    expect(completedByOnce({ enabled: false, run_count: 1, schedule: null })).toBe(
      false,
    );
  });

  it("displayNextFireAt:once 档消费后无下次(渲染 —),其余透传(CH11-1)", () => {
    const once = { kind: "once", at_ms: 123 } as const;
    expect(
      displayNextFireAt({ run_count: 1, schedule: once, next_fire_at: 999 }),
    ).toBeNull();
    // 未消费的 once(等点)仍展示存库展示值。
    expect(
      displayNextFireAt({ run_count: 0, schedule: once, next_fire_at: 999 }),
    ).toBe(999);
    expect(
      displayNextFireAt({
        run_count: 5,
        schedule: { kind: "daily", at: "09:00" },
        next_fire_at: 777,
      }),
    ).toBe(777);
  });
});

describe("formatFireTime", () => {
  it("epoch ms → 本地 MM-DD HH:mm", () => {
    // 2026-08-28 08:30 本地时间的 epoch ms(用 Date 反查,不依赖时区)。
    const d = new Date(2026, 7, 28, 8, 30, 0);
    expect(formatFireTime(d.getTime())).toBe("08-28 08:30");
  });

  it("formatLocalYMDHm:本地 YYYY-MM-DD HH:mm;非法 → 原样数字串", () => {
    const d = new Date(2026, 11, 31, 23, 59, 0);
    expect(formatLocalYMDHm(d.getTime())).toBe("2026-12-31 23:59");
    expect(formatLocalYMDHm(NaN)).toBe("NaN");
  });

  it("缺失 / 非法输入 → “—”", () => {
    expect(formatFireTime(null)).toBe("—");
    expect(formatFireTime(undefined)).toBe("—");
    expect(formatFireTime(NaN)).toBe("—");
  });
});

describe("summarizePrompt", () => {
  it("压平空白并保留单行摘要", () => {
    expect(summarizePrompt("汇总  昨日\n\n进展\t情况", 100)).toBe(
      "汇总 昨日 进展 情况",
    );
  });

  it("超长截断加省略号", () => {
    const out = summarizePrompt("a".repeat(80), 60);
    expect(out).toHaveLength(61);
    expect(out.endsWith("…")).toBe(true);
  });
});
