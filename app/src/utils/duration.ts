// Duration display helpers — extracted from `MessageItem.vue` so
// the formatting rules (and the boundary cases) can be unit-tested
// without spinning up a Vue renderer.
//
// F5 (LLM Latency Tracking): the chat panel renders two
// per-message latencies via this helper:
//   1. Assistant message bottom-right chip (totalMs):
//      e.g. "0.4s", "1.2s", "32.4s", "1m 23s", "12m 4s"
//   2. ToolCallCard status row (durationMs):
//      same scale.
//
// Format rules (locked in the PRD §"Definition of Done" + spec
// "Wrong vs Correct" in `.trellis/spec/backend/llm-contract.md`
// "Scenario: Latency Tracking" §7):
//   - < 1000 ms   → "0.0s" / "0.4s" / "999.0s" (one decimal, "s" suffix)
//   - 1000-59999  → "1.0s" / "32.4s" / "59.9s"  (one decimal, "s" suffix)
//   - 60_000+     → "1m 23s" / "12m 4s" / "59m 32s" (minute-rounded;
//     seconds keep one decimal only if they need to disambiguate;
//     "1m 0s" instead of "1m" to match the "m s" grammar).
//
// Negative or non-finite inputs are clamped to 0 (defensive
// against user clock changes that make `Date.now() - start`
// negative, and against NaN propagation from a buggy IPC
// payload). The clamp is silent (returns "0.0s") — the caller
// doesn't need to special-case the result.

/** Abbreviate a millisecond duration to a human-readable label.
 *  See file-header comment for the format rules. */
export function abbreviateDuration(ms: number): string {
  // Defensive: NaN, negative, Infinity all collapse to 0.
  if (!Number.isFinite(ms) || ms < 0) {
    return "0.0s";
  }
  // Sub-minute range: "Ns" with one decimal.
  if (ms < 60_000) {
    return `${(ms / 1000).toFixed(1)}s`;
  }
  // Minute range: "Mm Ss" — total seconds still one decimal
  // for sub-minute precision, but a whole-minute value renders
  // as "Xm 0s" (not "Xm") to keep the suffix consistent.
  const totalSeconds = ms / 1000;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds - minutes * 60;
  // Format seconds with one decimal when fractional, integer
  // when whole. The leading-zero is implied ("1m 0s" not
  // "1m 0.0s").
  const secondsLabel =
    seconds % 1 === 0 ? `${seconds.toFixed(0)}s` : `${seconds.toFixed(1)}s`;
  return `${minutes}m ${secondsLabel}`;
}
