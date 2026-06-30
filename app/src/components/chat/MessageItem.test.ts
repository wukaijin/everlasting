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
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
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
    questionCardsStore.addPending({
      sessionId: "sess-1",
      toolUseId: "tu-1",
      questions: QUESTIONS,
      ts: 1,
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
    questionCardsStore.addPending({
      sessionId: "sess-1",
      toolUseId: "tu-ask",
      questions: QUESTIONS,
      ts: 1,
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
    questionCardsStore.addPending({
      sessionId: "sess-1",
      toolUseId: "tu-pending",
      questions: QUESTIONS,
      ts: 1,
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
    questionCardsStore.addPending({
      sessionId: "sess-1",
      toolUseId: "tu-OTHER-pending",
      questions: QUESTIONS,
      ts: 1,
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
    questionCardsStore.addPending({
      sessionId: "sess-1",
      toolUseId: "tu-1",
      questions: QUESTIONS,
      ts: 1,
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