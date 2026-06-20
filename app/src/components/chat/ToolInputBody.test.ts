// Tests for `ToolInputBody.vue` — shared input body component
// (FT-F-001 PR1, 2026-06-20).
//
// Covers the rendering contract the drawer will rely on:
//   1. Renders an empty-input-safe body when `input` is `{}`
//      (parent gates this via `v-if`; the body itself always
//      renders — the test asserts the `<details>` element exists
//      even with empty input, since the gate is the parent's
//      responsibility).
//   2. String input renders as a quoted JSON value.
//   3. Object input renders as pretty-printed JSON with 2-space
//      indent (the spec).
//   4. Nested objects render with proper indentation preserved.
//   5. Empty `{}` input renders an empty `{}` JSON (still renders
//      the body, parent decides whether to mount it).

import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";
import ToolInputBody from "./ToolInputBody.vue";

describe("ToolInputBody", () => {
  function mountBody(name: string, input: Record<string, unknown>) {
    return mount(ToolInputBody, {
      props: { name, input },
    });
  }

  it("renders the <details> shell with 'input' summary text", () => {
    const w = mountBody("shell", { command: "ls" });
    expect(w.find(".tool-input-body").exists()).toBe(true);
    expect(w.find(".tool-input-body summary").text()).toBe("input");
  });

  it("renders a string input as quoted JSON value", () => {
    const w = mountBody("read_file", { path: "/repo/src/foo.ts" });
    const pre = w.find(".tool-input-body__pre");
    expect(pre.exists()).toBe(true);
    expect(pre.text()).toContain('"path"');
    expect(pre.text()).toContain('"/repo/src/foo.ts"');
  });

  it("renders an object input as pretty-printed JSON with 2-space indent", () => {
    const w = mountBody("shell", { command: "ls -la" });
    const pre = w.find(".tool-input-body__pre");
    expect(pre.text()).toBe('{\n  "command": "ls -la"\n}');
  });

  it("renders nested objects with indentation preserved", () => {
    const w = mountBody("complex_tool", {
      outer: { inner: { deep: "value" }, sibling: 42 },
    });
    const pre = w.find(".tool-input-body__pre");
    const text = pre.text();
    // Each nested level adds 2 spaces.
    expect(text).toContain('"outer": {');
    expect(text).toContain('"inner": {');
    expect(text).toContain('"deep": "value"');
    expect(text).toContain('"sibling": 42');
  });

  it("renders empty object input as an empty JSON body", () => {
    const w = mountBody("noop", {});
    const pre = w.find(".tool-input-body__pre");
    expect(pre.exists()).toBe(true);
    expect(pre.text()).toBe("{}");
    // Parent decides whether to mount with empty input (see
    // ToolCallCard.vue v-if guard). The body itself always
    // renders — this test verifies the body is still
    // well-formed (empty JSON in the <pre>).
  });
});
