// sessionGrouping — pure helpers for the sidebar's session list.
//
// 2026-06-27 sidebar 搜索/密度/分组 (PR-of-PRs, 3 features):
//   - `bucketKey(date, now)` classifies a session's `updated_at`
//     into one of 4 time buckets (today / yesterday / thisWeek /
//     older). The 4-way split matches user mental model better
//     than a flat chronological list — at 7+ sessions the sidebar
//     becomes unreadable without grouping.
//   - `groupSessions(sessions, now)` buckets sessions into a Map
//     keyed by bucket. Empty buckets are omitted so the render
//     layer only iterates populated groups.
//   - `filterByQuery(sessions, query)` is the search filter —
//     case-insensitive title substring match. Empty / whitespace-
//     only query returns the input unchanged (no filtering), which
//     keeps the sidebar showing grouped view (not flat).
//
// All three are PURE: they take their dependencies (`now`, `query`)
// as args rather than reading from `Date.now()` / `localStorage`.
// This makes them trivially testable (no clock injection needed —
// pass an explicit `now`) and keeps the SessionList reactive layer
// responsible for side effects.
//
// Boundary semantics:
//   - "今天" = same calendar day as `now` in local time
//   - "昨天" = exactly 1 calendar day before `now` (not "within 24h")
//   - "本周" = 2-7 calendar days before `now`
//   - "更早" = more than 7 calendar days before `now`
//
// Calendar-day arithmetic (vs 24-hour windows) is deliberate: "昨天"
// must mean "yesterday" the way users think of it, not "20 hours
// ago". A session from 23:50 yesterday and a session from 00:10
// today are 50 minutes apart but in different buckets, matching
// the user's "today vs yesterday" mental model.

import type { SessionSummary } from "../stores/chat.types";

/** The 4 time buckets the sidebar groups sessions into. Order
 *  matters: `BUCKET_ORDER` is the canonical display order so the
 *  sidebar always shows 今天 → 昨天 → 本周 → 更早 top-to-bottom. */
export type BucketKey = "today" | "yesterday" | "thisWeek" | "older";

/** Canonical display order. Frozen so callers can't accidentally
 *  mutate the shared array. */
export const BUCKET_ORDER: readonly BucketKey[] = Object.freeze([
  "today",
  "yesterday",
  "thisWeek",
  "older",
]);

/** Chinese labels keyed by BucketKey. Frozen so the render layer
 *  and tests share the exact same labels (no "今天" vs "今日"
 *  drift). */
export const BUCKET_LABELS: Readonly<Record<BucketKey, string>> =
  Object.freeze({
    today: "今天",
    yesterday: "昨天",
    thisWeek: "本周",
    older: "更早",
  });

/** Number of days a session must fall within to qualify for
 *  "本周". 7 days covers a full calendar week; the label is
 *  本周 (this week) rather than 7天 (7 days) because the user
 *  thinks in weeks, not raw day counts. */
const THIS_WEEK_DAYS = 7;

/** Truncate a Date to the local-time midnight of the same day.
 *  Used as the unit of "calendar day" in bucket arithmetic.
 *  `setHours(0,0,0,0)` is the canonical local-midnight reset;
 *  DST transitions are handled implicitly because we work in
 *  local time throughout. */
function startOfLocalDay(d: Date): Date {
  const out = new Date(d.getTime());
  out.setHours(0, 0, 0, 0);
  return out;
}

/** Whole-day delta from `from` to `to` (positive when `to` is
 *  AFTER `from`). Both dates are first floored to local midnight
 *  so the result counts calendar days, not 24-hour windows.
 *  Example: from=2026-06-26 23:50, to=2026-06-27 00:10 → 1 day
 *  (not 0, even though the wall-clock delta is 20 minutes). */
function calendarDayDelta(from: Date, to: Date): number {
  const a = startOfLocalDay(from).getTime();
  const b = startOfLocalDay(to).getTime();
  return Math.round((b - a) / (24 * 60 * 60 * 1000));
}

/** Classify `date` into a bucket relative to `now`. Pure — caller
 *  passes `now` explicitly so tests can pin time. Invalid date
 *  strings return `"older"` (defensive default; the sidebar will
 *  silently show them in the catch-all group rather than throwing). */
export function bucketKey(date: string | Date, now: Date): BucketKey {
  const d = typeof date === "string" ? new Date(date) : date;
  if (Number.isNaN(d.getTime())) return "older";
  const delta = calendarDayDelta(d, now);
  if (delta <= 0) return "today";
  if (delta === 1) return "yesterday";
  if (delta < THIS_WEEK_DAYS) return "thisWeek";
  return "older";
}

/** Bucket `sessions` by their `updated_at` field, preserving each
 *  bucket's input order (the sidebar already sorts by updated_at
 *  desc upstream). Returns a Map keyed by BucketKey, in
 *  `BUCKET_ORDER` iteration order. Empty buckets are omitted so
 *  the render layer never iterates zero-length arrays. */
export function groupSessions(
  sessions: readonly SessionSummary[],
  now: Date,
): Map<BucketKey, SessionSummary[]> {
  const out = new Map<BucketKey, SessionSummary[]>();
  for (const s of sessions) {
    const key = bucketKey(s.updated_at, now);
    let arr = out.get(key);
    if (!arr) {
      arr = [];
      out.set(key, arr);
    }
    arr.push(s);
  }
  return out;
}

/** Case-insensitive title-substring filter. Empty or whitespace-
 *  only `query` returns the input array unchanged (NOT a new
 *  empty array) so the caller can swap between "grouped" and
 *  "filtered flat" modes without an extra identity check. Match
 *  uses `String.prototype.includes` after `toLowerCase()` on both
 *  sides; unicode is folded via the default JS case map (locale-
 *  insensitive but adequate for our title set — all current
 *  titles are English + Chinese, and JS lowercase handles both). */
export function filterByQuery(
  sessions: readonly SessionSummary[],
  query: string,
): SessionSummary[] {
  const trimmed = query.trim().toLowerCase();
  if (trimmed.length === 0) return sessions.slice();
  return sessions.filter((s) => s.title.toLowerCase().includes(trimmed));
}
