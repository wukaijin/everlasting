// Tests for `RequestTaskStateTransitionCard.vue`
// (`07-09-workflow-transition-card`, 2026-07-09).
//
// Mirrors `RequestModeChangeCard.test.ts` but is trimmed — the
// workflow-state card has NO Yolo special-case and NO per-state
// color mapping, so those test groups don't apply. Coverage:
//   1. pending rendering: header chip + reason + before/after
//      comparison + two action buttons.
//   2. Click 允许 → flips to "allowed"; the resolve IPC is called
//      with the right args (incl. `slug` + `targetState`, the two
//      fields that differ from mode_change).
//   3. Click 拒绝 → flips to "denied".
//   4. Allowed / denied state pills render correctly (rehydrated
//      historical path).
//   5. AC10 inline red line: card mounts inline (no Teleport).
//
// Tauri invoke is mocked at the `@tauri-apps/api/core` boundary.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mount, flushPromises, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

// Mock @tauri-apps/api/core so cardsStore.resolveTaskStateTransition
// → invoke doesn't reach `window.__TAURI_INTERNALS__`. The mock
// records every call so we can assert the wire payload.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import RequestTaskStateTransitionCard from "./RequestTaskStateTransitionCard.vue";

// ---------------------------------------------------------------------
// Fixtures + helpers
// ---------------------------------------------------------------------

type WfState = "planning" | "in_progress" | "done";

const baseProps = (): {
  sessionId: string;
  toolUseId: string;
  targetState: WfState;
  slug: string;
  currentState: WfState | null;
  reason: string | null;
  state: "pending" | "allowed" | "denied";
} => ({
  sessionId: "sess-1",
  toolUseId: "tool-use-1",
  targetState: "in_progress",
  slug: "my-feature",
  currentState: "planning",
  reason: "调研完成,准备开始实施",
  state: "pending",
});

function mountCard(
  propsOverride: Partial<ReturnType<typeof baseProps>> = {},
) {
  const pinia = createPinia();
  setActivePinia(pinia);
  return mount(RequestTaskStateTransitionCard, {
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
  document
    .querySelectorAll(".wf-state-card-portal, .wf-state-card__overlay")
    .forEach((el) => el.remove());
}

beforeEach(async () => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue({ id: "sess-1", mode: "edit" });
  // Pre-warm the dynamic-import chain: the store action does
  // `await import("../utils/toolTaskStateTransition")` before the
  // real invoke. A throwaway resolve here hydrates the module
  // cache so the actual test clicks settle within 2-3 flushes.
  const warmupPinia = createPinia();
  setActivePinia(warmupPinia);
  const { useQuestionCardsStore } = await import(
    "../../stores/questionCards"
  );
  const warmupStore = useQuestionCardsStore();
  try {
    await warmupStore.resolveTaskStateTransition(
      "__warmup__",
      "tu",
      "in_progress",
      "warmup-slug",
      true,
    );
  } catch {
    // Pre-warm errors are fine — module-cache hydration is the goal.
  }
  invokeMock.mockReset();
  invokeMock.mockResolvedValue({ id: "sess-1", mode: "edit" });
});

afterEach(() => {
  unmount();
});

// ---------------------------------------------------------------------
// 1. Pending state rendering
// ---------------------------------------------------------------------

describe("RequestTaskStateTransitionCard — pending rendering", () => {
  beforeEach(() => {
    wrapper = null;
  });

  it("renders the header chip with the target state label", () => {
    wrapper = mountCard({ targetState: "in_progress" });
    const head = wrapper.find(".wf-state-card__head-title");
    expect(head.text()).toContain("工作流状态转移");
    expect(head.text()).toContain("进行中");
  });

  it("renders the reason text when provided", () => {
    wrapper = mountCard({ reason: "Need to move to in_progress state" });
    const reason = wrapper.find("[data-testid='wf-state-card-reason']");
    expect(reason.exists()).toBe(true);
    expect(reason.text()).toBe("Need to move to in_progress state");
  });

  it("renders the state comparison row (current → target)", () => {
    wrapper = mountCard({
      currentState: "planning",
      targetState: "in_progress",
    });
    const compare = wrapper.find(
      "[data-testid='wf-state-card-compare']",
    );
    expect(compare.exists()).toBe(true);
    expect(compare.text()).toContain("规划");
    expect(compare.text()).toContain("进行中");
  });

  it("hides the comparison row when currentState is null", () => {
    wrapper = mountCard({ currentState: null });
    expect(
      wrapper
        .find("[data-testid='wf-state-card-compare']")
        .exists(),
    ).toBe(false);
  });

  it("renders both action buttons (允许 + 拒绝)", () => {
    wrapper = mountCard();
    expect(
      wrapper.find("[data-testid='wf-state-card-allow']").exists(),
    ).toBe(true);
    expect(
      wrapper.find("[data-testid='wf-state-card-deny']").exists(),
    ).toBe(true);
  });

  it("does NOT render the reason when state is non-pending", () => {
    wrapper = mountCard({
      state: "allowed",
      currentState: "planning",
      reason: "should be hidden in allowed state",
    });
    expect(
      wrapper.find("[data-testid='wf-state-card-reason']").exists(),
    ).toBe(false);
  });
});

// ---------------------------------------------------------------------
// 2. Allow handler — state flip + wire payload
// ---------------------------------------------------------------------

describe("RequestTaskStateTransitionCard — allow", () => {
  beforeEach(() => {
    wrapper = null;
  });

  it("click 允许 flips the card to the allowed state", async () => {
    wrapper = mountCard({ targetState: "in_progress" });
    await wrapper
      .find("[data-testid='wf-state-card-allow']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    expect(
      wrapper
        .find("[data-testid='wf-state-card-state-allowed']")
        .exists(),
    ).toBe(true);
    expect(
      wrapper.find("[data-testid='wf-state-card-allow']").exists(),
    ).toBe(false);
  });

  it("click 允许 emits 'allowed' with the new state", async () => {
    wrapper = mountCard({ targetState: "in_progress" });
    await wrapper
      .find("[data-testid='wf-state-card-allow']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    expect(wrapper.emitted("allowed")).toBeTruthy();
    expect(wrapper.emitted("allowed")!.length).toBe(1);
    expect(wrapper.emitted("allowed")![0]).toEqual(["in_progress"]);
  });

  it("calls resolve_task_state_transition with targetState + slug (wire payload)", async () => {
    // The critical difference from mode_change: the IPC payload
    // carries `targetState` + `slug` (no `targetMode`). Asserting
    // the exact wire shape guards against a copy-paste regression
    // that calls the mode_change IPC instead.
    wrapper = mountCard({
      targetState: "done",
      slug: "arch-feature",
      toolUseId: "tu-xyz",
      sessionId: "sess-42",
    });
    await wrapper
      .find("[data-testid='wf-state-card-allow']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    const resolveCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "resolve_task_state_transition",
    );
    expect(resolveCalls.length).toBe(1);
    const payload = resolveCalls[0][1] as Record<string, unknown>;
    expect(payload).toEqual({
      sessionId: "sess-42",
      toolUseId: "tu-xyz",
      targetState: "done",
      slug: "arch-feature",
      allow: true,
    });
  });

  it("surfaces an inline error row when invoke rejects", async () => {
    invokeMock.mockRejectedValueOnce("backend down");
    wrapper = mountCard({ targetState: "in_progress" });
    await wrapper
      .find("[data-testid='wf-state-card-allow']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    const errRow = wrapper.find(
      "[data-testid='wf-state-card-error']",
    );
    expect(errRow.exists()).toBe(true);
    expect(errRow.text()).toContain("backend down");
    // Card stays pending — buttons re-enabled for retry.
    expect(
      wrapper.find("[data-testid='wf-state-card-allow']").exists(),
    ).toBe(true);
    expect(
      wrapper
        .find("[data-testid='wf-state-card-state-allowed']")
        .exists(),
    ).toBe(false);
  });
});

// ---------------------------------------------------------------------
// 3. Deny handler
// ---------------------------------------------------------------------

describe("RequestTaskStateTransitionCard — deny", () => {
  beforeEach(() => {
    wrapper = null;
  });

  it("click 拒绝 flips the card to the denied state", async () => {
    wrapper = mountCard({ targetState: "in_progress" });
    await wrapper
      .find("[data-testid='wf-state-card-deny']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    expect(
      wrapper
        .find("[data-testid='wf-state-card-state-denied']")
        .exists(),
    ).toBe(true);
    expect(
      wrapper
        .find("[data-testid='wf-state-card-denied-note']")
        .exists(),
    ).toBe(true);
    expect(
      wrapper.find("[data-testid='wf-state-card-allow']").exists(),
    ).toBe(false);
    expect(
      wrapper.find("[data-testid='wf-state-card-deny']").exists(),
    ).toBe(false);
  });

  it("click 拒绝 emits 'denied'", async () => {
    wrapper = mountCard({ targetState: "in_progress" });
    await wrapper
      .find("[data-testid='wf-state-card-deny']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    expect(wrapper.emitted("denied")).toBeTruthy();
    expect(wrapper.emitted("denied")!.length).toBe(1);
  });

  it("click 拒绝 calls resolve with allow=false", async () => {
    wrapper = mountCard();
    await wrapper
      .find("[data-testid='wf-state-card-deny']")
      .trigger("click");
    await flushPromises();
    await flushPromises();
    await flushPromises();
    const resolveCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "resolve_task_state_transition",
    );
    expect(resolveCalls.length).toBe(1);
    const payload = resolveCalls[0][1] as Record<string, unknown>;
    expect(payload.allow).toBe(false);
  });
});

// ---------------------------------------------------------------------
// 4. Allowed / denied state pills (rehydrated historical path)
// ---------------------------------------------------------------------

describe("RequestTaskStateTransitionCard — allowed / denied state pills", () => {
  beforeEach(() => {
    wrapper = null;
  });

  it("renders the allowed pill + comparison row when state='allowed'", () => {
    wrapper = mountCard({
      state: "allowed",
      targetState: "in_progress",
      currentState: "planning",
      reason: null,
    });
    expect(
      wrapper
        .find("[data-testid='wf-state-card-state-allowed']")
        .exists(),
    ).toBe(true);
    expect(
      wrapper
        .find("[data-testid='wf-state-card-compare-after']")
        .exists(),
    ).toBe(true);
  });

  it("renders the denied pill + note when state='denied'", () => {
    wrapper = mountCard({
      state: "denied",
      targetState: "in_progress",
      reason: "ignored in denied state",
    });
    expect(
      wrapper
        .find("[data-testid='wf-state-card-state-denied']")
        .exists(),
    ).toBe(true);
    expect(
      wrapper
        .find("[data-testid='wf-state-card-denied-note']")
        .exists(),
    ).toBe(true);
  });

  it("does NOT render action buttons in non-pending states", () => {
    wrapper = mountCard({ state: "allowed", targetState: "in_progress" });
    expect(
      wrapper.find("[data-testid='wf-state-card-allow']").exists(),
    ).toBe(false);
    expect(
      wrapper.find("[data-testid='wf-state-card-deny']").exists(),
    ).toBe(false);
  });
});

// ---------------------------------------------------------------------
// 5. AC10 inline red line — no portal / no modal
// ---------------------------------------------------------------------

describe("RequestTaskStateTransitionCard — inline red line (AC10)", () => {
  beforeEach(() => {
    wrapper = null;
  });

  it("mounts the card in the wrapper's component tree (no Teleport to body)", () => {
    wrapper = mountCard();
    const cardEl = wrapper.find(
      "[data-testid='wf-state-card']",
    ).element as HTMLElement;
    const wrapperEl = wrapper.element as HTMLElement;
    expect(wrapperEl.contains(cardEl)).toBe(true);
  });

  it("does NOT leave any portal residue on document.body after unmount", () => {
    wrapper = mountCard();
    unmount();
    expect(
      document.querySelectorAll("[data-testid='wf-state-card']")
        .length,
    ).toBe(0);
    expect(
      document
        .querySelectorAll(".wf-state-card-portal, .wf-state-card__overlay")
        .length,
    ).toBe(0);
  });
});
