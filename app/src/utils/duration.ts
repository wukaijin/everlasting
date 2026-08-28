// Duration display helpers — extracted from `MessageItem.vue` so
// the formatting rules (and the boundary cases) can be unit-tested
// without spinning up a Vue renderer.
//
// F5 (LLM Latency Tracking): the chat panel renders two
// per-message latencies via this helper:
//   1. Assistant message bottom-right chip (totalMs)
//   2. ToolCallCard status row (durationMs)
// 2026-08-29 ui-visual-polish: upgraded to the s/m/h ladder the
// subagent drawer also needs — SubagentDrawer statusDisplay and
// DrawerSection liveChip previously rolled their own toFixed(1)
// and rendered "14400.0s" for a 4-hour run; now everything funnels
// through here (single source of truth).
//
// Format rules (2026-08-29 revision, replaces the pre-08-29
// "always one decimal below a minute" lock):
//   - < 10_000 ms        → "0.4s" / "9.9s"   (one decimal — fast-tool
//                          precision is the whole point at this scale)
//   - 10_000–59_999      → "32s" / "54s"     (whole seconds; "54.0s"
//                          trailing ".0" is decimal noise)
//   - 60_000–3_599_999   → "1m 23s"; whole minute → "5m" (compact
//                          grammar, not "5m 0s")
//   - ≥ 3_600_000        → "1h 2m"; whole hour → "2h" (seconds are
//                          noise at hour scale)
//
// Negative or non-finite inputs are clamped to 0 (defensive
// against user clock changes that make `Date.now() - start`
// negative, and against NaN propagation from a buggy IPC
// payload). The clamp is silent (returns "0s") — the caller
// doesn't need to special-case the result.

/** Abbreviate a millisecond duration to a human-readable label.
 *  See file-header comment for the format rules. */
export function abbreviateDuration(ms: number): string {
  // Defensive: NaN, negative, Infinity all collapse to 0.
  if (!Number.isFinite(ms) || ms < 0) {
    return "0s";
  }
  // Fast-tool range: "Ns" with one decimal — precision matters
  // when comparing 0.4s vs 2.1s tool calls.
  if (ms < 10_000) {
    return `${(ms / 1000).toFixed(1)}s`;
  }
  const totalSeconds = Math.floor(ms / 1000);
  // Sub-minute: whole seconds ("32s" / "54s" — no trailing ".0").
  if (totalSeconds < 60) {
    return `${totalSeconds}s`;
  }
  const minutesTotal = Math.floor(totalSeconds / 60);
  // Hour range: "1h 2m" — seconds dropped as noise at this scale.
  if (minutesTotal >= 60) {
    const hours = Math.floor(minutesTotal / 60);
    const mins = minutesTotal % 60;
    return mins === 0 ? `${hours}h` : `${hours}h ${mins}m`;
  }
  // Minute range: "1m 23s"; whole minute compacts to "5m".
  const seconds = totalSeconds % 60;
  return seconds === 0 ? `${minutesTotal}m` : `${minutesTotal}m ${seconds}s`;
}
