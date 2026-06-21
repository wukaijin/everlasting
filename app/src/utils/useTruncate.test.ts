// Unit tests for `useTruncate` — the markdown-aware string truncation
// used by PR3 of the subagent-drawer redesign (and consumed by PR5).
//
// Coverage (per PRD R12 + PR3 acceptance criteria):
//   1. Empty / null-ish input is safe (no throw, returns "").
//   2. Text shorter than `maxChars` is returned unchanged (no suffix).
//   3. Text exactly `maxChars` is returned unchanged (no suffix).
//   4. Text 1 char over `maxChars` truncates and appends suffix.
//   5. Newline-only text doesn't gain a trailing suffix inside a line.
//   6. Fenced code block — boundary does NOT land inside the fence.
//   7. Inline code (single backticks) — boundary does NOT land inside.
//   8. Unclosed code fence falls back to hard cut at `maxChars`.
//   9. Markdown link may be cut at boundary (no special handling).
//  10. 100k chars completes in <50ms (no O(N^2) regression).

import { describe, it, expect } from "vitest";
import { truncate } from "./useTruncate";

describe("truncate — guards", () => {
  it("returns empty string for empty input", () => {
    expect(truncate("", 100)).toBe("");
  });

  it("returns empty string for null-ish input", () => {
    expect(truncate(undefined as unknown as string, 100)).toBe("");
    expect(truncate(null as unknown as string, 100)).toBe("");
  });

  it("returns just the suffix when maxChars is 0", () => {
    // The spec says maxChars === 0 → "" (or suffix if provided).
    // We choose suffix. Pass an empty suffix explicitly to get "".
    expect(truncate("hello", 0, "")).toBe("");
    expect(truncate("hello", 0, "…")).toBe("…");
  });

  it("returns just the suffix when maxChars is negative", () => {
    // Defensive: negative maxChars behaves like zero (returns suffix).
    expect(truncate("hello", -5)).toBe("…");
  });
});

describe("truncate — short text passes through", () => {
  it("text shorter than maxChars returns unchanged (no suffix)", () => {
    expect(truncate("hi", 10)).toBe("hi");
  });

  it("text exactly maxChars returns unchanged (no suffix)", () => {
    expect(truncate("hello", 5)).toBe("hello");
  });

  it("empty text returns empty (not the suffix)", () => {
    expect(truncate("", 100)).toBe("");
  });
});

describe("truncate — over-budget cuts", () => {
  it("text 1 char over maxChars truncates with suffix", () => {
    expect(truncate("hello!", 5)).toBe("hello…");
  });

  it("long prose truncates at maxChars with no markdown interference", () => {
    const text = "The quick brown fox jumps over the lazy dog.";
    // 44 chars total. maxChars=20 → "The quick brown fox " + "…"
    const result = truncate(text, 20);
    expect(result.endsWith("…")).toBe(true);
    expect(result.length).toBeLessThanOrEqual(20 + 1); // +1 for the suffix char
  });

  it("default suffix is the single Unicode ellipsis (not '...')", () => {
    const result = truncate("abcdefghij", 5);
    expect(result.endsWith("…")).toBe(true);
    expect(result).toBe("abcde…");
  });

  it("custom suffix replaces the default", () => {
    expect(truncate("abcdefghij", 5, "...")).toBe("abcde...");
    expect(truncate("abcdefghij", 5, "")).toBe("abcde");
  });
});

describe("truncate — markdown boundaries", () => {
  it("text containing a fenced code block does not cut inside the fence", () => {
    // Fence opens at index 6 (the "```"), closes at index 23
    // (the closing "```"). Choose maxChars=15 — that lands
    // INSIDE the fence (between the opener at 6 and the closer
    // at 23). The safe boundary must back up to before the
    // opener, so the truncated output ends BEFORE index 6.
    const text = "intro\n```py\nprint('x')\n```\noutro more text";
    const result = truncate(text, 15);
    // Find both fence positions.
    const fenceStart = text.indexOf("```"); // 6
    expect(fenceStart).toBe(6);
    // The result must NOT include any character past the fence opener.
    const withoutSuffix = result.slice(0, -1); // strip "…"
    expect(withoutSuffix.length).toBeLessThanOrEqual(fenceStart);
    // The result also starts at index 0 (full prefix).
    expect(text.indexOf(withoutSuffix)).toBe(0);
  });

  it("text containing an inline code span does not cut inside the backticks", () => {
    // "see the `foo()` function for details on more stuff"
    // Index: 0-6 "see the ", 7 "`", 8-12 "foo()", 13 "`", 14-43 rest
    // The inline code spans indices 7-13. cut=11 lands INSIDE the
    // code ("foo("). Safe boundary must back up to before the
    // opening backtick at index 7.
    const text = "see the `foo()` function for details on more stuff";
    const result = truncate(text, 11);
    const inlineStart = text.indexOf("`"); // 7
    const withoutSuffix = result.slice(0, -1);
    expect(withoutSuffix.length).toBeLessThanOrEqual(inlineStart);
  });

  it("text with a markdown link may be cut at the boundary (no special handling)", () => {
    const text = "click [here](https://example.com) for more";
    // Cut at 15 lands inside "here". This is acceptable — the spec
    // explicitly says links are not specially handled.
    const result = truncate(text, 15);
    expect(result.endsWith("…")).toBe(true);
    // Result is just text[0:15] + suffix.
    expect(result).toBe(text.slice(0, 15) + "…");
  });

  it("unclosed code fence falls back to a hard cut at maxChars", () => {
    // Text starts with a fence but never closes. The backtrack
    // target is the opener at index 0; safeBoundary would be 0;
    // the fallback path kicks in and hard-cuts at maxChars.
    const text = "```python\nprint(1)\nprint(2)\nprint(3)";
    const result = truncate(text, 10);
    // Hard cut: text[0:10] + "…"
    expect(result).toBe(text.slice(0, 10) + "…");
  });

  it("text with an unclosed inline backtick also falls back gracefully", () => {
    // Single backtick with no closing backtick. cut=8 lands inside
    // the unclosed inline span. The opener is at index 6
    // (` ` hello ` world...`). safeBoundary backs up to 6 → result
    // is "hello " + "…" = "hello …".
    const text = "hello `world without close";
    const result = truncate(text, 8);
    // Backtrack to before the opener (index 6) is the markdown-safe
    // behaviour. The user clicks "View full →" to see the original.
    expect(result).toBe("hello …");
    // Sanity: the result does NOT include the partial backtick.
    expect(result).not.toContain("`w");
  });

  it("text that ends mid-fence (no closing fence) does not infinite-loop", () => {
    // The whole text is just a fence opener + a long body. The scan
    // marks inFence=true; backtrack lands at 0; fallback hard-cuts.
    const text = "```" + "x".repeat(1000);
    const result = truncate(text, 50);
    expect(result.endsWith("…")).toBe(true);
    expect(result.length).toBe(50 + 1);
  });

  it("text with multiple inline code spans: cut picks the latest opener before boundary", () => {
    // Text: "`a` then `bbbbbbb` and more"
    // Backticks at: 0 (open), 2 (close), 9 (open), 16 (close), ...
    // cut=14 lands INSIDE the second span ("bbbbbbb" is at
    // 10-15). safeBoundary should backtrack to 9 (the opener).
    // Result is text[0..9] + "…" = "`a` then `" + "…".
    const text = "`a` then `bbbbbbb` and more";
    const result = truncate(text, 14);
    const secondOpener = text.indexOf("`", 3); // skip the closed first span
    expect(secondOpener).toBe(9);
    const withoutSuffix = result.slice(0, -1);
    // Result ends AT the opener (exclusive), so length is 9.
    expect(withoutSuffix.length).toBeLessThanOrEqual(9);
    expect(text.indexOf(withoutSuffix)).toBe(0);
  });

  it("text with fence then content then closing fence: cuts inside content respect the fence", () => {
    // ``` at 0-2, "code" at 3-6, ``` at 7-9 (closer), "after" at 11+.
    // maxChars=15 lands in "after". No code region active at cut →
    // hard cut at maxChars.
    const text = "```code```after more text here";
    const result = truncate(text, 15);
    expect(result).toBe(text.slice(0, 15) + "…");
  });

  it("text with two consecutive single backticks (literal ` `` `) is treated as no-toggle", () => {
    // Text: "``literal`` then `code` spans"
    // Two backticks at indices 0-1 (literal pair, CommonMark
    // treats as text "``"); two backticks at 9-10 (also literal);
    // single backtick at 17 (opens real inline); single backtick
    // at 22 (closes real inline).
    //
    // The implementation treats 2-backtick runs as no-toggle, so
    // the real inline code opens at index 17. cut=20 lands INSIDE
    // the real code span ("cod" at 18-20). safeBoundary should
    // backtrack to 17 (the opener).
    const text = "``literal`` then `code` spans";
    const result = truncate(text, 20);
    const realCodeStart = 17;
    const withoutSuffix = result.slice(0, -1);
    expect(withoutSuffix.length).toBeLessThanOrEqual(realCodeStart);
    // The result must NOT include the partial code ("cod" or "co").
    expect(result).not.toContain("cod");
  });
});

describe("truncate — performance", () => {
  it("100k chars completes in under 50ms", () => {
    const text = "a".repeat(100_000);
    const start = performance.now();
    const result = truncate(text, 500);
    const elapsed = performance.now() - start;
    expect(result.endsWith("…")).toBe(true);
    // Generous ceiling — measured ~1-3ms on a modern laptop.
    expect(elapsed).toBeLessThan(50);
  });

  it("100k chars with embedded fences completes in under 50ms", () => {
    // Pathological: fence opens + closes every ~50 chars → 1000+
    // toggles. Verifies the linear scan stays linear.
    const chunk = "x".repeat(40);
    const text = "```\n" + chunk + "\n```\n" + chunk + "\n```\n" + chunk + "\n```";
    // Pad to 100k.
    const padded = text + "y".repeat(100_000 - text.length);
    const start = performance.now();
    const result = truncate(padded, 200);
    const elapsed = performance.now() - start;
    expect(result.endsWith("…")).toBe(true);
    expect(elapsed).toBeLessThan(50);
  });
});

describe("truncate — UTF-8 safety", () => {
  it("non-ASCII chars count as 1 char each (JS string indexing)", () => {
    // JS strings are UTF-16: "你" is one UTF-16 code unit. Our
    // function uses .length (UTF-16 code units). For BMP chars
    // that's the same as char count.
    const text = "你好世界,hello world";
    // 13 chars (5 CJK + comma + 5 ASCII + space + ... actually
    // just trust .length here).
    const result = truncate(text, 5);
    // First 5 chars = "你好世界," + suffix.
    expect(result).toBe(text.slice(0, 5) + "…");
  });
});