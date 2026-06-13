// Tests for `PermissionModal.vue` — the ⑨ 关 three-button modal.
//
// Coverage (PR3 §"PermissionModal Acceptance Criteria" — 14 条 +
// store-level contract):
//   1. Renders nothing when no pending ask.
//   2. Renders all 3 buttons (拒绝 / 仅一次 / 始终允许) when an
//      ask is pending.
//   3. Clicking each button calls `store.respond` with the right
//      decision.
//   4. Esc triggers deny.
//   5. Enter on a non-critical ask triggers allow_once (the
//      default).
//   6. Enter on a critical ask triggers deny (per spec audit
//      §6.2 — "critical Enter 改'拒绝'").
//   7. Clicking the X button triggers deny.
//   8. Clicking the backdrop triggers deny (via @click.self).
//   9. JSON.stringify(toolInput, null, 2) renders in the <pre>
//      preview block.
//   10. Risk label is rendered in Chinese (低/中/高/极高).
//   11. Critical variant adds the `permission-modal--critical`
//       modifier class (which gives the 3px red left border).
//   12. Copy button invokes navigator.clipboard.writeText with
//       the formatted JSON.
//
// IMPORTANT: the modal uses `<Teleport to="body">`, so its DOM
// lives outside the test wrapper. We use `document.body.querySelector`
// to find elements rather than `wrapper.find(...)`. Vue Test Utils
// can't follow Teleport boundaries without a custom `attachTo`
// + `document`-aware finder.

import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import { mount, VueWrapper, flushPromises } from "@vue/test-utils";
import { nextTick } from "vue";
import { createPinia, setActivePinia } from "pinia";

// Mock Tauri APIs so the store's start() / respond() don't try
// to hit the real event channel.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

// Mock clipboard for the copy-button test.
const writeTextMock = vi.fn();
Object.defineProperty(navigator, "clipboard", {
  configurable: true,
  value: { writeText: writeTextMock },
});

import PermissionModal from "./PermissionModal.vue";
import {
  usePermissionsStore,
  type PermissionAsk,
} from "../../stores/permissions";
import { useChatStore } from "../../stores/chat";

function mountModal(): VueWrapper {
  return mount(PermissionModal, {
    attachTo: document.body,
  });
}

/** Query the teleported DOM directly. The Vue Test Utils wrapper
 *  cannot follow <Teleport> boundaries by default. */
function qs<T extends Element = Element>(selector: string): T | null {
  return document.body.querySelector<T>(selector);
}

describe("PermissionModal", () => {
  let wrapper: VueWrapper | null = null;

  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(true);
    writeTextMock.mockReset();
    writeTextMock.mockResolvedValue(undefined);
  });

  afterEach(() => {
    wrapper?.unmount();
    wrapper = null;
  });

  const baseAsk: PermissionAsk = {
    rid: "rid-1",
    toolName: "shell",
    toolInput: { command: "echo hi" },
    risk: "high",
  };

  function seedAsk(overrides: Partial<PermissionAsk> = {}) {
    const store = usePermissionsStore();
    store.setPending({ ...baseAsk, ...overrides });
    return store;
  }

  it("renders nothing when no ask is pending", () => {
    wrapper = mountModal();
    expect(qs(".permission-modal")).toBeNull();
  });

  it("renders all 3 buttons (拒绝 / 仅一次 / 始终允许) when an ask is pending", async () => {
    seedAsk();
    wrapper = mountModal();
    await nextTick();
    const deny = qs<HTMLButtonElement>(".permission-modal__btn--deny");
    const once = qs<HTMLButtonElement>(".permission-modal__btn--once");
    const always = qs<HTMLButtonElement>(".permission-modal__btn--always");
    expect(deny).not.toBeNull();
    expect(once).not.toBeNull();
    expect(always).not.toBeNull();
    expect(deny!.textContent).toContain("拒绝");
    expect(once!.textContent).toContain("仅一次");
    expect(always!.textContent).toContain("始终允许");
  });

  it("clicking 拒绝 calls store.respond with 'deny'", async () => {
    seedAsk({ rid: "rid-deny" });
    wrapper = mountModal();
    await nextTick();
    qs<HTMLButtonElement>(".permission-modal__btn--deny")?.click();
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-deny",
      decision: "deny",
    });
  });

  it("clicking 仅一次 calls store.respond with 'allow_once'", async () => {
    seedAsk({ rid: "rid-once" });
    wrapper = mountModal();
    await nextTick();
    qs<HTMLButtonElement>(".permission-modal__btn--once")?.click();
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-once",
      decision: "allow_once",
    });
  });

  it("clicking 始终允许 calls store.respond with 'allow_always'", async () => {
    seedAsk({ rid: "rid-always" });
    wrapper = mountModal();
    await nextTick();
    qs<HTMLButtonElement>(".permission-modal__btn--always")?.click();
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-always",
      decision: "allow_always",
    });
  });

  it("Esc triggers deny", async () => {
    seedAsk({ rid: "rid-esc" });
    wrapper = mountModal();
    await nextTick();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-esc",
      decision: "deny",
    });
  });

  it("Enter on non-critical ask triggers allow_once (default)", async () => {
    seedAsk({ rid: "rid-enter-medium", risk: "medium" });
    wrapper = mountModal();
    await nextTick();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-enter-medium",
      decision: "allow_once",
    });
  });

  it("Enter on critical ask triggers deny (per spec audit §6.2)", async () => {
    seedAsk({ rid: "rid-enter-critical", risk: "critical" });
    wrapper = mountModal();
    await nextTick();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-enter-critical",
      decision: "deny",
    });
  });

  it("clicking the X button triggers deny", async () => {
    seedAsk({ rid: "rid-x" });
    wrapper = mountModal();
    await nextTick();
    qs<HTMLButtonElement>(".permission-modal__close")?.click();
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-x",
      decision: "deny",
    });
  });

  it("clicking the backdrop triggers deny via @click.self", async () => {
    seedAsk({ rid: "rid-back" });
    wrapper = mountModal();
    await nextTick();
    const backdrop = qs(".permission-modal-backdrop") as HTMLElement | null;
    expect(backdrop).not.toBeNull();
    // Dispatch a real bubbling click event on the backdrop.
    backdrop!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("permission_response", {
      rid: "rid-back",
      decision: "deny",
    });
  });

  it("renders the JSON-stringified toolInput in the <pre> block", async () => {
    seedAsk({
      rid: "rid-pre",
      toolName: "edit_file",
      toolInput: {
        path: "/tmp/x.ts",
        old_string: "old",
        new_string: "new",
      },
    });
    wrapper = mountModal();
    await nextTick();
    const pre = qs(".permission-modal__preview-pre");
    expect(pre).not.toBeNull();
    const text = pre!.textContent ?? "";
    expect(text).toContain('"path": "/tmp/x.ts"');
    expect(text).toContain('"old_string": "old"');
    expect(text).toContain('"new_string": "new"');
  });

  it("renders the Chinese risk label per risk level", async () => {
    for (const [risk, label] of [
      ["low", "低"],
      ["medium", "中"],
      ["high", "高"],
      ["critical", "极高"],
    ] as const) {
      // Re-mount per case so the watch triggers fresh.
      wrapper?.unmount();
      seedAsk({ rid: `rid-${risk}`, risk });
      wrapper = mountModal();
      await nextTick();
      const riskLabel = qs(".permission-modal__risk-label");
      expect(riskLabel).not.toBeNull();
      expect(riskLabel!.textContent).toBe(label);
    }
  });

  it("adds the critical modifier class when risk === 'critical'", async () => {
    seedAsk({ rid: "rid-crit", risk: "critical" });
    wrapper = mountModal();
    await nextTick();
    const modal = qs(".permission-modal");
    expect(modal).not.toBeNull();
    expect(modal!.classList.contains("permission-modal--critical")).toBe(true);
  });

  it("does NOT add the critical modifier when risk is non-critical", async () => {
    seedAsk({ rid: "rid-high", risk: "high" });
    wrapper = mountModal();
    await nextTick();
    const modal = qs(".permission-modal");
    expect(modal).not.toBeNull();
    expect(modal!.classList.contains("permission-modal--critical")).toBe(false);
  });

  it("copy button writes the formatted JSON to clipboard", async () => {
    seedAsk({
      rid: "rid-copy",
      toolName: "shell",
      toolInput: { command: "ls -la" },
    });
    wrapper = mountModal();
    await nextTick();
    qs<HTMLButtonElement>(".permission-modal__copy")?.click();
    await flushPromises();
    expect(writeTextMock).toHaveBeenCalledTimes(1);
    const written = writeTextMock.mock.calls[0][0] as string;
    expect(written).toContain('"command": "ls -la"');
    expect(written).toContain("{");
    expect(written).toContain("}");
  });

  it("renders the tool name in the risk label row", async () => {
    seedAsk({ rid: "rid-toolname", toolName: "edit_file" });
    wrapper = mountModal();
    await nextTick();
    const tool = qs(".permission-modal__risk-tool");
    expect(tool).not.toBeNull();
    expect(tool!.textContent).toBe("edit_file");
  });

  it("renders the reason under the title when present", async () => {
    seedAsk({ rid: "rid-reason", reason: "matches denylist: rm -rf /" });
    wrapper = mountModal();
    await nextTick();
    const reason = qs(".permission-modal__reason");
    expect(reason).not.toBeNull();
    expect(reason!.textContent).toBe("matches denylist: rm -rf /");
  });

  // -----------------------------------------------------------------
  // Path range row (re-grill 2026-06-13 PR2, Q10)
  // -----------------------------------------------------------------
  //
  // The modal renders a "path range" row between the subtitle and
  // the command preview block ONLY when `ask.path` is set. The
  // badge text + color depend on whether the path is inside the
  // session's `currentCwd` (computed via the `isPathInRoot`
  // helper in `app/src/utils/path.ts`).
  //
  // The chat store's `currentCwd` is wired in `ChatWindow.vue`;
  // for the test we set it explicitly via the store API so the
  // in-repo / out-of-repo decision is reproducible without
  // spinning up a full Tauri session.

  it("renders path range row with 仓库内 badge when path is inside currentCwd", async () => {
    const store = useChatStore();
    store.currentCwd = "/repo";
    seedAsk({
      rid: "rid-path-inrepo",
      toolName: "write_file",
      toolInput: { path: "/repo/src/foo.ts", content: "..." },
      risk: "medium",
      path: "/repo/src/foo.ts",
    });
    wrapper = mountModal();
    await nextTick();
    const row = qs(".permission-modal__path-range");
    expect(row).not.toBeNull();
    const text = qs(".permission-modal__path-range-text");
    expect(text).not.toBeNull();
    expect(text!.textContent).toBe("/repo/src/foo.ts");
    const badge = qs(".permission-modal__path-range-badge");
    expect(badge).not.toBeNull();
    expect(badge!.textContent).toBe("仓库内");
    // Badge color references the emerald tool-color token.
    expect(badge!.getAttribute("style") ?? "").toContain(
      "--color-tool-write",
    );
  });

  it("renders path range row with 仓库外 badge when path is outside currentCwd", async () => {
    const store = useChatStore();
    store.currentCwd = "/repo";
    seedAsk({
      rid: "rid-path-outrepo",
      toolName: "read_file",
      toolInput: { path: "/etc/hosts" },
      risk: "low",
      path: "/etc/hosts",
    });
    wrapper = mountModal();
    await nextTick();
    const row = qs(".permission-modal__path-range");
    expect(row).not.toBeNull();
    const badge = qs(".permission-modal__path-range-badge");
    expect(badge).not.toBeNull();
    expect(badge!.textContent).toBe("仓库外");
    // Badge color references the amber tool-color token.
    expect(badge!.getAttribute("style") ?? "").toContain(
      "--color-tool-shell",
    );
  });

  it("does NOT render path range row when path is absent (shell)", async () => {
    seedAsk({
      rid: "rid-no-path-shell",
      toolName: "shell",
      toolInput: { command: "echo hi" },
      risk: "high",
    });
    wrapper = mountModal();
    await nextTick();
    expect(qs(".permission-modal__path-range")).toBeNull();
  });

  it("does NOT render path range row when path is absent (web_fetch)", async () => {
    seedAsk({
      rid: "rid-no-path-webfetch",
      toolName: "web_fetch",
      toolInput: { url: "https://example.com" },
      risk: "medium",
    });
    wrapper = mountModal();
    await nextTick();
    expect(qs(".permission-modal__path-range")).toBeNull();
  });

  it("treats prefix-trap path as out-of-repo (defends against /repo/foobar vs /repo/foo)", async () => {
    // Mirrors the Rust is_within_root edge case #5: /repo/foobar
    // must NOT be treated as inside /repo/foo. The frontend
    // helper has the same predicate so the badge should be
    // "仓库外" (amber) — better to ask one extra time than to
    // silently bypass the Tier 4 path gate.
    const store = useChatStore();
    store.currentCwd = "/repo/foo";
    seedAsk({
      rid: "rid-prefix-trap",
      toolName: "write_file",
      toolInput: { path: "/repo/foobar" },
      risk: "medium",
      path: "/repo/foobar",
    });
    wrapper = mountModal();
    await nextTick();
    const badge = qs(".permission-modal__path-range-badge");
    expect(badge).not.toBeNull();
    expect(badge!.textContent).toBe("仓库外");
  });
});