// Tests for `formatTime` — the FT-F-004 (2026-06-21) SubagentDrawer
// header timestamp formatter. The helper is small but has enough
// boundary cases (UTC→local conversion, padStart, invalid/empty
// fallback) to warrant a dedicated test file, exactly like
// ./duration.test.ts.
//
// Timezone note: the test runtime (Node/jsdom) inherits the host TZ,
// which is NOT guaranteed to be UTC. To stay TZ-independent, the
// "happy path" assertions compute the expected value from the SAME
// `new Date(iso)` local accessors the helper uses — so the test
// validates "the helper returns the LOCAL breakdown" regardless of
// which TZ CI runs in. (A wrong implementation that sliced the UTC
// string would still pass on a UTC host, but the accessor parity
// + structure + fallback tests below lock the contract; the
// UTC→local intent is documented in the helper itself.)

import { describe, it, expect } from "vitest";
import { formatTime } from "./time";

function pad(n: number): string {
  return n.toString().padStart(2, "0");
}

describe("formatTime", () => {
  it("formats a UTC ISO timestamp as the local HH:MM:SS breakdown", () => {
    const iso = "2026-06-20T10:00:30Z";
    const d = new Date(iso);
    const expected = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
    expect(formatTime(iso)).toBe(expected);
  });

  it("honors an explicit non-Z offset rather than treating it as UTC", () => {
    // +05:30 offset — the local accessor still derives wall-clock
    // from the resolved instant, so parity holds.
    const iso = "2026-06-20T10:00:30+05:30";
    const d = new Date(iso);
    const expected = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
    expect(formatTime(iso)).toBe(expected);
  });

  it("always emits two-digit H/M/S via padStart (single-digit second)", () => {
    // second = 5 (single digit) must render "05", not "5".
    // Seconds are not affected by whole-minute TZ offsets, so this
    // pins the padStart behavior deterministically.
    const iso = "2026-06-20T10:00:05Z";
    expect(formatTime(iso)).toMatch(/:05$/);
  });

  it("output shape is always HH:MM:SS for valid input", () => {
    expect(formatTime("2026-06-20T10:00:30Z")).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });

  it("returns the placeholder for empty input", () => {
    expect(formatTime("")).toBe("--:--:--");
    expect(formatTime(null)).toBe("--:--:--");
    expect(formatTime(undefined)).toBe("--:--:--");
  });

  it("returns the placeholder for unparseable input", () => {
    expect(formatTime("not-a-date")).toBe("--:--:--");
    expect(formatTime("garbage")).toBe("--:--:--");
  });
});
