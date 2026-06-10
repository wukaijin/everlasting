// Unit tests for `utils/tokenUsage.ts` (A4 threshold ladder +
// token abbreviator). The color band is a small but spec-locked
// piece of business logic; the abbreviator is used by the
// ChatInput hint's "14.2K · 7% / 200K" line.
//
// Boundaries (locked by `.trellis/spec/backend/llm-contract.md`
// "Scenario: Token Usage Tracking" §3 "Color thresholds (UI)"):
//   - 0-49%   → "ok"    (green)
//   - 50-74%  → "warn"  (amber)
//   - 75%+    → "alert" (red)

import { describe, it, expect } from "vitest";
import { abbreviateTokens, tokenUsageLevel } from "./tokenUsage";

describe("tokenUsageLevel", () => {
  it("returns 'ok' for 0%", () => {
    expect(tokenUsageLevel(0)).toBe("ok");
  });

  it("returns 'ok' for 49% (lower edge of green band)", () => {
    expect(tokenUsageLevel(0.49)).toBe("ok");
  });

  it("returns 'warn' for 50% (lower edge of amber band)", () => {
    expect(tokenUsageLevel(0.5)).toBe("warn");
  });

  it("returns 'warn' for 74% (upper edge of amber band)", () => {
    expect(tokenUsageLevel(0.74)).toBe("warn");
  });

  it("returns 'alert' for 75% (lower edge of red band)", () => {
    expect(tokenUsageLevel(0.75)).toBe("alert");
  });

  it("returns 'alert' for 100% (full context)", () => {
    expect(tokenUsageLevel(1.0)).toBe("alert");
  });

  it("returns 'alert' for > 100% (overshot the window — defensive)", () => {
    expect(tokenUsageLevel(1.5)).toBe("alert");
  });

  it("returns 'ok' for negative ratios (defensive clamp)", () => {
    // A negative `pct` shouldn't happen in production (u32 input
    // divided by positive context window), but the function
    // shouldn't produce a phantom "warn" / "alert" band.
    expect(tokenUsageLevel(-0.1)).toBe("ok");
  });
});

describe("abbreviateTokens", () => {
  it("returns '0' for zero", () => {
    expect(abbreviateTokens(0)).toBe("0");
  });

  it("returns exact numbers under 1000", () => {
    expect(abbreviateTokens(42)).toBe("42");
    expect(abbreviateTokens(999)).toBe("999");
  });

  it("formats exact thousands without a decimal", () => {
    expect(abbreviateTokens(1000)).toBe("1K");
    expect(abbreviateTokens(200_000)).toBe("200K");
  });

  it("formats non-exact thousands with 1 decimal, trimming trailing 0", () => {
    expect(abbreviateTokens(1200)).toBe("1.2K");
    expect(abbreviateTokens(14_200)).toBe("14.2K");
    expect(abbreviateTokens(1500)).toBe("1.5K");
  });

  it("formats exact millions without a decimal", () => {
    expect(abbreviateTokens(1_000_000)).toBe("1M");
  });

  it("formats non-exact millions with 1 decimal, trimming trailing 0", () => {
    expect(abbreviateTokens(1_200_000)).toBe("1.2M");
  });
});
