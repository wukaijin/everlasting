// Tests for L3b PR4 (2026-06-27) — SubagentDrawer Merge / Discard
// worker branch UI + the store actions + the util.
//
// Coverage (PRD §"Acceptance Criteria" + §"Definition of Done"):
//
//   Store (`subagentRuns.ts`):
//   S1. `mergeWorker` success → invoke fires + row.worktreePath
//       flips to null + spinner set during + cleared after.
//   S2. `mergeWorker` conflict → returns { kind: 'conflict',
//       files } + row.worktreePath UNCHANGED + spinner cleared.
//   S3. `mergeWorker` generic error → returns { kind: 'error' } +
//       spinner cleared.
//   S4. `discardWorker` success → invoke fires + row.worktreePath
//       flips to null + spinner cleared.
//   S5. `discardWorker` error → returns { kind: 'error' } +
//       spinner cleared.
//   S6. Spinner guard: a second `mergeWorker` while one is in
//       flight short-circuits (no double-invoke).
//
//   Util (`workerBranch.ts`):
//   U1. `formatWorkerBranchLabel('worker/abc12345-...')` →
//       'Worker abc12345'.
//   U2. `formatWorkerBranchLabel` handles bare run_id / full
//       worktree_path / empty input.
//
//   Parser (`parseConflictFiles`):
//   P1. Conflict message → file list extracted.
//   P2. Generic error → null.
//
//   Components (`WorkerBranchBadge` / `WorkerMergeControls`):
//   C1. Badge hidden when worktreePath null + status !== running.
//   C2. Badge shows 隔离中 when status running.
//   C3. Badge shows 已完成·保留分支 when completed + worktreePath.
//   C4. Merge controls hidden when worktreePath null (cancelled /
//       error / incomplete / swept / merged).
//   C5. Merge controls visible when completed + worktreePath.
//   C6. Click Merge → ConfirmDialog → confirm → store.mergeWorker
//       → success toast + buttons disappear.
//   C7. Click Merge → conflict → file list rendered + error toast
//       + branch NOT destroyed (buttons stay visible).
//   C8. Click Discard → ConfirmDialog → confirm → store.discardWorker
//       → success toast + buttons disappear.
//   C9. Loading state → spinner + buttons disabled (防双击).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import WorkerBranchBadge from "./WorkerBranchBadge.vue";
import WorkerMergeControls from "./WorkerMergeControls.vue";
import { useSubagentRunsStore } from "../../stores/subagentRuns";
import { useProjectsStore } from "../../stores/projects";
import type { SubagentRunRow } from "../../stores/subagentRuns.types";
import { parseConflictFiles } from "../../stores/subagentRuns.types";
import { formatWorkerBranchLabel } from "../../utils/workerBranch";

import * as tauriCore from "@tauri-apps/api/core";
const invokeMock = tauriCore.invoke as unknown as ReturnType<typeof vi.fn>;

// -----------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------

const baseRow: SubagentRunRow = {
  id: "run-merge-1",
  parentSessionId: "sess-1",
  parentRequestId: "parent-rid-sub-tu-1",
  subagentName: "general-purpose",
  status: "completed",
  startedAt: "2026-06-27T10:00:00Z",
  finishedAt: "2026-06-27T10:00:30Z",
  tokenUsageJson: null,
  summary: "edited 2 files",
  transcriptJson: null,
  transcriptTruncated: 0,
  createdAt: "2026-06-27T10:00:00Z",
  finalText: "done",
  task: "do work",
  turnCount: 3,
  worktreePath: "/data/worktrees/proj/worker/run-merge-1",
};

// -----------------------------------------------------------------------
// U. workerBranch util
// -----------------------------------------------------------------------

describe("formatWorkerBranchLabel (util)", () => {
  it("U1: formats worker/<run_id> form", () => {
    expect(formatWorkerBranchLabel("worker/abc12345-1234-1234-1234-123456789abc"))
      .toBe("Worker abc12345");
  });

  it("U2a: handles bare run_id", () => {
    expect(formatWorkerBranchLabel("abc12345-1234-1234-1234-123456789abc"))
      .toBe("Worker abc12345");
  });

  it("U2b: handles full worktree path", () => {
    expect(formatWorkerBranchLabel("/data/worktrees/proj/worker/abc12345-1234"))
      .toBe("Worker abc12345");
  });

  it("U2c: empty input returns empty string", () => {
    expect(formatWorkerBranchLabel(null)).toBe("");
    expect(formatWorkerBranchLabel(undefined)).toBe("");
    expect(formatWorkerBranchLabel("")).toBe("");
  });

  it("U2d: short run_id (< 8 chars) returned verbatim", () => {
    expect(formatWorkerBranchLabel("abc")).toBe("Worker abc");
  });
});

// -----------------------------------------------------------------------
// P. parseConflictFiles parser
// -----------------------------------------------------------------------

describe("parseConflictFiles (parser)", () => {
  it("P1: extracts files from conflict message", () => {
    const msg =
      "merge conflict: [src/foo.rs, src/bar.rs]. The worker branch 'worker/run-1' and parent branch 'session/sess-1' both modified these files. Resolve manually, then call merge_worker again (or discard_worker to drop the changes).";
    expect(parseConflictFiles(msg)).toEqual(["src/foo.rs", "src/bar.rs"]);
  });

  it("P1b: single-file conflict", () => {
    const msg = "merge conflict: [README.md]. The worker branch ...";
    expect(parseConflictFiles(msg)).toEqual(["README.md"]);
  });

  it("P2a: generic error returns null", () => {
    expect(parseConflictFiles("worker run not found: abc")).toBeNull();
    expect(parseConflictFiles("parent session has no worktree")).toBeNull();
  });

  it("P2b: empty conflict list returns empty array", () => {
    const msg = "merge conflict: []. The worker branch ...";
    expect(parseConflictFiles(msg)).toEqual([]);
  });
});

// -----------------------------------------------------------------------
// S. store actions
// -----------------------------------------------------------------------

describe("useSubagentRunsStore mergeWorker / discardWorker", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue("ok");
  });

  async function seedRow(row: Partial<SubagentRunRow> = {}): Promise<SubagentRunRow> {
    const store = useSubagentRunsStore();
    const full = { ...baseRow, ...row };
    store.getRunCache.set(full.id, full);
    return full;
  }

  it("S1: mergeWorker success → invoke + worktreePath null + spinner cleared", async () => {
    const store = useSubagentRunsStore();
    const row = await seedRow();
    const result = await store.mergeWorker(row.id);
    expect(result.kind).toBe("success");
    expect(invokeMock).toHaveBeenCalledWith("merge_worker_run", {
      rid: "merge-pr4",
      runId: row.id,
    });
    // worktreePath cleared
    expect(store.getRunCache.get(row.id)?.worktreePath).toBeNull();
    // spinner cleared
    expect(store.mergeStateByRunId.get(row.id)).toBeUndefined();
  });

  it("S2: mergeWorker conflict → returns files + worktreePath UNCHANGED", async () => {
    const store = useSubagentRunsStore();
    const row = await seedRow();
    invokeMock.mockRejectedValueOnce(
      "merge conflict: [src/a.rs, src/b.rs]. The worker branch 'worker/run-merge-1' and parent branch 'session/sess-1' both modified these files.",
    );
    const result = await store.mergeWorker(row.id);
    expect(result.kind).toBe("conflict");
    if (result.kind === "conflict") {
      expect(result.files).toEqual(["src/a.rs", "src/b.rs"]);
    }
    // worktreePath preserved (conflict keeps the branch)
    expect(store.getRunCache.get(row.id)?.worktreePath).toBe(row.worktreePath);
    // spinner cleared
    expect(store.mergeStateByRunId.get(row.id)).toBeUndefined();
  });

  it("S3: mergeWorker generic error → returns error + spinner cleared", async () => {
    const store = useSubagentRunsStore();
    const row = await seedRow();
    invokeMock.mockRejectedValueOnce("parent session has no worktree");
    const result = await store.mergeWorker(row.id);
    expect(result.kind).toBe("error");
    if (result.kind === "error") {
      expect(result.message).toContain("parent session");
    }
    // spinner cleared
    expect(store.mergeStateByRunId.get(row.id)).toBeUndefined();
  });

  it("S4: discardWorker success → invoke + worktreePath null + spinner cleared", async () => {
    const store = useSubagentRunsStore();
    const row = await seedRow();
    const result = await store.discardWorker(row.id);
    expect(result.kind).toBe("success");
    expect(invokeMock).toHaveBeenCalledWith("discard_worker_run", {
      rid: "discard-pr4",
      runId: row.id,
    });
    expect(store.getRunCache.get(row.id)?.worktreePath).toBeNull();
    expect(store.mergeStateByRunId.get(row.id)).toBeUndefined();
  });

  it("S5: discardWorker error → returns error + spinner cleared", async () => {
    const store = useSubagentRunsStore();
    const row = await seedRow();
    invokeMock.mockRejectedValueOnce("worker already destroyed");
    const result = await store.discardWorker(row.id);
    expect(result.kind).toBe("error");
    expect(store.mergeStateByRunId.get(row.id)).toBeUndefined();
  });

  it("S6: spinner guard short-circuits second concurrent call", async () => {
    const store = useSubagentRunsStore();
    const row = await seedRow();
    // Make invoke slow so the first call is still in flight when
    // the second one fires.
    let resolveFirst: (v: string) => void = () => {};
    invokeMock.mockImplementationOnce(
      () => new Promise<string>((r) => { resolveFirst = r; }),
    );
    const first = store.mergeWorker(row.id);
    // While first is pending, spinner should be set.
    expect(store.mergeStateByRunId.get(row.id)).toBeDefined();
    const second = await store.mergeWorker(row.id);
    // Second short-circuited with an error result, did NOT invoke again.
    expect(second.kind).toBe("error");
    expect(invokeMock).toHaveBeenCalledTimes(1);
    // Release the first.
    resolveFirst("merged");
    const firstResult = await first;
    expect(firstResult.kind).toBe("success");
  });
});

// -----------------------------------------------------------------------
// C. Components
// -----------------------------------------------------------------------

describe("WorkerBranchBadge", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  function mountBadge(props: { status: string; worktreePath: string | null }) {
    return mount(WorkerBranchBadge, {
      props: { status: props.status as never, worktreePath: props.worktreePath },
    });
  }

  it("C1: hidden when worktreePath null + status !== running", () => {
    const w = mountBadge({ status: "completed", worktreePath: null });
    expect(w.find(".worker-branch-badge").exists()).toBe(false);
    w.unmount();
  });

  it("C2: shows 隔离中 when status running", () => {
    const w = mountBadge({ status: "running", worktreePath: null });
    expect(w.find(".worker-branch-badge").exists()).toBe(true);
    expect(w.text()).toContain("隔离中");
    w.unmount();
  });

  it("C3: shows 已完成·保留分支 when completed + worktreePath", () => {
    const w = mountBadge({
      status: "completed",
      worktreePath: "/data/worktrees/p/worker/r1",
    });
    expect(w.find(".worker-branch-badge").exists()).toBe(true);
    expect(w.text()).toContain("已完成 · 保留分支");
    w.unmount();
  });

  it("C3b: hidden for cancelled/error/incomplete (even with stale worktreePath)", () => {
    // Edge case: status flips to cancelled AFTER merge button was
    // visible. Badge should hide (the worker is no longer "running"
    // nor "completed-with-branch" — it's terminal-failed).
    for (const s of ["cancelled", "error", "incomplete"]) {
      const w = mountBadge({ status: s, worktreePath: "/some/path" });
      expect(w.find(".worker-branch-badge").exists(), `status=${s}`).toBe(false);
      w.unmount();
    }
  });
});

describe("WorkerMergeControls", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue("ok");
  });

  function mountControls(opts: {
    runId: string;
    worktreePath?: string | null;
    /** Defaults to "completed". Override to test the strict
     *  status-gate (cancelled / error / incomplete must NOT
     *  render the buttons even when worktreePath is set). */
    status?: string;
  }) {
    // Touch projects store so its showToast is reactive-ready.
    useProjectsStore();
    // Seed the store cache from the test's `worktreePath` + `status`
    // so the component's reactive `worktreePath` / `status`
    // computeds (which read `store.getRunCache.get(runId)`) reflect
    // the test's intent. The component itself declares only `runId`
    // as a prop — `worktreePath` + `status` are derived from the
    // cache (single source of truth, see component header comment).
    if (opts.worktreePath) {
      const store = useSubagentRunsStore();
      store.getRunCache.set(opts.runId, {
        ...baseRow,
        id: opts.runId,
        worktreePath: opts.worktreePath,
        status: (opts.status ?? "completed") as SubagentRunRow["status"],
      });
    }
    return mount(WorkerMergeControls, {
      // 06-30 follow-up: WorkerMergeControls now requires
      // `parentSessionId` so a successful lazy auto-attach can
      // trigger `chatStore.loadSessions` for the parent row.
      // Tests use the run's `parentSessionId` from `baseRow`
      // (defined earlier in the file) — see the test fixtures
      // section.
      props: { runId: opts.runId, parentSessionId: baseRow.parentSessionId },
      global: { stubs: { Icon: true } },
    });
  }

  it("C4: hidden when worktreePath null (cancelled / error / swept / merged)", () => {
    const w = mountControls({ runId: "r1", worktreePath: null });
    expect(w.find(".worker-merge-controls").exists()).toBe(false);
    w.unmount();
  });

  it("C5: visible when completed + worktreePath non-null", () => {
    const w = mountControls({ runId: "r1", worktreePath: "/p/worker/r1" });
    expect(w.find(".worker-merge-controls").exists()).toBe(true);
    expect(w.findAll("button").length).toBe(2); // Merge + Discard
    w.unmount();
  });

  it("C5b: hidden when worktreePath set but status is cancelled (strict gate)", () => {
    // Edge case: a cancelled worker still has the branch preserved
    // on disk (worktree_path NOT NULL), but per PRD §"Requirements"
    // + §"Edge Cases" the user MUST NOT be offered merge/discard
    // — cancelled worker exit-state signals "not user-actionable".
    for (const s of ["cancelled", "error", "incomplete"]) {
      const w = mountControls({
        runId: "r1",
        worktreePath: "/p/worker/r1",
        status: s,
      });
      expect(
        w.find(".worker-merge-controls").exists(),
        `status=${s} should hide the controls`,
      ).toBe(false);
      w.unmount();
    }
  });

  it("C6: Merge → ConfirmDialog → confirm → success toast + buttons disappear", async () => {
    const store = useSubagentRunsStore();
    const projects = useProjectsStore();
    store.getRunCache.set("r1", { ...baseRow, id: "r1" });
    const showToastSpy = vi.spyOn(projects, "showToast");

    const w = mountControls({ runId: "r1", worktreePath: "/p/worker/r1" });

    // Click Merge → ConfirmDialog opens.
    const buttons = w.findAll("button");
    await buttons[0].trigger("click"); // [0] = Merge
    await flushPromises();
    // ConfirmDialog renders inline (no Teleport) → query via wrapper.
    const confirmBtn = w.find(".confirm-modal__btn--warning");
    expect(confirmBtn.exists()).toBe(true);
    await confirmBtn.trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("merge_worker_run", {
      rid: "merge-pr4",
      runId: "r1",
    });
    // worktreePath cleared in store → component v-if flips → hidden.
    await flushPromises();
    expect(w.find(".worker-merge-controls").exists()).toBe(false);
    expect(showToastSpy).toHaveBeenCalledWith("已合并到 session 分支", "info");
    w.unmount();
  });

  it("C7: Merge conflict → file list rendered + error toast + branch preserved", async () => {
    const store = useSubagentRunsStore();
    const projects = useProjectsStore();
    store.getRunCache.set("r1", { ...baseRow, id: "r1" });
    const showToastSpy = vi.spyOn(projects, "showToast");
    invokeMock.mockRejectedValueOnce(
      "merge conflict: [src/x.rs, src/y.rs]. The worker branch 'worker/r1' and parent branch 'session/s1' both modified these files.",
    );

    const w = mountControls({ runId: "r1", worktreePath: "/p/worker/r1" });

    // Click Merge → confirm.
    await w.findAll("button")[0].trigger("click");
    await flushPromises();
    await w.find(".confirm-modal__btn--warning").trigger("click");
    await flushPromises();

    // Conflict file list rendered.
    expect(w.find(".worker-merge-controls__conflict").exists()).toBe(true);
    const items = w.findAll(".worker-merge-controls__conflict-list li");
    expect(items.length).toBe(2);
    expect(items[0].text()).toContain("src/x.rs");
    // Branch preserved → buttons stay visible.
    expect(w.find(".worker-merge-controls").exists()).toBe(true);
    expect(showToastSpy).toHaveBeenCalledWith(
      "合并冲突(2 个文件),请到 git CLI 解决后重试",
      "error",
    );
    w.unmount();
  });

  it("C8: Discard → ConfirmDialog → confirm → success toast + buttons disappear", async () => {
    const store = useSubagentRunsStore();
    const projects = useProjectsStore();
    store.getRunCache.set("r1", { ...baseRow, id: "r1" });
    const showToastSpy = vi.spyOn(projects, "showToast");

    const w = mountControls({ runId: "r1", worktreePath: "/p/worker/r1" });

    // Click Discard (buttons[1]) → confirm.
    await w.findAll("button")[1].trigger("click");
    await flushPromises();
    await w.find(".confirm-modal__btn--danger").trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("discard_worker_run", {
      rid: "discard-pr4",
      runId: "r1",
    });
    await flushPromises();
    expect(w.find(".worker-merge-controls").exists()).toBe(false);
    expect(showToastSpy).toHaveBeenCalledWith("已丢弃 worker 分支", "info");
    w.unmount();
  });

  it("C9: Cancel confirm does NOT fire invoke", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("r1", { ...baseRow, id: "r1" });

    const w = mountControls({ runId: "r1", worktreePath: "/p/worker/r1" });

    await w.findAll("button")[0].trigger("click"); // Merge
    await flushPromises();
    // Click 取消 (cancel).
    await w.find(".confirm-modal__btn--cancel").trigger("click");
    await flushPromises();

    expect(invokeMock).not.toHaveBeenCalled();
    // Buttons still visible (no action taken).
    expect(w.find(".worker-merge-controls").exists()).toBe(true);
    w.unmount();
  });

  it("C9b: Loading state disables both buttons (防双击)", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("r1", { ...baseRow, id: "r1" });
    // Hold the invoke so the spinner shows.
    let resolveMerge: (v: string) => void = () => {};
    invokeMock.mockImplementationOnce(
      () => new Promise<string>((r) => { resolveMerge = r; }),
    );

    const w = mountControls({ runId: "r1", worktreePath: "/p/worker/r1" });

    await w.findAll("button")[0].trigger("click"); // Merge
    await flushPromises();
    await w.find(".confirm-modal__btn--warning").trigger("click");
    await flushPromises();

    // While merge in flight: spinner on Merge + both disabled.
    expect(store.mergeStateByRunId.get("r1")?.kind).toBe("merge");
    expect(w.find(".worker-merge-controls__spinner").exists()).toBe(true);
    expect(w.findAll("button")[0].attributes("disabled")).toBeDefined();
    expect(w.findAll("button")[1].attributes("disabled")).toBeDefined();

    // Release.
    resolveMerge("ok");
    await flushPromises();
    w.unmount();
  });
});
