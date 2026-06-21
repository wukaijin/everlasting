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
// PR6 (2026-06-21) additions — 3 boundary states:
//  16. Error card (R25): status=error renders ❌ card with message
//      extracted via the 4-level fallback (transcriptJson last error
//      event → finalText → summary → canned).
//  17. Cancelled chip (R23 downgraded): status=cancelled renders
//      `⊘ Cancelled · at X.Xs` chip at the top of the Reply segment.
//  18. PermissionAsk historical-mode not regressed by R24 downgrade
//      (card still renders; auto-denied notice is present).
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
import { usePermissionsStore } from "../../stores/permissions";

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

  // -----------------------------------------------------------------
  // PR6 (2026-06-21) — boundary states: error / cancelled / permission_ask
  // -----------------------------------------------------------------

  describe("PR6 R25 — error card (status=error)", () => {
    it("renders the error card when status=error", async () => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-06-20T10:00:11.700Z"));
      const store = useSubagentRunsStore();
      await openWith(store, {
        status: "error",
        startedAt: "2026-06-20T10:00:00.000Z",
        finishedAt: "2026-06-20T10:00:11.700Z",
        summary: "boom",
      });
      const w = makeDrawer();
      await flushPromises();

      const card = document.body.querySelector(".subagent-drawer__error-card");
      expect(card).not.toBeNull();
      // Header carries the "Worker error" title + shield-x icon.
      expect(card?.querySelector(".subagent-drawer__error-title")?.textContent)
        .toBe("Worker error");
      // Message body is present (fallback chain hit level 3 = summary).
      expect(card?.querySelector(".subagent-drawer__error-message")?.textContent)
        .toBe("boom");
      w.unmount();
      vi.useRealTimers();
    });

    it("error message prefers transcriptJson last chat_event/error entry (level 1)", async () => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-06-20T10:00:11.700Z"));
      const store = useSubagentRunsStore();
      // transcriptJson with an inner error event — the LAST chat_event
      // in the array carries kind="error" + message. Earlier events
      // are non-error (delta) and MUST be skipped by the reverse scan.
      const transcriptJson = JSON.stringify([
        { kind: "chat_event", payload_json: { kind: "delta", text: "partial..." } },
        { kind: "tool_call", payload_json: { name: "grep", tool_use_id: "tu-1" } },
        { kind: "chat_event", payload_json: { kind: "error", message: "LLM stream timeout after 30s" } },
      ]);
      await openWith(store, {
        status: "error",
        transcriptJson,
        finalText: "fallback final text",
        summary: "fallback summary",
      });
      const w = makeDrawer();
      await flushPromises();

      const msg = document.body.querySelector(".subagent-drawer__error-message")?.textContent;
      // Level 1 wins — the transcriptJson error message is used, not
      // finalText or summary.
      expect(msg).toBe("LLM stream timeout after 30s");
      w.unmount();
      vi.useRealTimers();
    });

    it("error message falls back to finalText when transcriptJson has no error event (level 2)", async () => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-06-20T10:00:11.700Z"));
      const store = useSubagentRunsStore();
      // transcriptJson with NO inner error event — fallback to finalText.
      const transcriptJson = JSON.stringify([
        { kind: "chat_event", payload_json: { kind: "delta", text: "partial..." } },
      ]);
      await openWith(store, {
        status: "error",
        transcriptJson,
        finalText: "worker exited with stderr output",
        summary: "summary-not-used-here",
      });
      const w = makeDrawer();
      await flushPromises();

      const msg = document.body.querySelector(".subagent-drawer__error-message")?.textContent;
      expect(msg).toBe("worker exited with stderr output");
      w.unmount();
      vi.useRealTimers();
    });

    it("error message falls back to summary when finalText is null (level 3)", async () => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-06-20T10:00:11.700Z"));
      const store = useSubagentRunsStore();
      await openWith(store, {
        status: "error",
        transcriptJson: null,
        finalText: null,
        summary: "only summary available",
      });
      const w = makeDrawer();
      await flushPromises();

      const msg = document.body.querySelector(".subagent-drawer__error-message")?.textContent;
      expect(msg).toBe("only summary available");
      w.unmount();
      vi.useRealTimers();
    });

    it("error message falls back to canned string when all sources empty (level 4)", async () => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-06-20T10:00:11.700Z"));
      const store = useSubagentRunsStore();
      await openWith(store, {
        status: "error",
        transcriptJson: null,
        finalText: null,
        summary: null,
      });
      const w = makeDrawer();
      await flushPromises();

      const msg = document.body.querySelector(".subagent-drawer__error-message")?.textContent;
      expect(msg).toBe("(no error text captured)");
      w.unmount();
      vi.useRealTimers();
    });

    it("does NOT render the error card when status is not error", async () => {
      const store = useSubagentRunsStore();
      await openWith(store, { status: "completed" });
      const w = makeDrawer();
      await flushPromises();
      expect(document.body.querySelector(".subagent-drawer__error-card")).toBeNull();
      w.unmount();
    });

    it("ignores non-error chat_event entries when scanning for the error message", async () => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-06-20T10:00:11.700Z"));
      const store = useSubagentRunsStore();
      // An error-like message appears in a DELTA event's text — this
      // is NOT an error event (kind=delta) and MUST be skipped. The
      // fallback chain should continue to finalText.
      const transcriptJson = JSON.stringify([
        { kind: "chat_event", payload_json: { kind: "delta", text: "Error: something failed" } },
      ]);
      await openWith(store, {
        status: "error",
        transcriptJson,
        finalText: "real final text",
        summary: null,
      });
      const w = makeDrawer();
      await flushPromises();

      const msg = document.body.querySelector(".subagent-drawer__error-message")?.textContent;
      // The delta's "Error: something failed" must NOT win — only
      // inner kind=error counts. finalText is the winner here.
      expect(msg).toBe("real final text");
      w.unmount();
      vi.useRealTimers();
    });
  });

  describe("PR6 R23 (downgraded) — cancelled chip in Reply segment", () => {
    it("renders ⊘ Cancelled chip with terminal duration when status=cancelled", async () => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-06-20T10:00:05.300Z"));
      const store = useSubagentRunsStore();
      await openWith(store, {
        status: "cancelled",
        startedAt: "2026-06-20T10:00:00.000Z",
        finishedAt: "2026-06-20T10:00:05.300Z",
      });
      const w = makeDrawer();
      await flushPromises();

      const chip = document.body.querySelector(".subagent-drawer__reply-cancelled");
      expect(chip).not.toBeNull();
      // 5.3s = (5300ms / 1000).toFixed(1)
      expect(chip?.textContent).toContain("⊘ Cancelled");
      expect(chip?.textContent).toContain("at 5.3s");
      w.unmount();
      vi.useRealTimers();
    });

    it("cancelled chip renders even when replyText is empty (segment visibility)", async () => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-06-20T10:00:05.300Z"));
      const store = useSubagentRunsStore();
      // No sections → replyText empty. The Reply segment MUST still
      // render because the template's v-if includes status === 'cancelled'.
      await openWith(store, {
        status: "cancelled",
        startedAt: "2026-06-20T10:00:00.000Z",
        finishedAt: "2026-06-20T10:00:05.300Z",
      }, []);
      const w = makeDrawer();
      await flushPromises();

      expect(document.body.querySelector('.drawer-section[data-type="reply"]')).not.toBeNull();
      expect(document.body.querySelector(".subagent-drawer__reply-cancelled")).not.toBeNull();
      // No reply body (empty) and no "Worker has not produced a reply"
      // placeholder (cancelled gate suppresses it).
      expect(document.body.querySelector(".subagent-drawer__reply-body")).toBeNull();
      expect(document.body.querySelector(".subagent-drawer__reply-empty")).toBeNull();
      w.unmount();
      vi.useRealTimers();
    });

    it("cancelled chip renders ABOVE reply body when worker produced text before stop", async () => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-06-20T10:00:05.300Z"));
      const store = useSubagentRunsStore();
      const sections: TranscriptSection[] = [
        { kind: "Text", text: "partial reply before stop", chars: 25 },
      ];
      await openWith(store, {
        status: "cancelled",
        startedAt: "2026-06-20T10:00:00.000Z",
        finishedAt: "2026-06-20T10:00:05.300Z",
      }, sections);
      const w = makeDrawer();
      await flushPromises();

      // BOTH the cancelled chip and the reply body render (PR6
      // implementation choice: chip + preserved reply so user can
      // inspect the worker's partial output).
      expect(document.body.querySelector(".subagent-drawer__reply-cancelled")).not.toBeNull();
      const replyBody = document.body.querySelector(".subagent-drawer__reply-body");
      expect(replyBody).not.toBeNull();
      expect(replyBody?.textContent).toContain("partial reply before stop");
      w.unmount();
      vi.useRealTimers();
    });

    it("does NOT render cancelled chip when status is not cancelled", async () => {
      const store = useSubagentRunsStore();
      await openWith(store, { status: "completed" });
      const w = makeDrawer();
      await flushPromises();
      expect(document.body.querySelector(".subagent-drawer__reply-cancelled")).toBeNull();
      w.unmount();
    });
  });

  describe("PR2 RULE-FrontSubagent-003 — permission_ask interactive reconciliation", () => {
    it("renders HISTORICAL card when rid is NOT live-pending (resolved / transcript-only)", async () => {
      const store = useSubagentRunsStore();
      const chatStore = useChatStore();
      chatStore.currentCwd = "/data/repo";
      const sections: TranscriptSection[] = [
        {
          kind: "PermissionAsk",
          payload_json: {
            rid: "r-historical-1",
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
      // PR2: rid not in the live permissions store → interactive=false.
      expect(cards[0].props("interactive")).toBe(false);
      // The "等待审批" status text MUST NOT render in historical mode.
      const cardEl = document.body.querySelector(".drawer-permission-ask-card");
      expect(cardEl?.textContent).not.toContain("等待审批");
      expect(cardEl?.textContent).toContain("已记录");
      // The 4 interactive action buttons MUST NOT render.
      expect(cardEl?.querySelector(".permission-ask-body__actions")).toBeNull();
      w.unmount();
    });

    it("renders INTERACTIVE card with Allow/Deny buttons when rid IS live-pending", async () => {
      const store = useSubagentRunsStore();
      const chatStore = useChatStore();
      const permissionsStore = usePermissionsStore();
      chatStore.currentCwd = "/data/repo";
      const sections: TranscriptSection[] = [
        {
          kind: "PermissionAsk",
          payload_json: {
            rid: "r-live-1",
            sessionId: "sess-1",
            toolUseId: "tu-live",
            toolName: "shell",
            toolInput: { command: "rm -rf /tmp/x" },
            risk: "critical",
            // N3 (2026-06-22): the transcript payload carries
            // `workerRunId` too — the PR1.5 backend emits the same
            // payload shape on BOTH the `permission:ask` IPC channel
            // (live store) AND the `subagent:event` transcript
            // channel. `DrawerPermissionAskCard` derives
            // `hideAllowAlways` from `ask.workerRunId` (sourced from
            // the transcript via `synthesizeAsk`), so the transcript
            // payload MUST carry it for the button to be hidden.
            workerRunId: "run-1",
          },
        },
      ];
      await openWith(store, {}, sections);
      // Seed the live pending ask — simulates the PR2 backend
      // emitting a real `permission:ask` IPC for this worker.
      permissionsStore.setPending({
        rid: "r-live-1",
        sessionId: "sess-1",
        toolUseId: "tu-live",
        toolName: "shell",
        toolInput: { command: "rm -rf /tmp/x" },
        risk: "critical",
        workerRunId: "run-1",
      });

      const w = makeDrawer();
      await flushPromises();

      const cards = w.findAllComponents(DrawerPermissionAskCard);
      expect(cards.length).toBe(1);
      // PR2: rid IS in the live permissions store → interactive=true.
      expect(cards[0].props("interactive")).toBe(true);
      const cardEl = document.body.querySelector(".drawer-permission-ask-card");
      // Status pill flips to "等待审批".
      expect(cardEl?.textContent).toContain("等待审批");
      // The interactive action row renders. N3 fix (RULE-FrontSubagent-003
      // check phase, 2026-06-22): worker asks hide the "始终允许" button
      // (backend treats worker AllowAlways as AllowOnce — persisting a
      // worker grant to `session_tool_permissions` would cross privilege
      // boundaries). So a worker ask renders 3 buttons, NOT 4.
      expect(cardEl?.querySelector(".permission-ask-body__actions"))
        .not.toBeNull();
      const btns = cardEl?.querySelectorAll(".permission-ask-body__btn");
      expect(btns?.length ?? 0).toBe(3);
      // Specifically: the "始终允许" button must NOT render for a worker ask.
      const alwaysBtn = cardEl?.querySelector(".permission-ask-body__btn--always");
      expect(alwaysBtn).toBeNull();
      w.unmount();
    });

    it("N3: worker ask (workerRunId present) hides the 始终允许 button", async () => {
      // Worker asks route through `DrawerPermissionAskCard`, which
      // derives `hideAllowAlways` from `ask.workerRunId`. The
      // main-chat path (no workerRunId) keeps all 4 buttons; that
      // half of the contract is unit-tested directly in
      // `PermissionAskBody.test.ts::hideAllowAlways prop` (no Tauri
      // runtime needed there). This test covers the integration:
      // drawer → card → body end-to-end for the worker case.
      const store = useSubagentRunsStore();
      const chatStore = useChatStore();
      const permissionsStore = usePermissionsStore();
      chatStore.currentCwd = "/data/repo";

      const workerSections: TranscriptSection[] = [
        {
          kind: "PermissionAsk",
          payload_json: {
            rid: "r-worker-1",
            sessionId: "sess-1",
            toolUseId: "tu-w",
            toolName: "write_file",
            toolInput: { path: "/outside/x" },
            risk: "medium",
            workerRunId: "run-1",
          },
        },
      ];
      await openWith(store, {}, workerSections);
      permissionsStore.setPending({
        rid: "r-worker-1",
        sessionId: "sess-1",
        toolUseId: "tu-w",
        toolName: "write_file",
        toolInput: { path: "/outside/x" },
        risk: "medium",
        workerRunId: "run-1",
      });
      const w = makeDrawer();
      await flushPromises();

      const workerCardEl = document.body.querySelector(".drawer-permission-ask-card");
      // 3 buttons: 仅一次 / 拒绝 / 拒绝并说明 (no 始终允许).
      expect(workerCardEl?.querySelectorAll(".permission-ask-body__btn").length ?? 0)
        .toBe(3);
      expect(workerCardEl?.querySelector(".permission-ask-body__btn--always"))
        .toBeNull();
      w.unmount();
    });

    it("flips from interactive to historical when the ask is resolved", async () => {
      const store = useSubagentRunsStore();
      const chatStore = useChatStore();
      const permissionsStore = usePermissionsStore();
      chatStore.currentCwd = "/data/repo";
      const sections: TranscriptSection[] = [
        {
          kind: "PermissionAsk",
          payload_json: {
            rid: "r-flip",
            sessionId: "sess-1",
            toolUseId: "tu-flip",
            toolName: "shell",
            toolInput: { command: "ls" },
            risk: "high",
          },
        },
      ];
      await openWith(store, {}, sections);
      permissionsStore.setPending({
        rid: "r-flip",
        sessionId: "sess-1",
        toolUseId: "tu-flip",
        toolName: "shell",
        toolInput: { command: "ls" },
        risk: "high",
        workerRunId: "run-1",
      });

      const w = makeDrawer();
      await flushPromises();
      let cards = w.findAllComponents(DrawerPermissionAskCard);
      expect(cards[0].props("interactive")).toBe(true);

      // User responds → store clears the slot → card flips to historical.
      await permissionsStore.respond("r-flip", "allow_once");
      await flushPromises();
      cards = w.findAllComponents(DrawerPermissionAskCard);
      expect(cards[0].props("interactive")).toBe(false);
      w.unmount();
    });
  });
});
