// Tests for `AuditLogItem.vue` — pure presentation row in the
// AuditLogModal list (C4 audit-log query UI).
//
// The component dispatches on the parsed payload's kind family
// (tool / tool_executed / mode / loop_intervention / raw). These
// tests drive the component with hand-built `AuditEventRow` props
// + assert on the rendered DOM. No store, no IPC, no async.
//
// C2+ (2026-07-05, task `07-05-c2-loop-active-intervention`):
// the `loop_intervention` kind's three actions land here — `asked`
// (QuestionStore registration succeeded), `terminated` (user
// picked「终止 loop」), `continued` (user picked「继续」). The
// kind label, icon family, and summary-line format are all
// asserted so a future refactor of the icon/color/label mapping
// surfaces as a test break instead of a silent regression.

import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";

import AuditLogItem from "./AuditLogItem.vue";
import {
  AUDIT_KIND_OPTIONS,
  iconFamilyForKind,
  labelForKind,
  type AuditEventRow,
} from "../../utils/audit";

function buildRow(kind: string, payloadJson: string | null): AuditEventRow {
  return {
    id: 1,
    sessionId: "s1",
    ts: "2026-07-05 12:34:56",
    kind,
    payloadJson,
    // E2 (2026-07-14): turn_seq column added by v7 migration.
    // The C4 AuditLogItem renders rows without inspecting this
    // field; the trace viewer reads it. The test fixture
    // defaults to `null` (pre-v7 historical shape) — the
    // AuditLogItem render path is unchanged.
    turnSeq: null,
  };
}

describe("AuditLogItem — loop_intervention (C2+)", () => {
  it("labelForKind returns 循环检测干预 for loop_intervention", () => {
    expect(labelForKind("loop_intervention")).toBe("循环检测干预");
  });

  it("AUDIT_KIND_OPTIONS includes loop_intervention with chinese label", () => {
    const opt = AUDIT_KIND_OPTIONS.find((o) => o.value === "loop_intervention");
    expect(opt).toBeDefined();
    expect(opt?.label).toBe("循环检测干预");
  });

  it("iconFamilyForKind routes loop_intervention to its own family", () => {
    expect(iconFamilyForKind("loop_intervention")).toBe("loop-intervention");
  });

  it("renders the asked-action summary with hard verdict", () => {
    const payload = JSON.stringify({
      hit_count: 3,
      verdict_kind: "hard",
      action: "asked",
      run_id: null,
    });
    const w = mount(AuditLogItem, {
      props: { row: buildRow("loop_intervention", payload) },
    });
    const text = w.text();
    expect(text).toContain("循环检测干预");
    expect(text).toContain("硬触发");
    expect(text).toContain("第 3 次命中");
    expect(text).toContain("询问");
  });

  it("renders the terminated action with soft verdict", () => {
    const payload = JSON.stringify({
      hit_count: 5,
      verdict_kind: "soft",
      action: "terminated",
      run_id: null,
    });
    const w = mount(AuditLogItem, {
      props: { row: buildRow("loop_intervention", payload) },
    });
    const text = w.text();
    expect(text).toContain("软触发");
    expect(text).toContain("第 5 次命中");
    expect(text).toContain("已终止");
  });

  it("renders the continued action", () => {
    const payload = JSON.stringify({
      hit_count: 3,
      verdict_kind: "hard",
      action: "continued",
      run_id: null,
    });
    const w = mount(AuditLogItem, {
      props: { row: buildRow("loop_intervention", payload) },
    });
    expect(w.text()).toContain("已继续");
  });

  it("renders summary line via the .audit-item__loop element", () => {
    const payload = JSON.stringify({
      hit_count: 3,
      verdict_kind: "hard",
      action: "asked",
      run_id: null,
    });
    const w = mount(AuditLogItem, {
      props: { row: buildRow("loop_intervention", payload) },
    });
    const loopEl = w.find(".audit-item__loop");
    expect(loopEl.exists()).toBe(true);
    expect(loopEl.text()).toContain("循环检测干预");
    expect(loopEl.text()).toContain("硬触发");
    expect(loopEl.text()).toContain("询问");
  });

  it("renders the kind chip with the 循环检测干预 label", () => {
    const payload = JSON.stringify({
      hit_count: 3,
      verdict_kind: "hard",
      action: "asked",
      run_id: null,
    });
    const w = mount(AuditLogItem, {
      props: { row: buildRow("loop_intervention", payload) },
    });
    const kindChip = w.find(".audit-item__kind");
    expect(kindChip.exists()).toBe(true);
    expect(kindChip.text()).toBe("循环检测干预");
  });

  it("defensive: missing fields fall back gracefully", () => {
    // Malformed payload missing verdict_kind + action — the
    // summary line should fall back without crashing.
    const payload = JSON.stringify({ hit_count: 3 });
    const w = mount(AuditLogItem, {
      props: { row: buildRow("loop_intervention", payload) },
    });
    const text = w.text();
    // verdict_kind defaults to "软触发" (else branch), action
    // defaults to "?" (the unknown-action fallback).
    expect(text).toContain("软触发");
    expect(text).toContain("第 3 次命中");
  });

  it("defensive: null payloadJson renders kind chip but no summary", () => {
    const w = mount(AuditLogItem, {
      props: { row: buildRow("loop_intervention", null) },
    });
    // kind chip is always rendered from the row.kind field
    const kindChip = w.find(".audit-item__kind");
    expect(kindChip.exists()).toBe(true);
    expect(kindChip.text()).toBe("循环检测干预");
    // no payload → no summary line
    const loopEl = w.find(".audit-item__loop");
    expect(loopEl.exists()).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// N5 (09-26-sandbox-failopen-audit): the `sandbox` attribution chip on
// shell-family `tool_executed` rows. Tone contract: anomaly
// (failopen / escalation / unknown) renders with the warn class;
// routine (sandboxed / yolo / …) renders muted; absence renders
// nothing (legacy row / non-shell tool / pre-decision failure).
// ---------------------------------------------------------------------------

describe("AuditLogItem — sandbox attribution chip (N5)", () => {
  const shellRow = (sandbox?: string) =>
    buildRow(
      "tool_executed",
      JSON.stringify({
        tool_name: "shell",
        duration_ms: 812,
        exit_code: 0,
        ...(sandbox === undefined ? {} : { sandbox }),
      }),
    );

  it("failopen renders the anomaly chip with the warn class", () => {
    const w = mount(AuditLogItem, { props: { row: shellRow("failopen") } });
    const chip = w.find(".audit-item__sandbox");
    expect(chip.exists()).toBe(true);
    expect(chip.text()).toContain("沙箱能力缺失,裸跑");
    expect(chip.classes()).toContain("audit-item__sandbox--anomaly");
  });

  it("escalation renders the anomaly chip", () => {
    const w = mount(AuditLogItem, { props: { row: shellRow("escalation") } });
    const chip = w.find(".audit-item__sandbox");
    expect(chip.text()).toContain("批准后不沙箱重跑");
    expect(chip.classes()).toContain("audit-item__sandbox--anomaly");
  });

  it("sandboxed / yolo render the routine chip WITHOUT the warn class", () => {
    for (const value of ["sandboxed", "yolo", "grant"]) {
      const w = mount(AuditLogItem, { props: { row: shellRow(value) } });
      const chip = w.find(".audit-item__sandbox");
      expect(chip.exists(), value).toBe(true);
      expect(chip.classes(), value).not.toContain("audit-item__sandbox--anomaly");
    }
  });

  it("missing field renders NO chip (legacy / non-shell row)", () => {
    const w = mount(AuditLogItem, { props: { row: shellRow() } });
    expect(w.find(".audit-item__sandbox").exists()).toBe(false);
  });

  it("unknown wire value degrades to anomaly-toned raw echo", () => {
    const w = mount(AuditLogItem, { props: { row: shellRow("quantum_tunnel") } });
    const chip = w.find(".audit-item__sandbox");
    expect(chip.text()).toBe("quantum_tunnel");
    expect(chip.classes()).toContain("audit-item__sandbox--anomaly");
  });
});
