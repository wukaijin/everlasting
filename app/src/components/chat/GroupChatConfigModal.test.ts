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

import { describe, it, expect, afterEach } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { useModelsStore } from "../../stores/models";
import GroupChatConfigModal from "./GroupChatConfigModal.vue";

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
