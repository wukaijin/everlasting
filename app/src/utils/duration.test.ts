// Tests for `abbreviateDuration` — the F5 (LLM Latency Tracking)
// formatter. The function is small but has enough boundary cases
// (sub-second / sub-minute / minute-rounded; negative; NaN; integer
// vs. fractional seconds) to warrant a dedicated test file. The
// formatter is the single source of truth for the assistant message
// bottom-right chip AND the ToolCallCard status row — if it changes,
// the lock in this file catches any visual drift.

import { describe, it, expect } from "vitest";
import { abbreviateDuration } from "./duration";

describe("abbreviateDuration", () => {
  it("formats sub-second durations with one decimal", () => {
    expect(abbreviateDuration(0)).toBe("0.0s");
    expect(abbreviateDuration(400)).toBe("0.4s");
    expect(abbreviateDuration(999)).toBe("1.0s"); // rounding to one decimal
  });

  it("formats sub-minute durations with one decimal", () => {
    expect(abbreviateDuration(1000)).toBe("1.0s");
    expect(abbreviateDuration(1500)).toBe("1.5s");
    expect(abbreviateDuration(3200)).toBe("3.2s");
    expect(abbreviateDuration(32400)).toBe("32.4s");
    expect(abbreviateDuration(59900)).toBe("59.9s");
  });

  it("switches to 'Xm Ys' format past 60 seconds", () => {
    expect(abbreviateDuration(60_000)).toBe("1m 0s");
    expect(abbreviateDuration(60_500)).toBe("1m 0.5s");
    expect(abbreviateDuration(83_000)).toBe("1m 23s");
    expect(abbreviateDuration(724_000)).toBe("12m 4s");
  });

  it("formats the seconds portion with one decimal only when fractional", () => {
    // Whole-number seconds: "30s" not "30.0s" — keeps the
    // label tight for the common case where the seconds portion
    // is whole. Fractional seconds: "30.5s" — keeps the
    // sub-minute precision for fast cancellations.
    expect(abbreviateDuration(90_000)).toBe("1m 30s");
    expect(abbreviateDuration(90_500)).toBe("1m 30.5s");
  });

  it("clamps negative inputs to 0.0s", () => {
    // Defensive: a user clock change can make
    // `Date.now() - start` go negative. The formatter must
    // collapse to "0.0s" rather than show a phantom
    // negative number.
    expect(abbreviateDuration(-100)).toBe("0.0s");
    expect(abbreviateDuration(-Number.MAX_SAFE_INTEGER)).toBe("0.0s");
  });

  it("clamps NaN / Infinity to 0.0s", () => {
    // Defensive: a buggy upstream could pass NaN (e.g. an
    // arithmetic that lost a value to division by zero).
    // The formatter must not produce "NaNs" or "Infinitys"
    // — both are visually broken and confusing.
    expect(abbreviateDuration(NaN)).toBe("0.0s");
    expect(abbreviateDuration(Infinity)).toBe("0.0s");
    expect(abbreviateDuration(-Infinity)).toBe("0.0s");
  });
});
