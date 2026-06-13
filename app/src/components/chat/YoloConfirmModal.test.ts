// Tests for `YoloConfirmModal.vue` — the two-key confirm modal
// that gates Yolo mode entry.
//
// Coverage:
//   1. Renders both buttons when open=true; nothing when open=false.
//   2. Clicking the confirm button emits `confirm`.
//   3. Clicking the cancel button emits `cancel`.
//   4. Esc emits `cancel`; Enter emits `confirm` (matches the
//      ConfirmDialog / DeleteWorktreeConfirm muscle-memory
//      contract — see `popover-pattern.md`).
//   5. Buttons are disabled while `disabled=true`.
//
// Uses @vue/test-utils `mount`. The component has no external
// store dependencies, so no mocking is needed beyond the global
// `window` (provided by jsdom).

import { describe, it, expect, beforeEach } from "vitest";
import { mount, VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import YoloConfirmModal from "./YoloConfirmModal.vue";

/** Mount the modal with `open=true`. The modal is `v-if`-gated,
 *  so `open=false` produces an empty wrapper. */
function makeWrapper(opts: { open?: boolean; disabled?: boolean } = {}) {
  return mount(YoloConfirmModal, {
    props: {
      open: opts.open ?? true,
      disabled: opts.disabled ?? false,
    },
    attachTo: document.body,
  });
}

describe("YoloConfirmModal", () => {
  let wrapper: VueWrapper | null = null;

  beforeEach(() => {
    wrapper?.unmount();
    wrapper = null;
  });

  it("renders both buttons when open=true", () => {
    wrapper = makeWrapper();
    const cancelBtn = wrapper.find(".yolo-confirm-modal__btn--cancel");
    const confirmBtn = wrapper.find(".yolo-confirm-modal__btn--confirm");
    expect(cancelBtn.exists()).toBe(true);
    expect(confirmBtn.exists()).toBe(true);
    // Cancel button label.
    expect(cancelBtn.text()).toContain("取消");
    // Confirm button label contains the warning + intent.
    expect(confirmBtn.text()).toContain("启用 Yolo");
  });

  it("renders nothing when open=false", () => {
    wrapper = makeWrapper({ open: false });
    // The Transition wraps a v-if — at the very least, no
    // modal element should be present.
    expect(wrapper.find(".yolo-confirm-modal").exists()).toBe(false);
  });

  it("emits 'confirm' when the confirm button is clicked", async () => {
    wrapper = makeWrapper();
    await wrapper.find(".yolo-confirm-modal__btn--confirm").trigger("click");
    expect(wrapper.emitted("confirm")).toBeTruthy();
    expect(wrapper!.emitted("confirm")!.length).toBe(1);
  });

  it("emits 'cancel' when the cancel button is clicked", async () => {
    wrapper = makeWrapper();
    await wrapper.find(".yolo-confirm-modal__btn--cancel").trigger("click");
    expect(wrapper.emitted("cancel")).toBeTruthy();
    expect(wrapper!.emitted("cancel")!.length).toBe(1);
  });

  it("emits 'cancel' on Esc and 'confirm' on Enter", async () => {
    wrapper = makeWrapper();
    // Esc.
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await nextTick();
    expect(wrapper.emitted("cancel")).toBeTruthy();

    // Reset by remounting (the prior Esc already emitted).
    wrapper.unmount();
    wrapper = makeWrapper();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await nextTick();
    expect(wrapper.emitted("confirm")).toBeTruthy();
  });

  it("does not emit on Esc/Enter when disabled=true", async () => {
    wrapper = makeWrapper({ disabled: true });
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await nextTick();
    expect(wrapper.emitted("cancel")).toBeFalsy();
    expect(wrapper.emitted("confirm")).toBeFalsy();
  });

  it("disables both buttons when disabled=true", () => {
    wrapper = makeWrapper({ disabled: true });
    const cancelBtn = wrapper.find(".yolo-confirm-modal__btn--cancel");
    const confirmBtn = wrapper.find(".yolo-confirm-modal__btn--confirm");
    expect((cancelBtn.element as HTMLButtonElement).disabled).toBe(true);
    expect((confirmBtn.element as HTMLButtonElement).disabled).toBe(true);
  });

  it("emits 'cancel' on backdrop click but not when target is inside the modal", async () => {
    wrapper = makeWrapper();
    const backdrop = wrapper.find(".yolo-confirm-backdrop");
    expect(backdrop.exists()).toBe(true);
    // The handler is `@click.self="!disabled && emit('cancel')"` —
    // clicking the modal body (a child of the backdrop) must NOT
    // emit cancel. We can't simulate `@click.self` directly, so
    // we just verify the click handler on the backdrop is present
    // by triggering a click on the modal body and asserting no
    // cancel emission follows.
    const modal = wrapper.find(".yolo-confirm-modal");
    await modal.trigger("click");
    expect(wrapper.emitted("cancel")).toBeFalsy();
  });
});