// Tests for `DrawerToolCallCard.vue` — PR4 of the subagent-drawer
// redesign (2026-06-21).
//
// Covers the visual contract + the "0 store coupling" lock:
//   1. Header renders tool name + file path (when present).
//   2. Header renders tool name without path for tools that don't
//      carry one (e.g. shell with `command`).
//   3. Status row: "running…" while no result; "done" with result;
//      "error" with isError result.
//   4. Duration chip renders `abbreviateDuration(result.durationMs)`
//      when set; "…" while running; empty string omits the chip.
//   5. 3px left bar accent follows `toolAccentVar` (read via inline
//      style on the root, asserting the CSS var resolution path).
//   6. `<ToolInputBody>` mounts when input is non-empty; hidden
//      when input is `{}` or absent.
//   7. `<ToolOutputBody>` mounts only when a result is present.
//   8. **0 store coupling lock** — the drawer card does NOT render:
//        - diff button / diff popover (`.tool-card__diff*`)
//        - inline approval UI (`.tool-card__approval*`)
//        - dispatch_subagent preview (`.tool-card--subagent*`)
//      These are main-panel concerns; their absence here is the
//      load-bearing contract (PRD R7 + Decision 1). If a future
//      refactor reuses `<ToolCallCard>` here, these assertions
//      break immediately.
//   9. Error variant: red left bar, error-tinted name + status.
//
// Icon stub note: `<ToolCallHeader>` (shared header, 2026-06-25)
// renders `<Icon>` (tool icon + status icon). We stub `Icon` for
// the same reason as the ThinkingBlock tests.
//
// RULE-FrontSubagent-001 (2026-06-25): header markup + CSS 抽到共享
// `<ToolCallHeader>`,故 header 元素的 class 从 `.drawer-tool-card__*`
// 改查 `.tool-call-header__*` (Vue Test Utils find 穿透子组件 DOM)。
// card 容器变体 (.drawer-tool-card / --error / --running) 仍在本组件，
// accent / body / 0-store lock / tokens 断言不变。

import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";
import DrawerToolCallCard from "./DrawerToolCallCard.vue";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";

function makeCall(overrides: Partial<ToolCallInfo> = {}): ToolCallInfo {
  return {
    id: "tu-1",
    name: "read_file",
    input: { path: "/repo/src/foo.ts" },
    ...overrides,
  };
}

function makeResult(overrides: Partial<ToolResultInfo> = {}): ToolResultInfo {
  return {
    toolUseId: "tu-1",
    content: "file contents here",
    isError: false,
    ...overrides,
  };
}

function mountCard(props: { call: ToolCallInfo; result?: ToolResultInfo }) {
  return mount(DrawerToolCallCard, {
    props,
    global: {
      stubs: { Icon: true },
    },
  });
}

describe("DrawerToolCallCard — header rendering", () => {
  it("renders the tool name in the header", () => {
    const w = mountCard({ call: makeCall({ name: "grep" }) });
    expect(w.find(".tool-call-header__name").text()).toBe("grep");
  });

  it("renders the file path chip when input.path is a non-empty string", () => {
    const w = mountCard({
      call: makeCall({ input: { path: "/repo/src/foo.ts" } }),
    });
    const chip = w.find(".tool-call-header__chip");
    expect(chip.exists()).toBe(true);
    expect(chip.text()).toContain("/repo/src/foo.ts");
  });

  // 2026-08-30 (task `08-30-shell-description`, PRD R4/AC3): shell 家族
  // 的 header chip 数据源换 `toolHeaderChip` —— 缺失 description 时兜底
  // command 首行(此前 shell 恒无 chip)。原「shell 无 path 行」断言随之
  // 演进为兜底分支断言(既有断言被 R4 有意取代,非行为漂移)。
  it("falls back to the command first line for shell without description", () => {
    const w = mountCard({
      call: makeCall({
        name: "shell",
        input: { command: "ls -la" },
      }),
    });
    const chip = w.find(".tool-call-header__chip");
    expect(chip.exists()).toBe(true);
    expect(chip.text()).toContain("ls -la");
  });

  it("prefers input.description over the command fallback for shell", () => {
    const w = mountCard({
      call: makeCall({
        name: "shell",
        input: { command: "ls -la", description: "列出目录内容" },
      }),
    });
    const chip = w.find(".tool-call-header__chip");
    expect(chip.text()).toContain("列出目录内容");
    expect(chip.text()).not.toContain("ls -la");
  });

  it("takes only the first non-empty line of a multi-line command (fallback)", () => {
    const w = mountCard({
      call: makeCall({
        name: "run_background_shell",
        input: { command: "\n  pnpm build \n--watch" },
      }),
    });
    expect(w.find(".tool-call-header__chip").text()).toContain("pnpm build");
  });

  it("does NOT render the chip when input.path is empty string", () => {
    const w = mountCard({
      call: makeCall({ input: { path: "" } }),
    });
    expect(w.find(".tool-call-header__chip").exists()).toBe(false);
  });

  it("does NOT render a chip for a non-shell tool without path", () => {
    // 非 shell 工具(如 dispatch_subagent)不享受 command 兜底 —
    // toolHeaderChip 的兜底分支被 isShellFamilyTool 封闭。
    const w = mountCard({
      call: makeCall({ name: "dispatch_subagent", input: { task: "x" } }),
    });
    expect(w.find(".tool-call-header__chip").exists()).toBe(false);
  });
});

describe("DrawerToolCallCard — status row", () => {
  it("shows 'running…' status text while no result is present", () => {
    const w = mountCard({ call: makeCall() });
    const status = w.find(".tool-call-header__status");
    expect(status.text()).toContain("running…");
  });

  it("shows 'done' status text when a non-error result is present", () => {
    const w = mountCard({ call: makeCall(), result: makeResult() });
    const status = w.find(".tool-call-header__status");
    expect(status.text()).toContain("done");
  });

  it("shows 'error' status text when the result reports isError", () => {
    const w = mountCard({
      call: makeCall(),
      result: makeResult({ isError: true, content: "exit 1" }),
    });
    const status = w.find(".tool-call-header__status");
    expect(status.text()).toContain("error");
  });

  it("applies the --error modifier class when result.isError is true", () => {
    const w = mountCard({
      call: makeCall(),
      result: makeResult({ isError: true }),
    });
    expect(w.find(".drawer-tool-card--error").exists()).toBe(true);
  });

  it("applies the --running modifier class while no result is present", () => {
    const w = mountCard({ call: makeCall() });
    expect(w.find(".drawer-tool-card--running").exists()).toBe(true);
  });

  it("does NOT apply --running once a result lands", () => {
    const w = mountCard({ call: makeCall(), result: makeResult() });
    expect(w.find(".drawer-tool-card--running").exists()).toBe(false);
  });

  // 08-30-shell-description R6: done 态 status(icon+文字)着成功色,
  // 颜色由共享 ToolCallHeader 的 isSuccess prop 驱动,这里锁接线。
  it("applies the header --success modifier when a non-error result lands", () => {
    const w = mountCard({ call: makeCall(), result: makeResult() });
    expect(w.find(".tool-call-header--success").exists()).toBe(true);
  });

  it("does NOT apply header --success on error results", () => {
    const w = mountCard({
      call: makeCall(),
      result: makeResult({ isError: true }),
    });
    expect(w.find(".tool-call-header--success").exists()).toBe(false);
  });
});

describe("DrawerToolCallCard — duration chip", () => {
  it("renders '…' duration while running (no result)", () => {
    const w = mountCard({ call: makeCall() });
    expect(w.find(".tool-call-header__duration").text()).toBe("…");
  });

  it("renders the abbreviated duration when result.durationMs is set", () => {
    const w = mountCard({
      call: makeCall(),
      result: makeResult({ durationMs: 1234 }),
    });
    // 1234ms → "1.2s" via abbreviateDuration.
    expect(w.find(".tool-call-header__duration").text()).toBe("1.2s");
  });

  it("renders minute-scale duration in the 'Mm Ss' format", () => {
    const w = mountCard({
      call: makeCall(),
      result: makeResult({ durationMs: 83500 }), // 1m 23s (分档秒取整,08-29 ladder)
    });
    expect(w.find(".tool-call-header__duration").text()).toBe("1m 23s");
  });

  it("omits the duration span when result is present but durationMs is undefined", () => {
    const w = mountCard({
      call: makeCall(),
      result: makeResult({ durationMs: undefined }),
    });
    expect(w.find(".tool-call-header__duration").exists()).toBe(false);
  });
});

describe("DrawerToolCallCard — 3px left bar accent", () => {
  it("uses var(--color-tool-read) for read_file", () => {
    const w = mountCard({ call: makeCall({ name: "read_file" }) });
    expect(w.find(".drawer-tool-card").attributes("style")).toContain(
      "var(--color-tool-read)",
    );
  });

  it("uses var(--color-tool-write) for write_file", () => {
    const w = mountCard({ call: makeCall({ name: "write_file" }) });
    expect(w.find(".drawer-tool-card").attributes("style")).toContain(
      "var(--color-tool-write)",
    );
  });

  it("uses var(--color-tool-shell) for shell", () => {
    const w = mountCard({
      call: makeCall({ name: "shell", input: { command: "ls" } }),
    });
    expect(w.find(".drawer-tool-card").attributes("style")).toContain(
      "var(--color-tool-shell)",
    );
  });

  it("uses var(--color-text-muted) for unknown tools", () => {
    const w = mountCard({ call: makeCall({ name: "custom_tool" }) });
    expect(w.find(".drawer-tool-card").attributes("style")).toContain(
      "var(--color-text-muted)",
    );
  });

  it("overrides to var(--color-tool-error) when result.isError", () => {
    // Error flips the accent regardless of tool name.
    const w = mountCard({
      call: makeCall({ name: "read_file" }),
      result: makeResult({ isError: true }),
    });
    const style = w.find(".drawer-tool-card").attributes("style") ?? "";
    expect(style).toContain("var(--color-tool-error)");
    expect(style).not.toContain("var(--color-tool-read)");
  });
});

describe("DrawerToolCallCard — body components mount", () => {
  it("mounts ToolInputBody when input is non-empty", () => {
    const w = mountCard({
      call: makeCall({ input: { path: "/x", content: "y" } }),
    });
    expect(w.findComponent({ name: "ToolInputBody" }).exists()).toBe(true);
    // The shared body's root class.
    expect(w.find(".tool-input-body").exists()).toBe(true);
  });

  it("does NOT mount ToolInputBody when input is an empty object", () => {
    const w = mountCard({
      call: makeCall({ input: {} }),
    });
    expect(w.find(".tool-input-body").exists()).toBe(false);
  });

  it("mounts ToolOutputBody when a result is present", () => {
    const w = mountCard({
      call: makeCall(),
      result: makeResult({ content: "ok" }),
    });
    expect(w.findComponent({ name: "ToolOutputBody" }).exists()).toBe(true);
    expect(w.find(".tool-output-body").exists()).toBe(true);
  });

  it("does NOT mount ToolOutputBody when no result is present", () => {
    const w = mountCard({ call: makeCall() });
    expect(w.find(".tool-output-body").exists()).toBe(false);
  });
});

describe("DrawerToolCallCard — 0 store coupling lock", () => {
  // These are the load-bearing assertions: the drawer card must NOT
  // render the main-panel-only concerns. If any of these start
  // appearing, the wrapper has accidentally pulled in store-driven
  // branches (or someone replaced the wrapper with `<ToolCallCard>`).

  it("does NOT render the diff button (edit_file w/ path + result)", () => {
    // The main panel renders the diff button under these exact
    // conditions (edit_file + path + active worktree). The drawer
    // variant must not.
    const w = mountCard({
      call: makeCall({ name: "edit_file", input: { path: "/repo/a.ts" } }),
      result: makeResult(),
    });
    expect(w.find(".tool-card__diff-btn").exists()).toBe(false);
    expect(w.find(".drawer-tool-card__diff-btn").exists()).toBe(false);
  });

  it("does NOT render the inline approval UI", () => {
    // The main panel renders `.tool-card__approval` when the
    // backend's permission:ask matches this tool_use. The drawer
    // variant must not (worker permission handling is PR6's job
    // and routes through a different surface).
    const w = mountCard({ call: makeCall() });
    expect(w.find(".tool-card__approval").exists()).toBe(false);
    expect(w.find(".drawer-tool-card__approval").exists()).toBe(false);
  });

  it("does NOT render the dispatch_subagent preview even for dispatch_subagent calls", () => {
    // The main panel renders `.tool-card__subagent-preview` for
    // dispatch_subagent (click opens the drawer). A drawer-side
    // dispatch_subagent (worker dispatching a sub-sub-agent) must
    // NOT recurse — the drawer renders the worker's call/result
    // like any other tool. (PRD Out of Scope: 单例 drawer.)
    const w = mountCard({
      call: makeCall({
        name: "dispatch_subagent",
        input: { subagent: "researcher", task: "x" },
      }),
      result: makeResult({ content: "[status: completed]\nsummary" }),
    });
    expect(w.find(".tool-card--subagent").exists()).toBe(false);
    expect(w.find(".tool-card__subagent-preview").exists()).toBe(false);
    expect(w.find(".drawer-tool-card--subagent").exists()).toBe(false);
    // Instead, the dispatch_subagent input should render via the
    // shared ToolInputBody (proves the drawer treats it as a
    // normal call).
    expect(w.find(".tool-input-body").exists()).toBe(true);
  });

  it("does NOT attach click/role=button affordances to the root", () => {
    // The main panel's dispatch_subagent variant turns the root
    // into a clickable button. The drawer variant must not.
    const w = mountCard({
      call: makeCall({
        name: "dispatch_subagent",
        input: { subagent: "researcher", task: "x" },
      }),
    });
    const root = w.find(".drawer-tool-card");
    expect(root.attributes("role")).toBeUndefined();
    expect(root.attributes("tabindex")).toBeUndefined();
  });
});

describe("DrawerToolCallCard — design tokens (CSS sanity)", () => {
  it("uses scoped CSS classes (no inline style hex colors on root)", () => {
    // The root has ONE inline style — `borderLeftColor` (the accent
    // CSS var). Spot-check that there are no hardcoded hex colors
    // anywhere in the inline style.
    const w = mountCard({ call: makeCall() });
    const style = w.find(".drawer-tool-card").attributes("style") ?? "";
    expect(style).not.toMatch(/#[0-9a-f]{3,8}/i);
  });
});
