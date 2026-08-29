// Timestamp display helper — extracted so the formatting rules
// (and the boundary cases) can be unit-tested without spinning up
// a Vue renderer (same rationale + pattern as ./duration.ts).
//
// FT-F-004 (2026-06-21): the SubagentDrawer header renders the
// worker run's `startedAt` / `finishedAt` via this helper. The raw
// backend value is a UTC ISO8601 string carrying an offset (e.g.
// `2026-06-20T05:38:54.053+00:00`); the drawer wants a compact
// LOCAL-time `HH:MM:SS` so the user reads wall-clock, not UTC.
//
// Format rules (locked by FT-F-004 grill, 2026-06-21):
//   - Drop the date — same-session drawer opens don't span days.
//   - Drop milliseconds — human precision; `05:39:05` is enough.
//   - LOCAL timezone — `new Date(iso).getHours()` returns the
//     viewer's local hours (NOT the UTC hours embedded in the
//     string). Slicing the raw ISO would show UTC and drift ~8h
//     from what the user expects — this is the core gotcha.
//   - Empty / invalid input → "--:--:--" placeholder (defensive;
//     the template guards `v-if="run?.startedAt"` so the invalid
//     path is rare in practice, but the helper stays safe if reused
//     elsewhere, mirroring abbreviateDuration's NaN clamp).

/** Format a UTC ISO8601 timestamp as a local `HH:MM:SS` string.
 *  See the file-header comment for the format rules + the UTC→local
 *  gotcha. Returns "--:--:--" for empty / unparseable input. */
export function formatTime(iso: string | null | undefined): string {
  if (!iso) return "--:--:--";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "--:--:--";
  const h = d.getHours().toString().padStart(2, "0");
  const m = d.getMinutes().toString().padStart(2, "0");
  const s = d.getSeconds().toString().padStart(2, "0");
  return `${h}:${m}:${s}`;
}

/** Format a SQLite `datetime('now')` value ("YYYY-MM-DD HH:MM:SS",
 *  UTC, no offset marker) as a local `HH:MM:SS` string.
 *
 *  BUGLIST CH13-1 (2026-08-29): moved here from ./audit.ts and fixed
 *  to actually convert — the old version sliced the raw string and
 *  displayed the UTC wall clock (06:54 for a 15:54 local event,
 *  ~8h drift from everything else in the UI). Same gotcha as
 *  `formatTime` above: hand `Date` an explicit `Z`, then read local
 *  getters. Malformed input (no space / time portion ≠ HH:MM:SS)
 *  returns the input verbatim, matching the old defensive contract. */
export function formatTimeOfDay(ts: string): string {
  const idx = ts.indexOf(" ");
  if (idx < 0) return ts;
  const time = ts.slice(idx + 1);
  if (time.length !== 8) return ts;
  return formatTime(`${ts.slice(0, idx)}T${time}Z`);
}
