// Tests for CodeBlockPrimitive.vue — B9 Child B
// (07-02-b9-code-block-primitive, 2026-07-02).
//
// Coverage:
//   1. Highlighting: a known language emits hljs token spans; an
//      unknown language falls through to highlightAuto without crashing.
//   2. Header: language label (or "code" fallback) + optional title.
//   3. Copy button: navigator.clipboard.writeText receives the raw
//      code; the label flips to 已复制 for 2s then reverts.
//
// jsdom has no navigator.clipboard → mocked per-test.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { nextTick } from "vue";

import CodeBlockPrimitive from "./CodeBlockPrimitive.vue";
import type { UiPrimitive } from "../uiCard.types";

const writeTextMock = vi.fn();

function mountPrim(over: Partial<UiPrimitive> = {}) {
  return mount(CodeBlockPrimitive, {
    props: {
      primitive: {
        type: "code_block",
        code: "fn main() {}",
        language: "rust",
        ...over,
      } as UiPrimitive,
    },
  });
}

beforeEach(() => {
  writeTextMock.mockReset();
  writeTextMock.mockResolvedValue(undefined);
  // jsdom lacks navigator.clipboard; assign a stub.
  Object.assign(navigator, {
    clipboard: { writeText: writeTextMock },
  });
});

describe("CodeBlockPrimitive — highlighting", () => {
  it("emits hljs token spans for a known language", () => {
    const w = mountPrim({ code: "fn main() {}", language: "rust" });
    // `fn` is a rust keyword → wrapped in <span class="hljs-keyword">.
    const spans = w.findAll(".ui-prim__code code span.hljs-keyword");
    expect(spans.length).toBeGreaterThan(0);
  });

  it("does not crash on an unknown language (highlightAuto fallback)", () => {
    const w = mountPrim({
      code: "some random gibberish text",
      language: "totally-made-up-lang",
    });
    expect(w.find(".ui-prim__code code").exists()).toBe(true);
  });

  it("renders the language label when present", () => {
    const w = mountPrim({ language: "python" });
    expect(w.find(".ui-prim__type").text()).toBe("python");
  });

  it("falls back to 'code' label when language is omitted", () => {
    const w = mountPrim({ language: undefined });
    expect(w.find(".ui-prim__type").text()).toBe("code");
  });

  it("renders the title when present", () => {
    const w = mountPrim({ title: "example snippet" });
    expect(w.find(".ui-prim__title").text()).toBe("example snippet");
  });
});

describe("CodeBlockPrimitive — copy button", () => {
  it("calls navigator.clipboard.writeText with the raw code", async () => {
    const w = mountPrim({ code: "let x = 1;" });
    await w.find(".ui-prim__copy").trigger("click");
    await flushPromises();
    expect(writeTextMock).toHaveBeenCalledTimes(1);
    expect(writeTextMock).toHaveBeenCalledWith("let x = 1;");
  });

  it("flips the label to 已复制 then reverts after 2s", async () => {
    vi.useFakeTimers();
    try {
      const w = mountPrim({ code: "let x = 1;" });
      expect(w.find(".ui-prim__copy").text()).toBe("复制");
      await w.find(".ui-prim__copy").trigger("click");
      await flushPromises();
      expect(w.find(".ui-prim__copy").text()).toBe("已复制");
      vi.advanceTimersByTime(2000);
      await nextTick();
      expect(w.find(".ui-prim__copy").text()).toBe("复制");
    } finally {
      vi.useRealTimers();
    }
  });
});
