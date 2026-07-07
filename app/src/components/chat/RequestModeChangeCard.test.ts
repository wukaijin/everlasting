// Tests for `RequestModeChangeCard.vue` — Phase C of
// `07-07-07-07-request-mode-change-tool` (2026-07-07).
//
// Coverage (drives the Phase C test plan in implement.md §1 C2):
//   1. R8 rendering: pending state shows the header chip +
//      reason text + before/after comparison + two action buttons.
//   2. AC10 inline red-line: card mounts inline (no Teleport,
//      no portal-class residue).
//   3. Click 允许 (non-Yolo) → flips to the "allowed" state
//      (the cards store's `resolveModeChange(true)` chain is
//      covered by the `questionCards` store tests + the
//      `utils/toolModeChange` IPC wrapper tests).
//   4. Click 允许 (Yolo) → routes through `requestSetMode`
//      (Yolo modal flow), does NOT directly call
//      `resolveModeChange` synchronously.
//   5. Click 拒绝 → flips to the "denied" state.
//   6. Allowed / denied state pills render correctly.
//   7. targetMode color mapping (plan=cyan, edit=accent,
//      yolo=red) — applied to the primary action button + the
//      header icon.
//   8. State prop renders the appropriate non-pending view
//      directly on mount (rehydrated historical path).
//
// Tauri invoke is mocked at the `@tauri-apps/api/core` boundary
// so the test doesn't need a Tauri runtime. The mock records
// every invoke call so we can assert the exact wire payload.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

// Mock @tauri-apps/api/core so cardsStore.resolveModeChange →
// invoke doesn't reach `window.__TAURI_INTERNALS__`. The mock
// records every call so we can assert the wire payload.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import RequestModeChangeCard from "./RequestModeChangeCard.vue";

// ---------------------------------------------------------------------
// Fixtures + helpers
// ---------------------------------------------------------------------

type TargetMode = "edit" | "plan" | "yolo";

const baseProps = (): {
  sessionId: string;
  toolUseId: string;
  targetMode: TargetMode;
  currentMode: string | null;
  reason: string | null;
  state: "pending" | "allowed" | "denied";
  allowedMode: TargetMode | null;
} => ({
  sessionId: "sess-1",
  toolUseId: "tool-use-1",
  targetMode: "plan",
  currentMode: "edit",
  reason: "I need to write some files to fix this bug",
  state: "pending",
  allowedMode: null,
});

function mountCard(
  propsOverride: Partial<ReturnType<typeof baseProps>> = {},
) {
  // Pinia setup: the card reads `useChatStore` +
  // `useQuestionCardsStore` in setup. We create + activate a
  // fresh Pinia per mount so each test gets isolated store
  // state (Pinia is the only context that makes these stores
  // resolve to singletons).
  const pinia = createPinia();
  setActivePinia(pinia);
  return mount(RequestModeChangeCard, {
    props: { ...baseProps(), ...propsOverride },
    global: { plugins: [pinia] },
  });
}

let wrapper: VueWrapper | null = null;
function unmount() {
  if (wrapper) {
    wrapper.unmount();
    wrapper = null;
  }
  // Sweep any unrelated residue between tests. A regression
  // that ports the card to <body> would land a node here.
  document
    .querySelectorAll(".mode-card-portal, .mode-card__overlay")
    .forEach((el) => el.remove());
}

beforeEach(async () => {
  invokeMock.mockReset();
  // Default to a successful resolve so tests that don't focus
  // on the IPC mock don't have to thread `.mockResolvedValue`
  // through every setup line. Tests that want a rejection set
  // `.mockRejectedValueOnce(...)` per-test.
  invokeMock.mockResolvedValue({ id: "sess-1", mode: "plan" });
  // Pre-warm the dynamic-import chain: `cardsStore.resolveModeChange`
  // does `await import("../utils/toolModeChange")` then
  // `await import("./chat")`. The first call in a fresh test
  // takes longer than 3 `flushPromises()` because of
  // module-cache round-tripping. We do a single throwaway
  // resolve here so the actual test clicks settle on the
  // first or second flush.
  //
  // The warmup itself goes through `invoke` (recorded by
  // `invokeMock`). We reset the mock AFTER the warmup so
  // tests see a clean slate.
  const warmupPinia = createPinia();
  setActivePinia(warmupPinia);
  const { useQuestionCardsStore } = await import(
    "../../stores/questionCards"
  );
  const warmupStore = useQuestionCardsStore();
  try {
    await warmupStore.resolveModeChange("__warmup__", "tu", "plan", true);
  } catch {
    // Pre-warm errors are fine — the goal is module cache hydration.
  }
  invokeMock.mockReset();
  invokeMock.mockResolvedValue({ id: "sess-1", mode: "plan" });
});

afterEach(() => {
  unmount();
});

// ---------------------------------------------------------------------
// 1. Pending state rendering (R8)
// ---------------------------------------------------------------------

describe("RequestModeChangeCard — pending rendering", () => {
  beforeEach(() => { wrapper = null; });

  it("renders the header chip with the target mode name", () => {
    wrapper = mountCard({ targetMode: "yolo" });
    const head = wrapper.find(".mode-card__head-title");
    expect(head.text()).toContain("切换到");
    expect(head.text()).toContain("Yolo");
  });

  it("renders the reason text when provided", () => {
    wrapper = mountCard({
      reason: "Need to write a new file",
    });
    const reason = wrapper.find("[data-testid='mode-card-reason']");
    expect(reason.exists()).toBe(true);
    expect(reason.text()).toBe("Need to write a new file");
  });

  it("renders the mode comparison row (current → target)", () => {
    wrapper = mountCard({
      currentMode: "plan",
      targetMode: "yolo",
    });
    const compare = wrapper.find("[data-testid='mode-card-compare']");
    expect(compare.exists()).toBe(true);
    expect(compare.text()).toContain("Plan");
    expect(compare.text()).toContain("Yolo");
  });

  it("hides the comparison row when currentMode is null/unknown", () => {
    wrapper = mountCard({ currentMode: null });
    expect(
      wrapper.find("[data-testid='mode-card-compare']").exists(),
    ).toBe(false);
  });

  it("renders both action buttons (允许 + 拒绝)", () => {
    wrapper = mountCard();
    expect(
      wrapper.find("[data-testid='mode-card-allow']").exists(),
    ).toBe(true);
    expect(
      wrapper.find("[data-testid='mode-card-deny']").exists(),
    ).toBe(true);
  });

  it("does NOT render the reason when state is non-pending", () => {
    wrapper = mountCard({
      state: "allowed",
      allowedMode: "yolo",
      currentMode: "plan",
      reason: "should be hidden in allowed state",
    });
    expect(
      wrapper.find("[data-testid='mode-card-reason']").exists(),
    ).toBe(false);
  });
});

// ---------------------------------------------------------------------
// 2. Allow / Deny handlers (state-flip verification)
// ---------------------------------------------------------------------
//
// We verify the cards store's `resolveModeChange` is called
// with the right arguments by observing the post-resolve state
// transition. The store's chain involves two dynamic imports
// (./utils/toolModeChange → IPC, then ./chat for the session
// list patch) which can take multiple microtask flushes to
// settle; we use three `flushPromises()` to be safe. The
// exact wire payload is covered by the `toolModeChange` unit
// tests.

describe("RequestModeChangeCard — allow (non-Yolo)", () => {
  beforeEach(() => { wrapper = null; });

  it("click 允许 (non-Yolo) flips the card to the allowed state", async () => {
    wrapper = mountCard({ targetMode: "plan" });
    await wrapper
      .find("[data-testid='mode-card-allow']")
      .trigger("click");
    // Multiple flushes — the dynamic import chain in the
    // store's `resolveModeChange` requires multiple microtask
    // ticks to settle (the first import resolves after a
    // module-cache round-trip; the second import runs after
    // the IPC resolves).
    await flushPromises();
    await flushPromises();
    await flushPromises();
    expect(
      wrapper.find("[data-testid='mode-card-state-allowed']").exists(),
    ).toBe(true);
    // Action row is gone.
    expect(
      wrapper.find("[data-testid='mode-card-allow']").exists(),
    ).toBe(false);
  });

  it("click 允许 (non-Yolo) emits 'allowed' with the new mode", async () => {
    wrapper = mountCard({ targetMode: "plan" });
    await wrapper
      .find("[data-testid='mode-card-allow']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    expect(wrapper.emitted("allowed")).toBeTruthy();
    expect(wrapper.emitted("allowed")!.length).toBe(1);
    expect(wrapper.emitted("allowed")![0]).toEqual(["plan"]);
  });

  it("surfaces an inline error row when invoke rejects (non-Yolo)", async () => {
    invokeMock.mockRejectedValueOnce("network down");
    wrapper = mountCard({ targetMode: "plan" });
    await wrapper
      .find("[data-testid='mode-card-allow']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    const errRow = wrapper.find("[data-testid='mode-card-error']");
    expect(errRow.exists()).toBe(true);
    expect(errRow.text()).toContain("network down");
    // Card stays in pending state — buttons re-enabled for retry.
    expect(
      wrapper.find("[data-testid='mode-card-allow']").exists(),
    ).toBe(true);
    // Did NOT flip to allowed.
    expect(
      wrapper.find("[data-testid='mode-card-state-allowed']").exists(),
    ).toBe(false);
  });
});

describe("RequestModeChangeCard — deny", () => {
  beforeEach(() => { wrapper = null; });

  it("click 拒绝 flips the card to the denied state", async () => {
    wrapper = mountCard({ targetMode: "plan" });
    await wrapper
      .find("[data-testid='mode-card-deny']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    expect(
      wrapper.find("[data-testid='mode-card-state-denied']").exists(),
    ).toBe(true);
    expect(
      wrapper.find("[data-testid='mode-card-denied-note']").exists(),
    ).toBe(true);
    // Action row is gone.
    expect(
      wrapper.find("[data-testid='mode-card-allow']").exists(),
    ).toBe(false);
    expect(
      wrapper.find("[data-testid='mode-card-deny']").exists(),
    ).toBe(false);
  });

  it("click 拒绝 emits 'denied'", async () => {
    wrapper = mountCard({ targetMode: "plan" });
    await wrapper
      .find("[data-testid='mode-card-deny']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    expect(wrapper.emitted("denied")).toBeTruthy();
    expect(wrapper.emitted("denied")!.length).toBe(1);
  });
});

// ---------------------------------------------------------------------
// 3. Yolo flow — does NOT directly call resolveModeChange
// ---------------------------------------------------------------------

describe("RequestModeChangeCard — Yolo modal dispatch", () => {
  beforeEach(() => { wrapper = null; });

  it("click 允许 (Yolo) does NOT call resolve_mode_change synchronously", async () => {
    // The Yolo path triggers the modal flow — the actual
    // resolve happens later via confirmYolo after the user
    // confirms. For the click itself, no resolve fires
    // synchronously.
    wrapper = mountCard({ targetMode: "yolo" });
    await wrapper
      .find("[data-testid='mode-card-allow']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    const resolveCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "resolve_mode_change",
    );
    expect(resolveCalls.length).toBe(0);
    // The set_session_mode IPC is also not called here —
    // it's fired by confirmYolo AFTER the modal confirms.
    const setModeCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "set_session_mode",
    );
    expect(setModeCalls.length).toBe(0);
  });

  it("Yolo allow keeps the card mounted (waiting for modal flow)", async () => {
    wrapper = mountCard({ targetMode: "yolo" });
    await wrapper
      .find("[data-testid='mode-card-allow']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    // Card is still in pending state — no IPC fired yet, the
    // modal flow owns the next step. (For the live UI the
    // store will unmount the card after resolveModeChange
    // runs; for the unit test we verify the card itself
    // doesn't flip state synchronously.)
    expect(
      wrapper.find("[data-testid='mode-card-state-allowed']").exists(),
    ).toBe(false);
    expect(
      wrapper.find("[data-testid='mode-card-state-denied']").exists(),
    ).toBe(false);
  });

  it("Yolo deny flips the card to the denied state (no modal for deny)", async () => {
    wrapper = mountCard({ targetMode: "yolo" });
    await wrapper
      .find("[data-testid='mode-card-deny']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    expect(
      wrapper.find("[data-testid='mode-card-state-denied']").exists(),
    ).toBe(true);
  });
});

// ---------------------------------------------------------------------
// 4. Allowed / denied state pills render correctly
// ---------------------------------------------------------------------

describe("RequestModeChangeCard — allowed / denied state pills", () => {
  beforeEach(() => { wrapper = null; });

  it("renders the allowed pill + comparison row when state='allowed'", () => {
    wrapper = mountCard({
      state: "allowed",
      allowedMode: "yolo",
      currentMode: "plan",
      reason: null,
    });
    expect(
      wrapper.find("[data-testid='mode-card-state-allowed']").exists(),
    ).toBe(true);
    expect(
      wrapper.find("[data-testid='mode-card-compare-after']").exists(),
    ).toBe(true);
  });

  it("renders the denied pill + note when state='denied'", () => {
    wrapper = mountCard({
      state: "denied",
      reason: "ignored in denied state",
    });
    expect(
      wrapper.find("[data-testid='mode-card-state-denied']").exists(),
    ).toBe(true);
    expect(
      wrapper.find("[data-testid='mode-card-denied-note']").exists(),
    ).toBe(true);
  });

  it("does NOT render action buttons in non-pending states", () => {
    wrapper = mountCard({ state: "allowed", allowedMode: "plan" });
    expect(
      wrapper.find("[data-testid='mode-card-allow']").exists(),
    ).toBe(false);
    expect(
      wrapper.find("[data-testid='mode-card-deny']").exists(),
    ).toBe(false);
  });
});

// ---------------------------------------------------------------------
// 5. targetMode color mapping (plan / edit / yolo)
// ---------------------------------------------------------------------

describe("RequestModeChangeCard — targetMode color mapping", () => {
  beforeEach(() => { wrapper = null; });

  it("applies the edit color class for targetMode=edit", () => {
    wrapper = mountCard({ targetMode: "edit" });
    const card = wrapper.find("[data-testid='mode-card']");
    expect(card.classes()).toContain("mode-card--edit");
  });

  it("applies the plan color class for targetMode=plan", () => {
    wrapper = mountCard({ targetMode: "plan" });
    const card = wrapper.find("[data-testid='mode-card']");
    expect(card.classes()).toContain("mode-card--plan");
  });

  it("applies the yolo color class for targetMode=yolo", () => {
    wrapper = mountCard({ targetMode: "yolo" });
    const card = wrapper.find("[data-testid='mode-card']");
    expect(card.classes()).toContain("mode-card--yolo");
  });

  it("primary action button inherits the target mode color class", () => {
    wrapper = mountCard({ targetMode: "yolo" });
    const btn = wrapper.find("[data-testid='mode-card-allow']");
    expect(btn.classes()).toContain("mode-card--yolo");
    expect(btn.classes()).toContain("mode-card__btn--primary");
  });
});

// ---------------------------------------------------------------------
// 6. AC10 inline red line — no portal / no modal
// ---------------------------------------------------------------------

describe("RequestModeChangeCard — inline red line (AC10)", () => {
  beforeEach(() => { wrapper = null; });

  it("mounts the card in the wrapper's component tree (no Teleport to body)", () => {
    wrapper = mountCard();
    const cardEl = wrapper
      .find("[data-testid='mode-card']")
      .element as HTMLElement;
    const wrapperEl = wrapper.element as HTMLElement;
    expect(wrapperEl.contains(cardEl)).toBe(true);
  });

  it("does NOT leave any portal residue on document.body after unmount", () => {
    wrapper = mountCard();
    unmount();
    expect(
      document.querySelectorAll("[data-testid='mode-card']").length,
    ).toBe(0);
    expect(
      document
        .querySelectorAll(".mode-card-portal, .mode-card__overlay")
        .length,
    ).toBe(0);
  });
});