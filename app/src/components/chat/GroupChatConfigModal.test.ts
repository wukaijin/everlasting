// GroupChatConfigModal — minimal coverage for the Phase 4 Step 3
// modal's core invariants (validation + participant uniqueness +
// save shape). Doesn't go for full DOM integration — the modal
// reuses reka-ui primitives (Dialog/Select) that already have
// their own contract; we only test the modal's own logic.
//
// Focus: the validation rules (D5: 2-3 participants, name
// unique, name non-empty, model selected) + the submit disabled
// state mirror. We don't fire the IPC here — the chat store +
// transport layer have their own tests; we just verify the
// modal's UI-contract behavior.
//
// Note: reka-ui's Dialog uses `<DialogPortal>` which teleports
// to `<body>`. The testid selectors in this file therefore
// query `document` directly (not `wrapper.find(...)`) — the
// mounted wrapper's DOM doesn't contain the teleported subtree.

import { describe, it, expect, afterEach, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { useModelsStore } from "../../stores/models";
import { useChatStore } from "../../stores/chat";
import type { SessionSummary } from "../../stores/chat.types";
import GroupChatConfigModal from "./GroupChatConfigModal.vue";

// The edit-mode cache-rate feature invokes `group_chat_cache_rates`
// over the transport. Mock the transport module (same file-level
// `vi.mock` pattern as `app/src/stores/traceStore.test.ts`) so the
// modal tests drive the IPC response without a real backend.
const invokeMock = vi.fn();

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

const MODEL_LIST = [
  {
    id: "m1",
    providerId: "p1",
    modelName: "gpt-4",
    displayName: "GPT-4",
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: false,
    contextWindow: 128000,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    providerDisplayName: "OpenAI",
    providerProtocol: "openai",
  },
  {
    id: "m2",
    providerId: "p2",
    modelName: "claude-3-5",
    displayName: "Claude 3.5",
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: true,
    contextWindow: 200000,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    providerDisplayName: "Anthropic",
    providerProtocol: "anthropic",
  },
];

function mountModal(
  props: Partial<InstanceType<typeof GroupChatConfigModal>["$props"]> = {},
) {
  const pinia = createPinia();
  setActivePinia(pinia);
  const modelsStore = useModelsStore();
  modelsStore.models = MODEL_LIST as never;
  return mount(GroupChatConfigModal, {
    props: { open: true, mode: "create", ...props },
    global: { plugins: [pinia] },
    attachTo: document.body,
  });
}

function byTestId(id: string): HTMLElement | null {
  return document.querySelector(`[data-testid="${id}"]`);
}
function allByTestIdPrefix(prefix: string): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>(`[data-testid^="${prefix}"]`));
}

describe("GroupChatConfigModal — validation", () => {
  afterEach(() => {
    // Remove any teleported DOM residue.
    document
      .querySelectorAll(".gcfg-content, .gcfg-overlay")
      .forEach((el) => el.remove());
  });

  it("seeds 2 empty participants in create mode with disabled submit", async () => {
    mountModal();
    await new Promise((r) => setTimeout(r, 0));
    const rows = document.querySelectorAll<HTMLElement>(".gcfg-row");
    expect(rows.length).toBe(2);
    const submit = byTestId("gcfg-submit") as HTMLButtonElement | null;
    expect(submit).toBeTruthy();
    expect(submit!.disabled).toBe(true);
  });

  it("hides '+' button when 3 participants reached (D5 max)", async () => {
    mountModal();
    await new Promise((r) => setTimeout(r, 0));
    const addBtn = byTestId("gcfg-add") as HTMLButtonElement | null;
    expect(addBtn).toBeTruthy();
    addBtn!.click();
    addBtn!.click();
    await new Promise((r) => setTimeout(r, 0));
    const rows = document.querySelectorAll<HTMLElement>(".gcfg-row");
    expect(rows.length).toBe(3);
    expect(byTestId("gcfg-add")).toBeNull();
  });

  it("delete (-remove) buttons are disabled when only 2 participants remain", async () => {
    mountModal();
    await new Promise((r) => setTimeout(r, 0));
    const removes = allByTestIdPrefix("gcfg-remove-");
    expect(removes.length).toBe(2);
    for (const r of removes) {
      expect((r as HTMLButtonElement).disabled).toBe(true);
    }
  });

  it("emits update:open with false when cancel is clicked", async () => {
    const wrapper = mountModal();
    await new Promise((r) => setTimeout(r, 0));
    const cancel = byTestId("gcfg-cancel") as HTMLButtonElement | null;
    expect(cancel).toBeTruthy();
    cancel!.click();
    await new Promise((r) => setTimeout(r, 0));
    const events = wrapper.emitted("update:open");
    expect(events).toBeTruthy();
    expect(events![0]).toEqual([false]);
  });
});

describe("GroupChatConfigModal — cache rates (edit mode)", () => {
  // Group-chat cache rate (08-10-group-chat-cache-rate, R6/R7):
  // edit mode shows each speaker's latest LLM call cache rate;
  // create mode never loads or shows it.
  const roster = [
    { name: "Alice", model: "m1" },
    { name: "Bob", model: "m2" },
  ];

  function seedSession() {
    const chatStore = useChatStore();
    chatStore.sessions = [
      {
        id: "sess-1",
        title: "group",
        updated_at: "2026-01-01T00:00:00Z",
        preview: "",
        project_id: "proj-1",
        current_cwd: "/tmp",
        worktree_state: "none",
        worktree_path: null,
        last_worktree_path: null,
        model_id: "m1",
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
        session_type: "group_chat",
        metadata: null,
      } as SessionSummary,
    ];
  }

  afterEach(() => {
    invokeMock.mockReset();
    document
      .querySelectorAll(".gcfg-content, .gcfg-overlay")
      .forEach((el) => el.remove());
  });

  it("renders per-participant + moderator cache rates from the IPC payload", async () => {
    invokeMock.mockResolvedValue([
      { speaker: "Alice", cache_read: 50, context_input: 200 },
      { speaker: "moderator", cache_read: 40, context_input: 100 },
    ]);
    mountModal({
      mode: "edit",
      sessionId: "sess-1",
      initialParticipants: roster,
    });
    // Seed the session AFTER mount (the store needs the pinia that
    // mountModal activates); the moderator computed is reactive.
    seedSession();
    await new Promise((r) => setTimeout(r, 0));

    // One IPC fetch per open, with the session id.
    expect(invokeMock).toHaveBeenCalledWith("group_chat_cache_rates", {
      sessionId: "sess-1",
    });

    // Participant rows: Alice has a row (25%), Bob has none yet.
    expect(byTestId("gcfg-cache-rate-0")?.textContent).toContain("缓存率 25%");
    expect(byTestId("gcfg-cache-rate-1")?.textContent).toContain("缓存率 —");

    // Moderator zone: model label from the session's model_id +
    // its own cache rate (40%).
    const mod = byTestId("gcfg-moderator");
    expect(mod).toBeTruthy();
    expect(mod?.textContent).toContain("主持人");
    expect(mod?.textContent).toContain("GPT-4 (OpenAI)");
    expect(byTestId("gcfg-moderator-cache-rate")?.textContent).toContain("缓存率 40%");
  });

  it("shows '—' placeholders when the IPC returns no rows", async () => {
    invokeMock.mockResolvedValue([]);
    mountModal({
      mode: "edit",
      sessionId: "sess-1",
      initialParticipants: roster,
    });
    await new Promise((r) => setTimeout(r, 0));

    expect(byTestId("gcfg-cache-rate-0")?.textContent).toContain("缓存率 —");
    expect(byTestId("gcfg-cache-rate-1")?.textContent).toContain("缓存率 —");
    expect(byTestId("gcfg-moderator-cache-rate")?.textContent).toContain("缓存率 —");
  });

  it("shows '—' and stays usable when the IPC fetch fails", async () => {
    invokeMock.mockRejectedValue(new Error("boom"));
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    mountModal({
      mode: "edit",
      sessionId: "sess-1",
      initialParticipants: roster,
    });
    await new Promise((r) => setTimeout(r, 0));
    errorSpy.mockRestore();

    // Silent degradation — the edit form itself is untouched.
    expect(byTestId("gcfg-cache-rate-0")?.textContent).toContain("缓存率 —");
    expect(byTestId("gcfg-moderator-cache-rate")?.textContent).toContain("缓存率 —");
    const submit = byTestId("gcfg-submit") as HTMLButtonElement | null;
    expect(submit).toBeTruthy();
  });

  it("keeps rate rows aligned to their speaker when a participant is removed", async () => {
    // 3 participants so removal is allowed; each has a rate row.
    invokeMock.mockResolvedValue([
      { speaker: "Alice", cache_read: 50, context_input: 200 }, // 25%
      { speaker: "Bob", cache_read: 40, context_input: 100 }, // 40%
      { speaker: "Carol", cache_read: 60, context_input: 100 }, // 60%
    ]);
    mountModal({
      mode: "edit",
      sessionId: "sess-1",
      initialParticipants: [...roster, { name: "Carol", model: "m2" }],
    });
    await new Promise((r) => setTimeout(r, 0));

    expect(byTestId("gcfg-cache-rate-0")?.textContent).toContain("缓存率 25%");
    expect(byTestId("gcfg-cache-rate-1")?.textContent).toContain("缓存率 40%");

    // Remove Alice (row 0). Bob's row shifts to index 0 and must
    // STILL show Bob's rate — the roster snapshot is spliced in
    // lockstep with the draft (08-10-group-chat-cache-rate).
    const remove0 = byTestId("gcfg-remove-0") as HTMLButtonElement | null;
    expect(remove0).toBeTruthy();
    remove0!.click();
    await new Promise((r) => setTimeout(r, 0));

    expect(byTestId("gcfg-cache-rate-0")?.textContent).toContain("缓存率 40%");
    expect(byTestId("gcfg-cache-rate-1")?.textContent).toContain("缓存率 60%");
    expect(byTestId("gcfg-cache-rate-2")).toBeNull();
  });

  it("never loads or renders cache rates in create mode", async () => {
    mountModal(); // mode = "create" (default)
    await new Promise((r) => setTimeout(r, 0));

    expect(invokeMock).not.toHaveBeenCalled();
    expect(document.querySelector('[data-testid^="gcfg-cache-rate-"]')).toBeNull();
    expect(byTestId("gcfg-moderator")).toBeNull();
  });
});
