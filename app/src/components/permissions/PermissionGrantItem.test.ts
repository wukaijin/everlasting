// Tests for `PermissionGrantItem.vue` — a single grant row in the
// PermissionGrantsModal list (task 07-01-permission-grant-list-ui).
//
// Coverage: renders the three match_kind variants (tool / path /
// prefix) with the right badge + value, and emits `revoke` with
// the full row when the 撤销 button is clicked.

import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";
import PermissionGrantItem from "./PermissionGrantItem.vue";
import type { PermissionGrantRow } from "../../stores/permissionGrants";

const pathRow = (): PermissionGrantRow => ({
  sessionId: "sess-1",
  toolName: "read_file",
  matchKind: "path",
  matchValue: "src/*",
  grantedAt: "2026-07-01 10:00:00",
});

describe("PermissionGrantItem", () => {
  it("renders a path-kind row with badge + tool + glob value", () => {
    const w = mount(PermissionGrantItem, { props: { row: pathRow() } });
    expect(w.text()).toContain("路径");
    expect(w.text()).toContain("read_file");
    expect(w.text()).toContain("src/*");
    expect(w.text()).toContain("撤销");
  });

  it("renders a tool-kind row with '—' for the null matchValue", () => {
    const w = mount(PermissionGrantItem, {
      props: {
        row: {
          sessionId: "sess-1",
          toolName: "web_fetch",
          matchKind: "tool",
          matchValue: null,
          grantedAt: "2026-07-01 10:00:00",
        } satisfies PermissionGrantRow,
      },
    });
    expect(w.text()).toContain("整工具");
    expect(w.text()).toContain("web_fetch");
    expect(w.text()).toContain("—");
    // The null-value span is rendered (not a <code> glob).
    expect(w.find(".grant-item__value--null").exists()).toBe(true);
  });

  it("renders a prefix-kind row with the token", () => {
    const w = mount(PermissionGrantItem, {
      props: {
        row: {
          sessionId: "sess-1",
          toolName: "shell",
          matchKind: "prefix",
          matchValue: "git",
          grantedAt: "2026-07-01 10:00:00",
        } satisfies PermissionGrantRow,
      },
    });
    expect(w.text()).toContain("前缀");
    expect(w.text()).toContain("git");
  });

  it("clicking 撤销 emits revoke with the full row", async () => {
    const row = pathRow();
    const w = mount(PermissionGrantItem, { props: { row } });
    await w.get(".grant-item__revoke").trigger("click");
    expect(w.emitted("revoke")).toBeTruthy();
    expect(w.emitted("revoke")![0]).toEqual([row]);
  });
});
