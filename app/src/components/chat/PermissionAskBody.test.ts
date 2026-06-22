// Tests for `PermissionAskBody.vue` — shared permission-ask body
// component (FT-F-001 PR1, 2026-06-20).
//
// Covers both modes:
//   - `interactive`: renders the 4-action approval UI (仅一次 /
//     始终允许 / 拒绝 / 拒绝并说明), the feedback textarea on
//     "拒绝并说明", the path range row + 仓库内/仓库外 badge.
//   - `historical`: renders an info-only marker, NO buttons.
//
// Both modes share the risk dot + risk label + reason line.

import { describe, it, expect, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import {
  type PermissionAsk,
} from "../../stores/permissions";
import PermissionAskBody from "./PermissionAskBody.vue";

function makeAsk(overrides: Partial<PermissionAsk> = {}): PermissionAsk {
  return {
    rid: "rid-1",
    sessionId: "sess-1",
    toolUseId: "tu-1",
    toolName: "shell",
    toolInput: { command: "rm -rf /tmp/x" },
    risk: "high",
    ...overrides,
  };
}

describe("PermissionAskBody interactive mode", () => {
  it("renders the 4 approval action buttons", () => {
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk(),
        onRespond: vi.fn(),
        repoRoot: "/data/repo",
      },
    });
    const text = w.text();
    expect(text).toContain("仅一次");
    expect(text).toContain("始终允许");
    expect(text).toContain("拒绝");
    expect(text).toContain("拒绝并说明");
    // The header reads "需要权限" in interactive mode.
    expect(text).toContain("需要权限");
    // The risk label appears in Chinese.
    expect(text).toContain("高");
  });

  it("clicking 仅一次 fires onRespond('allow_once')", async () => {
    const onRespond = vi.fn();
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk(),
        onRespond,
        repoRoot: "/data/repo",
      },
    });
    await w
      .get(".permission-ask-body__btn--once")
      .trigger("click");
    expect(onRespond).toHaveBeenCalledWith("allow_once");
  });

  it("clicking 始终允许 fires onRespond('allow_always')", async () => {
    const onRespond = vi.fn();
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk(),
        onRespond,
        repoRoot: "/data/repo",
      },
    });
    await w
      .get(".permission-ask-body__btn--always")
      .trigger("click");
    expect(onRespond).toHaveBeenCalledWith("allow_always");
  });

  it("clicking 拒绝 fires onRespond('deny') with no reason", async () => {
    const onRespond = vi.fn();
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk(),
        onRespond,
        repoRoot: "/data/repo",
      },
    });
    // First --deny button is "拒绝", second is "拒绝并说明".
    await w
      .findAll(".permission-ask-body__btn--deny")[0]
      .trigger("click");
    expect(onRespond).toHaveBeenCalledWith("deny");
  });

  it("拒绝并说明 opens a textarea + submits feedback as the deny reason", async () => {
    const onRespond = vi.fn();
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk(),
        onRespond,
        repoRoot: "/data/repo",
      },
    });
    // No textarea before opening.
    expect(w.find(".permission-ask-body__textarea").exists()).toBe(false);
    // Open the feedback form (second --deny button).
    await w
      .findAll(".permission-ask-body__btn--deny")[1]
      .trigger("click");
    expect(w.find(".permission-ask-body__textarea").exists()).toBe(true);
    // Type feedback + submit.
    await w.get("textarea").setValue("用 git clean 代替");
    await w
      .get(
        ".permission-ask-body__feedback-actions .permission-ask-body__btn--deny",
      )
      .trigger("click");
    expect(onRespond).toHaveBeenCalledWith("deny", "用 git clean 代替");
  });

  it("拒绝并说明 with empty feedback submits deny with no reason", async () => {
    const onRespond = vi.fn();
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk(),
        onRespond,
        repoRoot: "/data/repo",
      },
    });
    // Open feedback form but leave textarea empty.
    await w
      .findAll(".permission-ask-body__btn--deny")[1]
      .trigger("click");
    await w
      .get(
        ".permission-ask-body__feedback-actions .permission-ask-body__btn--deny",
      )
      .trigger("click");
    // Empty feedback → reason is undefined (not empty string).
    // The actual call shape: onRespond("deny", undefined). The
    // .trim() guard turns an empty string into undefined.
    expect(onRespond).toHaveBeenCalledWith("deny", undefined);
  });

  it("取消 button closes the feedback form without firing onRespond", async () => {
    const onRespond = vi.fn();
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk(),
        onRespond,
        repoRoot: "/data/repo",
      },
    });
    // Open feedback form.
    await w
      .findAll(".permission-ask-body__btn--deny")[1]
      .trigger("click");
    expect(w.find(".permission-ask-body__textarea").exists()).toBe(true);
    // Click 取消 — second button in feedback-actions row.
    await w
      .get(".permission-ask-body__feedback-actions .permission-ask-body__btn:not(.permission-ask-body__btn--deny)")
      .trigger("click");
    expect(w.find(".permission-ask-body__textarea").exists()).toBe(false);
    expect(onRespond).not.toHaveBeenCalled();
  });

  it("does NOT fire onRespond when no callback is provided (silent no-op)", async () => {
    // Defensive: even if onRespond is omitted, clicking a button
    // does not throw. Matches the historical-mode silent behavior
    // when called with mode='interactive' by mistake.
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk(),
        // onRespond intentionally omitted.
        repoRoot: "/data/repo",
      },
    });
    // Buttons are gated to not render at all when onRespond is
    // absent (the v-if guard excludes them).
    expect(w.find(".permission-ask-body__btn--once").exists()).toBe(false);
    expect(w.find(".permission-ask-body__btn--always").exists()).toBe(false);
    expect(
      w.findAll(".permission-ask-body__btn--deny").length,
    ).toBe(0);
  });

  it("renders the reason line when ask.reason is present", () => {
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk({ reason: "matches denylist: rm -rf /" }),
        onRespond: vi.fn(),
        repoRoot: "/data/repo",
      },
    });
    expect(w.find(".permission-ask-body__reason").exists()).toBe(true);
    expect(w.text()).toContain("matches denylist");
  });

  it("renders the path range row with 仓库内 badge when path is inside repoRoot", () => {
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk({ path: "/data/repo/src/foo.ts" }),
        onRespond: vi.fn(),
        repoRoot: "/data/repo",
      },
    });
    expect(w.find(".permission-ask-body__path").exists()).toBe(true);
    expect(w.find(".permission-ask-body__path code").text()).toBe(
      "/data/repo/src/foo.ts",
    );
    expect(w.find(".permission-ask-body__badge").text()).toBe("仓库内");
  });

  it("renders the path range row with 仓库外 badge when path is outside repoRoot", () => {
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk({ path: "/etc/passwd" }),
        onRespond: vi.fn(),
        repoRoot: "/data/repo",
      },
    });
    expect(w.find(".permission-ask-body__path").exists()).toBe(true);
    expect(w.find(".permission-ask-body__badge").text()).toBe("仓库外");
  });

  it("omits the path range row entirely when ask.path is absent (shell / web_fetch)", () => {
    const w = mount(PermissionAskBody, {
      props: {
        mode: "interactive",
        ask: makeAsk({ path: undefined }),
        onRespond: vi.fn(),
        repoRoot: "/data/repo",
      },
    });
    // The .permission-ask-body__path row must not render at all
    // (matches the backend's #[serde(skip_serializing_if =
    // "Option::is_none")] contract).
    expect(w.find(".permission-ask-body__path").exists()).toBe(false);
  });

  // N3 (RULE-FrontSubagent-003 check phase, 2026-06-22): the
  // "始终允许" (allow_always) button must be hideable via the
  // `hideAllowAlways` prop. The worker-ask path
  // (`DrawerPermissionAskCard` derives this from `ask.workerRunId`)
  // hides it because the backend worker AllowAlways arm treats
  // the response as AllowOnce — persisting worker grants to
  // `session_tool_permissions` would cross privilege boundaries.
  // The main-chat ToolCallCard path does NOT set the prop and
  // keeps all 4 buttons.
  describe("hideAllowAlways prop (N3: worker asks hide 始终允许)", () => {
    it("default (false) renders all 4 buttons including 始终允许", () => {
      const w = mount(PermissionAskBody, {
        props: {
          mode: "interactive",
          ask: makeAsk(),
          onRespond: vi.fn(),
          repoRoot: "/data/repo",
          // hideAllowAlways intentionally omitted — default false.
        },
      });
      expect(w.find(".permission-ask-body__btn--always").exists()).toBe(true);
      expect(w.text()).toContain("始终允许");
      // 4 buttons total: 仅一次 / 始终允许 / 拒绝 / 拒绝并说明.
      expect(w.findAll(".permission-ask-body__btn").length).toBe(4);
    });

    it("hideAllowAlways=true does NOT render the 始终允许 button", () => {
      const w = mount(PermissionAskBody, {
        props: {
          mode: "interactive",
          ask: makeAsk(),
          onRespond: vi.fn(),
          repoRoot: "/data/repo",
          hideAllowAlways: true,
        },
      });
      expect(w.find(".permission-ask-body__btn--always").exists()).toBe(false);
      expect(w.text()).not.toContain("始终允许");
      // 3 buttons total: 仅一次 / 拒绝 / 拒绝并说明.
      expect(w.findAll(".permission-ask-body__btn").length).toBe(3);
      // The other 3 buttons still render.
      expect(w.find(".permission-ask-body__btn--once").exists()).toBe(true);
      expect(w.findAll(".permission-ask-body__btn--deny").length).toBe(2);
    });

    it("hideAllowAlways=true does NOT affect historical mode (no buttons either way)", () => {
      const w = mount(PermissionAskBody, {
        props: {
          mode: "historical",
          ask: makeAsk(),
          repoRoot: "/data/repo",
          hideAllowAlways: true,
        },
      });
      // Historical mode never renders action buttons regardless.
      expect(w.find(".permission-ask-body__btn--always").exists()).toBe(false);
      expect(w.find(".permission-ask-body__btn--once").exists()).toBe(false);
      expect(w.findAll(".permission-ask-body__btn--deny").length).toBe(0);
    });
  });
});

describe("PermissionAskBody historical mode", () => {
  it("renders the info-only marker", () => {
    const w = mount(PermissionAskBody, {
      props: {
        mode: "historical",
        ask: makeAsk({ toolName: "shell" }),
        repoRoot: "/data/repo",
      },
    });
    expect(w.find(".permission-ask-body__historical-note").exists()).toBe(
      true,
    );
    expect(w.text()).toContain("worker asked for shell");
  });

  it("does NOT render any action buttons in historical mode", () => {
    const w = mount(PermissionAskBody, {
      props: {
        mode: "historical",
        ask: makeAsk(),
        repoRoot: "/data/repo",
      },
    });
    expect(w.find(".permission-ask-body__btn--once").exists()).toBe(false);
    expect(w.find(".permission-ask-body__btn--always").exists()).toBe(false);
    expect(
      w.findAll(".permission-ask-body__btn--deny").length,
    ).toBe(0);
    expect(w.find(".permission-ask-body__textarea").exists()).toBe(false);
  });

  it("does NOT fire onRespond even when provided in historical mode", async () => {
    const onRespond = vi.fn();
    const w = mount(PermissionAskBody, {
      props: {
        mode: "historical",
        ask: makeAsk(),
        onRespond,
        repoRoot: "/data/repo",
      },
    });
    // No buttons → no way to fire onRespond through UI.
    // The historical note is the only interactive surface and
    // it's pure text. Defensive: even a synthetic click on the
    // note does nothing.
    await w
      .find(".permission-ask-body__historical-note")
      .trigger("click");
    await flushPromises();
    expect(onRespond).not.toHaveBeenCalled();
  });

  it("renders the risk label (still surfaces in historical mode)", () => {
    const w = mount(PermissionAskBody, {
      props: {
        mode: "historical",
        ask: makeAsk({ risk: "critical" }),
        repoRoot: "/data/repo",
      },
    });
    expect(w.text()).toContain("极高");
    // The title text is "权限询问" (not "需要权限") in historical.
    expect(w.text()).toContain("权限询问");
  });

  it("renders the path range row when ask.path is present", () => {
    const w = mount(PermissionAskBody, {
      props: {
        mode: "historical",
        ask: makeAsk({ path: "/data/repo/src/foo.ts" }),
        repoRoot: "/data/repo",
      },
    });
    expect(w.find(".permission-ask-body__path").exists()).toBe(true);
    expect(w.find(".permission-ask-body__badge").text()).toBe("仓库内");
  });

  // ----------------------------------------------------------------
  // 2026-06-22 (RULE-WorkerAsk-001): outcome badge in historical mode
  // ----------------------------------------------------------------
  //
  // When the `outcome` prop is present, the historical card renders
  // an outcome badge (✓ 已允许 / ✗ 已拒绝 / ⏱ 已超时 / ⊘ 已取消)
  // above the neutral ask-context line. When absent (no matching
  // resolved transcript entry — pre-this-task transcripts or
  // live-pending asks), the card renders the neutral line only
  // (backward compat).

  describe("outcome badge (RULE-WorkerAsk-001)", () => {
    it("renders the ✓ 已允许 badge when outcome='allow'", () => {
      const w = mount(PermissionAskBody, {
        props: {
          mode: "historical",
          ask: makeAsk({ toolName: "write_file" }),
          repoRoot: "/data/repo",
          outcome: "allow",
        },
      });
      const badge = w.find(".permission-ask-body__outcome-badge");
      expect(badge.exists()).toBe(true);
      expect(badge.text()).toBe("✓ 已允许");
      // The neutral ask-context line still renders below the badge.
      expect(w.text()).toContain("worker asked for write_file");
    });

    it("renders the ✗ 已拒绝 badge when outcome='deny'", () => {
      const w = mount(PermissionAskBody, {
        props: {
          mode: "historical",
          ask: makeAsk(),
          repoRoot: "/data/repo",
          outcome: "deny",
        },
      });
      const badge = w.find(".permission-ask-body__outcome-badge");
      expect(badge.exists()).toBe(true);
      expect(badge.text()).toBe("✗ 已拒绝");
    });

    it("renders the ⏱ 已超时 badge when outcome='timeout'", () => {
      const w = mount(PermissionAskBody, {
        props: {
          mode: "historical",
          ask: makeAsk(),
          repoRoot: "/data/repo",
          outcome: "timeout",
        },
      });
      const badge = w.find(".permission-ask-body__outcome-badge");
      expect(badge.exists()).toBe(true);
      expect(badge.text()).toBe("⏱ 已超时");
    });

    it("renders the ⊘ 已取消 badge when outcome='cancel'", () => {
      const w = mount(PermissionAskBody, {
        props: {
          mode: "historical",
          ask: makeAsk(),
          repoRoot: "/data/repo",
          outcome: "cancel",
        },
      });
      const badge = w.find(".permission-ask-body__outcome-badge");
      expect(badge.exists()).toBe(true);
      expect(badge.text()).toBe("⊘ 已取消");
    });

    it("does NOT render the badge when outcome is undefined (backward compat)", () => {
      // Pre-this-task transcript (no resolved entries) → ask card
      // renders with outcome === undefined → neutral ask-context
      // line only, NO outcome badge. Critical for backward compat.
      const w = mount(PermissionAskBody, {
        props: {
          mode: "historical",
          ask: makeAsk(),
          repoRoot: "/data/repo",
          // outcome intentionally omitted.
        },
      });
      expect(w.find(".permission-ask-body__outcome-badge").exists()).toBe(false);
      // The neutral ask-context line still renders.
      expect(w.find(".permission-ask-body__historical-note").exists()).toBe(true);
    });

    it("does NOT render the badge in interactive mode (outcome is historical-only)", () => {
      // The outcome badge is ONLY for historical cards (resolved
      // asks). Interactive cards are live-pending — no outcome
      // yet. Even if the prop is passed, interactive mode should
      // not render the badge (defensive — a caller shouldn't have
      // to remember to clear the prop when flipping to interactive).
      const w = mount(PermissionAskBody, {
        props: {
          mode: "interactive",
          ask: makeAsk(),
          onRespond: vi.fn(),
          repoRoot: "/data/repo",
          outcome: "allow",
        },
      });
      expect(w.find(".permission-ask-body__outcome-badge").exists()).toBe(false);
    });

    it("binds the outcome color token to the badge style (per-outcome)", () => {
      // Sanity: each outcome maps to its documented color token.
      // allow → --color-tool-write (emerald).
      // deny  → --color-tool-error (red).
      // timeout / cancel → --color-text-muted (neutral).
      const cases: Array<{
        outcome: "allow" | "deny" | "timeout" | "cancel";
        expectedColor: string;
      }> = [
        { outcome: "allow", expectedColor: "var(--color-tool-write)" },
        { outcome: "deny", expectedColor: "var(--color-tool-error)" },
        { outcome: "timeout", expectedColor: "var(--color-text-muted)" },
        { outcome: "cancel", expectedColor: "var(--color-text-muted)" },
      ];
      for (const { outcome, expectedColor } of cases) {
        const w = mount(PermissionAskBody, {
          props: {
            mode: "historical",
            ask: makeAsk(),
            repoRoot: "/data/repo",
            outcome,
          },
        });
        const badge = w.get(".permission-ask-body__outcome-badge");
        const style = badge.attributes("style") ?? "";
        expect(style).toContain(`color: ${expectedColor}`);
        expect(style).toContain(`border-color: ${expectedColor}`);
      }
    });
  });
});
