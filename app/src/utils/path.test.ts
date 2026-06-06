// Unit tests for `simplifyPath` (PR3).
//
// What we're protecting:
//   1. Happy path: `/home/carlos/code/foo` → `~/code/foo`.
//   2. The home-dir prefix replacement must be *boundary-safe*:
//      `/home/carlosOther` must NOT be treated as under
//      `/home/carlos`. A naive `startsWith(homeDir)` would falsely
//      match. The implementation uses `path === homeDir` and
//      `path.startsWith(homeDir + "/")` to gate the replacement.
//   3. Null / empty inputs must degrade gracefully (no throw, no
//      unexpected `~` artifact).
//
// Reference: `docs/BACKLOG.md` §5.1 + PR3 PRD.

import { describe, it, expect } from "vitest";
import { simplifyPath } from "./path";

describe("simplifyPath", () => {
  describe("happy path", () => {
    it("replaces a child of homeDir with ~", () => {
      expect(simplifyPath("/home/carlos/code/foo", "/home/carlos")).toBe(
        "~/code/foo",
      );
    });

    it("returns ~ when path equals homeDir exactly", () => {
      expect(simplifyPath("/home/carlos", "/home/carlos")).toBe("~");
    });

    it("preserves trailing slash under homeDir", () => {
      expect(simplifyPath("/home/carlos/code/", "/home/carlos")).toBe(
        "~/code/",
      );
    });
  });

  describe("boundary safety", () => {
    it("does NOT treat /home/carlosOther as under /home/carlos", () => {
      // The naive `startsWith(homeDir)` would match and produce
      // `~Other`; the boundary check must prevent it.
      expect(simplifyPath("/home/carlosOther", "/home/carlos")).toBe(
        "/home/carlosOther",
      );
    });

    it("does NOT treat /home/carlos2/x as under /home/carlos", () => {
      expect(simplifyPath("/home/carlos2/x", "/home/carlos")).toBe(
        "/home/carlos2/x",
      );
    });
  });

  describe("paths outside homeDir", () => {
    it("returns the path unchanged for /etc/hosts", () => {
      expect(simplifyPath("/etc/hosts", "/home/carlos")).toBe("/etc/hosts");
    });

    it("returns the path unchanged when only suffix matches", () => {
      expect(simplifyPath("/var/carlos/x", "/home/carlos")).toBe(
        "/var/carlos/x",
      );
    });
  });

  describe("null / empty inputs", () => {
    it("returns the path unchanged when homeDir is null", () => {
      expect(simplifyPath("/home/carlos/x", null)).toBe("/home/carlos/x");
    });

    it("returns the path unchanged when homeDir is empty string", () => {
      // Empty string is falsy, so the guard `if (!homeDir) return path`
      // short-circuits — the prefix-replacement branch is unreachable
      // with an empty homeDir.
      expect(simplifyPath("/home/carlos/x", "")).toBe("/home/carlos/x");
    });

    it("returns the empty path unchanged when path is empty", () => {
      expect(simplifyPath("", "/home/carlos")).toBe("");
    });
  });
});
