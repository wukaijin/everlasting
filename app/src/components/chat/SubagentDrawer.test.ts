// Tests for `SubagentDrawer.vue` — B6 PR3 worker subagent transcript
// drawer.
//
// Coverage (per PRD R8):
//   1. Renders when `store.openRunId !== null`; returns nothing when
//      closed.
//   2. Header carries status + subagentName + summary.
//   3. Body renders transcript from `store.liveTranscript` first;
//      falls back to parsing `getRunCache.transcriptJson` when no
//      live events.
//   4. Kind badges render with the right label per `TranscriptKind`.
//   5. `transcriptTruncated !== 0` shows the "原 transcript 已截断"
//      notice.
//   6. chat_event entries are hidden by default; the "Show chat
//      events" toggle reveals them.
//   7. Empty state shows "Worker is starting..." when no transcript.
//   8. Clicking the X (DialogClose) clears `openRunId`.
//
// Uses real Pinia + mocked Tauri (no IPC actually fires in these
// tests — we drive the store directly). The reka-ui Dialog portal is
// not jsdom-friendly (it uses <Teleport to="body">), so we rely on
// `attachTo: document.body` and query via `document.body`.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import SubagentDrawer from "./SubagentDrawer.vue";
import {
  useSubagentRunsStore,
  type SubagentRunRow,
} from "../../stores/subagentRuns";

const sampleRow: SubagentRunRow = {
  id: "run-1",
  parentSessionId: "sess-1",
  parentRequestId: "parent-rid-sub-tooluse-1",
  subagentName: "researcher",
  status: "completed",
  startedAt: "2026-06-20T10:00:00Z",
  finishedAt: "2026-06-20T10:00:30Z",
  tokenUsageJson: null,
  summary: "found 3 files",
  transcriptJson: null,
  transcriptTruncated: 0,
  createdAt: "2026-06-20T10:00:00Z",
};

function makeDrawer() {
  return mount(SubagentDrawer, {
    attachTo: document.body,
    global: {
      // reka-ui portals via <Teleport to="body">; shallow keeps the
      // test fast and avoids pulling in Icon's heroicon imports.
      stubs: { Icon: true },
    },
  });
}

describe("SubagentDrawer", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    // vue-test-utils + reka-ui Teleport quirk: `w.unmount()` removes
    // the component root, but the dialog content (portaled to body
    // via `DialogContent`) sometimes stays attached, leaking into
    // the next test's `document.body.querySelector(...)` results.
    // Belt-and-braces cleanup so each test starts with a fresh body.
    document.body.querySelectorAll(
      ".subagent-drawer, .subagent-drawer__overlay, .subagent-drawer__banner",
    ).forEach((el) => el.remove());
  });

  it("renders nothing visible when store.openRunId is null", () => {
    const store = useSubagentRunsStore();
    expect(store.openRunId).toBeNull();
    const w = makeDrawer();
    // The Dialog is closed — no .subagent-drawer content in the DOM.
    expect(document.body.querySelector(".subagent-drawer")).toBeNull();
    w.unmount();
  });

  it("renders header status + name + summary once a run is open", async () => {
    const store = useSubagentRunsStore();
    // Pre-seed cache so openDrawer doesn't fire IPC.
    store.getRunCache.set("run-1", sampleRow);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const text = document.body.textContent ?? "";
    expect(text).toContain("researcher");
    expect(text).toContain("完成"); // completed → 完成 label
    expect(text).toContain("found 3 files");
    w.unmount();
  });

  it("renders transcript entries from store.liveTranscript first", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    store.liveTranscript.set("run-1", [
      { kind: "tool_call", payload_json: { name: "grep", input: { pattern: "foo" } } },
      { kind: "tool_result", payload_json: { content: "match" } },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const entries = document.body.querySelectorAll(".subagent-drawer__entry");
    expect(entries.length).toBe(2);
    // Kind labels rendered.
    const labels = [...document.body.querySelectorAll(".subagent-drawer__kind")].map(
      (e) => e.textContent?.trim() ?? "",
    );
    expect(labels).toEqual(["call", "result"]);
    w.unmount();
  });

  it("falls back to parsing transcriptJson when no live events", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", {
      ...sampleRow,
      transcriptJson: JSON.stringify([
        { kind: "tool_call", payload_json: { name: "read_file" } },
        { kind: "permission_ask", payload_json: { toolName: "shell" } },
      ]),
    });
    // liveTranscript is empty (no in-flight stream).
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const entries = document.body.querySelectorAll(".subagent-drawer__entry");
    expect(entries.length).toBe(2);
    w.unmount();
  });

  it("chat_event entries are hidden by default; toggle reveals them", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    store.liveTranscript.set("run-1", [
      { kind: "tool_call", payload_json: { name: "grep" } },
      { kind: "chat_event", payload_json: { text: "verbose delta" } },
      { kind: "tool_result", payload_json: { content: "match" } },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    // Default: 2 visible (chat_event hidden).
    let entries = document.body.querySelectorAll(".subagent-drawer__entry");
    expect(entries.length).toBe(2);

    // Toggle on.
    const checkbox = document.body.querySelector(
      ".subagent-drawer__toggle input",
    ) as HTMLInputElement;
    checkbox.checked = true;
    checkbox.dispatchEvent(new Event("change"));
    await flushPromises();

    entries = document.body.querySelectorAll(".subagent-drawer__entry");
    expect(entries.length).toBe(3);
    w.unmount();
  });

  it("transcriptTruncated flag shows the '原 transcript 已截断' notice", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", { ...sampleRow, transcriptTruncated: 1 });
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const text = document.body.textContent ?? "";
    expect(text).toContain("原 transcript 已截断");
    w.unmount();
  });

  it("empty state shows 'Worker is starting...' when no transcript", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow); // transcriptJson is null
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const text = document.body.textContent ?? "";
    expect(text).toContain("Worker is starting");
    w.unmount();
  });

  it("kind badge labels cover all four TranscriptKind values", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    store.liveTranscript.set("run-1", [
      { kind: "chat_event", payload_json: {} },
      { kind: "tool_call", payload_json: {} },
      { kind: "tool_result", payload_json: {} },
      { kind: "permission_ask", payload_json: {} },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    // Turn on chat-event visibility so all 4 kinds render.
    const checkbox = document.body.querySelector(
      ".subagent-drawer__toggle input",
    ) as HTMLInputElement;
    checkbox.checked = true;
    checkbox.dispatchEvent(new Event("change"));
    await flushPromises();

    const labels = [...document.body.querySelectorAll(".subagent-drawer__kind")].map(
      (e) => e.textContent?.trim() ?? "",
    );
    expect(labels.sort()).toEqual(["call", "chat", "perm", "result"]);
    w.unmount();
  });

  // -------------------------------------------------------------------
  // B6 PR3b (2026-06-20): live duration timer + jump-to-latest +
  // auto-scroll polish.
  // -------------------------------------------------------------------

  it("running state shows live duration suffix in status pill", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T10:00:08.200Z"));
    const store = useSubagentRunsStore();
    // running row: started 8.2s ago, no finishedAt
    store.getRunCache.set("run-1", {
      ...sampleRow,
      status: "running",
      finishedAt: null,
    });
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const statusEl = document.body.querySelector(".subagent-drawer__status");
    expect(statusEl?.textContent?.trim()).toBe("运行中 8.2s");
    w.unmount();
    vi.useRealTimers();
  });

  it("completed state shows 'done in X.Xs' suffix in status pill", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T10:00:30.000Z"));
    const store = useSubagentRunsStore();
    // sampleRow has startedAt 10:00:00 + finishedAt 10:00:30 = 30s
    await store.openDrawer("run-1");
    store.getRunCache.set("run-1", sampleRow);
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const statusEl = document.body.querySelector(".subagent-drawer__status");
    expect(statusEl?.textContent?.trim()).toBe("完成 30.0s");
    w.unmount();
    vi.useRealTimers();
  });

  it("jump-to-latest button is hidden initially (autoFollow=true) and shows after scroll-up + new entry", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", { ...sampleRow, status: "running", finishedAt: null });
    store.liveTranscript.set("run-1", [
      { kind: "tool_call", payload_json: { name: "read_file" } },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    // Initially autoFollow=true → no jump-to-latest button.
    expect(document.body.querySelector(".subagent-drawer__jump-latest")).toBeNull();
    expect(document.body.querySelector(".subagent-drawer__new-events")).toBeNull();

    // Simulate the user scrolling up + a new entry arriving.
    const body = document.body.querySelector(".subagent-drawer__body") as HTMLElement;
    // Pretend the body is taller than its viewport so scrolling is meaningful.
    Object.defineProperty(body, "scrollHeight", { configurable: true, value: 1000 });
    Object.defineProperty(body, "clientHeight", { configurable: true, value: 200 });
    body.scrollTop = 500; // user scrolled up
    body.dispatchEvent(new Event("scroll"));
    await flushPromises();
    // Now autoFollow=false (we're not near the bottom). The header
    // jump-to-latest button appears because there ARE visible entries.
    expect(document.body.querySelector(".subagent-drawer__jump-latest")).not.toBeNull();

    // Append a new entry — watch(visibleTranscript.length) fires,
    // autoFollow is off so newCount goes to 1, the floating button appears.
    const current = store.liveTranscript.get("run-1") ?? [];
    store.liveTranscript.set("run-1", [
      ...current,
      { kind: "tool_result", payload_json: { content: "ok" } },
    ]);
    await flushPromises();
    const newBtn = document.body.querySelector(".subagent-drawer__new-events");
    expect(newBtn?.textContent?.trim()).toBe("↓ 1 new");
    w.unmount();
  });

  it("clicking jump-to-latest button clears the newCount + restores auto-follow", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", { ...sampleRow, status: "running", finishedAt: null });
    store.liveTranscript.set("run-1", [
      { kind: "tool_call", payload_json: {} },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const body = document.body.querySelector(".subagent-drawer__body") as HTMLElement;
    Object.defineProperty(body, "scrollHeight", { configurable: true, value: 1000 });
    Object.defineProperty(body, "clientHeight", { configurable: true, value: 200 });
    body.scrollTop = 500;
    body.dispatchEvent(new Event("scroll"));
    await flushPromises();
    // Add a new entry to populate newCount.
    const current = store.liveTranscript.get("run-1") ?? [];
    store.liveTranscript.set("run-1", [
      ...current,
      { kind: "tool_result", payload_json: {} },
    ]);
    await flushPromises();
    expect(document.body.querySelector(".subagent-drawer__new-events")).not.toBeNull();

    // Click the header jump-to-latest button.
    const headerBtn = document.body.querySelector(
      ".subagent-drawer__jump-latest",
    ) as HTMLButtonElement;
    headerBtn.click();
    await flushPromises();

    // newCount cleared, floating button hidden (autoFollow restored).
    expect(document.body.querySelector(".subagent-drawer__new-events")).toBeNull();
    w.unmount();
  });

  // -------------------------------------------------------------------
  // Session 50 hotfix (B1, 2026-06-20): terminal-state durations
  // (error / cancelled) must use `finishedAt - startedAt`, NOT the
  // live ticker. The bug surfaced when a failed worker was left
  // open in the drawer — the badge kept growing past the actual
  // run time. With the system clock 3 hours past the run, the
  // correct terminal duration is still the frozen `finishedAt -
  // startedAt` value, regardless of how long the drawer has been
  // open.
  // -------------------------------------------------------------------

  it("error state shows terminal duration (finishedAt - startedAt), not the live ticker", async () => {
    vi.useFakeTimers();
    // System clock is 3 hours past the run. Without the fix, the
    // badge would read "failed at 10800.0s" (nowTick - startedAt).
    // With the fix, it reads the frozen 11.7s.
    vi.setSystemTime(new Date("2026-06-20T13:00:11.700Z"));
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", {
      ...sampleRow,
      status: "error",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:11.700Z",
    });
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const statusEl = document.body.querySelector(".subagent-drawer__status");
    expect(statusEl?.textContent?.trim()).toBe("failed at 11.7s");
    w.unmount();
    vi.useRealTimers();
  });

  it("cancelled state shows terminal duration (finishedAt - startedAt), not the live ticker", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T13:00:05.300Z"));
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", {
      ...sampleRow,
      status: "cancelled",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:05.300Z",
    });
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const statusEl = document.body.querySelector(".subagent-drawer__status");
    expect(statusEl?.textContent?.trim()).toBe("已停止 at 5.3s");
    w.unmount();
    vi.useRealTimers();
  });

  // -------------------------------------------------------------------
  // Session 50 hotfix (B2, 2026-06-20): tool_result entries carry the
  // cwd envelope as a stringified JSON value in payload_json.content
  // (REQ-16, same shape as the main panel's ToolCallCard). The drawer
  // must unwrap the envelope and render the inner result string, NOT
  // the double-encoded JSON with `\"cwd\":\"...\"` escape chars. This
  // test mirrors the `extractToolResultDisplay` unit test in
  // `utils/messageFormat.test.ts` but at the drawer level.
  // -------------------------------------------------------------------

  it("tool_result entries unwrap the cwd envelope and render the inner result text", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    const envelope = JSON.stringify({
      result: "actual file contents here",
      cwd: "/data/worktrees/p1/s1",
    });
    store.liveTranscript.set("run-1", [
      { kind: "tool_call", payload_json: { name: "read_file", input: { path: "/foo" } } },
      { kind: "tool_result", payload_json: { content: envelope } },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const payloads = [...document.body.querySelectorAll(".subagent-drawer__payload")];
    const payloadTexts = payloads.map((p) => p.textContent ?? "");
    // The inner result string is rendered (as plain text, no JSON wrapping).
    expect(payloadTexts.some((t) => t.trim() === "actual file contents here")).toBe(true);
    // The envelope is NOT visible anywhere — neither the escape noise
    // nor the bare `"cwd":` key that the old double-stringify would emit.
    expect(payloadTexts.some((t) => t.includes("\\\"cwd\\\"") || t.includes('"cwd":'))).toBe(false);
    // tool_call entries keep the old JSON.stringify path (different
    // shape — no envelope).
    expect(payloadTexts.some((t) => t.includes('"name"'))).toBe(true);
    w.unmount();
  });

  // -------------------------------------------------------------------
  // FT-F-005 (2026-06-20): failure-reason banner in the header.
  // Covers the 4 acceptance criteria from the prd.md AC section:
  //   - failed + summary → banner shows "Worker exited with error: <truncated>"
  //   - failed + empty summary → banner falls back to "Worker exited unexpectedly at N.Ns"
  //   - cancelled → banner shows "Worker stopped by user at N.Ns"
  //   - running / completed → no banner (regression guard)
  // -------------------------------------------------------------------

  it("failed drawer shows the error banner with summary text", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T10:00:11.700Z"));
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", {
      ...sampleRow,
      status: "error",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:11.700Z",
      summary: "shell: timeout after 10.0s",
    });
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const banner = document.body.querySelector(".subagent-drawer__banner");
    expect(banner).not.toBeNull();
    // Error variant class (red tint via --color-tool-error).
    expect(banner?.classList.contains("subagent-drawer__banner--error")).toBe(true);
    // Status badge (existing) AND banner (new) both render — they
    // coexist per D5.
    expect(document.body.querySelector(".subagent-drawer__status")?.textContent?.trim())
      .toBe("failed at 11.7s");
    // Icon stub renders an empty <icon-stub name="warn" /> — its
    // textContent is empty, so banner.textContent is just the
    // <span class="subagent-drawer__banner-text"> content (no "⚠").
    expect(banner?.querySelector(".subagent-drawer__banner-text")?.textContent)
      .toBe("Worker exited with error: shell: timeout after 10.0s");
    // The warn icon IS present (asserted via stub attribute).
    expect(banner?.querySelector('icon-stub[name="warn"]')).not.toBeNull();
    w.unmount();
    vi.useRealTimers();
  });

  it("failed drawer falls back to 'unexpectedly' message when summary is empty", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T10:00:11.700Z"));
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", {
      ...sampleRow,
      status: "error",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:11.700Z",
      summary: null, // backend wrote no error text (rare but possible)
    });
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const banner = document.body.querySelector(".subagent-drawer__banner");
    expect(banner).not.toBeNull();
    // Falls back to the frozen duration message — reuses the
    // statusDisplay.suffix so the number matches the badge.
    expect(banner?.querySelector(".subagent-drawer__banner-text")?.textContent)
      .toBe("Worker exited unexpectedly at 11.7s");
    w.unmount();
    vi.useRealTimers();
  });

  it("failed drawer truncates summary longer than 80 chars with ellipsis", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T10:00:05.000Z"));
    const store = useSubagentRunsStore();
    const longSummary = "shell: error executing command (rc=1): " +
      "command not found: extremely-long-tool-name-that-the-shell-could-not-locate-" +
      "and-this-should-definitely-be-truncated-in-the-banner-because-it-is-way-over-80-chars-long";
    store.getRunCache.set("run-1", {
      ...sampleRow,
      status: "error",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:05.000Z",
      summary: longSummary,
    });
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const banner = document.body.querySelector(".subagent-drawer__banner");
    expect(banner).not.toBeNull();
    const bannerText = banner?.querySelector(".subagent-drawer__banner-text")?.textContent ?? "";
    // The displayed body is shorter than the original summary (truncated).
    expect(bannerText.length).toBeLessThan(longSummary.length);
    // Truncation: ends with "…" suffix.
    expect(bannerText.endsWith("…")).toBe(true);
    // Truncation: starts with the standard prefix.
    expect(bannerText.startsWith("Worker exited with error: ")).toBe(true);
    // Body portion (after prefix) is exactly 80 chars + "…" = 81 chars.
    // Prefix length is 25 ("Worker exited with error: "), so total = 25 + 81 = 106.
    expect(bannerText.length).toBe("Worker exited with error: ".length + 80 + 1);
    w.unmount();
    vi.useRealTimers();
  });

  it("cancelled drawer shows the stopped-by-user banner with warning color", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T10:00:05.300Z"));
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", {
      ...sampleRow,
      status: "cancelled",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:05.300Z",
      summary: "partial work before stop",
    });
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const banner = document.body.querySelector(".subagent-drawer__banner");
    expect(banner).not.toBeNull();
    // Warning variant (amber tint via --color-tool-shell) — NOT the
    // error red, because cancel is not a "failure" per se.
    expect(banner?.classList.contains("subagent-drawer__banner--warning")).toBe(true);
    expect(banner?.classList.contains("subagent-drawer__banner--error")).toBe(false);
    // Generic text — we don't read `summary` for cancelled (it carries
    // partial work, not a reason). Banner text omits the icon glyph
    // because Icon is stubbed in tests; we assert against the
    // <span class="...__banner-text"> content directly.
    expect(banner?.querySelector(".subagent-drawer__banner-text")?.textContent)
      .toBe("Worker stopped by user at 5.3s");
    w.unmount();
    vi.useRealTimers();
  });

  it("running and completed drawers do NOT render the failure banner", async () => {
    const store = useSubagentRunsStore();
    // Running state — bannerText computed returns null.
    store.getRunCache.set("run-1", {
      ...sampleRow,
      status: "running",
      finishedAt: null,
    });
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    expect(document.body.querySelector(".subagent-drawer__banner")).toBeNull();
    w.unmount();

    // Completed state — bannerText computed returns null. Use a
    // separate store + drawer mount so we don't depend on the
    // running-state drawer closing cleanly (reka-ui Teleport can
    // leak DOM across in-test open/close cycles).
    const store2 = useSubagentRunsStore();
    store2.getRunCache.set("run-2", { ...sampleRow, status: "completed" });
    await store2.openDrawer("run-2");
    await flushPromises();
    const w2 = makeDrawer();
    await flushPromises();

    expect(document.body.querySelector(".subagent-drawer__banner")).toBeNull();
    w2.unmount();
  });
});
