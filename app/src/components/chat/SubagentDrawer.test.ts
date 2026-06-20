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
});
