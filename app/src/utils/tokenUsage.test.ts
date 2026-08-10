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
import {
  abbreviateTokens,
  cacheRatePercent,
  parseTokenUsageJson,
  tokenUsageLevel,
} from "./tokenUsage";

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

describe("parseTokenUsageJson", () => {
  it("returns null for null / undefined / empty string", () => {
    expect(parseTokenUsageJson(null)).toBeNull();
    expect(parseTokenUsageJson(undefined)).toBeNull();
    expect(parseTokenUsageJson("")).toBeNull();
  });

  it("returns null for non-JSON / non-object input", () => {
    expect(parseTokenUsageJson("not json")).toBeNull();
    expect(parseTokenUsageJson("123")).toBeNull();
    expect(parseTokenUsageJson('"a string"')).toBeNull();
    expect(parseTokenUsageJson("[]")).toBeNull();
  });

  it("parses a full 5-field snapshot (snake_case)", () => {
    const json = JSON.stringify({
      input_tokens: 100,
      output_tokens: 50,
      cache_creation_input_tokens: 200,
      cache_read_input_tokens: 300,
      context_input_tokens: 600,
    });
    const snap = parseTokenUsageJson(json)!;
    expect(snap.context_input_tokens).toBe(600);
    expect(snap.output_tokens).toBe(50);
    expect(snap.cache_read_input_tokens).toBe(300);
  });

  it("defaults context_input_tokens to 0 for legacy 4-field rows (#[serde(default)])", () => {
    // Pre-2026-06-26 rows omit context_input_tokens entirely.
    const json = JSON.stringify({
      input_tokens: 100,
      output_tokens: 50,
      cache_creation_input_tokens: 200,
      cache_read_input_tokens: 300,
    });
    const snap = parseTokenUsageJson(json)!;
    expect(snap.context_input_tokens).toBe(0);
    expect(snap.input_tokens).toBe(100);
  });

  it("coerces non-number fields to 0 (defensive against corrupt rows)", () => {
    const json = JSON.stringify({
      input_tokens: "oops",
      output_tokens: 50,
      cache_creation_input_tokens: true,
      cache_read_input_tokens: null,
      context_input_tokens: 600,
    });
    const snap = parseTokenUsageJson(json)!;
    expect(snap.input_tokens).toBe(0); // string → 0
    expect(snap.output_tokens).toBe(50); // number passes through
    expect(snap.cache_creation_input_tokens).toBe(0); // boolean → 0
    expect(snap.cache_read_input_tokens).toBe(0); // null → 0
    expect(snap.context_input_tokens).toBe(600); // number passes through
  });
});

describe("cacheRatePercent", () => {
  it("returns null for contextInput <= 0 (legacy rows → '—' placeholder)", () => {
    expect(cacheRatePercent(50, 0)).toBeNull();
    expect(cacheRatePercent(50, -1)).toBeNull();
    expect(cacheRatePercent(0, 0)).toBeNull();
  });

  it("returns 0% when nothing was cache-read", () => {
    expect(cacheRatePercent(0, 200)).toBe(0);
  });

  it("returns exact percentages for clean ratios", () => {
    expect(cacheRatePercent(50, 200)).toBe(25);
    expect(cacheRatePercent(100, 100)).toBe(100);
    expect(cacheRatePercent(200, 100)).toBe(200);
  });

  it("rounds non-integer ratios to the nearest integer percent", () => {
    expect(cacheRatePercent(1, 3)).toBe(33); // 33.33… → 33
    expect(cacheRatePercent(2, 3)).toBe(67); // 66.66… → 67
    expect(cacheRatePercent(150, 400)).toBe(38); // 37.5 → 38
  });
});
