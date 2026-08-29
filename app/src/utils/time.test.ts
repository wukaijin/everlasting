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
import { formatTime, formatTimeOfDay } from "./time";

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

// BUGLIST CH13-1 (2026-08-29): SQLite `datetime('now')` values were
// sliced raw, showing the UTC wall clock in the audit/trace UI while
// everything else rendered local time. The helper must funnel the
// value through the same UTC→local Date conversion as `formatTime`.
describe("formatTimeOfDay (SQLite datetime, UTC → local)", () => {
  it("converts a SQLite UTC datetime to the local HH:MM:SS breakdown", () => {
    const ts = "2026-08-29 06:54:55";
    const d = new Date("2026-08-29T06:54:55Z");
    const expected = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
    expect(formatTimeOfDay(ts)).toBe(expected);
  });

  it("preserves seconds verbatim (not TZ-shifted out of the minute)", () => {
    // Whole-hour offsets never move the seconds digit, so this pins
    // the padStart + passthrough behavior deterministically.
    expect(formatTimeOfDay("2026-08-29 06:54:05")).toMatch(/:05$/);
  });

  it("returns malformed input verbatim (defensive contract)", () => {
    expect(formatTimeOfDay("2026-08-29")).toBe("2026-08-29");
    expect(formatTimeOfDay("2026-08-29 06:54")).toBe("2026-08-29 06:54");
    expect(formatTimeOfDay("")).toBe("");
  });
});
