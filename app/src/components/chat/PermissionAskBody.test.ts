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
    expect(w.text()).toContain("worker wanted shell");
    expect(w.text()).toContain("ask collapsed");
    expect(w.text()).toContain("worker context");
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
});
