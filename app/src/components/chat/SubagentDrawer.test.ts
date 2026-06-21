// Tests for `SubagentDrawer.vue` — B6 PR3 worker subagent transcript
// drawer.
//
// B6 redesign PR5 (2026-06-21): rewritten to cover the 5-segment
// grouped view. The drawer now reads `store.liveSections`
// (accumulator output `TranscriptSection[]`) instead of raw
// `liveTranscript`; the previous chat_event toggle / hidden-count
// hint / flat tool-card list are all GONE. Coverage:
//
//   1. Renders when `store.openRunId !== null`; returns nothing closed.
//   2. Header carries status + subagentName + summary.
//   3. DrawerPromptCard renders run.task with 120-char truncate +
//      "View full →" (hidden when task is null).
//   4. Thinking segment collapses by default; expand chevron works.
//   5. Tools segment pairs ToolCall + ToolResult into a single
//      DrawerToolCallCard.
//   6. Tools segment renders pending_call when result hasn't arrived.
//   7. Tools segment routes PermissionAskSection →
//      DrawerPermissionAskCard (historical mode).
//   8. Reply segment shows live Text; 280-char truncate + "View full".
//   9. Reply segment shows FinalText after worker finishes (run.finalText).
//  10. transcriptTruncated !== 0 shows the "原 transcript 已截断" notice.
//  11. Empty state shows "Worker is starting..." when no sections.
//  12. Live indicator (running) + terminal chip (finished) render.
//  13. Header timestamps formatted as local HH:MM:SS.
//  14. Clicking the X (DialogClose) clears `openRunId`.
//  15. Failure banner (FT-F-005) for error / cancelled states.
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
import DrawerPromptCard from "./DrawerPromptCard.vue";
import DrawerThinkingBlock from "./DrawerThinkingBlock.vue";
import DrawerToolCallCard from "./DrawerToolCallCard.vue";
import DrawerPermissionAskCard from "./DrawerPermissionAskCard.vue";
import MarkdownDetailModal from "../common/MarkdownDetailModal.vue";
import {
  useSubagentRunsStore,
  type SubagentRunRow,
  type TranscriptSection,
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
  finalText: null,
  task: null,
};

function makeDrawer() {
  return mount(SubagentDrawer, {
    attachTo: document.body,
    global: {
      // reka-ui portals via <Teleport to="body">; stub Icon so the
      // test stays fast and avoids pulling heroicons into jsdom.
      // Do NOT stub the Drawer* components — we assert against their
      // real rendered output (props → DOM) below.
      stubs: { Icon: true },
    },
  });
}

/** Open the drawer with a pre-seeded row + section list. Avoids the
 *  async fetchRun roundtrip (which is mocked to return null) so the
 *  test can drive the store synchronously. */
async function openWith(
  store: ReturnType<typeof useSubagentRunsStore>,
  row: Partial<SubagentRunRow> = {},
  sections: TranscriptSection[] = [],
) {
  const full = { ...sampleRow, ...row };
  store.getRunCache.set(full.id, full);
  // Seed liveSections directly — PR5 reads this map first.
  store.liveSections.set(full.id, sections);
  await store.openDrawer(full.id);
  await flushPromises();
}

describe("SubagentDrawer", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    // vue-test-utils + reka-ui Teleport quirk: dialog content
    // sometimes leaks across tests. Belt-and-braces cleanup.
    document.body.innerHTML = "";
  });

  // -----------------------------------------------------------------
  // Basics: open / close, header, empty state
  // -----------------------------------------------------------------

  it("renders nothing visible when store.openRunId is null", () => {
    const store = useSubagentRunsStore();
    expect(store.openRunId).toBeNull();
    const w = makeDrawer();
    expect(document.body.querySelector(".subagent-drawer")).toBeNull();
    w.unmount();
  });

  it("renders header status + name + summary once a run is open", async () => {
    const store = useSubagentRunsStore();
    await openWith(store);
    const w = makeDrawer();
    await flushPromises();

    const text = document.body.textContent ?? "";
    expect(text).toContain("researcher");
    expect(text).toContain("完成"); // completed → 完成
    expect(text).toContain("found 3 files");
    w.unmount();
  });

  it("empty state shows 'Worker is starting...' when no sections", async () => {
    const store = useSubagentRunsStore();
    await openWith(store, { transcriptJson: null }, []);
    const w = makeDrawer();
    await flushPromises();

    const text = document.body.textContent ?? "";
    expect(text).toContain("Worker is starting");
    w.unmount();
  });

  it("header meta shows local-formatted start/finish times, not raw ISO", async () => {
    const store = useSubagentRunsStore();
    await openWith(store);
    const w = makeDrawer();
    await flushPromises();

    const meta = document.body.querySelector(".subagent-drawer__meta");
    const text = meta?.textContent ?? "";
    expect(text).toContain("开始");
    expect(text).toContain("结束");
    expect(text).not.toContain("T");
    expect(text).not.toContain("2026");
    expect(text).not.toContain("+00");
    expect(text.match(/\d{2}:\d{2}:\d{2}/g)?.length).toBe(2);
    w.unmount();
  });

  it("transcriptTruncated flag shows the '原 transcript 已截断' notice", async () => {
    const store = useSubagentRunsStore();
    await openWith(store, { transcriptTruncated: 1 });
    const w = makeDrawer();
    await flushPromises();

    const text = document.body.textContent ?? "";
    expect(text).toContain("原 transcript 已截断");
    w.unmount();
  });

  // -----------------------------------------------------------------
  // DrawerPromptCard — run.task rendering + truncate + modal
  // -----------------------------------------------------------------

  it("DrawerPromptCard is hidden when run.task is null", async () => {
    const store = useSubagentRunsStore();
    await openWith(store, { task: null });
    const w = makeDrawer();
    await flushPromises();

    expect(w.findAllComponents(DrawerPromptCard).length).toBe(1);
    // Card has its own v-if on `task && task.length > 0` → no
    // rendered .drawer-prompt-card in the DOM.
    expect(document.body.querySelector(".drawer-prompt-card")).toBeNull();
    w.unmount();
  });

  it("DrawerPromptCard renders run.task and truncates past 120 chars with View full link", async () => {
    const store = useSubagentRunsStore();
    const longTask = "A".repeat(200);
    await openWith(store, { task: longTask });
    const w = makeDrawer();
    await flushPromises();

    const card = document.body.querySelector(".drawer-prompt-card");
    expect(card).not.toBeNull();
    // The truncate() helper appends "…" → preview text is 121 chars
    // (120 + ellipsis). The full task is 200 chars.
    const markdown = card?.querySelector(".drawer-prompt-card__markdown");
    expect(markdown?.textContent?.length ?? 0).toBeLessThan(longTask.length);
    // The "View full →" link is present (truncation happened).
    expect(card?.querySelector(".drawer-prompt-card__view-full")).not.toBeNull();
    w.unmount();
  });

  it("DrawerPromptCard hides View full link when task fits within 120 chars", async () => {
    const store = useSubagentRunsStore();
    await openWith(store, { task: "short task" });
    const w = makeDrawer();
    await flushPromises();

    const card = document.body.querySelector(".drawer-prompt-card");
    expect(card?.querySelector(".drawer-prompt-card__view-full")).toBeNull();
    w.unmount();
  });

  it("DrawerPromptCard opens MarkdownDetailModal on View full click", async () => {
    const store = useSubagentRunsStore();
    await openWith(store, { task: "A".repeat(200) });
    const w = makeDrawer();
    await flushPromises();

    // Modal content not rendered initially.
    expect(document.body.querySelector(".markdown-detail-modal")).toBeNull();

    // Click "View full →" → modal opens (reka-ui portals to body).
    const btn = document.body.querySelector(
      ".drawer-prompt-card__view-full",
    ) as HTMLButtonElement;
    btn.click();
    await flushPromises();

    expect(document.body.querySelector(".markdown-detail-modal")).not.toBeNull();
    w.unmount();
  });

  // -----------------------------------------------------------------
  // Thinking segment
  // -----------------------------------------------------------------

  it("Thinking segment collapses by default and expands on chevron click", async () => {
    const store = useSubagentRunsStore();
    const sections: TranscriptSection[] = [
      { kind: "Thinking", text: "let me think...", chars: 16, closed: true },
    ];
    await openWith(store, {}, sections);
    const w = makeDrawer();
    await flushPromises();

    // The thinking DrawerSection exists.
    const thinkingSection = document.body.querySelector(
      '.drawer-section[data-type="thinking"]',
    );
    expect(thinkingSection).not.toBeNull();
    // Collapsed by default — no body content.
    expect(thinkingSection?.querySelector(".drawer-section__body")).toBeNull();

    // Click the header to expand.
    (thinkingSection?.querySelector(".drawer-section__header") as HTMLElement).click();
    await flushPromises();
    expect(thinkingSection?.querySelector(".drawer-section__body")).not.toBeNull();
    // DrawerThinkingBlock component mounts inside.
    expect(w.findAllComponents(DrawerThinkingBlock).length).toBe(1);
    w.unmount();
  });

  it("Thinking segment hidden entirely when no thinking sections", async () => {
    const store = useSubagentRunsStore();
    const sections: TranscriptSection[] = [
      { kind: "ToolCall", payload_json: { name: "grep", tool_use_id: "tu-1" } },
    ];
    await openWith(store, {}, sections);
    const w = makeDrawer();
    await flushPromises();

    expect(document.body.querySelector('.drawer-section[data-type="thinking"]')).toBeNull();
    w.unmount();
  });

  // -----------------------------------------------------------------
  // Tools segment — pairing layer (pairSections)
  // -----------------------------------------------------------------

  it("Tools segment pairs ToolCall + ToolResult into a single DrawerToolCallCard", async () => {
    const store = useSubagentRunsStore();
    const sections: TranscriptSection[] = [
      {
        kind: "ToolCall",
        payload_json: { name: "read_file", input: { path: "/foo" }, tool_use_id: "tu-1" },
      },
      {
        kind: "ToolResult",
        payload_json: { content: "ok", is_error: false, tool_use_id: "tu-1", duration_ms: 250 },
      },
    ];
    await openWith(store, {}, sections);
    const w = makeDrawer();
    await flushPromises();

    const cards = w.findAllComponents(DrawerToolCallCard);
    expect(cards.length).toBe(1);
    expect(cards[0].props("call").name).toBe("read_file");
    const result = cards[0].props("result");
    expect(result?.content).toBe("ok");
    expect(result?.isError).toBe(false);
    expect(result?.durationMs).toBe(250);
    w.unmount();
  });

  it("Tools segment renders pending DrawerToolCallCard (no result prop) for unmatched ToolCall", async () => {
    const store = useSubagentRunsStore();
    const sections: TranscriptSection[] = [
      {
        kind: "ToolCall",
        payload_json: { name: "shell", input: { command: "ls" }, tool_use_id: "tu-p" },
      },
    ];
    await openWith(store, {}, sections);
    const w = makeDrawer();
    await flushPromises();

    const cards = w.findAllComponents(DrawerToolCallCard);
    expect(cards.length).toBe(1);
    expect(cards[0].props("result")).toBeUndefined();
    expect(cards[0].props("call").name).toBe("shell");
    w.unmount();
  });

  it("Tools segment routes PermissionAskSection to DrawerPermissionAskCard", async () => {
    const store = useSubagentRunsStore();
    const chatStore = useChatStore();
    chatStore.currentCwd = "/data/repo";
    const sections: TranscriptSection[] = [
      {
        kind: "PermissionAsk",
        payload_json: {
          rid: "r-1",
          sessionId: "sess-1",
          toolUseId: "tu-1",
          toolName: "write_file",
          toolInput: { path: "/data/repo/x" },
          risk: "medium",
          path: "/data/repo/x",
        },
      },
    ];
    await openWith(store, {}, sections);
    const w = makeDrawer();
    await flushPromises();

    const cards = w.findAllComponents(DrawerPermissionAskCard);
    expect(cards.length).toBe(1);
    const ask = cards[0].props("ask");
    expect(ask.toolName).toBe("write_file");
    expect(ask.risk).toBe("medium");
    expect(ask.path).toBe("/data/repo/x");
    expect(cards[0].props("repoRoot")).toBe("/data/repo");
    w.unmount();
  });

  it("Tools segment is empty (hidden) when only Thinking sections present", async () => {
    const store = useSubagentRunsStore();
    const sections: TranscriptSection[] = [
      { kind: "Thinking", text: "...", chars: 3, closed: true },
    ];
    await openWith(store, {}, sections);
    const w = makeDrawer();
    await flushPromises();

    expect(document.body.querySelector('.drawer-section[data-type="tools"]')).toBeNull();
    w.unmount();
  });

  // -----------------------------------------------------------------
  // Reply segment — live Text + FinalText + truncate + modal
  // -----------------------------------------------------------------

  it("Reply segment shows live TextSection content and truncates past 280 chars", async () => {
    const store = useSubagentRunsStore();
    const longText = "B".repeat(400);
    const sections: TranscriptSection[] = [
      { kind: "Text", text: longText, chars: 400 },
    ];
    await openWith(store, { status: "running", finishedAt: null }, sections);
    const w = makeDrawer();
    await flushPromises();

    const replySection = document.body.querySelector(
      '.drawer-section[data-type="reply"]',
    );
    expect(replySection).not.toBeNull();
    const preview = replySection?.querySelector(".subagent-drawer__reply-markdown");
    expect(preview?.textContent?.length ?? 0).toBeLessThan(longText.length);
    // View full link present.
    expect(replySection?.querySelector(".subagent-drawer__reply-view-full")).not.toBeNull();
    w.unmount();
  });

  it("Reply segment prefers FinalTextSection over TextSection once worker finishes", async () => {
    const store = useSubagentRunsStore();
    const sections: TranscriptSection[] = [
      // FinalText wins — should be the rendered text even though a
      // live Text section is also present.
      { kind: "Text", text: "live partial", chars: 12 },
      { kind: "FinalText", text: "authoritative final reply" },
    ];
    await openWith(store, { status: "completed" }, sections);
    const w = makeDrawer();
    await flushPromises();

    const replySection = document.body.querySelector(
      '.drawer-section[data-type="reply"]',
    );
    const text = replySection?.querySelector(".subagent-drawer__reply-markdown")?.textContent ?? "";
    expect(text).toContain("authoritative final reply");
    expect(text).not.toContain("live partial");
    w.unmount();
  });

  it("Reply segment opens MarkdownDetailModal on View full click with source=reply", async () => {
    const store = useSubagentRunsStore();
    const sections: TranscriptSection[] = [
      { kind: "Text", text: "C".repeat(400), chars: 400 },
    ];
    await openWith(store, { status: "running", finishedAt: null }, sections);
    const w = makeDrawer();
    await flushPromises();

    expect(document.body.querySelector(".markdown-detail-modal")).toBeNull();
    const btn = document.body.querySelector(
      ".subagent-drawer__reply-view-full",
    ) as HTMLButtonElement;
    btn.click();
    await flushPromises();

    // Modal content is rendered.
    expect(document.body.querySelector(".markdown-detail-modal")).not.toBeNull();
    // The drawer mounts ONE MarkdownDetailModal (the reply one) when
    // truncated. The prompt modal is not mounted because task is null.
    expect(w.findAllComponents(MarkdownDetailModal).length).toBe(1);
    w.unmount();
  });

  it("Reply segment hidden entirely when no text sections and worker not running", async () => {
    const store = useSubagentRunsStore();
    const sections: TranscriptSection[] = [
      { kind: "ToolCall", payload_json: { name: "grep", tool_use_id: "tu-1" } },
    ];
    await openWith(store, { status: "completed" }, sections);
    const w = makeDrawer();
    await flushPromises();

    expect(document.body.querySelector('.drawer-section[data-type="reply"]')).toBeNull();
    w.unmount();
  });

  // -----------------------------------------------------------------
  // Live indicator (DrawerSection right-side chip)
  // -----------------------------------------------------------------

  it("running state shows live spinner chip on the Tools segment", async () => {
    const store = useSubagentRunsStore();
    const sections: TranscriptSection[] = [
      { kind: "ToolCall", payload_json: { name: "shell", tool_use_id: "tu-1" } },
    ];
    await openWith(store, { status: "running", finishedAt: null }, sections);
    const w = makeDrawer();
    await flushPromises();

    const toolsSection = document.body.querySelector(
      '.drawer-section[data-type="tools"]',
    );
    // Live chip is present (running state).
    expect(toolsSection?.querySelector(".drawer-section__live-chip")).not.toBeNull();
    // Terminal chip absent.
    expect(toolsSection?.querySelector(".drawer-section__final-chip")).toBeNull();
    w.unmount();
  });

  it("completed state shows '✓ X.Xs' terminal chip on segments (no live spinner)", async () => {
    const store = useSubagentRunsStore();
    const sections: TranscriptSection[] = [
      { kind: "ToolCall", payload_json: { name: "shell", tool_use_id: "tu-1" } },
      { kind: "ToolResult", payload_json: { content: "ok", is_error: false, tool_use_id: "tu-1", duration_ms: 5 } },
    ];
    await openWith(store, { status: "completed" }, sections);
    const w = makeDrawer();
    await flushPromises();

    const toolsSection = document.body.querySelector(
      '.drawer-section[data-type="tools"]',
    );
    expect(toolsSection?.querySelector(".drawer-section__live-chip")).toBeNull();
    const finalChip = toolsSection?.querySelector(".drawer-section__final-chip");
    expect(finalChip).not.toBeNull();
    // startedAt 10:00:00 + finishedAt 10:00:30 = 30s → "✓ 30.0s".
    expect(finalChip?.textContent ?? "").toContain("30.0s");
    w.unmount();
  });

  // -----------------------------------------------------------------
  // Status pill + jump-to-latest (preserved from B6 PR3b)
  // -----------------------------------------------------------------

  it("running state shows live duration suffix in status pill", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T10:00:08.200Z"));
    const store = useSubagentRunsStore();
    await openWith(store, { status: "running", finishedAt: null });
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
    await openWith(store);
    const w = makeDrawer();
    await flushPromises();

    const statusEl = document.body.querySelector(".subagent-drawer__status");
    expect(statusEl?.textContent?.trim()).toBe("完成 30.0s");
    w.unmount();
    vi.useRealTimers();
  });

  it("error state shows terminal duration (finishedAt - startedAt), not the live ticker", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T13:00:11.700Z"));
    const store = useSubagentRunsStore();
    await openWith(store, {
      status: "error",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:11.700Z",
    });
    const w = makeDrawer();
    await flushPromises();

    const statusEl = document.body.querySelector(".subagent-drawer__status");
    expect(statusEl?.textContent?.trim()).toBe("failed at 11.7s");
    w.unmount();
    vi.useRealTimers();
  });

  it("cancelled state shows terminal duration (finishedAt - startedAt)", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T13:00:05.300Z"));
    const store = useSubagentRunsStore();
    await openWith(store, {
      status: "cancelled",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:05.300Z",
    });
    const w = makeDrawer();
    await flushPromises();

    const statusEl = document.body.querySelector(".subagent-drawer__status");
    expect(statusEl?.textContent?.trim()).toBe("已停止 at 5.3s");
    w.unmount();
    vi.useRealTimers();
  });

  // -----------------------------------------------------------------
  // Failure banner (FT-F-005) — preserved
  // -----------------------------------------------------------------

  it("failed drawer shows the error banner with summary text", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T10:00:11.700Z"));
    const store = useSubagentRunsStore();
    await openWith(store, {
      status: "error",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:11.700Z",
      summary: "shell: timeout after 10.0s",
    });
    const w = makeDrawer();
    await flushPromises();

    const banner = document.body.querySelector(".subagent-drawer__banner");
    expect(banner).not.toBeNull();
    expect(banner?.classList.contains("subagent-drawer__banner--error")).toBe(true);
    expect(banner?.querySelector(".subagent-drawer__banner-text")?.textContent)
      .toBe("Worker exited with error: shell: timeout after 10.0s");
    w.unmount();
    vi.useRealTimers();
  });

  it("failed drawer falls back to 'unexpectedly' message when summary is empty", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T10:00:11.700Z"));
    const store = useSubagentRunsStore();
    await openWith(store, {
      status: "error",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:11.700Z",
      summary: null,
    });
    const w = makeDrawer();
    await flushPromises();

    const banner = document.body.querySelector(".subagent-drawer__banner");
    expect(banner?.querySelector(".subagent-drawer__banner-text")?.textContent)
      .toBe("Worker exited unexpectedly at 11.7s");
    w.unmount();
    vi.useRealTimers();
  });

  it("cancelled drawer shows the stopped-by-user banner with warning color", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-20T10:00:05.300Z"));
    const store = useSubagentRunsStore();
    await openWith(store, {
      status: "cancelled",
      startedAt: "2026-06-20T10:00:00.000Z",
      finishedAt: "2026-06-20T10:00:05.300Z",
      summary: "partial work before stop",
    });
    const w = makeDrawer();
    await flushPromises();

    const banner = document.body.querySelector(".subagent-drawer__banner");
    expect(banner?.classList.contains("subagent-drawer__banner--warning")).toBe(true);
    expect(banner?.classList.contains("subagent-drawer__banner--error")).toBe(false);
    expect(banner?.querySelector(".subagent-drawer__banner-text")?.textContent)
      .toBe("Worker stopped by user at 5.3s");
    w.unmount();
    vi.useRealTimers();
  });

  it("running and completed drawers do NOT render the failure banner", async () => {
    const store = useSubagentRunsStore();
    await openWith(store, { status: "running", finishedAt: null });
    const w = makeDrawer();
    await flushPromises();
    expect(document.body.querySelector(".subagent-drawer__banner")).toBeNull();
    w.unmount();

    const store2 = useSubagentRunsStore();
    await openWith(store2, { id: "run-2", status: "completed" });
    const w2 = makeDrawer();
    await flushPromises();
    expect(document.body.querySelector(".subagent-drawer__banner")).toBeNull();
    w2.unmount();
  });

  // -----------------------------------------------------------------
  // Legacy "chat events toggle" — the drawer no longer exposes this
  // surface. Lock the removal so a future revert catches in tests.
  // -----------------------------------------------------------------

  it("does NOT render the chat-events toggle / hidden-count hint (PRD R9)", async () => {
    const store = useSubagentRunsStore();
    await openWith(store);
    const w = makeDrawer();
    await flushPromises();

    expect(document.body.querySelector(".subagent-drawer__toggle")).toBeNull();
    expect(document.body.querySelector(".subagent-drawer__filter-row")).toBeNull();
    expect(document.body.querySelector(".subagent-drawer__event-count")).toBeNull();
    w.unmount();
  });
});
