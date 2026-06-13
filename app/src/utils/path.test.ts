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
import { simplifyPath, isPathInRoot } from "./path";

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

// Tests for `isPathInRoot` (re-grill 2026-06-13 PR2).
//
// Mirrors the Rust `projects/boundary::is_within_root` edge cases
// documented in `.trellis/spec/backend/project-cwd-boundary.md §6`:
//   1. target === root → inside
//   2. target is a direct child of root → inside
//   3. target is a multi-level descendant → inside
//   4. target is a sibling of root → outside
//   5. target is a prefix-trap (`/repo/foobar` vs `/repo/foo`) → outside
//   6. target uses `..` to escape root → outside
//   7. empty / relative inputs → outside (defensive)
describe("isPathInRoot", () => {
  describe("happy paths (inside root)", () => {
    it("returns true when target equals root exactly", () => {
      expect(isPathInRoot("/repo", "/repo")).toBe(true);
    });

    it("returns true when target is a direct child of root", () => {
      expect(isPathInRoot("/repo/src", "/repo")).toBe(true);
    });

    it("returns true when target is a multi-level descendant", () => {
      expect(isPathInRoot("/repo/src/chat/foo.ts", "/repo")).toBe(true);
    });
  });

  describe("prefix-trap edge case", () => {
    it("returns false for /repo/foobar vs /repo/foo", () => {
      // The classic prefix trap: a naive `startsWith("/repo/foo")`
      // would match `/repo/foobar`; the component-wise check must
      // reject it. Mirrors Rust `Path::starts_with` semantics.
      expect(isPathInRoot("/repo/foobar", "/repo/foo")).toBe(false);
    });
  });

  describe("sibling / outside cases", () => {
    it("returns false for a sibling of root", () => {
      expect(isPathInRoot("/other", "/repo")).toBe(false);
    });

    it("returns false when target is a deep sibling", () => {
      expect(isPathInRoot("/other/deep/file", "/repo")).toBe(false);
    });
  });

  describe("lexical `..` escape", () => {
    it("returns false when target uses .. to escape root", () => {
      // `/repo/../sibling/file` lexically normalizes to
      // `/sibling/file`, which is OUTSIDE `/repo`. The backend's
      // `is_within_root` does the same lexical_normalize dance and
      // must agree on this case.
      expect(isPathInRoot("/repo/../sibling/file", "/repo")).toBe(false);
    });
  });

  describe("null / empty / relative inputs", () => {
    it("returns false when target is empty", () => {
      expect(isPathInRoot("", "/repo")).toBe(false);
    });

    it("returns false when root is empty", () => {
      expect(isPathInRoot("/repo/foo", "")).toBe(false);
    });

    it("returns false when target is relative", () => {
      expect(isPathInRoot("repo/foo", "/repo")).toBe(false);
    });

    it("returns false when root is relative", () => {
      expect(isPathInRoot("/repo/foo", "repo")).toBe(false);
    });
  });
});
