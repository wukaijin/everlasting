// Tests for DiffPrimitive.vue — B9 Child C (07-02-b9-diff-primitive,
// 2026-07-02).
//
// Coverage:
//   1. Renders DiffView for a unified diff (file card + +/- lines).
//   2. Path cleaning: `a/`/`b/` prefix stripped.
//   3. added/removed counts surface in the file header.
//   4. Multi-file unified diff → multiple file cards.
//   5. Empty / unparseable diff_text → raw fallback (no crash).
//   6. Copy button → navigator.clipboard.writeText with the raw diff.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";

import DiffPrimitive from "./DiffPrimitive.vue";
import type { UiPrimitive } from "../uiCard.types";

const writeTextMock = vi.fn();

beforeEach(() => {
  writeTextMock.mockReset();
  writeTextMock.mockResolvedValue(undefined);
  Object.assign(navigator, {
    clipboard: { writeText: writeTextMock },
  });
});

const SINGLE_FILE_DIFF = `--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,3 @@
 fn main() {
-    println!("old");
+    println!("new");
 }
`;

const MULTI_FILE_DIFF = `--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,1 @@
-old
+new
--- a/bar.rs
+++ b/bar.rs
@@ -1,1 +1,1 @@
-x
+y
`;

function mountPrim(diff_text: string, over: Partial<UiPrimitive> = {}) {
  return mount(DiffPrimitive, {
    props: {
      primitive: { type: "diff", diff_text, ...over } as UiPrimitive,
    },
  });
}

describe("DiffPrimitive — single-file rendering", () => {
  it("renders a DiffView file card", () => {
    const w = mountPrim(SINGLE_FILE_DIFF);
    expect(w.findAll(".diff-file").length).toBe(1);
  });

  it("renders +/- diff lines (after expanding the collapsed modified file)", async () => {
    const w = mountPrim(SINGLE_FILE_DIFF);
    // DiffView collapses "modified" files by default; expand the body first.
    await w.find(".diff-file__header").trigger("click");
    expect(w.findAll(".diff-line--add").length).toBeGreaterThan(0);
    expect(w.findAll(".diff-line--del").length).toBeGreaterThan(0);
  });

  it("strips the a//b/ prefix from the path", () => {
    const w = mountPrim(SINGLE_FILE_DIFF);
    expect(w.find(".diff-file__path").text()).toBe("foo.rs");
  });

  it("surfaces added/removed counts in the header", () => {
    const w = mountPrim(SINGLE_FILE_DIFF);
    expect(w.find(".diff-file__add").text()).toContain("1");
    expect(w.find(".diff-file__del").text()).toContain("1");
  });

  it("renders the title when present", () => {
    const w = mountPrim(SINGLE_FILE_DIFF, { title: "before → after" });
    expect(w.find(".ui-prim__title").text()).toBe("before → after");
  });
});

describe("DiffPrimitive — multi-file", () => {
  it("renders one file card per parsed patch", () => {
    const w = mountPrim(MULTI_FILE_DIFF);
    const paths = w.findAll(".diff-file__path").map((e) => e.text());
    expect(paths).toEqual(["foo.rs", "bar.rs"]);
  });
});

describe("DiffPrimitive — malformed / empty fallback", () => {
  it("renders nothing for an empty diff_text", () => {
    const w = mountPrim("");
    // No file card; the head still renders (type label + copy).
    expect(w.findAll(".diff-file").length).toBe(0);
  });

  it("does not crash on non-diff text (raw fallback path)", () => {
    const w = mountPrim("just some prose, not a diff at all");
    // parsePatch returns [] → DiffPrimitive wraps as a single raw file;
    // the card mounts without throwing.
    expect(w.find(".ui-prim--diff").exists()).toBe(true);
  });
});

describe("DiffPrimitive — copy button", () => {
  it("calls navigator.clipboard.writeText with the raw diff_text", async () => {
    const w = mountPrim(SINGLE_FILE_DIFF);
    await w.find(".ui-prim__copy").trigger("click");
    await flushPromises();
    expect(writeTextMock).toHaveBeenCalledTimes(1);
    expect(writeTextMock).toHaveBeenCalledWith(SINGLE_FILE_DIFF);
  });
});
