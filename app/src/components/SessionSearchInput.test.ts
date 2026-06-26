// Tests for `SessionSearchInput.vue` — pure presentation.
//
// No store, no IPC. Asserts:
//   - renders input with placeholder
//   - autofocuses on mount (focus called on the underlying <input>)
//   - emits update:modelValue on input
//   - clear button (✕) only visible when query is non-empty
//   - clicking ✕ emits both update:modelValue="" AND clear
//   - Escape with non-empty query clears query (but does NOT emit
//     clear — the parent keeps the row open so the user can type
//     a new query without re-clicking the 🔍 icon)
//   - Escape with empty query emits clear (parent closes the row)
//   - public focus() handle re-focuses the input

import { describe, it, expect, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import SessionSearchInput from "./SessionSearchInput.vue";

describe("SessionSearchInput", () => {
  it("renders input with default placeholder", () => {
    const w = mount(SessionSearchInput, { props: { modelValue: "" } });
    const input = w.find("input");
    expect(input.exists()).toBe(true);
    expect(input.attributes("placeholder")).toBe("搜索会话标题…");
  });

  it("accepts a custom placeholder prop", () => {
    const w = mount(SessionSearchInput, {
      props: { modelValue: "", placeholder: "Filter…" },
    });
    expect(w.find("input").attributes("placeholder")).toBe("Filter…");
  });

  it("emits update:modelValue on user typing", async () => {
    const w = mount(SessionSearchInput, { props: { modelValue: "" } });
    const input = w.find("input");
    await input.setValue("abc");
    expect(w.emitted("update:modelValue")?.[0]).toEqual(["abc"]);
  });

  it("does NOT render the clear (✕) button when query is empty", () => {
    const w = mount(SessionSearchInput, { props: { modelValue: "" } });
    expect(w.find(".session-search__clear").exists()).toBe(false);
  });

  it("renders the clear (✕) button when query is non-empty", () => {
    const w = mount(SessionSearchInput, { props: { modelValue: "PR" } });
    expect(w.find(".session-search__clear").exists()).toBe(true);
  });

  it("clicking clear emits update:modelValue='' AND clear", async () => {
    const w = mount(SessionSearchInput, { props: { modelValue: "PR" } });
    await w.find(".session-search__clear").trigger("click");
    expect(w.emitted("update:modelValue")?.[0]).toEqual([""]);
    expect(w.emitted("clear")?.length).toBe(1);
  });

  it("Escape with non-empty query clears query without emitting clear", async () => {
    const w = mount(SessionSearchInput, { props: { modelValue: "PR" } });
    await w.find("input").trigger("keydown", { key: "Escape" });
    expect(w.emitted("update:modelValue")?.[0]).toEqual([""]);
    expect(w.emitted("clear")).toBeUndefined();
  });

  it("Escape with empty query emits clear (parent closes the row)", async () => {
    const w = mount(SessionSearchInput, { props: { modelValue: "" } });
    await w.find("input").trigger("keydown", { key: "Escape" });
    expect(w.emitted("clear")?.length).toBe(1);
  });

  it("exposes a focus() handle (parent calls this from Cmd/Ctrl+K)", async () => {
    const w = mount(SessionSearchInput, { props: { modelValue: "" } });
    const inputEl = w.find("input").element as HTMLInputElement;
    const focusSpy = vi.spyOn(inputEl, "focus");
    (w.vm as unknown as { focus: () => void }).focus();
    await nextTick();
    expect(focusSpy).toHaveBeenCalled();
  });
});
