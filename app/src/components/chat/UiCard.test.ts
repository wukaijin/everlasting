// Tests for UiCard.vue — B9 Child A (07-02-b9-use-ui-infra, 2026-07-02).
//
// Coverage:
//   1. Renders one mock primitive per entry in call.input.primitives.
//   2. Registry dispatch: diff / code_block each render via the mock,
//      surfacing their `type` label (+ optional title).
//   3. Unknown type degrades to the fallback (still renders, does
//      not crash the message stream).
//   4. Empty / missing / non-array primitives → renders nothing
//      (v-if guard; defensive against stale or hand-edited messages).
//
// No Tauri mock needed: UiCard + MockPrimitive are pure display
// (no invoke). Child B/C tests will cover the real renderers and
// their type-specific payloads.

import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";

import UiCard from "./UiCard.vue";
import type { ToolCallInfo } from "../../stores/chat.types";

function makeCall(input: Record<string, unknown>): ToolCallInfo {
  return { id: "tool-use-1", name: "use_ui", input };
}

function mountCard(input: Record<string, unknown>) {
  return mount(UiCard, { props: { call: makeCall(input) } });
}

describe("UiCard — primitive rendering", () => {
  it("renders one mock primitive per entry in primitives", () => {
    const w = mountCard({
      primitives: [{ type: "diff" }, { type: "code_block" }],
    });
    // Child B: code_block now renders CodeBlockPrimitive; only diff stays mock.
    expect(w.findAll(".ui-prim").length).toBe(2);
    expect(w.findAll(".ui-prim--mock").length).toBe(1);
    expect(w.findAll(".ui-prim--code").length).toBe(1);
  });

  it("surfaces each primitive's type label (registry dispatch)", () => {
    const w = mountCard({
      primitives: [
        { type: "diff", title: "v1 vs v2" },
        { type: "code_block" },
      ],
    });
    const types = w.findAll(".ui-prim__type").map((e) => e.text());
    // diff → "diff" (MockPrimitive); code_block → language||"code" (CodeBlockPrimitive).
    expect(types).toEqual(["diff", "code"]);
  });

  it("renders the title when present", () => {
    const w = mountCard({
      primitives: [{ type: "diff", title: "v1 vs v2" }],
    });
    expect(w.find(".ui-prim__title").text()).toBe("v1 vs v2");
  });

  it("does not render the title node when absent", () => {
    const w = mountCard({
      primitives: [{ type: "code_block" }],
    });
    expect(w.find(".ui-prim__title").exists()).toBe(false);
  });
});

describe("UiCard — unknown-type fallback", () => {
  it("still renders an unknown type via the fallback (no crash)", () => {
    // A hallucinated type that slipped past backend validation
    // (or a stale message) must degrade, not break the stream.
    const w = mountCard({
      primitives: [{ type: "chart" }],
    });
    expect(w.findAll(".ui-prim--mock").length).toBe(1);
    expect(w.find(".ui-prim__type").text()).toBe("chart");
  });
});

describe("UiCard — empty / missing primitives guard", () => {
  it("renders nothing when primitives is an empty array", () => {
    const w = mountCard({ primitives: [] });
    expect(w.find(".ui-card").exists()).toBe(false);
  });

  it("renders nothing when primitives is missing", () => {
    const w = mountCard({});
    expect(w.find(".ui-card").exists()).toBe(false);
  });

  it("renders nothing when primitives is a non-array (stale shape)", () => {
    const w = mountCard({ primitives: "not-an-array" });
    expect(w.find(".ui-card").exists()).toBe(false);
  });
});
