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
import ToolInputBody from "./ToolInputBody.vue";
import ToolOutputBody from "./ToolOutputBody.vue";
import PermissionAskBody from "./PermissionAskBody.vue";
import WorkerTextTimeline from "./WorkerTextTimeline.vue";
import {
  useSubagentRunsStore,
  type SubagentRunRow,
} from "../../stores/subagentRuns";
import { useChatStore } from "../../stores/chat";

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
        // FT-F-001 stage 2 (check phase fix 2026-06-20): the Rust
        // `PermissionAskPayload` serializes with `#[serde(rename_all =
        // "camelCase")]`, so the actual stored payload_json carries
        // `toolName` (NOT `tool_name`). `synthesizeAsk` reads both
        // spellings defensively; the production-realistic fixture is
        // camelCase. Provide a risk field so PermissionAskBody doesn't
        // crash on RISK_META[undefined].
        {
          kind: "permission_ask",
          payload_json: { toolName: "shell", risk: "high" },
        },
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
      { kind: "chat_event", payload_json: { kind: "start" } },
      { kind: "tool_call", payload_json: { name: "x" } },
      { kind: "tool_result", payload_json: { content: "y", is_error: false } },
      // FT-F-001 stage 2 (check phase fix 2026-06-20): permission_ask
      // routes through synthesizeAsk. The Rust PermissionAskPayload
      // serializes with camelCase, so the production-realistic fixture
      // uses `toolName` (synthesizeAsk also reads snake_case fallback).
      // Provide a risk so PermissionAskBody doesn't crash on
      // RISK_META[undefined].
      {
        kind: "permission_ask",
        payload_json: { toolName: "shell", risk: "high" },
      },
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

  it("tool_result entries unwrap the cwd envelope and render via ToolOutputBody", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    const envelope = JSON.stringify({
      result: "actual file contents here",
      cwd: "/data/worktrees/p1/s1",
    });
    store.liveTranscript.set("run-1", [
      { kind: "tool_call", payload_json: { name: "read_file", input: { path: "/foo" } } },
      { kind: "tool_result", payload_json: { content: envelope, is_error: false } },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    // FT-F-001 stage 2: tool_result entries now render through the
    // shared ToolOutputBody component (PR1). The drawer's outer wrapper
    // no longer renders a `<pre>` blob — the body component owns the
    // envelope-unwrap (via extractToolResultDisplay) + truncate logic.
    const outputBodies = w.findAllComponents(ToolOutputBody);
    expect(outputBodies.length).toBe(1);
    // ToolOutputBody's internal `display` computed unwraps the cwd
    // envelope (see PR1 — extractToolResultDisplay strips `{result,cwd}`).
    // We assert the unwrapped string lands in the rendered `<pre>`.
    const pre = outputBodies[0].find(".tool-output-body__pre");
    expect(pre.text()).toContain("actual file contents here");
    // The envelope is NOT visible — neither the escape noise nor the
    // bare `"cwd":` key that the old double-stringify would emit.
    expect(pre.text()).not.toContain("cwd");

    // tool_call entries route to ToolInputBody (the name + input render
    // through the shared body component, not as inline JSON).
    const inputBodies = w.findAllComponents(ToolInputBody);
    expect(inputBodies.length).toBe(1);
    expect(inputBodies[0].props("name")).toBe("read_file");
    expect(inputBodies[0].props("input")).toEqual({ path: "/foo" });
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

  // -------------------------------------------------------------------
  // FT-F-001 stage 2 (2026-06-20): typed-card routing. Each
  // TranscriptKind routes to its matching shared body component
  // (PR1's ToolInputBody / ToolOutputBody / PermissionAskBody) or the
  // drawer-local WorkerTextTimeline. Verifies AC1-AC4 at the component-
  // instance level (findComponent instead of brittle DOM-string
  // matching).
  // -------------------------------------------------------------------

  it("tool_call entry routes to ToolInputBody with name + input props", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    store.liveTranscript.set("run-1", [
      {
        kind: "tool_call",
        payload_json: {
          request_id: "req-1",
          id: "tu-1",
          name: "grep",
          input: { pattern: "TODO", path: "/src" },
        },
      },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const bodies = w.findAllComponents(ToolInputBody);
    expect(bodies.length).toBe(1);
    expect(bodies[0].props("name")).toBe("grep");
    expect(bodies[0].props("input")).toEqual({ pattern: "TODO", path: "/src" });
    w.unmount();
  });

  it("tool_result entry routes to ToolOutputBody with content + isError props (no durationMs)", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    store.liveTranscript.set("run-1", [
      {
        kind: "tool_result",
        payload_json: {
          request_id: "req-1",
          tool_use_id: "tu-1",
          content: "line 1\nline 2",
          is_error: false,
        },
      },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const bodies = w.findAllComponents(ToolOutputBody);
    expect(bodies.length).toBe(1);
    expect(bodies[0].props("content")).toBe("line 1\nline 2");
    expect(bodies[0].props("isError")).toBe(false);
    // Per R1 mapping table: tool_result payload_json has NO duration_ms
    // field, so durationMs prop is never set (undefined). The body's
    // summary omits the duration chip accordingly.
    expect(bodies[0].props("durationMs")).toBeUndefined();
    w.unmount();
  });

  it("tool_result entry with is_error=true forwards isError to ToolOutputBody", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    store.liveTranscript.set("run-1", [
      {
        kind: "tool_result",
        payload_json: { content: "boom", is_error: true },
      },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const bodies = w.findAllComponents(ToolOutputBody);
    expect(bodies[0].props("isError")).toBe(true);
    // Visual: the error variant class is applied to the `<details>`.
    expect(bodies[0].classes()).toContain("tool-output-body--error");
    w.unmount();
  });

  it("permission_ask entry routes to PermissionAskBody in historical mode with synthesizeAsk + repoRoot", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    // Seed chatStore.currentCwd so the path badge has a repoRoot.
    const chatStore = useChatStore();
    chatStore.currentCwd = "/data/repo";
    store.liveTranscript.set("run-1", [
      {
        kind: "permission_ask",
        // Check phase fix (2026-06-20): the Rust
        // `PermissionAskPayload` carries `#[serde(rename_all =
        // "camelCase")]` (see `app/src-tauri/src/agent/permissions/
        // mod.rs:406`), so production `payload_json` actually has
        // camelCase keys — `sessionId` / `toolUseId` / `toolName` /
        // `toolInput`. The PRD's snake_case claim was wrong (only
        // `ToolCallPayload` / `ToolResultPayload` are snake_case —
        // they have NO `rename_all`). `synthesizeAsk` reads both
        // spellings defensively; this fixture uses the production-
        // realistic camelCase shape.
        payload_json: {
          rid: "r-1",
          sessionId: "sess-1",
          toolUseId: "tu-1",
          toolName: "write_file",
          toolInput: { path: "/data/repo/src/x.ts" },
          risk: "medium",
          path: "/data/repo/src/x.ts",
        },
      },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const bodies = w.findAllComponents(PermissionAskBody);
    expect(bodies.length).toBe(1);
    expect(bodies[0].props("mode")).toBe("historical");
    expect(bodies[0].props("repoRoot")).toBe("/data/repo");
    const ask = bodies[0].props("ask");
    // synthesizeAsk maps payload_json → camelCase PermissionAsk.
    // (Reads both `toolName` (production) and `tool_name` (legacy)
    // defensively.)
    expect(ask.toolName).toBe("write_file");
    expect(ask.risk).toBe("medium");
    expect(ask.path).toBe("/data/repo/src/x.ts");
    expect(ask.toolUseId).toBe("tu-1");
    expect(ask.sessionId).toBe("sess-1");
    expect(ask.rid).toBe("r-1");
    // Historical mode: NO onRespond callback is provided (D6 — no
    // interactive buttons in the drawer).
    expect(bodies[0].props("onRespond")).toBeUndefined();
    // The historical note text uses the PR1 phrasing (no "denied").
    expect(bodies[0].text()).toContain("worker wanted write_file");
    expect(bodies[0].text()).toContain("ask collapsed");
    expect(bodies[0].text()).not.toContain("denied");
    w.unmount();
  });

  // Check phase (2026-06-20): lock the snake_case fallback in
  // `synthesizeAsk`. Production `payload_json` is camelCase (Rust
  // `PermissionAskPayload` carries `rename_all = "camelCase"`), but
  // `synthesizeAsk` defensively reads BOTH spellings so a future
  // backend refactor that drops `rename_all` doesn't silently render
  // blank permission cards. If this fallback is ever removed, the
  // test fails — prompting an explicit decision rather than a silent
  // regression.
  it("synthesizeAsk also accepts snake_case payload_json (defensive fallback)", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    store.liveTranscript.set("run-1", [
      {
        kind: "permission_ask",
        payload_json: {
          rid: "r-2",
          session_id: "sess-2",
          tool_use_id: "tu-2",
          tool_name: "shell",
          tool_input: { command: "ls" },
          risk: "high",
        },
      },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    const bodies = w.findAllComponents(PermissionAskBody);
    const ask = bodies[0].props("ask");
    expect(ask.toolName).toBe("shell");
    expect(ask.toolUseId).toBe("tu-2");
    expect(ask.sessionId).toBe("sess-2");
    expect(ask.rid).toBe("r-2");
    expect(ask.toolInput).toEqual({ command: "ls" });
    w.unmount();
  });

  it("chat_event entry routes to WorkerTextTimeline; start + done render as milestones, deltas ignored", async () => {
    const store = useSubagentRunsStore();
    store.getRunCache.set("run-1", sampleRow);
    store.liveTranscript.set("run-1", [
      // A start/done pair plus noise deltas — the timeline should
      // render exactly 2 milestone rows and drop everything else.
      {
        kind: "chat_event",
        payload_json: { request_id: "req-1", kind: "start" },
      },
      {
        kind: "chat_event",
        payload_json: {
          request_id: "req-1",
          kind: "delta",
          text: "streaming token noise",
        },
      },
      {
        kind: "chat_event",
        payload_json: {
          request_id: "req-1",
          kind: "thinking_delta",
          text: "thinking noise",
        },
      },
      {
        kind: "chat_event",
        payload_json: {
          request_id: "req-1",
          kind: "done",
          stop_reason: "end_turn",
          // usage present but NOT rendered per Q3 (drawer header has
          // the aggregate).
          usage: { input_tokens: 100, output_tokens: 50 },
        },
      },
    ]);
    await store.openDrawer("run-1");
    await flushPromises();
    const w = makeDrawer();
    await flushPromises();

    // chat_event entries are hidden by default — flip the toggle so
    // the timeline components mount.
    const checkbox = document.body.querySelector(
      ".subagent-drawer__toggle input",
    ) as HTMLInputElement;
    checkbox.checked = true;
    checkbox.dispatchEvent(new Event("change"));
    await flushPromises();

    const timelines = w.findAllComponents(WorkerTextTimeline);
    // Each chat_event transcript entry becomes its own WorkerTextTimeline
    // (single-entry array). The component filters internally; delta/
    // thinking_delta entries produce an empty-state `<p>` instead of
    // milestone rows.
    expect(timelines.length).toBe(4);
    // The start + done timelines each render exactly one milestone row.
    const milestoneRows = document.body.querySelectorAll(
      ".worker-text-timeline__row",
    );
    expect(milestoneRows.length).toBe(2);
    // start dot + done dot classes.
    const rowClasses = [...milestoneRows].map((r) =>
      [...r.classList].filter((c) =>
        c.startsWith("worker-text-timeline__row--"),
      ),
    );
    expect(rowClasses).toEqual([
      ["worker-text-timeline__row--start"],
      ["worker-text-timeline__row--done"],
    ]);
    // The done row surfaces the stop_reason inline.
    const text = document.body.textContent ?? "";
    expect(text).toContain("agent 开始响应");
    expect(text).toContain("agent 完成");
    expect(text).toContain("end_turn");
    // Token usage is NOT surfaced by the timeline (Q3 — header has the
    // aggregate).
    expect(text).not.toContain("input_tokens");
    expect(text).not.toContain("output_tokens");
    w.unmount();
  });
});
