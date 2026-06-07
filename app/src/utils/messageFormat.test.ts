// Tests for `extractToolResultDisplay` — the bridge between the
// LLM-facing tool result envelope (`{ result, cwd }`) and the
// human-readable display. The envelope is the LLM's contract so
// it can see which on-disk path a tool ran against (REQ-16 in
// prd.md); the UI must NOT show the raw JSON to the user.
//
// Why a separate test file (vs. adding to messageFormat.ts as a
// private test): the helper is a single, pure function with
// multiple branches (envelope / no envelope / non-JSON / empty).
// A dedicated test file keeps the fixture readable.

import { describe, it, expect } from "vitest";
import { extractToolResultDisplay } from "./messageFormat";

describe("extractToolResultDisplay", () => {
  it("unwraps the cwd envelope to the result string", () => {
    const envelope = JSON.stringify({
      result: "hello world",
      cwd: "/data/worktrees/p1/s1",
    });
    expect(extractToolResultDisplay(envelope)).toBe("hello world");
  });

  it("preserves multi-line content in the result field", () => {
    const envelope = JSON.stringify({
      result: "line 1\nline 2\nline 3",
      cwd: "/data/wt",
    });
    expect(extractToolResultDisplay(envelope)).toBe("line 1\nline 2\nline 3");
  });

  it("preserves special characters (quotes, backslashes)", () => {
    const content = 'has "quotes" and \\ a backslash';
    const envelope = JSON.stringify({ result: content, cwd: "/data/wt" });
    expect(extractToolResultDisplay(envelope)).toBe(content);
  });

  it("returns the raw input when it's not JSON", () => {
    // Pre-follow-up sessions stored plain strings; on rehydrate
    // the content has no envelope, so the helper must pass it
    // through unchanged.
    const plain = "this is not JSON";
    expect(extractToolResultDisplay(plain)).toBe(plain);
  });

  it("returns the raw input when JSON lacks the envelope shape", () => {
    // A JSON object that doesn't have both `result` (string) and
    // `cwd` (string) is not an envelope — pass through. This
    // protects against false positives from random tool output
    // that happens to be valid JSON.
    const other = JSON.stringify({ output: "data", meta: "info" });
    expect(extractToolResultDisplay(other)).toBe(other);
  });

  it("returns the raw input when result field is non-string", () => {
    const wrongType = JSON.stringify({ result: 42, cwd: "/data/wt" });
    expect(extractToolResultDisplay(wrongType)).toBe(wrongType);
  });

  it("returns empty string for empty input", () => {
    expect(extractToolResultDisplay("")).toBe("");
  });

  it("fast-paths strings that don't start with '{'", () => {
    // Common case: short, non-JSON tool output ("ok", "success",
    // "Wrote file", etc). The helper should not try to parse.
    expect(extractToolResultDisplay("ok")).toBe("ok");
    expect(extractToolResultDisplay("Wrote /tmp/foo.txt")).toBe(
      "Wrote /tmp/foo.txt",
    );
  });
});
