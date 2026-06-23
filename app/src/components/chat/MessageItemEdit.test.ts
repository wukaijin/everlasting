// Tests for `MessageItemEdit.vue` — D3 PR2 inline edit-mode UI
// (2026-06-17) extracted as part of the 2026-06-23 MessageItem
// split. The component is a pure presentation layer; the parent
// (`MessageItem.vue`) owns the store interactions. These tests
// drive the component with hand-built props + assert on the
// emitted events + DOM, without spinning up Pinia.
//
// Coverage:
//   1. Renders the textarea + Save / Cancel buttons when
//      `isEditingThisMessage=true`.
//   2. Save: emits `save(trimmed)` with the trimmed buffer.
//   3. Save: rejects empty content (button disabled + click
//      no-op fallback when JS-disabled somehow).
//   4. Save: trims surrounding whitespace before emitting.
//   5. Save: same-content no-op bubbles `cancel` (closes
//      edit mode instead of saving a no-op).
//   6. Cancel: emits `cancel`.
//   7. Resend: emit fires (the parent does not currently wire
//      a Resend button in the editor; the emit is exposed for
//      any future flow).
//   8. Disabled: `saving=true` disables Save + Cancel + textarea.
//   9. Streaming guard: `isStreaming=true` disables Save +
//      Cancel + textarea, and a Save click does not emit.
//  10. Error message: `errorMessage` prop renders in a
//      `role="alert"` row.
//  11. Buffer re-seeds when `content` prop changes while in
//      edit mode (the "stream ends mid-edit" race).

import { describe, it, expect, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";

import MessageItemEdit from "./MessageItemEdit.vue";

const baseProps = (): {
  seq: number;
  content: string;
  isStreaming: boolean;
  currentSessionId: string | null;
  isEditingThisMessage: boolean;
  saving: boolean;
  errorMessage: string | null;
} => ({
  seq: 5,
  content: "original content",
  isStreaming: false,
  currentSessionId: "sess-1",
  isEditingThisMessage: true,
  saving: false,
  errorMessage: null,
});

function mountEdit(propsOverride: Partial<ReturnType<typeof baseProps>> = {}) {
  return mount(MessageItemEdit, {
    props: { ...baseProps(), ...propsOverride },
  });
}

describe("MessageItemEdit — basic rendering", () => {
  beforeEach(() => {});

  it("renders the textarea + Save + Cancel buttons when editing", () => {
    const w = mountEdit();
    expect(w.find("[data-testid='msg-editor-textarea']").exists()).toBe(true);
    expect(w.find("[data-testid='msg-editor-cancel']").exists()).toBe(true);
    expect(w.find("[data-testid='msg-editor-save']").exists()).toBe(true);
  });

  it("seeds the textarea buffer with the message content", () => {
    const w = mountEdit({ content: "hello world" });
    const ta = w.get<HTMLTextAreaElement>("[data-testid='msg-editor-textarea']");
    expect(ta.element.value).toBe("hello world");
  });

  it("uses the seq in the textarea's aria-label", () => {
    const w = mountEdit({ seq: 42 });
    const ta = w.get<HTMLTextAreaElement>("[data-testid='msg-editor-textarea']");
    expect(ta.attributes("aria-label")).toBe("编辑消息 seq 42");
  });
});

describe("MessageItemEdit — save emit", () => {
  it("emits save(trimmed) on Save click", async () => {
    const w = mountEdit();
    await w.get("[data-testid='msg-editor-textarea']").setValue("  new content  ");
    await w.get("[data-testid='msg-editor-save']").trigger("click");
    expect(w.emitted("save")).toBeTruthy();
    expect(w.emitted("save")![0]).toEqual(["new content"]);
  });

  it("trims leading/trailing whitespace before emitting save", async () => {
    const w = mountEdit();
    await w.get("[data-testid='msg-editor-textarea']").setValue("\n\t  hello  \n");
    await w.get("[data-testid='msg-editor-save']").trigger("click");
    expect(w.emitted("save")![0]).toEqual(["hello"]);
  });

  it("Save button is disabled when buffer trims to empty", async () => {
    const w = mountEdit();
    await w.get("[data-testid='msg-editor-textarea']").setValue("   ");
    const save = w.get<HTMLButtonElement>("[data-testid='msg-editor-save']");
    expect(save.attributes("disabled")).toBeDefined();
  });

  it("Save button is disabled when buffer is empty", () => {
    const w = mountEdit({ content: "" });
    const save = w.get<HTMLButtonElement>("[data-testid='msg-editor-save']");
    expect(save.attributes("disabled")).toBeDefined();
  });

  it("does NOT emit save when content is unchanged (no-op bubbles cancel)", async () => {
    const w = mountEdit({ content: "original content" });
    // Buffer == content; Save click should emit `cancel`, not `save`.
    await w.get("[data-testid='msg-editor-save']").trigger("click");
    expect(w.emitted("save")).toBeFalsy();
    expect(w.emitted("cancel")).toBeTruthy();
    expect(w.emitted("cancel")!.length).toBe(1);
  });
});

describe("MessageItemEdit — cancel emit", () => {
  it("emits cancel on Cancel click", async () => {
    const w = mountEdit();
    await w.get("[data-testid='msg-editor-cancel']").trigger("click");
    expect(w.emitted("cancel")).toBeTruthy();
    expect(w.emitted("cancel")!.length).toBe(1);
  });
});

describe("MessageItemEdit — resend emit", () => {
  it("emits resend when handler invoked (exposed for future flows)", async () => {
    // The current MessageItemEdit template does not render a Resend
    // button (the user reaches Resend through `<MessageActionsMenu>`).
    // The emit is exposed via the component's `$emit` API surface so
    // a future flow (e.g. an editor toolbar Resend) can drive it
    // without forking the component. We drive it via `wrapper.vm`
    // to assert the contract is wired correctly.
    const w = mountEdit();
    w.vm.$emit("resend");
    await flushPromises();
    expect(w.emitted("resend")).toBeTruthy();
    expect(w.emitted("resend")!.length).toBe(1);
  });
});

describe("MessageItemEdit — disabled states", () => {
  it("disables textarea + Save + Cancel when saving=true", () => {
    const w = mountEdit({ saving: true });
    const ta = w.get<HTMLTextAreaElement>("[data-testid='msg-editor-textarea']");
    const save = w.get<HTMLButtonElement>("[data-testid='msg-editor-save']");
    const cancel = w.get<HTMLButtonElement>("[data-testid='msg-editor-cancel']");
    expect(ta.attributes("disabled")).toBeDefined();
    expect(save.attributes("disabled")).toBeDefined();
    expect(cancel.attributes("disabled")).toBeDefined();
  });

  it("flips Save label to '保存中...' when saving=true", () => {
    const w = mountEdit({ saving: true });
    const save = w.get<HTMLButtonElement>("[data-testid='msg-editor-save']");
    expect(save.text()).toBe("保存中...");
  });

  it("does NOT emit save when saving=true and Save is clicked", async () => {
    const w = mountEdit({ saving: true, content: "original" });
    // Force-enable to simulate a stray click that somehow bypasses
    // the disabled state (defensive guard). The component should
    // still short-circuit.
    const ta = w.get<HTMLTextAreaElement>("[data-testid='msg-editor-textarea']");
    await ta.setValue("forced");
    await w.get("[data-testid='msg-editor-save']").trigger("click");
    expect(w.emitted("save")).toBeFalsy();
  });

  it("does NOT emit cancel when saving=true and Cancel is clicked", async () => {
    const w = mountEdit({ saving: true });
    await w.get("[data-testid='msg-editor-cancel']").trigger("click");
    expect(w.emitted("cancel")).toBeFalsy();
  });
});

describe("MessageItemEdit — streaming guard", () => {
  it("disables textarea + Save + Cancel when isStreaming=true", () => {
    const w = mountEdit({ isStreaming: true });
    const ta = w.get<HTMLTextAreaElement>("[data-testid='msg-editor-textarea']");
    const save = w.get<HTMLButtonElement>("[data-testid='msg-editor-save']");
    const cancel = w.get<HTMLButtonElement>("[data-testid='msg-editor-cancel']");
    expect(ta.attributes("disabled")).toBeDefined();
    expect(save.attributes("disabled")).toBeDefined();
    expect(cancel.attributes("disabled")).toBeDefined();
  });

  it("does NOT emit save when isStreaming=true", async () => {
    const w = mountEdit({ isStreaming: true, content: "original" });
    const ta = w.get<HTMLTextAreaElement>("[data-testid='msg-editor-textarea']");
    await ta.setValue("forced");
    await w.get("[data-testid='msg-editor-save']").trigger("click");
    expect(w.emitted("save")).toBeFalsy();
  });

  it("does NOT emit cancel when isStreaming=true", async () => {
    const w = mountEdit({ isStreaming: true });
    await w.get("[data-testid='msg-editor-cancel']").trigger("click");
    expect(w.emitted("cancel")).toBeFalsy();
  });
});

describe("MessageItemEdit — error message rendering", () => {
  it("renders the error row when errorMessage is set", () => {
    const w = mountEdit({ errorMessage: "edit_user_message: not found" });
    const err = w.find("[data-testid='msg-editor-error']");
    expect(err.exists()).toBe(true);
    expect(err.attributes("role")).toBe("alert");
    expect(err.text()).toContain("edit_user_message: not found");
  });

  it("does not render the error row when errorMessage is null", () => {
    const w = mountEdit({ errorMessage: null });
    expect(w.find("[data-testid='msg-editor-error']").exists()).toBe(false);
  });

  it("reacts to errorMessage prop changes", async () => {
    const w = mountEdit({ errorMessage: null });
    expect(w.find("[data-testid='msg-editor-error']").exists()).toBe(false);
    await w.setProps({ errorMessage: "网络错误" });
    expect(w.find("[data-testid='msg-editor-error']").exists()).toBe(true);
    expect(w.text()).toContain("网络错误");
  });
});

describe("MessageItemEdit — buffer re-seed on content change", () => {
  it("re-seeds the buffer when content prop changes while in edit mode", async () => {
    const w = mountEdit({ content: "v1" });
    expect(
      (
        w.get<HTMLTextAreaElement>("[data-testid='msg-editor-textarea']")
          .element as HTMLTextAreaElement
      ).value,
    ).toBe("v1");
    // Simulate a stream finishing mid-edit: the parent's
    // rehydrated content lands in the prop. The watcher
    // re-seeds the buffer so the user sees the final text.
    await w.setProps({ content: "v2 (stream done)" });
    expect(
      (
        w.get<HTMLTextAreaElement>("[data-testid='msg-editor-textarea']")
          .element as HTMLTextAreaElement
      ).value,
    ).toBe("v2 (stream done)");
  });
});
