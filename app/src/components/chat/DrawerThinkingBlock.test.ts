// Tests for `DrawerThinkingBlock.vue` — PR4 of the subagent-drawer
// redesign (2026-06-21).
//
// Covers the adapter contract between the drawer's accumulator
// output (`ThinkingSection`) and the shared `ThinkingBlock` visual
// primitive:
//   1. Renders the underlying ThinkingBlock (`.thinking` class).
//   2. The segment's concatenated `text` shows up in the body when
//      the `<details>` is expanded (via `thinkingDisplayText`).
//   3. Empty text does NOT crash; the body renders empty.
//   4. `section.closed === false` → streaming hint visible
//      (`span.thinking__streaming` mounts).
//   5. `section.closed === true` → streaming hint hidden.
//   6. `showStreamingHint` prop override beats the `closed` default
//      in both directions (force-on / force-off).
//   7. The `blocks.length > 1` "N blocks" badge is NOT rendered
//      (the wrapper always produces a single-element array).
//
// Icon stub note (from memory `subagentdrawer-banner-test-gotchas.md`):
// ThinkingBlock renders `<Icon name="thinking">` in its header. We
// stub `Icon` so the test doesn't pull in the full heroicons/lucide
// registry. Stubbed icons render as `<icon-stub>` with empty
// textContent; we assert on spans/classes that carry real text, not
// on the icon stub.

import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";
import DrawerThinkingBlock from "./DrawerThinkingBlock.vue";
import type { ThinkingSection } from "../../stores/subagentRuns.types";

function makeSection(overrides: Partial<ThinkingSection> = {}): ThinkingSection {
  return {
    kind: "Thinking",
    text: "Let me analyze this step by step.",
    chars: 31,
    closed: false,
    ...overrides,
  };
}

function mountBlock(props: { section: ThinkingSection; showStreamingHint?: boolean }) {
  return mount(DrawerThinkingBlock, {
    props,
    global: {
      stubs: { Icon: true },
    },
  });
}

describe("DrawerThinkingBlock — underlying ThinkingBlock mounts", () => {
  it("renders the shared ThinkingBlock (.thinking root)", () => {
    const w = mountBlock({ section: makeSection() });
    expect(w.find(".thinking").exists()).toBe(true);
  });
});

describe("DrawerThinkingBlock — body text", () => {
  it("shows the segment's concatenated text inside the <pre> body", () => {
    const w = mountBlock({
      section: makeSection({ text: "considering the options carefully" }),
    });
    const body = w.find(".thinking__body");
    expect(body.exists()).toBe(true);
    expect(body.text()).toBe("considering the options carefully");
  });

  it("does NOT crash on empty text — body renders empty", () => {
    const w = mountBlock({
      section: makeSection({ text: "", chars: 0 }),
    });
    const body = w.find(".thinking__body");
    expect(body.exists()).toBe(true);
    expect(body.text()).toBe("");
  });

  it("renders multi-paragraph text verbatim (accumulator already joined)", () => {
    // The accumulator concatenates thinking_delta chunks via `+=`;
    // the wrapper passes the result as a single ThinkingBlockInfo.
    // Multi-paragraph thinking from the model would already be one
    // string by the time it reaches this component.
    const w = mountBlock({
      section: makeSection({
        text: "first thought\n\nsecond thought",
        chars: 26,
      }),
    });
    expect(w.find(".thinking__body").text()).toBe(
      "first thought\n\nsecond thought",
    );
  });
});

describe("DrawerThinkingBlock — streaming hint derived from section.closed", () => {
  it("shows the streaming hint when section.closed === false (default)", () => {
    const w = mountBlock({
      section: makeSection({ closed: false }),
    });
    expect(w.find(".thinking__streaming").exists()).toBe(true);
  });

  it("hides the streaming hint when section.closed === true", () => {
    const w = mountBlock({
      section: makeSection({ closed: true }),
    });
    expect(w.find(".thinking__streaming").exists()).toBe(false);
  });
});

describe("DrawerThinkingBlock — showStreamingHint override", () => {
  it("force-shows the streaming hint even when section.closed === true", () => {
    const w = mountBlock({
      section: makeSection({ closed: true }),
      showStreamingHint: true,
    });
    expect(w.find(".thinking__streaming").exists()).toBe(true);
  });

  it("force-hides the streaming hint even when section.closed === false", () => {
    const w = mountBlock({
      section: makeSection({ closed: false }),
      showStreamingHint: false,
    });
    expect(w.find(".thinking__streaming").exists()).toBe(false);
  });
});

describe("DrawerThinkingBlock — header label fallback", () => {
  it("renders the 'Thought for —' header (no duration data in drawer)", () => {
    // The drawer accumulator doesn't track wall-clock duration per
    // thinking segment, so the header falls back to "—" (matches
    // ThinkingBlock.vue's `headerLabel` for `thinkingDurationMs ===
    // undefined`). Asserting this locks the fallback: a future
    // change that accidentally threads a fake duration through
    // would change the header label and fail this test.
    const w = mountBlock({ section: makeSection() });
    expect(w.find(".thinking__summary").text()).toContain("Thought for —");
  });

  it("does NOT render the 'N blocks' count badge (single-element array)", () => {
    // The wrapper always produces `[{text, signature: ""}]` — a
    // single-element array. ThinkingBlock renders the
    // `.thinking__count` badge only when `blocks.length > 1`.
    const w = mountBlock({ section: makeSection() });
    expect(w.find(".thinking__count").exists()).toBe(false);
  });
});
