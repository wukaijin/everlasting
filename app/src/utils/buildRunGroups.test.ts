// D2 (08-17-cross-session-search) — `buildRunGroups` extraction
// equivalence: the SearchPreviewBody must render with the SAME
// interleaved-thinking grouping contract as MessageList
// (5b1fc81): real user turn opens a group; ghost-user
// tool_results / orphan-repair rows fold into the current group;
// a non-user first row gets a defensive standalone group.

import { describe, it, expect } from "vitest";
import { buildRunGroups } from "./messageFormat";

function msg(id: string, role: "user" | "assistant", extra: Record<string, unknown> = {}) {
  return { id, role, ...extra };
}

describe("buildRunGroups", () => {
  it("opens a group per real user turn and folds assistant rows in", () => {
    const groups = buildRunGroups([
      msg("u1", "user"),
      msg("a1", "assistant"),
      msg("a2", "assistant"),
      msg("u2", "user"),
      msg("a3", "assistant"),
    ]);
    expect(groups.map((g) => g.key)).toEqual(["u1", "u2"]);
    expect(groups[0].items.map((m) => m.id)).toEqual(["u1", "a1", "a2"]);
    expect(groups[1].items.map((m) => m.id)).toEqual(["u2", "a3"]);
  });

  it("ghost user rows (toolResults) do NOT open a group", () => {
    const groups = buildRunGroups([
      msg("u1", "user"),
      msg("ghost-1", "user", { toolResults: [{ id: "t1" }] }),
      msg("ghost-2", "user", { toolResults: [{ id: "t2" }] }),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0].items.map((m) => m.id)).toEqual(["u1", "ghost-1", "ghost-2"]);
  });

  it("orphan-repair synthetic rows fold into the current group", () => {
    const groups = buildRunGroups([
      msg("u1", "user"),
      msg("x-orphan-repair", "user"),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0].items).toHaveLength(2);
  });

  it("non-user first row gets a defensive standalone group (no drop)", () => {
    const groups = buildRunGroups([msg("a1", "assistant"), msg("u1", "user")]);
    expect(groups.map((g) => g.key)).toEqual(["a1", "u1"]);
  });

  it("empty input → empty groups", () => {
    expect(buildRunGroups([])).toEqual([]);
  });
});
