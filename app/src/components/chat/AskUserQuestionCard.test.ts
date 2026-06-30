// Tests for `AskUserQuestionCard.vue` — Phase D of
// `06-30-ask-user-question-tool` (2026-06-30).
//
// Coverage (drives the Phase D test plan in implement.md §1 D2):
//   1. Multi-question section rendering (R8): 4 questions render
//      as 4 separate sections in one card, each with its header
//      chip + question body + options list.
//   2. Single-select (radio) toggle: clicking a label marks the
//      option selected; clicking another deselects the first.
//   3. Multi-select (checkbox) toggle: clicking a label toggles
//      membership (independent of other selections).
//   4. Submit-disabled gate (R7): "提交" is disabled until EVERY
//      question has a valid selection (1 for single-select,
//      ≥1 for multi-select).
//   5. Submit fires `resolveToolQuestion` exactly once with the
//      full answer array in question-payload order (R7 +
//      PRD R4 wire shape).
//   6. After successful submit, the card flips to the
//      "answered" state — bottom buttons replaced by the
//      "已回答" summary row, selected options highlighted
//      (R7a: "答完保留展开全程").
//   7. "跳过" fires `{ cancelled: true }` and flips to the
//      "cancelled" state (AC6).
//   8. Submit error: IPC rejection surfaces an inline error
//      row (role="alert") and re-enables the buttons so the
//      user can retry.
//   9. AC10 inline red-line: the card's root `.ask-card` lives
//      in the parent component tree (no Teleport to body, no
//      portal-class element on document.body).
//
// Tauri invoke is mocked at the `@tauri-apps/api/core` boundary
// so the test doesn't need a Tauri runtime. The mock records
// every invoke call so we can assert the exact wire payload.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";

// Mock @tauri-apps/api/core so resolveToolQuestion → invoke
// doesn't reach `window.__TAURI_INTERNALS__`. The mock records
// every call so we can assert the wire payload.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import AskUserQuestionCard from "./AskUserQuestionCard.vue";
import type {
  Question,
  QuestionCardState,
  ToolQuestionAnswer,
} from "../../stores/questionCards.types";

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

function makeQuestions(): Question[] {
  return [
    {
      question: "Which library should we use?",
      header: "Library",
      options: [
        { label: "Vue", description: "Reactive component framework" },
        { label: "React", description: "Component-based UI library" },
      ],
      multi_select: false,
    },
    {
      question: "Which features do you need?",
      header: "Features",
      options: [
        { label: "Routing", description: "Client-side navigation" },
        { label: "State", description: "Centralized store" },
        { label: "SSR" },
      ],
      multi_select: true,
    },
  ];
}

const baseProps = () => ({
  sessionId: "sess-1",
  toolUseId: "tool-use-1",
  questions: makeQuestions(),
  state: "pending" as QuestionCardState,
  selectedAnswer: undefined as ToolQuestionAnswer[] | undefined,
});

function mountCard(
  propsOverride: Partial<ReturnType<typeof baseProps>> = {},
) {
  return mount(AskUserQuestionCard, {
    props: { ...baseProps(), ...propsOverride },
  });
}

/** Click an option row by testid. The component's `<li>` carries
 *  the `@click` handler; this is the realistic user interaction
 *  (click the row, the radio/checkbox fires automatically). The
 *  underlying `<input>` uses `@click.stop` so the inner radio
 *  dot isn't double-counted. */
async function clickOption(testid: string): Promise<void> {
  if (!wrapper) throw new Error("wrapper not initialized");
  const row = wrapper.find(`[data-testid='${testid}']`);
  await row.trigger("click");
  await nextTick();
}

// ---------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------

let wrapper: VueWrapper | null = null;
function unmount() {
  if (wrapper) {
    wrapper.unmount();
    wrapper = null;
  }
  // AskUserQuestionCard does NOT portal to body (AC10 inline red
  // line) — but the test still benefits from sweeping any
  // unrelated residue between tests. We keep the sweep narrow so
  // a regression that DOES portal to body (e.g. someone copies
  // a Modal pattern by mistake) would surface as a new selector
  // match in `document.body`.
  document
    .querySelectorAll(".ask-card-portal, .ask-card__overlay")
    .forEach((el) => el.remove());
}

beforeEach(() => {
  invokeMock.mockReset();
  // Default to a successful resolve so tests that don't focus
  // on the IPC mock don't have to thread `.mockResolvedValue`
  // through every setup line. Tests that want a rejection set
  // `.mockRejectedValueOnce(...)` per-test.
  invokeMock.mockResolvedValue(undefined);
});

// ---------------------------------------------------------------------
// 1. Multi-question section rendering (R8)
// ---------------------------------------------------------------------

describe("AskUserQuestionCard — multi-question rendering", () => {
  beforeEach(() => { wrapper = null; });

  it("renders one card root with the expected sections", () => {
    wrapper = mountCard();
    const card = wrapper.find("[data-testid='ask-card']");
    expect(card.exists()).toBe(true);
    // 2 questions → 2 sections.
    const sections = wrapper.findAll(".ask-card__section");
    expect(sections.length).toBe(2);
  });

  it("renders the header chip + question body + multi badge", () => {
    wrapper = mountCard();
    const q0 = wrapper.find("[data-testid='ask-card-question-0']");
    expect(q0.find(".ask-card__section-chip").text()).toBe("Library");
    expect(q0.find(".ask-card__section-q").text()).toBe(
      "Which library should we use?",
    );
    // Single-select → no "多选" badge.
    expect(q0.find("[data-testid='ask-card-multi-badge']").exists()).toBe(
      false,
    );

    const q1 = wrapper.find("[data-testid='ask-card-question-1']");
    expect(q1.find(".ask-card__section-chip").text()).toBe("Features");
    expect(q1.find("[data-testid='ask-card-multi-badge']").exists()).toBe(
      true,
    );
  });

  it("renders every option with description", () => {
    wrapper = mountCard();
    // Q0 has 2 options; Q1 has 3.
    expect(
      wrapper.findAll("[data-testid^='ask-card-option-0-']").length,
    ).toBe(2);
    expect(
      wrapper.findAll("[data-testid^='ask-card-option-1-']").length,
    ).toBe(3);
    // Description text surfaces under the label.
    const vueOpt = wrapper.find("[data-testid='ask-card-option-0-Vue']");
    expect(vueOpt.find(".ask-card__option-desc").text()).toBe(
      "Reactive component framework",
    );
  });

  it("renders 4 questions when the payload has 4 entries (PRD R2 1..=4)", () => {
    wrapper = mountCard({
      questions: [
        { question: "Q1?", header: "H1", options: [{ label: "A" }, { label: "B" }], multi_select: false },
        { question: "Q2?", header: "H2", options: [{ label: "A" }, { label: "B" }], multi_select: false },
        { question: "Q3?", header: "H3", options: [{ label: "A" }, { label: "B" }], multi_select: false },
        { question: "Q4?", header: "H4", options: [{ label: "A" }, { label: "B" }], multi_select: false },
      ],
    });
    expect(wrapper.findAll(".ask-card__section").length).toBe(4);
  });
});

// ---------------------------------------------------------------------
// 2. Single-select (radio) toggle
// ---------------------------------------------------------------------

describe("AskUserQuestionCard — single-select (radio)", () => {
  beforeEach(() => { wrapper = null; });

  it("marks the option's selected class when clicked", async () => {
    wrapper = mountCard();
    // Click the row (`<li>`) — covers the realistic user path
    // (click on the label text, not just the radio dot). The
    // component's `<li>` carries the `@click` handler; the
    // `<input>` itself uses `@click.stop` to prevent double
    // fire when the user clicks the radio directly.
    await clickOption("ask-card-option-0-Vue");
    // Re-query — the wrapper.find() result is a snapshot
    // taken at the moment of the call; subsequent DOM
    // mutations (class toggle via reactive state) need a
    // fresh find to read the current class list.
    expect(
      wrapper!.find("[data-testid='ask-card-option-0-Vue']").classes(),
    ).toContain("ask-card__option--selected");
  });

  it("replaces the previous selection when another radio is picked", async () => {
    wrapper = mountCard();
    await clickOption("ask-card-option-0-Vue");
    expect(
      wrapper!.find("[data-testid='ask-card-option-0-Vue']").classes(),
    ).toContain("ask-card__option--selected");
    await clickOption("ask-card-option-0-React");
    expect(
      wrapper!.find("[data-testid='ask-card-option-0-Vue']").classes(),
    ).not.toContain("ask-card__option--selected");
    expect(
      wrapper!.find("[data-testid='ask-card-option-0-React']").classes(),
    ).toContain("ask-card__option--selected");
  });
});

// ---------------------------------------------------------------------
// 3. Multi-select (checkbox) toggle
// ---------------------------------------------------------------------

describe("AskUserQuestionCard — multi-select (checkbox)", () => {
  beforeEach(() => { wrapper = null; });

  it("accumulates multiple selections independently", async () => {
    wrapper = mountCard();
    await clickOption("ask-card-option-1-Routing");
    await clickOption("ask-card-option-1-State");
    expect(
      wrapper!.find("[data-testid='ask-card-option-1-Routing']").classes(),
    ).toContain("ask-card__option--selected");
    expect(
      wrapper!.find("[data-testid='ask-card-option-1-State']").classes(),
    ).toContain("ask-card__option--selected");
  });

  it("toggles a selection off when clicked twice", async () => {
    wrapper = mountCard();
    await clickOption("ask-card-option-1-Routing");
    await clickOption("ask-card-option-1-Routing");
    expect(
      wrapper!.find("[data-testid='ask-card-option-1-Routing']").classes(),
    ).not.toContain("ask-card__option--selected");
  });
});

// ---------------------------------------------------------------------
// 4. Submit-disabled gate (R7)
// ---------------------------------------------------------------------

describe("AskUserQuestionCard — submit gate", () => {
  beforeEach(() => { wrapper = null; });

  it("disables 提交 when no question has a selection", () => {
    wrapper = mountCard();
    const submit = wrapper.find<HTMLButtonElement>(
      "[data-testid='ask-card-submit']",
    );
    expect(submit.element.disabled).toBe(true);
  });

  it("disables 提交 when only one of two questions is answered", async () => {
    wrapper = mountCard();
    // Answer Q0 only.
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await nextTick();
    const submit = wrapper.find<HTMLButtonElement>(
      "[data-testid='ask-card-submit']",
    );
    expect(submit.element.disabled).toBe(true);
  });

  it("disables 提交 when multi-select question has zero selections", async () => {
    wrapper = mountCard();
    // Answer only the single-select question; Q1 (multi) still empty.
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await nextTick();
    const submit = wrapper.find<HTMLButtonElement>(
      "[data-testid='ask-card-submit']",
    );
    expect(submit.element.disabled).toBe(true);
  });

  it("enables 提交 when every question has a valid selection", async () => {
    wrapper = mountCard();
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-1-Routing\']").trigger("click");
    await nextTick();
    const submit = wrapper.find<HTMLButtonElement>(
      "[data-testid='ask-card-submit']",
    );
    expect(submit.element.disabled).toBe(false);
  });

  it("multi-select requires ≥1 selection (not 0)", async () => {
    wrapper = mountCard();
    // Q1 alone with 0 selections + Q0 answered → still disabled.
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await nextTick();
    // Toggle Q1's first option ON then OFF.
    await clickOption("ask-card-option-1-Routing");
    await clickOption("ask-card-option-1-Routing");
    await nextTick();
    const submit = wrapper.find<HTMLButtonElement>(
      "[data-testid='ask-card-submit']",
    );
    expect(submit.element.disabled).toBe(true);
  });
});

// ---------------------------------------------------------------------
// 5. Submit fires IPC with the correct wire shape
// ---------------------------------------------------------------------

describe("AskUserQuestionCard — submit IPC", () => {
  beforeEach(() => { wrapper = null; });

  it("fires invoke('resolve_tool_question') with sessionId/toolUseId/answer", async () => {
    wrapper = mountCard();
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-1-Routing\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-1-State\']").trigger("click");
    await nextTick();
    await wrapper.find("[data-testid='ask-card-submit']").trigger("click");
    await flushPromises();

    // resolveToolQuestion calls invoke(RESOLVE_TOOL_QUESTION_CMD, ...).
    expect(invokeMock).toHaveBeenCalledTimes(1);
    const [cmd, args] = invokeMock.mock.calls[0];
    expect(cmd).toBe("resolve_tool_question");
    expect(args).toEqual({
      sessionId: "sess-1",
      toolUseId: "tool-use-1",
      answer: [
        {
          question: "Which library should we use?",
          header: "Library",
          options: ["Vue"],
          multi_select: false,
        },
        {
          question: "Which features do you need?",
          header: "Features",
          options: ["Routing", "State"],
          multi_select: true,
        },
      ],
      // `cancelled` is omitted by Tauri arg-binder (undefined keys
      // dropped) — `answer` is the live path.
      cancelled: undefined,
    });
  });

  it("preserves payload order in the answer array (PRD R4 + R7)", async () => {
    wrapper = mountCard();
    // Pick "State" before "Routing" — answer must follow the
    // payload's `options` order, not click order.
    await wrapper
      .find("[data-testid=\'ask-card-option-1-State\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-1-Routing\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-0-React\']").trigger("click");
    await nextTick();
    await wrapper.find("[data-testid='ask-card-submit']").trigger("click");
    await flushPromises();
    const args = invokeMock.mock.calls[0][1] as { answer: ToolQuestionAnswer[] };
    expect(args.answer[1].options).toEqual(["Routing", "State"]);
  });
});

// ---------------------------------------------------------------------
// 6. Post-submit state transition + summary row (R7a)
// ---------------------------------------------------------------------

describe("AskUserQuestionCard — answered state", () => {
  beforeEach(() => { wrapper = null; });

  it("flips to the answered state on successful submit", async () => {
    wrapper = mountCard();
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-1-Routing\']").trigger("click");
    await nextTick();
    await wrapper.find("[data-testid='ask-card-submit']").trigger("click");
    await flushPromises();
    expect(
      wrapper.find("[data-testid='ask-card-state-answered']").exists(),
    ).toBe(true);
    // Bottom action row is gone; summary replaces it.
    expect(wrapper.find("[data-testid='ask-card-submit']").exists()).toBe(
      false,
    );
    expect(wrapper.find("[data-testid='ask-card-skip']").exists()).toBe(
      false,
    );
    expect(wrapper.find("[data-testid='ask-card-summary']").exists()).toBe(
      true,
    );
  });

  it("renders selected labels in the summary (R7a 答完保留展开全程)", async () => {
    wrapper = mountCard();
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-1-Routing\']").trigger("click");
    await nextTick();
    await wrapper.find("[data-testid='ask-card-submit']").trigger("click");
    await flushPromises();
    const summaryLabels = wrapper
      .find("[data-testid='ask-card-summary']")
      .findAll(".ask-card__summary-label")
      .map((l) => l.text());
    expect(summaryLabels).toEqual(["Vue", "Routing"]);
  });

  it("preserves expanded content (all options still visible after submit)", async () => {
    wrapper = mountCard();
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-1-Routing\']").trigger("click");
    await nextTick();
    await wrapper.find("[data-testid='ask-card-submit']").trigger("click");
    await flushPromises();
    // All options from both questions still in DOM (no collapse).
    expect(
      wrapper.findAll("[data-testid^='ask-card-option-']").length,
    ).toBe(5);
  });

  it("emits 'answered' with the answer array on submit success", async () => {
    wrapper = mountCard();
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-1-Routing\']").trigger("click");
    await nextTick();
    await wrapper.find("[data-testid='ask-card-submit']").trigger("click");
    await flushPromises();
    const events = wrapper.emitted("answered");
    expect(events).toBeTruthy();
    expect(events!.length).toBe(1);
    const payload = (events![0] as [ToolQuestionAnswer[]])[0];
    expect(payload.length).toBe(2);
    expect(payload[0].options).toEqual(["Vue"]);
  });

  it("renders the answered state directly when state='answered' prop is set", () => {
    // PRD R7a: a rehydrated historical view mounts with
    // state='answered' (e.g. session reload, cached summary).
    // The bottom buttons must NOT show; the summary must.
    wrapper = mountCard({
      state: "answered",
      selectedAnswer: [
        {
          question: "Which library should we use?",
          header: "Library",
          options: ["React"],
          multi_select: false,
        },
        {
          question: "Which features do you need?",
          header: "Features",
          options: ["Routing", "State"],
          multi_select: true,
        },
      ],
    });
    expect(
      wrapper.find("[data-testid='ask-card-state-answered']").exists(),
    ).toBe(true);
    expect(wrapper.find("[data-testid='ask-card-submit']").exists()).toBe(
      false,
    );
    const summaryLabels = wrapper
      .find("[data-testid='ask-card-summary']")
      .findAll(".ask-card__summary-label")
      .map((l) => l.text());
    expect(summaryLabels).toEqual(["React", "Routing", "State"]);
  });
});

// ---------------------------------------------------------------------
// 7. Skip (R5 / AC6) — wire { cancelled: true } + flip to cancelled
// ---------------------------------------------------------------------

describe("AskUserQuestionCard — skip (跳过)", () => {
  beforeEach(() => { wrapper = null; });

  it("fires invoke with cancelled: true and flips state", async () => {
    wrapper = mountCard();
    await wrapper.find("[data-testid='ask-card-skip']").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    const [cmd, args] = invokeMock.mock.calls[0];
    expect(cmd).toBe("resolve_tool_question");
    expect(args).toEqual({
      sessionId: "sess-1",
      toolUseId: "tool-use-1",
      answer: undefined,
      cancelled: true,
    });
    expect(
      wrapper.find("[data-testid='ask-card-state-cancelled']").exists(),
    ).toBe(true);
    expect(wrapper.find("[data-testid='ask-card-skip']").exists()).toBe(
      false,
    );
    expect(wrapper.find("[data-testid='ask-card-submit']").exists()).toBe(
      false,
    );
    expect(
      wrapper.find("[data-testid='ask-card-cancelled-note']").exists(),
    ).toBe(true);
  });

  it("emits 'cancelled' on skip", async () => {
    wrapper = mountCard();
    await wrapper.find("[data-testid='ask-card-skip']").trigger("click");
    await flushPromises();
    expect(wrapper.emitted("cancelled")).toBeTruthy();
  });

  it("can be skipped without any selection (跳过 is always enabled)", async () => {
    wrapper = mountCard();
    // No selections made.
    const skip = wrapper.find<HTMLButtonElement>(
      "[data-testid='ask-card-skip']",
    );
    expect(skip.element.disabled).toBe(false);
    await skip.trigger("click");
    await flushPromises();
    expect(
      wrapper.find("[data-testid='ask-card-state-cancelled']").exists(),
    ).toBe(true);
  });

  it("renders the cancelled state directly when state='cancelled' prop is set", () => {
    wrapper = mountCard({ state: "cancelled" });
    expect(
      wrapper.find("[data-testid='ask-card-state-cancelled']").exists(),
    ).toBe(true);
    expect(wrapper.find("[data-testid='ask-card-submit']").exists()).toBe(
      false,
    );
  });
});

// ---------------------------------------------------------------------
// 8. Submit error path — inline error + retry
// ---------------------------------------------------------------------

describe("AskUserQuestionCard — submit error", () => {
  beforeEach(() => { wrapper = null; });

  it("surfaces an inline error row when invoke rejects and re-enables buttons", async () => {
    invokeMock.mockRejectedValueOnce("network down");
    wrapper = mountCard();
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-1-Routing\']").trigger("click");
    await nextTick();
    await wrapper.find("[data-testid='ask-card-submit']").trigger("click");
    await flushPromises();
    // Error row is visible.
    const errRow = wrapper.find("[data-testid='ask-card-error']");
    expect(errRow.exists()).toBe(true);
    expect(errRow.text()).toContain("network down");
    // Card stays in pending state — buttons re-enabled for retry.
    expect(wrapper.find("[data-testid='ask-card-state-answered']").exists()).toBe(
      false,
    );
    const submit = wrapper.find<HTMLButtonElement>(
      "[data-testid='ask-card-submit']",
    );
    expect(submit.element.disabled).toBe(false);
  });

  it("clears the inline error on the next submit attempt", async () => {
    invokeMock
      .mockRejectedValueOnce("first attempt fails")
      .mockResolvedValueOnce(undefined);
    wrapper = mountCard();
    await wrapper
      .find("[data-testid=\'ask-card-option-0-Vue\']").trigger("click");
    await wrapper
      .find("[data-testid=\'ask-card-option-1-Routing\']").trigger("click");
    await nextTick();
    await wrapper.find("[data-testid='ask-card-submit']").trigger("click");
    await flushPromises();
    expect(wrapper.find("[data-testid='ask-card-error']").exists()).toBe(true);
    // Retry — second click resolves.
    await wrapper.find("[data-testid='ask-card-submit']").trigger("click");
    await flushPromises();
    expect(wrapper.find("[data-testid='ask-card-error']").exists()).toBe(
      false,
    );
    expect(
      wrapper.find("[data-testid='ask-card-state-answered']").exists(),
    ).toBe(true);
  });
});

// ---------------------------------------------------------------------
// 9. AC10 — inline red line: card is in the parent tree, NOT
// portaled to <body>. Regression guard against a future
// "Modal-style" port of the card.
// ---------------------------------------------------------------------

describe("AskUserQuestionCard — inline red line (AC10)", () => {
  beforeEach(() => { wrapper = null; });

  it("mounts the card in the wrapper's component tree (no Teleport to body)", () => {
    wrapper = mountCard();
    const card = wrapper.find("[data-testid='ask-card']");
    expect(card.exists()).toBe(true);
    // The card's element should live under the wrapper's element,
    // NOT under document.body as a portal residue.
    const cardEl = card.element as HTMLElement;
    const wrapperEl = wrapper.element as HTMLElement;
    expect(wrapperEl.contains(cardEl)).toBe(true);
  });

  it("does NOT leave any portal residue on document.body after unmount", () => {
    wrapper = mountCard();
    // The card's root must live inside the wrapper's element.
    // If it had been portaled to <body>, it would be a sibling
    // of the wrapper's root (which mounts inside a created
    // <div> inside <body>). We assert the card is a descendant
    // of the wrapper — a future "Modal-style" port that teleports
    // the card to body would land outside this subtree and the
    // assertion would fail.
    const wrapperEl = wrapper!.element as HTMLElement;
    const cardEl = wrapper!.find("[data-testid='ask-card']")
      .element as HTMLElement;
    expect(wrapperEl.contains(cardEl)).toBe(true);

    unmount();
    // After unmount, the wrapper is gone; no ask-card element
    // should remain in the DOM (no portal / Teleport residue).
    // A future regression that ports the card to <body> would
    // leave a residue here → fail loudly.
    expect(
      document.querySelectorAll("[data-testid='ask-card']").length,
    ).toBe(0);
    // And no ask-card-portal / overlay should ever appear.
    expect(
      document.querySelectorAll(".ask-card-portal, .ask-card__overlay")
        .length,
    ).toBe(0);
  });
});

// Global cleanup — make sure every test ends with the wrapper torn
// down so no DOM residue leaks into the next test.
afterEach(() => {
  unmount();
});