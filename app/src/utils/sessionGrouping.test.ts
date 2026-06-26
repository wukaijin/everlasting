// Tests for `sessionGrouping` (bucketKey / groupSessions / filterByQuery).
//
// Pure-function tests — all helpers take `now` / `query` as args
// so we can pin time and avoid real-clock flakiness. The tests
// exercise every documented boundary:
//   - calendar-day vs 24-hour arithmetic (the 23:50 vs 00:10 case)
//   - invalid date strings (defensive default to "older")
//   - empty / whitespace / case-insensitive filter
//   - empty / single-bucket / multi-bucket grouping
//   - empty-bucket omission in groupSessions

import { describe, it, expect } from "vitest";
import {
  bucketKey,
  groupSessions,
  filterByQuery,
  BUCKET_ORDER,
  BUCKET_LABELS,
} from "./sessionGrouping";
import type { SessionSummary } from "../stores/chat.types";

/** Fixed "now" for all tests: 2026-06-27 12:00 local. Pick a
 *  weekday noon so calendar-day boundaries are unambiguous. */
const NOW = new Date(2026, 5, 27, 12, 0, 0); // month is 0-indexed

/** Build a minimal SessionSummary-shaped object. The helpers only
 *  read `id` / `title` / `updated_at` so we keep the fixture lean. */
function s(
  id: string,
  title: string,
  updatedAt: string,
): SessionSummary {
  return {
    id,
    title,
    updated_at: updatedAt,
    preview: "",
    project_id: "p1",
    current_cwd: "/tmp",
    worktree_path: null,
    worktree_state: "none",
    last_worktree_path: null,
    model_id: null,
    input_tokens_total: null,
    output_tokens_total: null,
    cache_creation_total: null,
    cache_read_total: null,
    last_context_input_tokens: null,
    last_input_tokens: null,
    last_output_tokens: null,
    last_cache_creation: null,
    last_cache_read: null,
    color_tag: null,
    mode: "edit",
    created_at: updatedAt,
  } as unknown as SessionSummary;
}

describe("sessionGrouping", () => {
  describe("BUCKET_ORDER / BUCKET_LABELS", () => {
    it("order is 今天 → 昨天 → 本周 → 更早", () => {
      expect(BUCKET_ORDER).toEqual([
        "today",
        "yesterday",
        "thisWeek",
        "older",
      ]);
      expect(BUCKET_LABELS.today).toBe("今天");
      expect(BUCKET_LABELS.yesterday).toBe("昨天");
      expect(BUCKET_LABELS.thisWeek).toBe("本周");
      expect(BUCKET_LABELS.older).toBe("更早");
    });
  });

  describe("bucketKey", () => {
    it("classifies a same-day timestamp as today", () => {
      expect(bucketKey("2026-06-27T11:00:00", NOW)).toBe("today");
      expect(bucketKey("2026-06-27T00:00:01", NOW)).toBe("today");
      expect(bucketKey("2026-06-27T23:59:59", NOW)).toBe("today");
    });

    it("classifies a 1-calendar-day-old timestamp as yesterday (not today)", () => {
      // 23:50 yesterday is still "yesterday" even though it's only
      // 12h10m before NOW in wall-clock terms — calendar-day
      // arithmetic is deliberate (see file header).
      expect(bucketKey("2026-06-26T23:50:00", NOW)).toBe("yesterday");
    });

    it("classifies a same-midnight previous-day timestamp as yesterday", () => {
      expect(bucketKey("2026-06-26T00:00:01", NOW)).toBe("yesterday");
    });

    it("classifies a 2-to-6-day-old timestamp as thisWeek", () => {
      expect(bucketKey("2026-06-25T12:00:00", NOW)).toBe("thisWeek"); // 2d
      expect(bucketKey("2026-06-21T12:00:00", NOW)).toBe("thisWeek"); // 6d
    });

    it("classifies a 7+ day-old timestamp as older (本周 boundary is inclusive <7d)", () => {
      expect(bucketKey("2026-06-20T12:00:00", NOW)).toBe("older"); // 7d
      expect(bucketKey("2026-06-01T12:00:00", NOW)).toBe("older"); // 26d
      expect(bucketKey("2025-01-01T12:00:00", NOW)).toBe("older"); // old
    });

    it("accepts Date objects as well as ISO strings", () => {
      const d = new Date(2026, 5, 27, 9, 0, 0);
      expect(bucketKey(d, NOW)).toBe("today");
    });

    it("returns 'older' for invalid date strings (defensive default)", () => {
      expect(bucketKey("not-a-date", NOW)).toBe("older");
      expect(bucketKey("", NOW)).toBe("older");
    });

    it("returns 'today' for invalid Date objects (NaN getTime path)", () => {
      const bad = new Date("not-a-date");
      // new Date("not-a-date") returns Invalid Date, getTime() === NaN
      expect(bucketKey(bad, NOW)).toBe("older");
    });
  });

  describe("groupSessions", () => {
    it("returns an empty Map for an empty input array", () => {
      const out = groupSessions([], NOW);
      expect(out.size).toBe(0);
    });

    it("places a single session in its bucket", () => {
      const out = groupSessions(
        [s("a", "today", "2026-06-27T10:00:00")],
        NOW,
      );
      expect(out.size).toBe(1);
      expect(out.get("today")?.length).toBe(1);
      expect(out.get("today")?.[0].id).toBe("a");
    });

    it("partitions across all 4 buckets when all populated", () => {
      const sessions = [
        s("a", "today", "2026-06-27T10:00:00"),
        s("b", "yesterday", "2026-06-26T15:00:00"),
        s("c", "this week", "2026-06-23T15:00:00"),
        s("d", "older", "2026-06-01T15:00:00"),
      ];
      const out = groupSessions(sessions, NOW);
      expect(out.size).toBe(4);
      expect(out.get("today")?.map((x) => x.id)).toEqual(["a"]);
      expect(out.get("yesterday")?.map((x) => x.id)).toEqual(["b"]);
      expect(out.get("thisWeek")?.map((x) => x.id)).toEqual(["c"]);
      expect(out.get("older")?.map((x) => x.id)).toEqual(["d"]);
    });

    it("omits empty buckets from the Map", () => {
      // Only today + older populated — yesterday + thisWeek omitted
      const sessions = [
        s("a", "today", "2026-06-27T10:00:00"),
        s("b", "older", "2026-06-01T15:00:00"),
      ];
      const out = groupSessions(sessions, NOW);
      expect(out.size).toBe(2);
      expect(out.has("yesterday")).toBe(false);
      expect(out.has("thisWeek")).toBe(false);
    });

    it("preserves input order within each bucket (caller sorts upstream)", () => {
      const sessions = [
        s("a", "first", "2026-06-27T10:00:00"),
        s("b", "second", "2026-06-27T11:00:00"),
        s("c", "third", "2026-06-27T12:00:00"),
      ];
      const out = groupSessions(sessions, NOW);
      expect(out.get("today")?.map((x) => x.id)).toEqual(["a", "b", "c"]);
    });

    it("places invalid-date sessions into 'older' (defensive default)", () => {
      const sessions = [
        s("a", "good", "2026-06-27T10:00:00"),
        s("b", "bad", "not-a-date"),
      ];
      const out = groupSessions(sessions, NOW);
      expect(out.size).toBe(2);
      expect(out.get("today")?.map((x) => x.id)).toEqual(["a"]);
      expect(out.get("older")?.map((x) => x.id)).toEqual(["b"]);
    });
  });

  describe("filterByQuery", () => {
    const sessions = [
      s("a", "PR review", "2026-06-27T10:00:00"),
      s("b", "Fix login bug", "2026-06-26T10:00:00"),
      s("c", "PR 描述 模板", "2026-06-25T10:00:00"),
      s("d", "Unrelated session", "2026-06-24T10:00:00"),
    ];

    it("returns input unchanged (a copy) for empty query", () => {
      const out = filterByQuery(sessions, "");
      expect(out).toEqual(sessions);
      // Must be a copy, not the same reference (so caller can't
      // accidentally mutate upstream state via sort etc.)
      expect(out).not.toBe(sessions);
    });

    it("treats whitespace-only query as empty", () => {
      expect(filterByQuery(sessions, "   ").length).toBe(4);
      expect(filterByQuery(sessions, "\t\n  ").length).toBe(4);
    });

    it("filters case-insensitively on ASCII titles", () => {
      expect(filterByQuery(sessions, "PR").length).toBe(2); // a + c
      expect(filterByQuery(sessions, "pr").length).toBe(2);
      expect(filterByQuery(sessions, "Pr").length).toBe(2);
    });

    it("trims the query before matching", () => {
      expect(filterByQuery(sessions, "  PR  ").length).toBe(2);
    });

    it("returns an empty array when no titles match", () => {
      expect(filterByQuery(sessions, "nonexistent")).toEqual([]);
    });

    it("matches partial substrings (not just prefix)", () => {
      expect(filterByQuery(sessions, "login")).toEqual([sessions[1]]);
      expect(filterByQuery(sessions, "bug")).toEqual([sessions[1]]);
    });

    it("matches Chinese titles (CJK codepoints are unaffected by toLowerCase)", () => {
      // "PR 描述 模板" contains both 描述 and 模板 as substrings.
      // JS `String.prototype.toLowerCase()` does not fold CJK
      // codepoints (no case mapping), so this works for our title
      // set. CJK is the dominant non-ASCII in titles today.
      expect(filterByQuery(sessions, "模板")).toEqual([sessions[2]]);
      expect(filterByQuery(sessions, "描述")).toEqual([sessions[2]]);
    });

    it("does not mutate the input array", () => {
      const before = sessions.slice();
      filterByQuery(sessions, "PR");
      expect(sessions).toEqual(before);
    });
  });
});
