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
    supportsImages: false,
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
    supportsImages: true,
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

// =====================================================================
// C1.2 (09-08-gc-c1-stoploss): token 预算输入——create 写键/留空不写、
// edit 合并保键(created_via / scheduled_task_name 不丢)、清空 = 删键、
// 非法输入禁提交。
// =====================================================================
import { useProjectsStore } from "../../stores/projects";

describe("GroupChatConfigModal — token budget (C1.2)", () => {
  afterEach(() => {
    document
      .querySelectorAll(".gcfg-content, .gcfg-overlay")
      .forEach((el) => el.remove());
  });

  function flush() {
    return new Promise((r) => setTimeout(r, 0));
  }

  function fillRoster() {
    allByTestIdPrefix("gcfg-name-").forEach((el, i) => {
      const input = el as HTMLInputElement;
      input.value = `P${i + 1}`;
      input.dispatchEvent(new Event("input"));
    });
    // Model selects default to the first enabled model — no interaction needed.
  }

  function setBudget(value: string) {
    const el = byTestId("gcfg-budget") as HTMLInputElement | null;
    if (!el) throw new Error("budget input not found");
    el.value = value;
    el.dispatchEvent(new Event("input"));
  }

  function lastInvoke(cmd: string): Record<string, unknown> | undefined {
    const calls = invokeMock.mock.calls.filter((c: unknown[]) => c[0] === cmd);
    return calls[calls.length - 1]?.[1] as Record<string, unknown> | undefined;
  }

  it("create:填写预算 → metadata 携带 token_budget;留空 → 不写键", async () => {
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "create_session"
        ? {
            id: "new-sess",
            title: "新对话",
            created_at: "",
            updated_at: "",
            model: "m",
            project_id: "p1",
            current_cwd: "",
          }
        : cmd === "list_sessions"
          ? []
          : null,
    );
    const pinia = createPinia();
    setActivePinia(pinia);
    const modelsStore = useModelsStore();
    modelsStore.models = MODEL_LIST as never;
    useProjectsStore().currentProjectId = "p1";
    const wrapper = mount(GroupChatConfigModal, {
      props: { open: true, mode: "create" },
      global: { plugins: [pinia] },
      attachTo: document.body,
    });
    await flush();
    fillRoster();
    await flush();

    setBudget("500000");
    await flush();
    (byTestId("gcfg-submit") as HTMLButtonElement).click();
    await flush();
    let meta = lastInvoke("create_session")?.metadata as Record<string, unknown>;
    expect(meta?.token_budget).toBe(500000);

    setBudget("");
    await flush();
    (byTestId("gcfg-submit") as HTMLButtonElement).click();
    await flush();
    meta = lastInvoke("create_session")?.metadata as Record<string, unknown>;
    expect(meta).not.toHaveProperty("token_budget");
    wrapper.unmount();
  });

  it("edit:合并保键——participants 更新、created_via/scheduled_task_name 保留、预算可设可清", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const chat = useChatStore();
    chat.sessions = [
      {
        id: "s-edit",
        title: "讨论场",
        updated_at: "",
        preview: "",
        project_id: "p1",
        current_cwd: "",
        worktree_path: null,
        worktree_state: "none",
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
        session_type: "group_chat",
        metadata: {
          participants: [{ name: "旧A", model: "m1" }, { name: "旧B", model: "m2" }],
          created_via: "scheduled",
          scheduled_task_name: "每周评审",
          token_budget: 1000,
        },
        busy: false,
      } as never,
    ];
    const wrapper = mount(GroupChatConfigModal, {
      props: {
        open: true,
        mode: "edit",
        sessionId: "s-edit",
        initialParticipants: [
          { name: "旧A", model: "m1" },
          { name: "旧B", model: "m2" },
        ],
        initialTokenBudget: 1000,
      },
      global: { plugins: [pinia] },
      attachTo: document.body,
    });
    await flush();
    expect((byTestId("gcfg-budget") as HTMLInputElement).value).toBe("1000");

    // 改预算 → 保存 → metadata 合并:participants 原样、归因键保留、预算更新。
    setBudget("9999");
    await flush();
    (byTestId("gcfg-submit") as HTMLButtonElement).click();
    await flush();
    let meta = lastInvoke("update_session_metadata")?.metadata as Record<string, unknown>;
    expect(meta?.token_budget).toBe(9999);
    expect(meta?.created_via).toBe("scheduled");
    expect(meta?.scheduled_task_name).toBe("每周评审");
    expect((meta?.participants as unknown[]).length).toBe(2);

    // 清空 → 保存 → token_budget 键删除(其余键仍在)。
    setBudget("");
    await flush();
    (byTestId("gcfg-submit") as HTMLButtonElement).click();
    await flush();
    meta = lastInvoke("update_session_metadata")?.metadata as Record<string, unknown>;
    expect(meta).not.toHaveProperty("token_budget");
    expect(meta?.created_via).toBe("scheduled");
    wrapper.unmount();
  });

  it("非法预算(0 / 负数 / 非整数 / 非数字)→ 提交禁用;清空恢复", async () => {
    const wrapper = mountModal({ mode: "create" });
    await flush();
    fillRoster();
    await flush(); // let Vue re-render the disabled binding
    const submit = () => byTestId("gcfg-submit") as HTMLButtonElement;
    expect(submit().disabled).toBe(false);
    // Note: "abc" is not settable on a type="number" input (DOM
    // sanitizes value to "") — non-numeric rejection is enforced by the
    // browser, so only numeric-but-invalid cases are testable here.
    for (const bad of ["0", "-5", "1.5"]) {
      setBudget(bad);
      await flush();
      expect(submit().disabled, `budget=${bad} must disable submit`).toBe(true);
    }
    setBudget("");
    await flush();
    expect(submit().disabled).toBe(false);
    wrapper.unmount();
  });
});
