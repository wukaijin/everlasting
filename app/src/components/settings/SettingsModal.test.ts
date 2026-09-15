// Tests for the SettingsModal `initialCategory` one-shot landing
// channel (N1 onboarding, 2026-09-15).
//
// Coverage:
//   1. Guided open (initialCategory="models"): the nav lands on the
//      Models category this time, skipping the localStorage restore.
//   2. The one-shot does NOT pollute the "last visited" memory —
//      localStorage keeps its previous entry, and a subsequent plain
//      open restores it (not the guided category).
//   3. Invalid one-shot id: falls back to the normal restore path,
//      still consumed (emitted) so no stale value lingers.
//   4. Plain open (no initialCategory): restores the saved category
//      exactly as before.
//
// Test notes: reka-ui DialogPortal teleports the content to
// <body>, so assertions query `document` rather than the wrapper
// (SearchModal.test.ts precedent). The portal residue is swept in
// afterEach to prevent cross-test leaks. Transport is mocked — the
// modal itself issues no loads on open beyond the projects-store
// fallback.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mount, flushPromises, VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import SettingsModal from "./SettingsModal.vue";

const NAV_LS_KEY = "everlasting.settingsNav";

function activeNavText(): string {
  const el = document.querySelector(".settings-modal__nav-item--active");
  return el?.textContent?.trim() ?? "";
}

describe("SettingsModal — initialCategory one-shot landing", () => {
  let wrapper: VueWrapper | null = null;

  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);
    localStorage.clear();
    wrapper = null;
  });

  afterEach(() => {
    if (wrapper) {
      wrapper.unmount();
      wrapper = null;
    }
    // reka-ui portals to <body>; sweep residue so the next test's
    // `.settings-modal__nav-item--active` query only sees its own DOM.
    document
      .querySelectorAll(".settings-modal__overlay, .settings-modal")
      .forEach((el) => el.remove());
  });

  function mountModal(props: Record<string, unknown> = {}): VueWrapper {
    return mount(SettingsModal, {
      props: { open: false, ...props },
    });
  }

  it("guided open(initialCategory='models'):直落 Models 分类", async () => {
    wrapper = mountModal({ initialCategory: "models" });
    await wrapper.setProps({ open: true });
    await flushPromises();
    expect(activeNavText()).toBe("Models");
    expect(wrapper.emitted("initial-category-consumed")).toBeTruthy();
  });

  it("一次性落点不污染「上次停留」记忆:localStorage 保持原值,普通打开恢复原分类", async () => {
    localStorage.setItem(
      NAV_LS_KEY,
      JSON.stringify({ scope: "global", id: "general" }),
    );
    wrapper = mountModal({ initialCategory: "models" });
    await wrapper.setProps({ open: true });
    expect(activeNavText()).toBe("Models");
    // 消费后 store 清空(Sidebar 侧的回写以 prop 变 null 表达)。
    await wrapper.setProps({ initialCategory: null, open: false });
    await wrapper.setProps({ open: true });
    await flushPromises();
    // 普通打开:恢复 localStorage 记忆的 general,而非引导落点的 models。
    expect(activeNavText()).toBe("通用");
    expect(JSON.parse(localStorage.getItem(NAV_LS_KEY) ?? "null")).toEqual({
      scope: "global",
      id: "general",
    });
  });

  it("无效 initialCategory id:回退正常恢复,仍 emit 消费(无残留)", async () => {
    localStorage.setItem(
      NAV_LS_KEY,
      JSON.stringify({ scope: "global", id: "providers" }),
    );
    wrapper = mountModal({ initialCategory: "nonexistent" });
    await wrapper.setProps({ open: true });
    expect(activeNavText()).toBe("Providers");
    expect(wrapper.emitted("initial-category-consumed")).toBeTruthy();
  });

  it("普通打开(无 initialCategory):恢复 localStorage 记忆(既有行为)", async () => {
    localStorage.setItem(
      NAV_LS_KEY,
      JSON.stringify({ scope: "global", id: "disk" }),
    );
    wrapper = mountModal();
    await wrapper.setProps({ open: true });
    expect(activeNavText()).toBe("磁盘");
    expect(wrapper.emitted("initial-category-consumed")).toBeFalsy();
  });
});
