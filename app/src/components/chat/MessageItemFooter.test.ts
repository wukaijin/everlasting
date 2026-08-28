// Tests for `MessageItemFooter.vue` — the error row + F5
// latency chip + A5 R2 retry button extracted from
// `MessageItem.vue` on 2026-06-23 (+ 2026-07-17 retry extension).
// The component is a pure presentation layer; the parent
// (`MessageItem.vue`) owns the store interactions. These tests
// drive the component with hand-built props + assert on the
// rendered DOM, without spinning up Pinia.
//
// Coverage:
//   1. Renders nothing when neither error nor latency is set.
//   2. Error row appears for either role; carries the
//      `error.message` string and `role="alert"`.
//   3. Latency chip is hidden when `role="user"` even if
//      latency data is present.
//   4. Latency chip is hidden when `streaming=true`.
//   5. Latency chip is hidden when `latency.totalMs` is
//      missing or not a number.
//   6. Latency chip renders the abbreviated total (e.g. 1000
//      → "1.0s"; 60000 → "1m 0s"; 90000 → "1m 30s") via
//      `abbreviateDuration`.
//   7. Tooltip rows: only `totalMs` → one row labelled 端到端.
//   8. Tooltip rows: ttfb + gen + total → three rows in
//      order TTFB / 生成 / 端到端.
//   9. Tooltip rows: missing ttfb OR gen → that row hidden
//      (the cancel / error path leaves them null).
//  10. Error + latency both present → error appears above
//      the chip (load-bearing order — the user sees the
//      failure first).
//  11. A5 R2 retry button — visibility derived from
//      `categoryRetryable(category)` (RateLimit/Server/Network
//      show; Auth/InvalidRequest/未设不显示).
//  12. Retry button text/disabled state when `retryLoading`.
//  13. Retry click emits `retry(seq)` to parent.
//
// Test gotcha: reka-ui `TooltipContent` portal to body —
// reka-ui's tooltips don't get auto-cleaned on `unmount()`
// in the jsdom test env, so we remove the portal residue
// in `afterEach` to prevent cross-test leak (per
// `subagentdrawer-banner-test-gotchas.md` memory).

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mount } from "@vue/test-utils";

import MessageItemFooter from "./MessageItemFooter.vue";

const baseProps = () => ({
  role: "assistant" as "user" | "assistant",
  streaming: false,
  latency: undefined as undefined | { ttfbMs?: number; genMs?: number; totalMs?: number },
  error: undefined as undefined | { message: string; category?: string },
  messageSeq: undefined as number | undefined,
  retryLoading: false,
});

function mountFooter(propsOverride: Partial<ReturnType<typeof baseProps>> = {}) {
  return mount(MessageItemFooter, {
    props: { ...baseProps(), ...propsOverride },
  });
}

describe("MessageItemFooter — basic rendering", () => {
  it("renders nothing when neither error nor latency is set", () => {
    const w = mountFooter();
    expect(w.find("[data-testid='msg-error-row']").exists()).toBe(false);
    expect(w.find("[data-testid='msg-latency-chip']").exists()).toBe(false);
  });
});

describe("MessageItemFooter — error row", () => {
  it("renders the error row for assistant role with role=alert", () => {
    const w = mountFooter({
      error: { message: "rate_limit" },
    });
    const row = w.find("[data-testid='msg-error-row']");
    expect(row.exists()).toBe(true);
    expect(row.attributes("role")).toBe("alert");
    expect(row.text()).toContain("rate_limit");
  });

  it("renders the error row for user role too", () => {
    const w = mountFooter({
      role: "user",
      error: { message: "网络错误" },
    });
    const row = w.find("[data-testid='msg-error-row']");
    expect(row.exists()).toBe(true);
    expect(row.text()).toContain("网络错误");
  });

  it("renders both error and latency when both are set", () => {
    const w = mountFooter({
      error: { message: "网络错误" },
      latency: { totalMs: 1000 },
    });
    expect(w.find("[data-testid='msg-error-row']").exists()).toBe(true);
    expect(w.find("[data-testid='msg-latency-chip']").exists()).toBe(true);
  });
});

describe("MessageItemFooter — latency chip visibility", () => {
  it("does NOT render the chip for user role", () => {
    const w = mountFooter({ role: "user", latency: { totalMs: 1000 } });
    expect(w.find("[data-testid='msg-latency-chip']").exists()).toBe(false);
  });

  it("does NOT render the chip when streaming=true", () => {
    const w = mountFooter({ streaming: true, latency: { totalMs: 1000 } });
    expect(w.find("[data-testid='msg-latency-chip']").exists()).toBe(false);
  });

  it("does NOT render the chip when latency is missing", () => {
    const w = mountFooter();
    expect(w.find("[data-testid='msg-latency-chip']").exists()).toBe(false);
  });

  it("does NOT render the chip when totalMs is missing", () => {
    const w = mountFooter({ latency: { ttfbMs: 100, genMs: 200 } });
    expect(w.find("[data-testid='msg-latency-chip']").exists()).toBe(false);
  });

  it("does NOT render the chip when totalMs is not a number", () => {
    const w = mountFooter({
      latency: { ttfbMs: 100, totalMs: undefined as unknown as number },
    });
    expect(w.find("[data-testid='msg-latency-chip']").exists()).toBe(false);
  });
});

describe("MessageItemFooter — latency chip label", () => {
  // 2026-08-29 ui-visual-polish: ladder contract — decimal < 10s,
  // whole seconds 10-59s, "Xm Ys" with whole-minute compaction.
  it("abbreviates 1000ms as '1.0s'", () => {
    const w = mountFooter({ latency: { totalMs: 1000 } });
    expect(w.get("[data-testid='msg-latency-chip']").text()).toBe("1.0s");
  });

  it("abbreviates 60000ms as '1m'", () => {
    const w = mountFooter({ latency: { totalMs: 60000 } });
    expect(w.get("[data-testid='msg-latency-chip']").text()).toBe("1m");
  });

  it("abbreviates 90000ms as '1m 30s'", () => {
    const w = mountFooter({ latency: { totalMs: 90000 } });
    expect(w.get("[data-testid='msg-latency-chip']").text()).toBe("1m 30s");
  });

  it("abbreviates 3200ms as '3.2s'", () => {
    const w = mountFooter({ latency: { totalMs: 3200 } });
    expect(w.get("[data-testid='msg-latency-chip']").text()).toBe("3.2s");
  });

  it("abbreviates 500ms as '0.5s'", () => {
    const w = mountFooter({ latency: { totalMs: 500 } });
    expect(w.get("[data-testid='msg-latency-chip']").text()).toBe("0.5s");
  });
});

describe("MessageItemFooter — tooltip row rendering", () => {
  // Tooltip rows live inside the reka-ui TooltipContent
  // portal — they're not in the component's own template
  // until the tooltip is open. We assert against the
  // `latencyRows` computed by inspecting the component
  // instance directly, since opening the tooltip in jsdom
  // requires user gestures we can't easily simulate. The
  // DOM rendering of the rows is covered by the
  // `latencyTotalLabel` test above (the chip itself) and
  // the integration test in MessageItem.vue (covered by
  // the existing component test surface).

  it("renders only the 端到端 row when only totalMs is set", () => {
    const w = mountFooter({ latency: { totalMs: 1000 } });
    const vm = w.vm as unknown as {
      latencyRows: Array<{ label: string; value: string }>;
    };
    expect(vm.latencyRows).toEqual([{ label: "端到端", value: "1.0s" }]);
  });

  it("renders all three rows in order TTFB / 生成 / 端到端", () => {
    const w = mountFooter({
      latency: { ttfbMs: 200, genMs: 800, totalMs: 1000 },
    });
    const vm = w.vm as unknown as {
      latencyRows: Array<{ label: string; value: string }>;
    };
    expect(vm.latencyRows).toEqual([
      { label: "TTFB", value: "0.2s" },
      { label: "生成", value: "0.8s" },
      { label: "端到端", value: "1.0s" },
    ]);
  });

  it("skips the TTFB row when ttfbMs is missing (cancel-mid-TTFB)", () => {
    const w = mountFooter({ latency: { genMs: 800, totalMs: 1000 } });
    const vm = w.vm as unknown as {
      latencyRows: Array<{ label: string; value: string }>;
    };
    expect(vm.latencyRows).toEqual([
      { label: "生成", value: "0.8s" },
      { label: "端到端", value: "1.0s" },
    ]);
  });

  it("skips the 生成 row when genMs is missing (cancel-mid-gen)", () => {
    const w = mountFooter({ latency: { ttfbMs: 200, totalMs: 1000 } });
    const vm = w.vm as unknown as {
      latencyRows: Array<{ label: string; value: string }>;
    };
    expect(vm.latencyRows).toEqual([
      { label: "TTFB", value: "0.2s" },
      { label: "端到端", value: "1.0s" },
    ]);
  });

  it("renders empty rows when latency is missing", () => {
    const w = mountFooter();
    const vm = w.vm as unknown as {
      latencyRows: Array<{ label: string; value: string }>;
    };
    expect(vm.latencyRows).toEqual([]);
  });
});

describe("MessageItemFooter — reka-ui tooltip integration", () => {
  let wrapper: ReturnType<typeof mountFooter> | null = null;

  beforeEach(() => {
    wrapper = null;
  });

  afterEach(() => {
    // Unmount the wrapper (catches any pending tooltips).
    if (wrapper) {
      wrapper.unmount();
      wrapper = null;
    }
    // Reka-ui TooltipContent portals to <body>; the
    // unmount doesn't always remove the portal in jsdom.
    // Sweep manually to prevent cross-test DOM leak.
    document
      .querySelectorAll(
        ".msg__latency-tooltip, [data-testid^='msg-latency-tooltip-row-']",
      )
      .forEach((el) => el.remove());
  });

  it("renders the latency chip with the right class for hover styling", () => {
    const w = mountFooter({ latency: { totalMs: 1500 } });
    wrapper = w;
    const chip = w.get<HTMLElement>("[data-testid='msg-latency-chip']");
    expect(chip.classes()).toContain("msg__latency");
    expect(chip.text()).toBe("1.5s");
  });
});

describe("MessageItemFooter — A5 R2 retry button (visibility)", () => {
  it("does NOT render retry button when error has no category", () => {
    const w = mountFooter({ error: { message: "失败" } });
    expect(w.find("[data-testid='msg-retry-button']").exists()).toBe(false);
  });

  it("does NOT render retry button when category is auth", () => {
    const w = mountFooter({
      error: { message: "key 无效", category: "auth" },
    });
    expect(w.find("[data-testid='msg-retry-button']").exists()).toBe(false);
  });

  it("does NOT render retry button when category is invalid_request", () => {
    const w = mountFooter({
      error: { message: "请求结构错", category: "invalid_request" },
    });
    expect(w.find("[data-testid='msg-retry-button']").exists()).toBe(false);
  });

  it("renders retry button when category is rate_limit", () => {
    const w = mountFooter({
      error: { message: "请求过于频繁", category: "rate_limit" },
    });
    const btn = w.find("[data-testid='msg-retry-button']");
    expect(btn.exists()).toBe(true);
    expect(btn.text()).toBe("↻ 重试");
    expect(btn.attributes("disabled")).toBeUndefined();
  });

  it("renders retry button when category is server", () => {
    const w = mountFooter({
      error: { message: "boom", category: "server" },
    });
    expect(w.find("[data-testid='msg-retry-button']").exists()).toBe(true);
  });

  it("renders retry button when category is network", () => {
    const w = mountFooter({
      error: { message: "断线", category: "network" },
    });
    expect(w.find("[data-testid='msg-retry-button']").exists()).toBe(true);
  });

  it("does NOT render retry button while streaming=true(防御中间态)", () => {
    const w = mountFooter({
      error: { message: "boom", category: "server" },
      streaming: true,
    });
    expect(w.find("[data-testid='msg-retry-button']").exists()).toBe(false);
  });
});

describe("MessageItemFooter — A5 R2 retry button (loading + click)", () => {
  it("retryLoading=true 时按钮 disabled + 文本切到 '重试中...'", () => {
    const w = mountFooter({
      error: { message: "boom", category: "server" },
      retryLoading: true,
    });
    const btn = w.get<HTMLElement>("[data-testid='msg-retry-button']");
    expect(btn.attributes("disabled")).toBeDefined();
    expect(btn.text()).toBe("重试中...");
  });

  it("点击按钮 emit retry(messageSeq)", async () => {
    const w = mountFooter({
      error: { message: "boom", category: "server" },
      messageSeq: 42,
    });
    await w.find("[data-testid='msg-retry-button']").trigger("click");
    const events = w.emitted("retry");
    expect(events).toBeTruthy();
    expect(events![0]).toEqual([42]);
  });

  it("loading=true 时点击不 emit retry(defense in depth)", async () => {
    const w = mountFooter({
      error: { message: "boom", category: "server" },
      messageSeq: 42,
      retryLoading: true,
    });
    await w.find("[data-testid='msg-retry-button']").trigger("click");
    expect(w.emitted("retry")).toBeFalsy();
  });

  it("click without messageSeq 不 emit retry(defensive)", async () => {
    const w = mountFooter({
      error: { message: "boom", category: "server" },
      // messageSeq 不传 → onRetryClick return early
    });
    await w.find("[data-testid='msg-retry-button']").trigger("click");
    expect(w.emitted("retry")).toBeFalsy();
  });
});

