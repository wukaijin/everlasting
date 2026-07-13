// Tests for ButtonPrimitive.vue — B9+ D3
// (07-13-b9plus-generative-ui-followup, 2026-07-13).
//
// Coverage:
//   1. Default label per action (apply_diff → "应用", copy → "复制",
//      dismiss → "关闭") when LLM omits `label`.
//   2. Custom `label` overrides the default.
//   3. `apply_diff` action invokes `apply_ui_diff` with
//      `payload.diff_text` and the current session id; success →
//      toast + card hides.
//   4. `apply_diff` failure surfaces inline error keyed by `kind`.
//   5. `copy` action invokes `navigator.clipboard.writeText` with
//      `payload.text`; success → toast + card hides.
//   6. `dismiss` action hides the card locally (no IPC, no toast).
//   7. Disabled state when there's no active session (apply_diff only).
//   8. Unknown action type degrades gracefully (button shows, click
//      is a no-op — the Rust validator already rejects unknown
//      actions, but the frontend defensive layer keeps the card
//      mountable in case a stale message slips through).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";

import ButtonPrimitive from "./ButtonPrimitive.vue";
import type {
  UiButtonPrimitive,
  UiButtonAction,
} from "../uiCard.types";

// Mock @tauri-apps/api/core so we can spy on the invoke call without
// touching the real backend.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const showToastMock = vi.fn();
const chatState: { currentSessionId: string | null } = {
  currentSessionId: "sess-1",
};
vi.mock("../../../stores/chat", () => ({
  useChatStore: () => ({
    get currentSessionId() {
      return chatState.currentSessionId;
    },
  }),
}));
vi.mock("../../../stores/projects", () => ({
  useProjectsStore: () => ({
    showToast: showToastMock,
  }),
}));

const writeTextMock = vi.fn();

function mountBtn(
  action: UiButtonAction,
  over: Partial<UiButtonPrimitive> = {},
) {
  return mount(ButtonPrimitive, {
    props: {
      primitive: {
        type: "button",
        action,
        ...over,
      } as UiButtonPrimitive,
    },
  });
}

beforeEach(() => {
  writeTextMock.mockReset();
  writeTextMock.mockResolvedValue(undefined);
  Object.assign(navigator, {
    clipboard: { writeText: writeTextMock },
  });
  invokeMock.mockReset();
  showToastMock.mockReset();
  chatState.currentSessionId = "sess-1";
});

// ---- default labels ----

describe("ButtonPrimitive — default labels per action", () => {
  it("apply_diff default = '应用'", () => {
    const w = mountBtn("apply_diff", {
      payload: { diff_text: "diff" },
    });
    expect(w.find(".ui-prim__btn").text()).toBe("应用");
  });
  it("copy default = '复制'", () => {
    const w = mountBtn("copy", { payload: { text: "x" } });
    expect(w.find(".ui-prim__btn").text()).toBe("复制");
  });
  it("dismiss default = '关闭'", () => {
    const w = mountBtn("dismiss");
    expect(w.find(".ui-prim__btn").text()).toBe("关闭");
  });
  it("custom label overrides default", () => {
    const w = mountBtn("apply_diff", {
      label: "Yes, apply",
      payload: { diff_text: "diff" },
    });
    expect(w.find(".ui-prim__btn").text()).toBe("Yes, apply");
  });
});

// ---- apply_diff ----

describe("ButtonPrimitive — apply_diff action", () => {
  const SAMPLE_DIFF = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n";

  it("invokes apply_ui_diff with (sessionId, payload.diff_text)", async () => {
    invokeMock.mockResolvedValue({
      ok: true,
      files: [{ path: "x", added: 1, removed: 1 }],
    });
    const w = mountBtn("apply_diff", { payload: { diff_text: SAMPLE_DIFF } });
    await w.find(".ui-prim__btn").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("apply_ui_diff", {
      sessionId: "sess-1",
      diffText: SAMPLE_DIFF,
    });
  });

  it("success: toast + card hides", async () => {
    invokeMock.mockResolvedValue({
      ok: true,
      files: [{ path: "x", added: 1, removed: 1 }],
    });
    const w = mountBtn("apply_diff", { payload: { diff_text: SAMPLE_DIFF } });
    await w.find(".ui-prim__btn").trigger("click");
    await flushPromises();
    expect(showToastMock).toHaveBeenCalledWith("已应用 1 个文件", "info", 3000);
    // Card hides (v-if state === 'done').
    expect(w.find(".ui-prim--button").exists()).toBe(false);
  });

  it("failure: inline error keyed by kind + card stays visible", async () => {
    invokeMock.mockResolvedValue({
      ok: false,
      kind: "boundary",
      error: "outside root",
    });
    const w = mountBtn("apply_diff", { payload: { diff_text: SAMPLE_DIFF } });
    await w.find(".ui-prim__btn").trigger("click");
    await flushPromises();
    const errorEl = w.find(".ui-prim__error");
    expect(errorEl.exists()).toBe(true);
    expect(errorEl.text()).toContain("boundary");
    expect(showToastMock).not.toHaveBeenCalled();
    // Card stays visible for retry.
    expect(w.find(".ui-prim--button").exists()).toBe(true);
  });

  it("disables when there is no active session", async () => {
    chatState.currentSessionId = null;
    const w = mountBtn("apply_diff", { payload: { diff_text: SAMPLE_DIFF } });
    const btn = w.find(".ui-prim__btn");
    expect((btn.element as HTMLButtonElement).disabled).toBe(true);
    expect(btn.attributes("title")).toContain("无活跃会话");
    await btn.trigger("click");
    await flushPromises();
    expect(invokeMock).not.toHaveBeenCalled();
  });
});

// ---- copy ----

describe("ButtonPrimitive — copy action", () => {
  it("writes payload.text to clipboard + toast + card hides", async () => {
    const w = mountBtn("copy", { payload: { text: "the snippet" } });
    await w.find(".ui-prim__btn").trigger("click");
    await flushPromises();
    expect(writeTextMock).toHaveBeenCalledWith("the snippet");
    expect(showToastMock).toHaveBeenCalledWith("已复制到剪贴板", "info", 1500);
    expect(w.find(".ui-prim--button").exists()).toBe(false);
  });
});

// ---- dismiss ----

describe("ButtonPrimitive — dismiss action", () => {
  it("hides the card locally (no IPC, no toast)", async () => {
    const w = mountBtn("dismiss");
    await w.find(".ui-prim__btn").trigger("click");
    await flushPromises();
    expect(invokeMock).not.toHaveBeenCalled();
    expect(showToastMock).not.toHaveBeenCalled();
    expect(w.find(".ui-prim--button").exists()).toBe(false);
  });
});

// ---- defensive: unknown action ----

describe("ButtonPrimitive — defensive (unknown action)", () => {
  it("renders a no-op button when action is not in the enum", async () => {
    // The Rust validator already rejects this at execute-time, but
    // a stale / hand-edited message could still slip through. The
    // renderer must not crash on click.
    const w = mount(ButtonPrimitive, {
      props: {
        primitive: {
          type: "button",
          action: "nuke_everything",
        } as unknown as UiButtonPrimitive,
      },
    });
    expect(w.find(".ui-prim__btn").exists()).toBe(true);
    await w.find(".ui-prim__btn").trigger("click");
    await flushPromises();
    // No IPC, no toast, no crash.
    expect(invokeMock).not.toHaveBeenCalled();
    expect(showToastMock).not.toHaveBeenCalled();
  });
});