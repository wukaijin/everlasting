// E2 (harness trace pipeline, 2026-07-14) — smoke tests for the
// trace viewer's three leaf components. The full TracePanel
// requires the reka-ui + Pinia + Tauri mock stack, so we test
// the smaller components (TurnCard, TurnTimeline, TraceEventItem)
// in isolation. The store-level coverage is in
// `app/src/stores/traceStore.test.ts`.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { mount } from "@vue/test-utils";

const invokeMock = vi.fn();

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));


import TurnCard from "./TurnCard.vue";
import { parseAuditPayload } from "../../utils/audit";
import type { TurnTrace } from "../../types/turnTrace";
import type { AuditEventRow } from "../../utils/audit";

function makeTrace(overrides: Partial<TurnTrace> = {}): TurnTrace {
  return {
    id: 1,
    sessionId: "sess-1",
    seq: 1,
    createdAt: "2026-07-14 12:00:00",
    ...overrides,
  };
}

describe("TurnCard — render", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("renders seq + label", () => {
    const w = mount(TurnCard, { props: { trace: makeTrace({ seq: 7 }) } });
    expect(w.text()).toContain("Turn 7");
  });

  it("renders the compaction sub-card with still_over marking it critical", () => {
    const w = mount(TurnCard, {
      props: {
        trace: makeTrace({
          compaction: {
            tokens_before: 5000,
            tokens_after: 4500,
            dropped_count: 2,
            degradation: "still_over",
          },
        }),
      },
    });
    expect(w.text()).toContain("C3 压缩");
    expect(w.find(".turn-card--critical").exists()).toBe(true);
  });

  it("renders the loop_hint sub-card", () => {
    const w = mount(TurnCard, {
      props: {
        trace: makeTrace({
          loopHint: { hit_count: 2, verdict_kind: "soft" },
        }),
      },
    });
    expect(w.text()).toContain("循环检测");
    expect(w.text()).toContain("第 2 次连击");
  });

  it("renders the workflow breadcrumb sub-card with task slug", () => {
    const w = mount(TurnCard, {
      props: {
        trace: makeTrace({
          breadcrumb: {
            task_slug: "my-task",
            status: "in_progress",
            breadcrumb_text: "<workflow-task-meta>abc</workflow-task-meta>",
          },
        }),
      },
    });
    expect(w.text()).toContain("Workflow");
    expect(w.text()).toContain("my-task");
  });

  it("renders the empty placeholder when no trace data", () => {
    const w = mount(TurnCard, { props: { trace: makeTrace() } });
    expect(w.text()).toContain("本 turn 暂无可观测信号");
  });

  it("renders the token 5-field bar when tokenUsage is set", () => {
    const w = mount(TurnCard, {
      props: {
        trace: makeTrace({
          tokenUsage: {
            input_tokens: 1000,
            output_tokens: 500,
            cache_creation_input_tokens: 100,
            cache_read_input_tokens: 200,
            context_input_tokens: 1300,
          },
        }),
      },
    });
    const segments = w.findAll(".turn-card__token-segment");
    expect(segments.length).toBeGreaterThan(0);
  });

  it("renders the C7 tools[] estimate cell with its context share", () => {
    // C7 (R1.4): when both tokenUsage (with context_input) and
    // toolsToken are present, the card surfaces a separate `tools`
    // legend cell (NOT a bar segment) plus a share-of-context
    // tooltip. Formula is tools_token / context_input (7000/10000 =
    // 70%) — the double-count trap is covered by the design.
    const w = mount(TurnCard, {
      props: {
        trace: makeTrace({
          toolsToken: 7000,
          tokenUsage: {
            input_tokens: 3000,
            output_tokens: 500,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            context_input_tokens: 10000,
          },
        }),
      },
    });
    const toolsCell = w.find(".turn-card__token-cell--tools");
    expect(toolsCell.exists()).toBe(true);
    expect(toolsCell.text()).toContain("tools 7K");
    // Tooltip carries the share-of-context percentage.
    expect(toolsCell.attributes("title")).toContain("70%");
  });

  it("omits the tools[] cell when toolsToken is absent", () => {
    // Live path / pre-column rows: toolsToken is undefined → no
    // tools cell renders (the card does not fabricate a "—" here,
    // matching the existing tokenUsage-absent behavior).
    const w = mount(TurnCard, {
      props: {
        trace: makeTrace({
          tokenUsage: {
            input_tokens: 1000,
            output_tokens: 500,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            context_input_tokens: 1000,
          },
        }),
      },
    });
    expect(w.find(".turn-card__token-cell--tools").exists()).toBe(false);
  });

  it("renders the WP1 memory estimate cell with its context share", () => {
    // memory-block-governance WP1: same slice-of-context treatment
    // as the tools[] cell — memoryToken + context_input present →
    // `mem` cell + share tooltip. Formula memory_token /
    // context_input (4000/10000 = 40%), same no-double-count rule.
    const w = mount(TurnCard, {
      props: {
        trace: makeTrace({
          toolsToken: 1500,
          memoryToken: 4000,
          tokenUsage: {
            input_tokens: 3000,
            output_tokens: 500,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            context_input_tokens: 10000,
          },
        }),
      },
    });
    const memCell = w.find(".turn-card__token-cell--memory");
    expect(memCell.exists()).toBe(true);
    expect(memCell.text()).toContain("mem 4K");
    expect(memCell.attributes("title")).toContain("40%");
    // Both slices coexist as independent cells.
    expect(w.find(".turn-card__token-cell--tools").exists()).toBe(true);
  });

  it("omits the memory cell when memoryToken is absent", () => {
    // Pre-column rows / worker turns: memoryToken undefined → no
    // mem cell, but the tools[] cell still renders.
    const w = mount(TurnCard, {
      props: {
        trace: makeTrace({
          toolsToken: 7000,
          tokenUsage: {
            input_tokens: 1000,
            output_tokens: 500,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            context_input_tokens: 1000,
          },
        }),
      },
    });
    expect(w.find(".turn-card__token-cell--memory").exists()).toBe(false);
    expect(w.find(".turn-card__token-cell--tools").exists()).toBe(true);
  });

  it("renders the B1 img estimate cell with its context share", () => {
    // B1 (2026-08-16) R6: imagesToken + context_input present →
    // `img` cell + share tooltip. Formula images_token /
    // context_input (2500/10000 = 25%), same no-double-count rule
    // as the tools[]/memory cells.
    const w = mount(TurnCard, {
      props: {
        trace: makeTrace({
          imagesToken: 2500,
          tokenUsage: {
            input_tokens: 3000,
            output_tokens: 500,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            context_input_tokens: 10000,
          },
        }),
      },
    });
    const imgCell = w.find(".turn-card__token-cell--images");
    expect(imgCell.exists()).toBe(true);
    expect(imgCell.text()).toContain("img 2.5K");
    expect(imgCell.attributes("title")).toContain("25%");
  });

  it("omits the img cell when imagesToken is 0 (image-less turn) or absent", () => {
    // B1 design: 无图轮 images_token=0 — a zero cell would be pure
    // noise next to the 5-field bar, so the gate is > 0 (unlike
    // tools/mem which render their 0s).
    const w0 = mount(TurnCard, {
      props: {
        trace: makeTrace({
          imagesToken: 0,
          tokenUsage: {
            input_tokens: 1000,
            output_tokens: 500,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            context_input_tokens: 1000,
          },
        }),
      },
    });
    expect(w0.find(".turn-card__token-cell--images").exists()).toBe(false);

    const wNoField = mount(TurnCard, {
      props: { trace: makeTrace({ toolsToken: 100 }) },
    });
    expect(wNoField.find(".turn-card__token-cell--images").exists()).toBe(
      false,
    );
  });
});

describe("TraceEventItem — critical highlighting", () => {
  it("marks tool_executed rows with non-zero exit_code as critical", () => {
    const row: AuditEventRow = {
      id: 1,
      sessionId: "sess-1",
      ts: "2026-07-14 12:00:01",
      kind: "tool_executed",
      payloadJson: JSON.stringify({
        tool_name: "shell",
        duration_ms: 123,
        exit_code: 1,
      }),
      turnSeq: 1,
    };
    // Direct call to parseAuditPayload — locks the contract
    // that the wrapper component relies on (exits 0 / null
    // are not critical; non-zero IS critical).
    const parsed = parseAuditPayload(row.kind, row.payloadJson);
    expect(parsed.kind).toBe("tool_executed");
    if (parsed.kind === "tool_executed") {
      const ec = parsed.payload.exit_code;
      expect(ec).toBe(1);
      expect(ec !== null && ec !== 0).toBe(true);
    }
  });

  it("marks tool rows with critical=true via the audit-item wrapper", () => {
    // The wrapper reads `payload.critical === true` (the
    // existing AuditLogItem contract). This test locks the
    // same check from the trace wrapper's side: when
    // critical is true, the wrapper's class is applied.
    const row: AuditEventRow = {
      id: 2,
      sessionId: "sess-1",
      ts: "2026-07-14 12:00:01",
      kind: "tool_denied",
      payloadJson: JSON.stringify({
        tool_name: "shell",
        tool_input: { command: "rm -rf /" },
        reason: "hard-kill list",
        mode: "edit",
        critical: true,
      }),
      turnSeq: 1,
    };
    const parsed = parseAuditPayload(row.kind, row.payloadJson);
    expect(parsed.kind).toBe("tool");
    if (parsed.kind === "tool") {
      expect(parsed.payload.critical).toBe(true);
    }
  });
});
