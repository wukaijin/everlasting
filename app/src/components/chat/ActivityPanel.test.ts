// Tests for `ActivityPanel.vue` — the unified run-status overlay
// (2026-09-02, task `09-02-chat-task-panel`). Replaces ChecklistCard.
//
// Coverage (design §4, five cases):
//   1. Checklist-only → renders equivalent to the old ChecklistCard
//      (header, progress, item rows, all-done tint, ball done/total).
//   2. Subagent-only → running/completed rows render; clicking a row
//      calls `subagentRuns.openDrawer(runId)`.
//   3. Shell-only → completed + running rows render; clicking a
//      terminal row expands the stdout/stderr preview; exit-code chip;
//      the running row's kill button calls the kill IPC.
//   4. All sections empty → panel not rendered.
//   5. Ball counters + breathing-ring class.
//
// Stores are REAL (Pinia) and seeded through the transport mock —
// the component's mount-time `fetchForSession` calls must be answered
// by `invokeMock` or they'd overwrite direct seeds (test-env gotcha 6).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import ActivityPanel, { compareSubagentRuns, formatDuration } from "./ActivityPanel.vue";
import { useSubagentRunsStore } from "../../stores/subagentRuns";
import type { SubagentRunSummary } from "../../stores/subagentRuns.types";
import type { ChecklistItem } from "../../stores/checklist";
import { useProjectsStore } from "../../stores/projects";

// -----------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------

function makeSummary(
  overrides: Partial<SubagentRunSummary> = {},
): SubagentRunSummary {
  return {
    id: "run-1",
    parentSessionId: "sess-1",
    parentRequestId: "rid-sub-tu-1",
    subagentName: "researcher",
    status: "completed",
    startedAt: "2026-09-02T10:00:00Z",
    finishedAt: "2026-09-02T10:00:30Z",
    tokenUsageJson: null,
    summary: null,
    task: null,
    finalText: null,
    turnCount: null,
    worktreePath: null,
    modelDisplay: null,
    ...overrides,
  };
}

function makeShell(overrides: Record<string, unknown> = {}) {
  return {
    shellSessionId: "bsh_1",
    sessionId: "sess-1",
    command: "cargo test",
    status: "completed",
    startedAtMs: 1000,
    elapsedMs: 4200,
    exitCode: 0,
    stdoutPreview: "test result: ok",
    stderrPreview: null,
    fullOutputPath: null,
    originToolUseId: null,
    ...overrides,
  };
}

const checklistItems: ChecklistItem[] = [
  { content: "梳理需求", status: "done" },
  { content: "实现功能", status: "in_progress" },
  { content: "回归验证", status: "pending" },
];

/** Default transport answers: both pull IPCs return [] so mount-time
 *  fetches never wipe test-seeded state unintentionally; tests that
 *  seed via IPC override these per command. */
function answerPulls(sidebar?: {
  subagents?: SubagentRunSummary[];
  shells?: unknown[];
}) {
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "list_subagent_runs_by_session") {
      return sidebar?.subagents ?? [];
    }
    if (cmd === "list_background_shells") {
      return sidebar?.shells ?? [];
    }
    return null;
  });
}

function mountPanel(items: ChecklistItem[] | null, sessionId = "sess-1") {
  return mount(ActivityPanel, {
    props: { items, sessionId },
  });
}

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

describe("ActivityPanel", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    answerPulls();
  });

  describe("checklist-only (AC1: ChecklistCard equivalence)", () => {
    it("renders the panel with header, progress and item rows", async () => {
      const w = mountPanel(checklistItems);
      await flushPromises();
      expect(w.find(".activity-panel").exists()).toBe(true);
      expect(w.find(".activity-panel__title").text()).toContain("运行状态");
      expect(w.find(".activity-panel__progress").text()).toBe("1/3");
      // 清单 section header + rows (all three statuses).
      const sections = w.findAll(".activity-panel__section");
      expect(sections).toHaveLength(1);
      expect(sections[0].text()).toContain("清单");
      const rows = w.findAll(".activity-panel__check-item");
      expect(rows).toHaveLength(3);
      expect(rows[0].classes()).toContain("activity-panel__check-item--done");
      expect(rows[1].classes()).toContain(
        "activity-panel__check-item--in_progress",
      );
      expect(rows[2].classes()).toContain("activity-panel__check-item--pending");
      expect(rows[1].text()).toContain("实现功能");
      w.unmount();
    });

    it("all-done checklist applies the green tint class", async () => {
      const w = mountPanel([
        { content: "a", status: "done" },
        { content: "b", status: "done" },
      ]);
      await flushPromises();
      expect(w.find(".activity-panel").classes()).toContain(
        "activity-panel--all-done",
      );
      w.unmount();
    });

    it("empty array (model cleared) still renders the empty state", async () => {
      const w = mountPanel([]);
      await flushPromises();
      expect(w.find(".activity-panel").exists()).toBe(true);
      expect(w.text()).toContain("清单为空");
      w.unmount();
    });

    it("minimizes to the ball showing done/total; expands on click", async () => {
      const w = mountPanel(checklistItems);
      await flushPromises();
      await w.find(".activity-panel__minimize").trigger("click");
      expect(w.find(".activity-panel__ball").exists()).toBe(true);
      expect(w.find(".activity-panel__ball-count").text()).toBe("1/3");
      await w.find(".activity-panel__ball").trigger("click");
      expect(w.find(".activity-panel__panel").exists()).toBe(true);
      w.unmount();
    });
  });

  describe("subagent section", () => {
    it("renders running + completed rows (name, model chip, duration)", async () => {
      answerPulls({
        subagents: [
          makeSummary({
            id: "run-done",
            subagentName: "writer",
            status: "completed",
            startedAt: "2026-09-02T10:00:00Z",
            finishedAt: "2026-09-02T10:00:30Z",
            modelDisplay: "glm-4.7",
          }),
          makeSummary({
            id: "run-live",
            subagentName: "researcher",
            status: "running",
            finishedAt: null,
            modelDisplay: null,
          }),
        ],
      });
      const w = mountPanel(null);
      await flushPromises();
      const rows = w.findAll(".activity-panel__row");
      expect(rows).toHaveLength(2);
      // Running row first.
      expect(rows[0].text()).toContain("researcher");
      // Model chip: rendered only when modelDisplay is non-null
      // (null = inherit parent — AC14-15 precedent).
      const chips = w.findAll(".activity-panel__chip");
      expect(chips.map((c) => c.text())).toEqual(["glm-4.7"]);
      w.unmount();
    });

    it("clicking a row opens the SubagentDrawer via openDrawer(runId)", async () => {
      answerPulls({ subagents: [makeSummary({ id: "run-x" })] });
      const subagentRuns = useSubagentRunsStore();
      const openSpy = vi
        .spyOn(subagentRuns, "openDrawer")
        .mockResolvedValue();
      const w = mountPanel(null);
      await flushPromises();
      await w.findAll(".activity-panel__row")[0].trigger("click");
      await flushPromises();
      expect(openSpy).toHaveBeenCalledWith("run-x");
      w.unmount();
    });
  });

  describe("shell section", () => {
    it("renders completed + running rows with exit chip / elapsed chip", async () => {
      answerPulls({
        shells: [
          makeShell({ shellSessionId: "bsh_done", status: "completed", exitCode: 0 }),
          makeShell({
            shellSessionId: "bsh_run",
            status: "running",
            exitCode: null,
            stdoutPreview: null,
            stderrPreview: null,
            command: "sleep 30",
          }),
        ],
      });
      const w = mountPanel(null);
      await flushPromises();
      const rows = w.findAll(".activity-panel__row");
      expect(rows).toHaveLength(2);
      // Running first.
      expect(rows[0].text()).toContain("sleep 30");
      expect(rows[0].find(".activity-panel__chip--running").exists()).toBe(true);
      // Terminal row shows the exit-code chip (exit 0 → not red).
      const doneChip = rows[1].find(".activity-panel__chip");
      expect(doneChip.text()).toBe("exit 0");
      expect(doneChip.classes()).not.toContain("activity-panel__chip--error");
      w.unmount();
    });

    it("non-zero exit code renders the error chip variant", async () => {
      answerPulls({ shells: [makeShell({ exitCode: 127, status: "failed" })] });
      const w = mountPanel(null);
      await flushPromises();
      expect(
        w.find(".activity-panel__chip--error").text(),
      ).toBe("exit 127");
      w.unmount();
    });

    it("clicking a terminal row expands the stdout preview + spill hint", async () => {
      answerPulls({
        shells: [
          makeShell({
            stdoutPreview: "built ok",
            fullOutputPath: "/data/outputs/sess-1/x.txt",
          }),
        ],
      });
      const w = mountPanel(null);
      await flushPromises();
      expect(w.find(".activity-panel__pre").exists()).toBe(false);
      await w.find(".activity-panel__row").trigger("click");
      expect(w.find(".activity-panel__pre").text()).toContain("built ok");
      expect(w.find(".activity-panel__preview-hint").text()).toContain(
        "/data/outputs/sess-1/x.txt",
      );
      w.unmount();
    });

    it("clicking a running row without output shows the hint line", async () => {
      answerPulls({
        shells: [
          makeShell({
            status: "running",
            exitCode: null,
            stdoutPreview: null,
            stderrPreview: null,
          }),
        ],
      });
      const w = mountPanel(null);
      await flushPromises();
      await w.find(".activity-panel__row").trigger("click");
      expect(w.find(".activity-panel__preview-empty").text()).toContain(
        "运行中",
      );
      w.unmount();
    });

    it("running row's kill button invokes kill_background_shell (no row toggle)", async () => {
      answerPulls({
        shells: [
          makeShell({
            shellSessionId: "bsh_live",
            status: "running",
            exitCode: null,
            stdoutPreview: null,
            stderrPreview: null,
          }),
        ],
      });
      const w = mountPanel(null);
      await flushPromises();
      await w.find(".activity-panel__kill").trigger("click");
      await flushPromises();
      const killCall = invokeMock.mock.calls.find(
        (c) => c[0] === "kill_background_shell",
      );
      expect(killCall).toEqual([
        "kill_background_shell",
        { sessionId: "sess-1", shellSessionId: "bsh_live" },
      ]);
      // stopPropagation: the row's expand toggle must NOT have fired.
      expect(w.find(".activity-panel__preview").exists()).toBe(false);
      w.unmount();
    });

    it("kill failure toasts and leaves the panel state untouched", async () => {
      const runningShell = makeShell({
        shellSessionId: "bsh_live",
        status: "running",
        exitCode: null,
        stdoutPreview: null,
        stderrPreview: null,
      });
      invokeMock.mockImplementation(async (cmd: string) => {
        if (cmd === "kill_background_shell") throw new Error("后台 shell 不存在");
        if (cmd === "list_background_shells") return [runningShell];
        return null;
      });
      const projects = useProjectsStore();
      const toastSpy = vi.spyOn(projects, "showToast").mockImplementation(() => {});
      const w = mountPanel(null);
      await flushPromises();
      await w.find(".activity-panel__kill").trigger("click");
      await flushPromises();
      expect(toastSpy).toHaveBeenCalledWith(
        expect.stringContaining("后台 shell 不存在"),
        "warn",
      );
      // Row still running (no local fabrication).
      expect(w.find(".activity-panel__chip--running").exists()).toBe(true);
      w.unmount();
      toastSpy.mockRestore();
    });
  });

  describe("visibility + ball", () => {
    it("renders nothing when all three sections are empty", () => {
      const w = mountPanel(null);
      expect(w.find(".activity-panel").exists()).toBe(false);
      w.unmount();
    });

    it("ball shows the running badge + breathing ring while rows run", async () => {
      answerPulls({
        shells: [
          makeShell({
            shellSessionId: "bsh_run",
            status: "running",
            exitCode: null,
            stdoutPreview: null,
            stderrPreview: null,
          }),
        ],
      });
      const w = mountPanel(checklistItems);
      await flushPromises();
      // Root carries the active (breathing) class: 1 running shell.
      expect(w.find(".activity-panel").classes()).toContain(
        "activity-panel--active",
      );
      await w.find(".activity-panel__minimize").trigger("click");
      expect(w.find(".activity-panel__ball-badge").text()).toBe("1");
      // checklistItems: 1 done of 3.
      expect(w.find(".activity-panel__ball-count").text()).toBe("1/3");
      w.unmount();
    });

    it("hides the running badge on the ball when nothing runs", async () => {
      const w = mountPanel(checklistItems);
      await flushPromises();
      await w.find(".activity-panel__minimize").trigger("click");
      expect(w.find(".activity-panel__ball-badge").exists()).toBe(false);
      // in_progress checklist still trips the breathing ring.
      expect(w.find(".activity-panel").classes()).toContain(
        "activity-panel--active",
      );
      w.unmount();
    });

    it("centers the solo ball icon when no checklist exists (no count below)", async () => {
      // No checklist (items=null → no done/total) + a finished shell so
      // the ball renders with the icon alone: it must carry the solo
      // centering class (regression 2026-09-02 — icon stayed top-anchored
      // and read as off-center inside the 44px ball).
      answerPulls({
        shells: [makeShell({ shellSessionId: "bsh_done", status: "completed" })],
      });
      const w = mountPanel(null);
      await flushPromises();
      await w.find(".activity-panel__minimize").trigger("click");
      expect(w.find(".activity-panel__ball-icon").classes()).toContain(
        "activity-panel__ball-icon--solo",
      );
      // With a checklist present the icon returns to the top-anchored
      // icon+count pair (no solo class).
      const withChecklist = mountPanel(checklistItems);
      await flushPromises();
      await withChecklist.find(".activity-panel__minimize").trigger("click");
      expect(withChecklist.find(".activity-panel__ball-icon").classes()).not.toContain(
        "activity-panel__ball-icon--solo",
      );
      w.unmount();
      withChecklist.unmount();
    });
  });

  describe("pure helpers", () => {
    it("compareSubagentRuns: running first, then newest start", () => {
      const doneOld = makeSummary({ id: "a", status: "completed", startedAt: "2026-09-02T10:00:00Z" });
      const doneNew = makeSummary({ id: "b", status: "error", startedAt: "2026-09-02T11:00:00Z" });
      const running = makeSummary({ id: "c", status: "running", startedAt: "2026-09-02T09:00:00Z" });
      const sorted = [doneOld, running, doneNew].sort(compareSubagentRuns);
      expect(sorted.map((s) => s.id)).toEqual(["c", "b", "a"]);
    });

    it("formatDuration: seconds / minutes / hours ladder", () => {
      expect(formatDuration(400)).toBe("<1s");
      expect(formatDuration(42_000)).toBe("42s");
      expect(formatDuration(65_000)).toBe("1m 5s");
      expect(formatDuration(3_600_000)).toBe("1h");
      expect(formatDuration(3_660_000)).toBe("1h 1m");
    });
  });
});
