// Tests for `stores/settingsModal.ts` — the global "open Settings"
// channel (N1 onboarding, 2026-09-15).
//
// Coverage:
//   1. Initial state: closed, no one-shot category.
//   2. openSettings() without a category: flips open only, the
//      one-shot slot stays null (plain gear-button path unchanged).
//   3. openSettings('providers'): open + one-shot slot set together.
//   4. consumeInitialCategory: returns the current slot AND clears
//      it (one-shot semantics — a second consume sees null).
//   5. Consecutive openSettings calls: the later slot wins.

import { describe, it, expect, beforeEach } from "vitest";
import { createPinia, setActivePinia } from "pinia";

import { useSettingsModalStore } from "./settingsModal";

describe("settingsModal store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("初始态:关 + 无一次性落点", () => {
    const store = useSettingsModalStore();
    expect(store.open).toBe(false);
    expect(store.initialCategory).toBeNull();
  });

  it("openSettings() 无参:只翻 open,不设落点(普通入口行为)", () => {
    const store = useSettingsModalStore();
    store.openSettings();
    expect(store.open).toBe(true);
    expect(store.initialCategory).toBeNull();
  });

  it("openSettings('providers'):open + 落点同时设置", () => {
    const store = useSettingsModalStore();
    store.openSettings("providers");
    expect(store.open).toBe(true);
    expect(store.initialCategory).toBe("providers");
  });

  it("consumeInitialCategory:返回当前值并清空(一次性)", () => {
    const store = useSettingsModalStore();
    store.openSettings("models");
    expect(store.consumeInitialCategory()).toBe("models");
    expect(store.initialCategory).toBeNull();
    expect(store.consumeInitialCategory()).toBeNull();
  });

  it("连续 openSettings:后一次覆盖前一次落点", () => {
    const store = useSettingsModalStore();
    store.openSettings("providers");
    store.openSettings("models");
    expect(store.initialCategory).toBe("models");
  });
});
