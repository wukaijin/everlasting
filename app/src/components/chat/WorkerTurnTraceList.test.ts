// Tests for `WorkerTurnTraceList.vue` — 08-20-worker-turn-trace-persist
// PR3(SubagentDrawer per-run「Token 明细」折叠区)。
//
// Covers:
//   1. 默认收起(body 不渲染)。
//   2. 点击 header 展开 → 触发 store.loadRunTurnTraces(force 跟随
//      run 状态:running 态 force,终态不 force)。
//   3. 行渲染:轮号 / in / out / cache读 / 命中率 / tools / ctx占比
//      (数字来自 parseTurnTraceRow 解析后的 TurnTrace)。
//   4. 空态(后端返回 [] = 旧 run / 迁移前数据)。
//   5. 失败态(runTracesError 一行降级文案,不崩)。
//
// Mount 策略:真实 Pinia store + mock transport(与 store 测试同一套
// invokeMock 手法)—— 展开动作经组件 → store → transport 全链,断言
// loadRunTurnTraces 被以正确参数调用。Icon 桩同 DrawerToolCallCard
// 测试先例(svg 渲染无断言价值)。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import WorkerTurnTraceList from "./WorkerTurnTraceList.vue";
import { useSubagentRunsStore } from "../../stores/subagentRuns";
import type { TurnTraceRow } from "../../types/turnTrace";

const Icon = { template: "<span aria-hidden=\"true\" />" };

/** 单轮 worker 行 wire fixture:usage + tools/system/window(worker
 * 行契约列;memory/images/@文件 null)。 */
function traceRow(overrides: Partial<TurnTraceRow> = {}): TurnTraceRow {
  return {
    id: 1,
    sessionId: "sess-parent",
    runId: "run-1",
    seq: 42,
    tokenUsageJson:
      '{"input_tokens":50000,"output_tokens":800,"cache_creation_input_tokens":100,"cache_read_input_tokens":40000,"context_input_tokens":90100}',
    compactionJson: null,
    loopHintJson: null,
    breadcrumbJson: null,
    toolsToken: 1200,
    memoryToken: null,
    imagesToken: null,
    atFilesToken: null,
    systemToken: 2500,
    contextWindow: 100000,
    createdAt: "2026-08-20 00:00:00",
    ...overrides,
  };
}

function mountList(runId = "run-1") {
  // 不注入独立 pinia —— 组件经 useSubagentRunsStore() 解析
  // beforeEach 设的 active pinia,测试侧 store 取同一实例。
  return mount(WorkerTurnTraceList, {
    props: { runId },
    global: {
      stubs: { Icon },
    },
  });
}

describe("WorkerTurnTraceList", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    setActivePinia(createPinia());
  });

  it("renders collapsed by default (body hidden)", () => {
    invokeMock.mockResolvedValue([]);
    const wrapper = mountList();
    expect(wrapper.find(".worker-turn-trace__body").exists()).toBe(false);
    // 未展开不应有任何 IPC。
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("expand triggers loadRunTurnTraces without force for a terminal run", async () => {
    invokeMock.mockResolvedValue([traceRow()]);
    const wrapper = mountList();
    await wrapper.find(".worker-turn-trace__header").trigger("click");

    expect(invokeMock).toHaveBeenCalledWith("list_worker_turn_traces", {
      runId: "run-1",
    });
    expect(wrapper.find(".worker-turn-trace__table").exists()).toBe(true);
  });

  it("expand passes force=true while the run is still running", async () => {
    invokeMock.mockResolvedValue([traceRow()]);
    const wrapper = mountList();
    const store = useSubagentRunsStore();
    // Seed the detail cache so the component's isRunning computed
    // sees the run's status (store.getRunCache is the SoT).
    store.getRunCache.set("run-1", {
      id: "run-1",
      parentSessionId: "sess-parent",
      parentRequestId: "req-1",
      subagentName: "researcher",
      status: "running",
      startedAt: "2026-08-20 00:00:00",
      finishedAt: null,
      tokenUsageJson: null,
      summary: null,
      transcriptJson: null,
      transcriptTruncated: 0,
      createdAt: "2026-08-20 00:00:00",
      finalText: null,
      task: null,
      turnCount: null,
      worktreePath: null,
      modelDisplay: null,
    });

    await wrapper.find(".worker-turn-trace__header").trigger("click");
    // force 语义在 store 层测试已锁;这里断言展开动作确实发起了拉取
    // (running 态 expand = force 重拉路径)。
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it("renders per-turn cells (seq / usage / cache rate / tools / ctx%)", async () => {
    invokeMock.mockResolvedValue([traceRow()]);
    const wrapper = mountList();
    await wrapper.find(".worker-turn-trace__header").trigger("click");

    const cells = wrapper
      .findAll(".worker-turn-trace__table tbody tr td")
      .map((td) => td.text());
    // ctx 90100/100000 → 90%;cache 命中 40000/90100 → 44%;
    // tools 1200 未达 10k 阈值,原样展示。
    expect(cells).toEqual([
      "#42",
      "50k",
      "800",
      "40k",
      "44%",
      "1200",
      "90%",
    ]);
  });

  it("renders the empty state for a run with no trace rows", async () => {
    invokeMock.mockResolvedValue([]);
    const wrapper = mountList();
    await wrapper.find(".worker-turn-trace__header").trigger("click");

    expect(wrapper.find(".worker-turn-trace__empty").text()).toContain(
      "无 per-turn 记录",
    );
  });

  it("renders a degraded error line when the fetch fails", async () => {
    invokeMock.mockResolvedValue([traceRow()]);
    const wrapper = mountList();
    await wrapper.find(".worker-turn-trace__header").trigger("click");

    invokeMock.mockRejectedValue("ipc down");
    const store = useSubagentRunsStore();
    await store.loadRunTurnTraces("run-1", { force: true });

    const err = wrapper.find(".worker-turn-trace__error");
    expect(err.exists()).toBe(true);
    expect(err.text()).toContain("ipc down");
  });
});
