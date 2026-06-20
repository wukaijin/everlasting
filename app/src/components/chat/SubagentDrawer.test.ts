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
});
