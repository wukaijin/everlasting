// Tests for the `ask_user_question` tool dispatch in
// `MessageItem.vue` — Phase E of `06-30-ask-user-question-tool`
// (2026-06-30). R22 / AC11 verification.
//
// Coverage:
//   1. AC11 routing: a tool_use block with
//      `name === "ask_user_question"` renders an
//      `<AskUserQuestionCard>` BELOW its `<ToolCallCard>`
//      (sibling within `msg__tools`).
//   2. Default dispatch: all other tool names (shell, write_file,
//      …) render ONLY the `<ToolCallCard>` — no inline card.
//   3. State resolution (live pending): when
//      `questionCardsStore.pendingBySession` carries a matching
//      tool_use_id, the card mounts with `state="pending"` and
//      the live `questions` payload.
//   4. State resolution (historical answer): when the message
//      has a `tool_result` block whose content is an answer
//      envelope, the card mounts with `state="answered"` and
//      the parsed answer (so reload-after-restart shows the
//      answered summary row).
//   5. State resolution (historical cancelled): when the
//      message has a tool_result block with `{"cancelled": true}`,
//      the card mounts with `state="cancelled"`.
//   6. Defensive guard: when NEITHER pending NOR a tool_result
//      exists, the inline card is NOT rendered (avoids mounting
//      an empty card during the brief tool_use → tool_result
//      window).
//   7. AC10 inherited: the AskUserQuestionCard mounts inside the
//      wrapper's component tree (no Teleport to body / no portal
//      residue — guards the design's UI red line inherited from
//      Phase D).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia, type Pinia } from "pinia";

import MessageItem from "./MessageItem.vue";
import { useChatStore } from "../../stores/chat";
import { useQuestionCardsStore } from "../../stores/questionCards";
import type { ChatMessage } from "../../stores/chat.types";
import type { Question } from "../../stores/questionCards.types";

// Tauri APIs aren't used in this component tree (no invoke calls
// from MessageItem itself — the AskUserQuestionCard does its own
// invoke, mocked inside the existing AskUserQuestionCard test).
// We still stub the Tauri modules to avoid the vue-test-utils
// renderer complaining about missing globals in jsdom.
vi.mock("../../transport", () => ({
  transport: {
    invoke: vi.fn(async () => null),
    listen: vi.fn(async () => () => {}),
  },
}));

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

const QUESTIONS: Question[] = [
  {
    question: "Pick a library",
    header: "Library",
    options: [
      { label: "Vue" },
      { label: "React" },
    ],
    multi_select: false,
  },
];

function makeAssistantMessage(
  toolCalls: ChatMessage["toolCalls"],
  toolResults: ChatMessage["toolResults"] = [],
): ChatMessage {
  return {
    id: "msg-1",
    role: "assistant",
    content: "thinking out loud",
    toolCalls,
    toolResults,
  };
}

function mountItem(
  message: ChatMessage,
  pinia: Pinia,
) {
  return mount(MessageItem, {
    props: { message },
    global: { plugins: [pinia] },
  });
}

// ---------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------

let pinia: Pinia;
let chatStore: ReturnType<typeof useChatStore>;
let questionCardsStore: ReturnType<typeof useQuestionCardsStore>;

beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  chatStore = useChatStore();
  questionCardsStore = useQuestionCardsStore();
  chatStore.currentSessionId = "sess-1";
  chatStore.sessions = [
    {
      id: "sess-1",
      title: "test",
      updated_at: "2026-01-01T00:00:00Z",
      preview: "",
      project_id: "proj-1",
      current_cwd: "/tmp",
      worktree_state: "none",
      worktree_path: null,
      last_worktree_path: null,
      model_id: null,
      input_tokens_total: null,
      output_tokens_total: null,
      cache_creation_total: null,
      cache_read_total: null,
      last_context_input_tokens: null,
      last_input_tokens: null,
      last_output_tokens: null,
      last_cache_creation: null,
      last_cache_read: null,
      color_tag: null,
      mode: "edit",
      workflow_enabled: false,
        plugin_name: "dev",
        // Group chat (Phase 4) — classic chat session fixture;
        // new SessionSummary fields are required but unused by
        // the MessageItem code paths under test.
        session_type: "chat",
        metadata: null,
    },
  ];
});

afterEach(() => {
  document
    .querySelectorAll(".ask-card-portal, .ask-card__overlay")
    .forEach((el) => el.remove());
});

// ---------------------------------------------------------------------
// 1. AC11 routing — ask_user_question tool_use gets the inline card
// ---------------------------------------------------------------------

describe("MessageItem — ask_user_question tool dispatch", () => {
  it("renders ToolCallCard + AskUserQuestionCard for ask_user_question", async () => {
    const message = makeAssistantMessage([
      {
        id: "tu-1",
        name: "ask_user_question",
        input: { questions: QUESTIONS },
      },
    ]);
    // Pre-populate the questionCards store so the card has a
    // pending entry (mirrors the live `tool:question` event flow).
    questionCardsStore.addPending("sess-1", {
      kind: "question",
      payload: {
        session_id: "sess-1",
        tool_use_id: "tu-1",
        questions: QUESTIONS,
        ts: 1,
      },
    });

    const wrapper = mountItem(message, pinia);
    await flushPromises();

    // The default ToolCallCard still renders (per R22: "保留现有
    // ToolCallCard 渲染"). It carries the tool metadata header.
    expect(wrapper.find(".tool-card").exists()).toBe(true);
    // The inline AskUserQuestionCard mounts BELOW (the section
    // exists with the ask-card root testid).
    const card = wrapper.find("[data-testid='ask-card']");
    expect(card.exists()).toBe(true);
    // Both live inside the SAME wrapper (siblings in msg__tools).
    const wrapperEl = wrapper.element as HTMLElement;
    expect(wrapperEl.contains(card.element)).toBe(true);
  });

  it("renders ONLY ToolCallCard for non-ask_user_question tools", async () => {
    const message = makeAssistantMessage([
      {
        id: "tu-shell-1",
        name: "shell",
        input: { command: "ls -la" },
      },
      {
        id: "tu-read-1",
        name: "read_file",
        input: { path: "/tmp/foo.txt" },
      },
    ]);

    const wrapper = mountItem(message, pinia);
    await flushPromises();

    // Two tool cards rendered (one per tool_use).
    const toolCards = wrapper.findAll(".tool-card");
    expect(toolCards.length).toBe(2);
    // NO AskUserQuestionCard rendered.
    expect(wrapper.find("[data-testid='ask-card']").exists()).toBe(false);
  });

  it("renders mixed batch correctly (ask + other tools)", async () => {
    const message = makeAssistantMessage([
      {
        id: "tu-read",
        name: "read_file",
        input: { path: "/tmp/foo.txt" },
      },
      {
        id: "tu-ask",
        name: "ask_user_question",
        input: { questions: QUESTIONS },
      },
      {
        id: "tu-shell",
        name: "shell",
        input: { command: "wc -l /tmp/foo.txt" },
      },
    ]);
    questionCardsStore.addPending("sess-1", {
      kind: "question",
      payload: {
        session_id: "sess-1",
        tool_use_id: "tu-ask",
        questions: QUESTIONS,
        ts: 1,
      },
    });

    const wrapper = mountItem(message, pinia);
    await flushPromises();

    // 3 tool cards rendered.
    expect(wrapper.findAll(".tool-card").length).toBe(3);
    // 1 ask card rendered.
    expect(wrapper.findAll("[data-testid='ask-card']").length).toBe(1);
  });
});

// ---------------------------------------------------------------------
// 2. State resolution — pending / answered / cancelled / none
// ---------------------------------------------------------------------

describe("MessageItem — AskUserQuestionCard state resolution", () => {
  it("mounts the card with state='pending' when live pending matches tool_use_id", async () => {
    const message = makeAssistantMessage([
      {
        id: "tu-pending",
        name: "ask_user_question",
        input: { questions: QUESTIONS },
      },
    ]);
    questionCardsStore.addPending("sess-1", {
      kind: "question",
      payload: {
        session_id: "sess-1",
        tool_use_id: "tu-pending",
        questions: QUESTIONS,
        ts: 1,
      },
    });

    const wrapper = mountItem(message, pinia);
    await flushPromises();
    // The card's pending action row (提交 + 跳过 buttons) is the
    // marker for `state="pending"`.
    expect(
      wrapper.find("[data-testid='ask-card-submit']").exists(),
    ).toBe(true);
    expect(
      wrapper.find("[data-testid='ask-card-skip']").exists(),
    ).toBe(true);
  });

  it("does NOT mount the card when no pending + no tool_result exists (defensive guard)", async () => {
    // No pending in store, no tool_result on the message. The
    // brief tool_use → tool_result window — defensive guard
    // prevents mounting an empty card.
    const message = makeAssistantMessage([
      {
        id: "tu-lonely",
        name: "ask_user_question",
        input: { questions: QUESTIONS },
      },
    ]);

    const wrapper = mountItem(message, pinia);
    await flushPromises();

    // The ToolCallCard still renders (we always show the tool
    // metadata header), but the inline AskUserQuestionCard is
    // suppressed (no pending + no result to derive state from).
    expect(wrapper.find(".tool-card").exists()).toBe(true);
    expect(wrapper.find("[data-testid='ask-card']").exists()).toBe(false);
  });

  it("mounts with state='answered' when tool_result is an answer envelope", async () => {
    const message = makeAssistantMessage(
      [
        {
          id: "tu-answered",
          name: "ask_user_question",
          input: { questions: QUESTIONS },
        },
      ],
      [
        {
          toolUseId: "tu-answered",
          isError: false,
          content: JSON.stringify({
            answer: [
              {
                question: "Pick a library",
                header: "Library",
                options: ["Vue"],
                multi_select: false,
              },
            ],
          }),
        },
      ],
    );

    const wrapper = mountItem(message, pinia);
    await flushPromises();

    // The card renders; the "answered" status pill + summary row
    // are the markers for `state="answered"` (no 提交 / 跳过 buttons).
    expect(wrapper.find("[data-testid='ask-card']").exists()).toBe(true);
    expect(
      wrapper.find("[data-testid='ask-card-state-answered']").exists(),
    ).toBe(true);
    expect(wrapper.find("[data-testid='ask-card-summary']").exists()).toBe(
      true,
    );
    expect(
      wrapper.find("[data-testid='ask-card-submit']").exists(),
    ).toBe(false);
  });

  it("rehydrates a custom answer envelope (R6) — parses + summary includes custom", async () => {
    // Historical answer that carries a `custom` field (D1 互斥 wire:
    // options:[] + custom). The replay path must parse it and
    // synthesize the custom text into the answered summary.
    const message = makeAssistantMessage(
      [
        {
          id: "tu-custom",
          name: "ask_user_question",
          input: { questions: QUESTIONS },
        },
      ],
      [
        {
          toolUseId: "tu-custom",
          isError: false,
          content: JSON.stringify({
            answer: [
              {
                question: "Pick a library",
                header: "Library",
                options: [],
                multi_select: false,
                custom: "Svelte",
              },
            ],
          }),
        },
      ],
    );

    const wrapper = mountItem(message, pinia);
    await flushPromises();

    // Card mounts in the answered state.
    expect(wrapper.find("[data-testid='ask-card']").exists()).toBe(true);
    expect(
      wrapper.find("[data-testid='ask-card-state-answered']").exists(),
    ).toBe(true);
    expect(wrapper.find("[data-testid='ask-card-summary']").exists()).toBe(
      true,
    );
    // The summary renders the custom pill (selectedAnswer carries
    // `custom`, and synthQuestions synthesizes it as a label too).
    const summaryLabels = wrapper
      .find("[data-testid='ask-card-summary']")
      .findAll(".ask-card__summary-label")
      .map((l) => l.text());
    expect(summaryLabels).toContain("自定义: Svelte");
  });

  it("mounts with state='cancelled' when tool_result is { cancelled: true }", async () => {
    const message = makeAssistantMessage(
      [
        {
          id: "tu-cancelled",
          name: "ask_user_question",
          input: { questions: QUESTIONS },
        },
      ],
      [
        {
          toolUseId: "tu-cancelled",
          isError: true, // backend records is_error on cancel
          content: JSON.stringify({ cancelled: true }),
        },
      ],
    );

    const wrapper = mountItem(message, pinia);
    await flushPromises();

    expect(wrapper.find("[data-testid='ask-card']").exists()).toBe(true);
    expect(
      wrapper.find("[data-testid='ask-card-state-cancelled']").exists(),
    ).toBe(true);
    expect(
      wrapper.find("[data-testid='ask-card-cancelled-note']").exists(),
    ).toBe(true);
  });

  it("ignores pending entry whose tool_use_id does not match this row's tool_use", async () => {
    // Pending belongs to a different tool_use (race window — backend
    // answered a previous question for this session, a new one is
    // pending for a different tool_use_id; we want to render the
    // answered card for THIS row, not the pending one).
    const message = makeAssistantMessage(
      [
        {
          id: "tu-this-row",
          name: "ask_user_question",
          input: { questions: QUESTIONS },
        },
      ],
      [
        {
          toolUseId: "tu-this-row",
          isError: false,
          content: JSON.stringify({
            answer: [
              {
                question: "Pick a library",
                options: ["React"],
                multi_select: false,
              },
            ],
          }),
        },
      ],
    );
    questionCardsStore.addPending("sess-1", {
      kind: "question",
      payload: {
        session_id: "sess-1",
        tool_use_id: "tu-OTHER-pending",
        questions: QUESTIONS,
        ts: 1,
      },
    });

    const wrapper = mountItem(message, pinia);
    await flushPromises();

    // The pending for a DIFFERENT tool_use doesn't apply — we
    // fall through to the tool_result path and render 'answered'.
    expect(
      wrapper.find("[data-testid='ask-card-state-answered']").exists(),
    ).toBe(true);
  });
});

// ---------------------------------------------------------------------
// 3. AC10 inherited — inline, not portaled
// ---------------------------------------------------------------------

describe("MessageItem — AC10 inherited (no modal / no portal)", () => {
  it("mounts the AskUserQuestionCard inside the message tree (no Teleport to body)", async () => {
    const message = makeAssistantMessage([
      {
        id: "tu-1",
        name: "ask_user_question",
        input: { questions: QUESTIONS },
      },
    ]);
    questionCardsStore.addPending("sess-1", {
      kind: "question",
      payload: {
        session_id: "sess-1",
        tool_use_id: "tu-1",
        questions: QUESTIONS,
        ts: 1,
      },
    });

    const wrapper = mountItem(message, pinia);
    await flushPromises();

    const cardEl = wrapper
      .find("[data-testid='ask-card']")
      .element as HTMLElement;
    const wrapperEl = wrapper.element as HTMLElement;
    expect(wrapperEl.contains(cardEl)).toBe(true);
    // No portal residue.
    expect(
      document.querySelectorAll(".ask-card-portal, .ask-card__overlay").length,
    ).toBe(0);
  });
});
// ---------------------------------------------------------------------
// Group chat (07-29-group-chat, Phase 4 Step 4 G3): speaker chip
// rendering. Three cases cover the v-if gate + the two rendering
// branches (moderator → "主持人" + neutral, participant → name +
// palette hash, no-chip → undefined speaker).
// ---------------------------------------------------------------------

describe("MessageItem — group chat speaker chip", () => {
  it("renders the chip when message.speaker is set on an assistant row", async () => {
    const message = makeAssistantMessage([], []);
    message.speaker = "Alex";
    message.seq = 5;
    const wrapper = mountItem(message, pinia);
    await flushPromises();
    const chip = wrapper.find('[data-testid="msg-speaker-chip-5"]');
    expect(chip.exists()).toBe(true);
    expect(chip.text()).toContain("Alex");
    expect(chip.attributes("data-speaker")).toBe("Alex");
  });

  it("renders '主持人' + neutral accent for moderator turns", async () => {
    const message = makeAssistantMessage([], []);
    message.speaker = "moderator";
    message.seq = 7;
    const wrapper = mountItem(message, pinia);
    await flushPromises();
    const chip = wrapper.find('[data-testid="msg-speaker-chip-7"]');
    expect(chip.exists()).toBe(true);
    expect(chip.text()).toContain("主持人");
    // The moderator class is a fixed "neutral" — no palette hash.
    expect(chip.classes()).toContain("msg-speaker-chip--neutral");
  });

  it("does not render the chip when message.speaker is undefined", async () => {
    const message = makeAssistantMessage([], []);
    message.seq = 9;
    // speaker intentionally undefined (classic chat path).
    const wrapper = mountItem(message, pinia);
    await flushPromises();
    expect(wrapper.find('[data-testid="msg-speaker-chip-9"]').exists()).toBe(
      false,
    );
  });
});

// ---------------------------------------------------------------------
// B1 (2026-08-16) image-multimodal R2a: the user-message attachment
// thumbnail strip. MessageItem maps `message.metadata.attachments`
// (optimistic camelCase + rehydrated snake_case — see AttachmentView)
// into the MessageImages entry shape; these tests lock the mapping
// and the user-row-only gate.
// ---------------------------------------------------------------------

describe("MessageItem — B1 attachment thumbnails", () => {
  function makeUserMessage(metadata: Record<string, unknown>): ChatMessage {
    return {
      id: "msg-user-1",
      role: "user",
      content: "看看这张图",
      metadata,
    };
  }

  it("renders one thumbnail per metadata.attachments entry (both shapes)", async () => {
    const message = makeUserMessage({
      attachments: [
        // Optimistic (just-sent) entry: camelCase + localUrl.
        { file: "a1.png", localUrl: "blob:opt", mediaType: "image/png" },
        // Rehydrated (DB) entry: snake_case, no localUrl.
        { file: "b2.jpg", media_type: "image/jpeg", source: "paste" },
        // Garbage entries are dropped, not rendered.
        null,
        { localUrl: "", mediaType: "image/png" },
      ],
    });
    const wrapper = mountItem(message, pinia);
    await flushPromises();

    const thumbs = wrapper.findAll(".message-images__item");
    expect(thumbs.length).toBe(2);
    // file ref wins → daemon GET route (daemonBase in vitest DEV
    // jsdom is http://localhost:7456; no token → direct path).
    expect(thumbs[0].get("img").attributes("src")).toBe(
      "http://localhost:7456/api/v1/attachments/sess-1/a1.png",
    );
    expect(thumbs[1].get("img").attributes("src")).toBe(
      "http://localhost:7456/api/v1/attachments/sess-1/b2.jpg",
    );
  });

  it("does not render the strip for assistant rows or messages without attachments", async () => {
    const assistant = makeAssistantMessage([], []);
    const w1 = mountItem(assistant, pinia);
    await flushPromises();
    expect(w1.find(".message-images").exists()).toBe(false);

    const plain = makeUserMessage({});
    const w2 = mountItem(plain, pinia);
    await flushPromises();
    expect(w2.find(".message-images").exists()).toBe(false);
  });
});

// ---------------------------------------------------------------------
// D2②+ (08-17-search-history-card): search_history tool_use renders
// the dedicated SearchHistoryCard INSTEAD of the generic
// ToolCallCard (same replace-dispatch as end_discussion →
// DiscussionSummaryCard).
// ---------------------------------------------------------------------

describe("MessageItem — search_history tool dispatch", () => {
  it("replaces ToolCallCard with SearchHistoryCard for search_history", async () => {
    const message = makeAssistantMessage(
      [{ id: "tu-sh-1", name: "search_history", input: { query: "worktree" } }],
      [
        {
          toolUseId: "tu-sh-1",
          content: 'Found 1 hits for "worktree" (scope: all projects):',
          isError: false,
        },
      ],
    );
    const wrapper = mountItem(message, pinia);
    await flushPromises();

    const card = wrapper.find("[data-testid='search-history-card-tu-sh-1']");
    expect(card.exists()).toBe(true);
    // The generic tool card is REPLACED (not a sibling below it).
    expect(wrapper.find(".tool-card").exists()).toBe(false);
  });

  it("still renders the generic ToolCallCard for other read tools", async () => {
    const message = makeAssistantMessage([
      { id: "tu-grep-1", name: "grep", input: { pattern: "x", path: "." } },
    ]);
    const wrapper = mountItem(message, pinia);
    await flushPromises();
    expect(wrapper.find(".tool-card").exists()).toBe(true);
    expect(wrapper.find("[data-testid^='search-history-card']").exists()).toBe(false);
  });
});
