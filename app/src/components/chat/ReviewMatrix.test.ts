// Component tests for the C2 review visualization view
// (2026-07-26). Covers:
//   1. ReviewMatrixGrid: renders rounds × models cells with
//      verdict + count; failed model greyed; absent model shows
//      "未参与"; click cell expands findings inline.
//   2. ReviewDimensionCompare: dimension selector switches the
//      shown findings; models with no finding for the dimension
//      show "未提及".
//   3. ReviewFindingDetail: source_run_id button calls
//      `get_subagent_run` and opens the modal with `finalText`.
//   4. ReviewMatrix: state gating — only renders when state OR
//      error is present; tab switch toggles grid/dim.
//
// All tests mount the production components against a Pinia
// instance + a mocked transport (for the source_run_id IPC).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: vi.fn(async () => () => {}),
  },
}));

// Mock MarkdownDetailModal so we don't pull in reka-ui's Dialog
// plumbing (which needs a portal target the jsdom env lacks).
// We DO want to assert the open prop is wired through — so the
// stub forwards `open` + `markdown` as attributes.
vi.mock("../common/MarkdownDetailModal.vue", () => ({
  default: {
    name: "MarkdownDetailModal",
    props: ["open", "markdown", "title", "source"],
    emits: ["update:open"],
    template: '<div v-if="open" class="mock-md-modal">{{ markdown }}</div>',
  },
}));

import ReviewMatrixGrid from "./ReviewMatrixGrid.vue";
import ReviewDimensionCompare from "./ReviewDimensionCompare.vue";
import ReviewFindingDetail from "./ReviewFindingDetail.vue";
import ReviewMatrix from "./ReviewMatrix.vue";
import { useReviewStateStore } from "../../stores/reviewState";
import type { ReviewState } from "../../types/review-state";

/** Fixture: 3 rounds × 3 models, with one model failed in round 2.
 *  Models key = model_id (stable); display names differ slightly
 *  between rounds to exercise the re-label collapse. */
function fixtureState(): ReviewState {
  return {
    schema_version: "1.0",
    task_id: "demo-task",
    current_round: 3,
    rounds: [
      {
        round: 1,
        dimensions: ["清晰度", "范围边界"],
        models_present: ["model-a", "model-b", "model-c"],
        models: {
          "model-a": {
            model_display: "Model A",
            run_id: "run-a1",
            status: "completed",
            verdict: "revise",
            findings: [
              {
                finding_id: "f-a1-1",
                dimension: "清晰度",
                severity: "high",
                issue: "不够清楚",
                suggestion: "加例子",
                source_run_id: "run-a1",
              },
            ],
          },
          "model-b": {
            model_display: "Model B",
            run_id: "run-b1",
            status: "completed",
            verdict: "pass_with_minor",
            findings: [],
          },
          "model-c": {
            model_display: "Model C",
            run_id: "run-c1",
            status: "completed",
            verdict: "pass",
            findings: [],
          },
        },
      },
      {
        round: 2,
        dimensions: ["清晰度", "可行性"],
        models_present: ["model-a", "model-b", "model-c"],
        models: {
          "model-a": {
            model_display: "Model A (v2)",
            run_id: "run-a2",
            status: "error",
            verdict: "revise",
            findings: [
              {
                finding_id: "f-a2-1",
                dimension: "可行性",
                severity: "medium",
                issue: "依赖未确认",
                source_run_id: "run-a2",
              },
            ],
          },
          "model-b": {
            model_display: "Model B",
            run_id: "run-b2",
            status: "completed",
            verdict: "pass",
            findings: [],
          },
          "model-c": {
            model_display: "Model C",
            run_id: "run-c2",
            status: "completed",
            verdict: "pass",
            findings: [],
          },
        },
        convergence_note: "建议定稿",
      },
      {
        round: 3,
        dimensions: ["清晰度"],
        models_present: ["model-a", "model-b"],
        models: {
          "model-a": {
            model_display: "Model A (v2)",
            run_id: "run-a3",
            status: "completed",
            verdict: "pass",
            findings: [],
          },
          "model-b": {
            model_display: "Model B",
            run_id: "run-b3",
            status: "completed",
            verdict: "pass",
            findings: [],
          },
        },
      },
    ],
  };
}

/** Find the round row whose rowheader reads "第 <n> 轮".
 *  Defensive against `.text()` on empty wrappers (the header
 *  row has no `--row-head` cell; we filter via `.exists()`
 *  before reading text). */
function findRoundRow(
  w: ReturnType<typeof mount>,
  round: number,
) {
  const needle = `第 ${round} 轮`;
  return w.findAll(".review-matrix-grid__row").find((r) => {
    const h = r.find(".review-matrix-grid__cell--row-head");
    return h.exists() && h.text().includes(needle);
  });
}

describe("ReviewMatrixGrid", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("renders one header per model (union across rounds)", () => {
    const w = mount(ReviewMatrixGrid, {
      props: { state: fixtureState() },
    });
    const heads = w.findAll(".review-matrix-grid__cell--head");
    expect(heads.length).toBe(3);
    // Re-label collapses to the latest display.
    expect(heads[0].text()).toContain("Model A (v2)");
    expect(heads[1].text()).toContain("Model B");
    expect(heads[2].text()).toContain("Model C");
  });

  it("renders one body row per round", () => {
    const w = mount(ReviewMatrixGrid, {
      props: { state: fixtureState() },
    });
    const bodyRows = w.findAll(".review-matrix-grid__row:not(.review-matrix-grid__row--head)");
    // 3 round rows (the expanded row is separate from the round
    // row in DOM; round rows carry the rowheader cell).
    const roundRows = bodyRows.filter((r) =>
      r.find(".review-matrix-grid__cell--row-head").exists(),
    );
    expect(roundRows.length).toBe(3);
  });

  it("shows verdict + count in cells with findings", () => {
    const w = mount(ReviewMatrixGrid, {
      props: { state: fixtureState() },
    });
    const cells = w.findAll(".review-matrix-grid__cell--body");
    // Round 1, Model A is the first body cell.
    expect(cells[0].text()).toContain("修订");
    expect(cells[0].text()).toContain("1 条");
  });

  it("greys failed-model cells (status=error/incomplete/cancelled)", async () => {
    const w = mount(ReviewMatrixGrid, {
      props: { state: fixtureState() },
    });
    // Round 2 row, Model A cell → status error.
    const r2 = findRoundRow(w, 2);
    expect(r2).toBeDefined();
    const r2Cells = r2!.findAll(".review-matrix-grid__cell--body");
    expect(r2Cells[0].classes()).toContain("review-matrix-grid__cell--failed");
  });

  it("shows 未参与 for a model absent in a round", () => {
    const w = mount(ReviewMatrixGrid, {
      props: { state: fixtureState() },
    });
    // Round 3, Model C — absent (only A + B present).
    const r3 = findRoundRow(w, 3);
    expect(r3).toBeDefined();
    const r3Cells = r3!.findAll(".review-matrix-grid__cell--body");
    // Model C is the 3rd column.
    expect(r3Cells[2].text()).toContain("未参与");
  });

  it("expands findings inline when a cell with findings is clicked", async () => {
    const w = mount(ReviewMatrixGrid, {
      props: { state: fixtureState() },
    });
    const cells = w.findAll(".review-matrix-grid__cell--body");
    // No expanded row initially.
    expect(w.find(".review-matrix-grid__expanded").exists()).toBe(false);

    // Click Round 1 / Model A (has findings).
    await cells[0].trigger("click");
    const expanded = w.find(".review-matrix-grid__expanded");
    expect(expanded.exists()).toBe(true);
    expect(expanded.text()).toContain("不够清楚");
  });

  it("does nothing when an absent cell is clicked", async () => {
    const w = mount(ReviewMatrixGrid, {
      props: { state: fixtureState() },
    });
    const r3 = findRoundRow(w, 3);
    expect(r3).toBeDefined();
    const r3Cells = r3!.findAll(".review-matrix-grid__cell--body");
    await r3Cells[2].trigger("click"); // 未参与 cell
    expect(w.find(".review-matrix-grid__expanded").exists()).toBe(false);
  });
});

describe("ReviewDimensionCompare", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("lists the union of dimensions across rounds", () => {
    const w = mount(ReviewDimensionCompare, {
      props: { state: fixtureState() },
    });
    const opts = w.findAll(".review-dim-compare__select option");
    const labels = opts.map((o) => o.text());
    expect(labels).toEqual(expect.arrayContaining(["清晰度", "范围边界", "可行性"]));
  });

  it("renders a column per model in the selected round", () => {
    const w = mount(ReviewDimensionCompare, {
      props: { state: fixtureState() },
    });
    const cols = w.findAll(".review-dim-compare__column");
    // Default selectedRound = latest = round 3 → 2 models.
    expect(cols.length).toBe(2);
  });

  it("filters findings by the selected dimension", async () => {
    const w = mount(ReviewDimensionCompare, {
      props: { state: fixtureState() },
    });
    // Switch to round 2 (has 可行性 finding from Model A).
    const roundSelect = w.findAll(".review-dim-compare__select")[1];
    await roundSelect.setValue(2);

    // Switch dimension to 可行性.
    const dimSelect = w.findAll(".review-dim-compare__select")[0];
    await dimSelect.setValue("可行性");

    const cols = w.findAll(".review-dim-compare__column");
    const modelACol = cols.find((c) => c.text().includes("Model A"));
    expect(modelACol?.text()).toContain("依赖未确认");
  });

  it("shows 未提及 for a model with no finding on the dimension", async () => {
    const w = mount(ReviewDimensionCompare, {
      props: { state: fixtureState() },
    });
    const roundSelect = w.findAll(".review-dim-compare__select")[1];
    await roundSelect.setValue(2);
    const dimSelect = w.findAll(".review-dim-compare__select")[0];
    await dimSelect.setValue("可行性");

    const cols = w.findAll(".review-dim-compare__column");
    // Model B (no 可行性 finding).
    const modelBCol = cols.find((c) => c.text().includes("Model B"));
    expect(modelBCol?.text()).toContain("未提及");
  });
});

describe("ReviewFindingDetail source_run_id jump", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("calls get_subagent_run and opens the modal with finalText", async () => {
    invokeMock.mockResolvedValue({
      id: "run-a1",
      finalText: "reviewer 原始 markdown",
    });

    const w = mount(ReviewFindingDetail, {
      props: {
        finding: {
          finding_id: "f1",
          dimension: "清晰度",
          severity: "high",
          issue: "unclear",
          source_run_id: "run-a1",
        },
      },
    });
    // Modal closed initially.
    expect(w.find(".mock-md-modal").exists()).toBe(false);

    await w.find(".review-finding-detail__source-btn").trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("get_subagent_run", {
      runId: "run-a1",
    });
    expect(w.find(".mock-md-modal").exists()).toBe(true);
    expect(w.find(".mock-md-modal").text()).toContain("reviewer 原始 markdown");
  });

  it("surfaces an inline error when the run is missing (null row)", async () => {
    invokeMock.mockResolvedValue(null);
    const w = mount(ReviewFindingDetail, {
      props: {
        finding: {
          finding_id: "f1",
          dimension: "清晰度",
          severity: "high",
          issue: "unclear",
          source_run_id: "run-gone",
        },
      },
    });
    await w.find(".review-finding-detail__source-btn").trigger("click");
    await flushPromises();
    expect(w.find(".review-finding-detail__source-error").text()).toContain(
      "原始 run 不存在",
    );
    expect(w.find(".mock-md-modal").exists()).toBe(false);
  });
});

describe("ReviewMatrix state gating", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("renders the matrix panel when the store has state", async () => {
    // Drive the store through its public `start` so we exercise
    // the real three-state apply path (mocked transport returns
    // a State payload).
    invokeMock.mockResolvedValue({ kind: "state", state: fixtureState() });
    const store = useReviewStateStore();
    await store.start("demo-task", "/proj");

    const w = mount(ReviewMatrix);
    expect(w.find(".review-matrix").exists()).toBe(true);
    expect(w.findComponent(ReviewMatrixGrid).exists()).toBe(true);
  });

  it("renders the error card when the store has an invalid error", async () => {
    invokeMock.mockResolvedValue({
      kind: "invalid",
      detail: "boom",
    });
    const store = useReviewStateStore();
    await store.start("demo-task", "/proj");

    const w = mount(ReviewMatrix);
    expect(w.find(".review-matrix--error").exists()).toBe(true);
    expect(w.text()).toContain("boom");
  });

  it("renders nothing when the store is empty (no state, no error)", () => {
    // Default store: state=null, error=null.
    const w = mount(ReviewMatrix);
    // No root node renders in this state (v-if on every section).
    expect(w.find(".review-matrix").exists()).toBe(false);
    expect(w.find(".review-matrix--error").exists()).toBe(false);
  });

  it("switches to the dimension-compare tab when clicked", async () => {
    invokeMock.mockResolvedValue({ kind: "state", state: fixtureState() });
    const store = useReviewStateStore();
    await store.start("demo-task", "/proj");

    const w = mount(ReviewMatrix);
    // Default tab = matrix; dim component not yet mounted.
    expect(w.findComponent(ReviewMatrixGrid).exists()).toBe(true);
    expect(w.findComponent(ReviewDimensionCompare).exists()).toBe(false);

    // Click the 维度对比 tab.
    const tabs = w.findAll(".review-matrix__tab");
    const dimTab = tabs.find((t) => t.text().includes("维度对比"));
    await dimTab!.trigger("click");

    expect(w.findComponent(ReviewDimensionCompare).exists()).toBe(true);
  });

  it("collapses the body when the collapse toggle is clicked", async () => {
    invokeMock.mockResolvedValue({ kind: "state", state: fixtureState() });
    const store = useReviewStateStore();
    await store.start("demo-task", "/proj");

    const w = mount(ReviewMatrix);
    expect(w.find(".review-matrix__body").exists()).toBe(true);
    await w.find(".review-matrix__collapse").trigger("click");
    expect(w.find(".review-matrix__body").exists()).toBe(false);
  });
});
