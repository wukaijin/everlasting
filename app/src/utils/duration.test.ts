// Tests for `abbreviateDuration` — the F5 (LLM Latency Tracking)
// formatter. The function is small but has enough boundary cases
// (sub-second / sub-minute / minute / hour; negative; NaN; the
// 10s decimal cutoff) to warrant a dedicated test file. The
// formatter is the single source of truth for the assistant message
// bottom-right chip, the ToolCallCard status row, the subagent
// drawer status pill AND the drawer section live chips — if it
// changes, the lock in this file catches any visual drift.
//
// 2026-08-29 contract revision (ui-visual-polish): the pre-revision
// lock ("always one decimal below a minute") rendered "54.0s" and
// "14400.0s"; the ladder is now decimal < 10s → whole seconds →
// minutes → hours.

import { describe, it, expect } from "vitest";
import { abbreviateDuration } from "./duration";

describe("abbreviateDuration", () => {
  it("formats sub-10s durations with one decimal", () => {
    expect(abbreviateDuration(0)).toBe("0.0s");
    expect(abbreviateDuration(400)).toBe("0.4s");
    expect(abbreviateDuration(1500)).toBe("1.5s");
    expect(abbreviateDuration(9900)).toBe("9.9s");
  });

  it("drops the decimal at 10s and above", () => {
    // "10.0s" / "54.0s" trailing ".0" is decimal noise — the
    // decimal only earns its place below 10s.
    expect(abbreviateDuration(10_000)).toBe("10s");
    expect(abbreviateDuration(32_400)).toBe("32s");
    expect(abbreviateDuration(54_000)).toBe("54s");
    expect(abbreviateDuration(59_900)).toBe("59s");
  });

  it("switches to 'Xm Ys' format past 60 seconds", () => {
    expect(abbreviateDuration(83_000)).toBe("1m 23s");
    expect(abbreviateDuration(724_000)).toBe("12m 4s");
  });

  it("compacts whole minutes to '5m' (not '5m 0s')", () => {
    expect(abbreviateDuration(60_000)).toBe("1m");
    expect(abbreviateDuration(90_000)).toBe("1m 30s");
    expect(abbreviateDuration(300_000)).toBe("5m");
    expect(abbreviateDuration(3_599_000)).toBe("59m 59s");
  });

  it("switches to 'Xh Ym' format past 60 minutes", () => {
    // Seconds are noise at hour scale — dropped entirely.
    expect(abbreviateDuration(3_600_000)).toBe("1h");
    expect(abbreviateDuration(3_660_000)).toBe("1h 1m");
    expect(abbreviateDuration(7_261_000)).toBe("2h 1m");
  });

  it("clamps negative inputs to 0s", () => {
    // Defensive: a user clock change can make
    // `Date.now() - start` go negative. The formatter must
    // collapse to "0s" rather than show a phantom negative.
    expect(abbreviateDuration(-100)).toBe("0s");
    expect(abbreviateDuration(-Number.MAX_SAFE_INTEGER)).toBe("0s");
  });

  it("clamps NaN / Infinity to 0s", () => {
    // Defensive: a buggy upstream could pass NaN (e.g. an
    // arithmetic that lost a value to division by zero).
    // The formatter must not produce "NaNs" or "Infinitys"
    // — both are visually broken and confusing.
    expect(abbreviateDuration(NaN)).toBe("0s");
    expect(abbreviateDuration(Infinity)).toBe("0s");
    expect(abbreviateDuration(-Infinity)).toBe("0s");
  });
});
