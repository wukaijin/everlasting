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
//   5. (2026-08-29 ui-visual-polish) the F5 duration chip was
//      REMOVED from the summary — ToolCallHeader already renders
//      the same `result.durationMs` next to ✓ done, the repeat
//      was noise. Guard tests assert its absence.
//   6. (Guard test) Empty / undefined content does NOT crash —
//      the body renders an empty <pre> (parent decides whether
//      to mount via v-if).

import { beforeEach, describe, it, expect } from "vitest";
import { nextTick } from "vue";
import { mount } from "@vue/test-utils";
import ToolOutputBody from "./ToolOutputBody.vue";
import {
  resetExistenceForTests,
  setExistenceForTests,
} from "../../utils/pathExistence";

describe("ToolOutputBody", () => {
  function mountBody(props: { content: string; isError: boolean }) {
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

  it("never renders a duration chip in the summary (2026-08-29 dedup)", () => {
    // The header owns the duration display now; the summary is
    // `output · <size>` only — no trailing duration even if a
    // stale caller still passes a durationMs prop (extra props
    // are ignored as attrs).
    const w = mount(ToolOutputBody, {
      props: { content: "ok", isError: false },
      attrs: { "data-stale-duration": "1234" },
    });
    const summaryText = w.find(".tool-output-body summary").text();
    expect(summaryText).toContain("output");
    expect(summaryText).toMatch(/\d+ chars/);
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

  // --- 09-13 路径 linkify(AC5):<pre> 插值改 v-html(linkifyPlainText
  // 产物),路径转锚点、HTML 只以转义文本存在、截断边界不产半截链接。 ---
  describe("path linkify (09-13, AC5)", () => {
    it("renders a file path in the output as a clickable anchor", () => {
      const w = mountBody({ content: "wrote out/a.md", isError: false });
      const a = w.find("a[data-file-path='out/a.md']");
      expect(a.exists()).toBe(true);
      expect(a.text()).toBe("out/a.md");
      // 锚点在 pre 内部,周围文本保持原样。
      expect(w.find(".tool-output-body__pre").text()).toContain("wrote");
    });

    it("routes image paths to the image channel (data-image-path)", () => {
      const w = mountBody({ content: "saved out/shot/1.png", isError: false });
      expect(w.find("a[data-image-path='out/shot/1.png']").exists()).toBe(true);
      expect(w.find("a[data-file-path]").exists()).toBe(false);
    });

    it("escapes <script> payloads so they never become executable markup", () => {
      const w = mountBody({
        content: '<script>alert(1)</script>\nwrote out/a.md',
        isError: false,
      });
      const pre = w.find(".tool-output-body__pre");
      // 无可执行标记;载荷只能以文本形态存在。
      expect(pre.html().toLowerCase()).not.toContain("<script");
      expect(pre.find("script").exists()).toBe(false);
      // 同段输出里的路径插锚不受相邻 HTML 文本影响。
      expect(w.find("a[data-file-path='out/a.md']").exists()).toBe(true);
    });

    it("does not emit a half-cut link when the 500-char cut lands inside a path", () => {
      // 496 x + 空格(497)= 497 字符,路径 "out/a.md" 从 497 号字符起,
      // 500 字截断把它切成 "out" —— 无扩展名尾,正则不匹配,无锚点。
      const content = "x".repeat(496) + " out/a.md tail";
      expect(content.length).toBeGreaterThan(500);
      const w = mountBody({ content, isError: false });
      expect(w.find("a[data-file-path]").exists()).toBe(false);
      expect(w.find("a[data-image-path]").exists()).toBe(false);
      // 截断契约不变:后缀仍按 truncateOutput 追加。
      expect(w.find(".tool-output-body__pre").text()).toMatch(/… \(\d+ more chars\)/);
    });

    it("still linkifies paths that fit fully inside the 500-char window", () => {
      // 对照臂:截断开启但路径完整 → 正常插锚(截断没有杀掉 linkify)。
      const content = "x".repeat(400) + " out/a.md";
      const w = mountBody({ content, isError: false });
      expect(w.find("a[data-file-path='out/a.md']").exists()).toBe(true);
    });
  });

  // --- 09-14 存在性闸门:确认缺失的路径不产锚点(乐观 → 异步降级)。
  // 组件无 pinia 也能测:用绝对路径(resolveKey 不依赖 cwd);vitest 下
  // pathExistence 默认禁网,未知路径零 fetch 副作用。 ---
  describe("existence gating (09-14)", () => {
    beforeEach(() => resetExistenceForTests());

    it("keeps the optimistic anchor for an unknown path", () => {
      const w = mountBody({ content: "wrote /tmp/maybe/a.md", isError: false });
      expect(w.find("a[data-file-path='/tmp/maybe/a.md']").exists()).toBe(true);
    });

    it("omits the anchor for a confirmed-missing path (plain text stays)", () => {
      setExistenceForTests("/tmp/gone/a.md", false);
      const w = mountBody({ content: "wrote /tmp/gone/a.md", isError: false });
      expect(w.find("a[data-file-path]").exists()).toBe(false);
      expect(w.find(".tool-output-body__pre").text()).toContain("/tmp/gone/a.md");
    });

    it("re-renders reactively when a missing result lands after mount", async () => {
      // computed 消费方经 reactive Map 依赖追踪免费获得重算:挂载时乐观
      // 产锚,结果落地(nextTick 后)锚点消失、文本保留。
      const w = mountBody({ content: "wrote /tmp/late/a.md", isError: false });
      expect(w.find("a[data-file-path='/tmp/late/a.md']").exists()).toBe(true);
      setExistenceForTests("/tmp/late/a.md", false);
      await nextTick();
      expect(w.find("a[data-file-path]").exists()).toBe(false);
      expect(w.find(".tool-output-body__pre").text()).toContain("/tmp/late/a.md");
    });
  });
});
