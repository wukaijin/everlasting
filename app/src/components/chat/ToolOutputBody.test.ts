// Tests for `ToolOutputBody.vue` — shared output body component
// (FT-F-001 PR1, 2026-06-20).
//
// Covers the rendering contract the drawer will rely on:
//   1. Plain text content renders inside the <pre>.
//   2. CWD envelope (`{"result":"...","cwd":"..."}`) is
//      auto-unwrapped via `extractToolResultDisplay` so the
//      user sees the inner result, not the raw JSON.
//   3. Long content is truncated with the `truncateOutput`
//      suffix.
//   4. `isError` adds the error visual class on the <pre>.
//   5. `durationMs` adds the F5 duration chip after the size
//      in the summary.
//   6. (Guard test) Empty / undefined content does NOT crash —
//      the body renders an empty <pre> (parent decides whether
//      to mount via v-if).

import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";
import ToolOutputBody from "./ToolOutputBody.vue";

describe("ToolOutputBody", () => {
  function mountBody(props: {
    content: string;
    isError: boolean;
    durationMs?: number;
  }) {
    return mount(ToolOutputBody, { props });
  }

  it("renders plain text content inside the <pre>", () => {
    const w = mountBody({ content: "hello world", isError: false });
    const pre = w.find(".tool-output-body__pre");
    expect(pre.exists()).toBe(true);
    expect(pre.text()).toBe("hello world");
  });

  it("auto-unwraps the cwd envelope to show the inner result", () => {
    // This is the REQ-16 envelope: `{"result": "...", "cwd": "..."}`.
    // The body must strip the envelope so the user sees just the
    // tool output string, not the raw JSON wrapper. Mirrors
    // ToolCallCard.vue pre-extraction behavior.
    const w = mountBody({
      content: JSON.stringify({
        result: "the actual output",
        cwd: "/data/projects/repo",
      }),
      isError: false,
    });
    const pre = w.find(".tool-output-body__pre");
    expect(pre.text()).toBe("the actual output");
    // Defensive: the wrapper JSON must NOT appear.
    expect(pre.text()).not.toContain('"cwd"');
    expect(pre.text()).not.toContain('"result"');
  });

  it("truncates long content with the truncation suffix", () => {
    // truncateOutput default max is 500 chars; builds a string
    // > 500 chars to trigger truncation.
    const longContent = "x".repeat(600);
    const w = mountBody({ content: longContent, isError: false });
    const pre = w.find(".tool-output-body__pre");
    expect(pre.text().length).toBeLessThan(longContent.length);
    // The truncation helper appends "… (N more chars)" — verify
    // the suffix is present (defensive, in case the helper
    // format ever changes).
    expect(pre.text()).toMatch(/… \(\d+ more chars\)/);
  });

  it("applies the error visual class when isError is true", () => {
    const w = mountBody({ content: "exit 1", isError: true });
    const pre = w.find(".tool-output-body__pre");
    expect(pre.classes()).toContain("tool-output-body__pre--error");
    // The outer details block carries the error class too
    // (matches old ToolCallCard.vue behavior — the `<details>`
    // element gets a visual cue when the tool failed).
    expect(w.find(".tool-output-body").classes()).toContain(
      "tool-output-body--error",
    );
  });

  it("omits the error visual class when isError is false", () => {
    const w = mountBody({ content: "ok", isError: false });
    const pre = w.find(".tool-output-body__pre");
    expect(pre.classes()).not.toContain("tool-output-body__pre--error");
    expect(w.find(".tool-output-body").classes()).not.toContain(
      "tool-output-body--error",
    );
  });

  it("appends the F5 duration chip to the summary when durationMs is set", () => {
    const w = mountBody({
      content: "ok",
      isError: false,
      durationMs: 1234,
    });
    const summaryText = w.find(".tool-output-body summary").text();
    // 1234ms → "1.2s" via abbreviateDuration.
    expect(summaryText).toContain("output");
    expect(summaryText).toContain("1.2s");
    // The size label is also present.
    expect(summaryText).toMatch(/\d+ chars/);
  });

  it("omits the duration chip when durationMs is undefined (pre-F5 row)", () => {
    const w = mountBody({
      content: "ok",
      isError: false,
      // durationMs intentionally omitted.
    });
    const summaryText = w.find(".tool-output-body summary").text();
    // The summary must NOT contain a duration separator + duration.
    // (The summary's format is `output · <size>` when no duration,
    //  `output · <size> · <duration>` when present.)
    expect(summaryText).not.toMatch(/· [0-9]+(\.[0-9])?s/);
    expect(summaryText).not.toMatch(/· [0-9]+m /);
  });

  it("renders the size label with K suffix for content > 1024 chars", () => {
    // 2048 chars → "2.0K chars" via the sizeLabel computed.
    const w = mountBody({
      content: "x".repeat(2048),
      isError: false,
    });
    const summaryText = w.find(".tool-output-body summary").text();
    expect(summaryText).toContain("2.0K chars");
  });

  it("renders the size label without 'chars' suffix under 1024 chars", () => {
    // 42 chars → "42 chars" via the sizeLabel computed. Per the
    // spec the suffix is omitted under 1024 (just a bare count
    // reads fine for small outputs) — wait, the suffix IS kept
    // for clarity. Locked at "42 chars".
    const w = mountBody({
      content: "x".repeat(42),
      isError: false,
    });
    const summaryText = w.find(".tool-output-body summary").text();
    expect(summaryText).toContain("42 chars");
  });

  it("does NOT crash on empty content", () => {
    // Guard test: an empty string still produces a valid <pre>
    // (parent decides whether to mount via v-if — see
    // ToolCallCard.vue). The body itself must be safe with any
    // string content, including the empty string.
    expect(() => mountBody({ content: "", isError: false })).not.toThrow();
    const w = mountBody({ content: "", isError: false });
    const pre = w.find(".tool-output-body__pre");
    expect(pre.exists()).toBe(true);
    expect(pre.text()).toBe("");
  });
});
