// Tests for `scheduledTaskFormat.ts` — F2 定时任务的人话格式化纯函数。
// Wire 契约锁:weekday 三字母小写(chrono Weekday serde 形)、档位 kind
// 三值、未知/损坏 spec 降级「未知档位」不抛。

import { describe, it, expect } from "vitest";
import {
  describeSchedule,
  formatFireTime,
  summarizePrompt,
  weekdayLabel,
  WEEKDAY_OPTIONS,
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

describe("formatFireTime", () => {
  it("epoch ms → 本地 MM-DD HH:mm", () => {
    // 2026-08-28 08:30 本地时间的 epoch ms(用 Date 反查,不依赖时区)。
    const d = new Date(2026, 7, 28, 8, 30, 0);
    expect(formatFireTime(d.getTime())).toBe("08-28 08:30");
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
