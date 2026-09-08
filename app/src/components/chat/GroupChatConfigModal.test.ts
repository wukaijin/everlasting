// GroupChatConfigModal — coverage for the modal's core invariants.
// Sections:
//   1. validation (D5: 2-3 participants, unique + non-empty names,
//      model selected, submit mirror);
//   2. edit 成本区(gce-m4c 09-08):`group_chat_token_usage` +
//      `group_chat_cache_rates` 两命令 → per-speaker「tokens · 缓存率」
//      合并行 + 预算进度条;失败降级「—」;speaker 快照稳定;
//   3. token 预算两态(C1.2 保留);
//   4. preset 单选卡 + 主持人 Select(gce-m4c create;persona 组装与
//      共享模块 composePersonaMd 同形断言、主持人落 create_session 的
//      model 参数)。
//
// Note: reka-ui's Dialog uses `<DialogPortal>` which teleports
// to `<body>`. The testid selectors in this file therefore
// query `document` directly (not `wrapper.find(...)`) — the
// mounted wrapper's DOM doesn't contain the teleported subtree.
// reka 交互(RadioGroup / Select)走 `vm.$emit("update:modelValue")`
// 接线测法(jsdom 点 label 的转发不可靠,ScheduledTasksTab 同款)。

import { describe, it, expect, afterEach, beforeAll, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { RadioGroupRoot, SelectRoot } from "reka-ui";
import { useModelsStore } from "../../stores/models";
import { useChatStore } from "../../stores/chat";
import type { SessionSummary } from "../../stores/chat.types";
import { GC_PRESETS } from "../../utils/groupChatPresets";
import GroupChatConfigModal from "./GroupChatConfigModal.vue";

// The edit-mode cost zone invokes `group_chat_cache_rates` +
// `group_chat_token_usage` over the transport. Mock the transport
// module (canonical `invokeMock` pattern) so the modal tests drive
// the IPC responses without a real backend.
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

/** gce-m4c preset 测试目录:preset JSON 里的名字(MiniMax-M3 / glm-5.3 /
 *  GLM-5.3-Flash / deepseek-v4-flash)可解析成 UUID。 */
function modelEntry(
  id: string,
  modelName: string,
  displayName: string,
  providerDisplayName: string,
) {
  return {
    id,
    providerId: `prov-${id}`,
    modelName,
    displayName,
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: false,
    supportsImages: false,
    contextWindow: 128000,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    providerDisplayName,
    providerProtocol: "openai",
  };
}

const GC_MODEL_LIST = [
  modelEntry("uuid-mini", "MiniMax-M3", "MiniMax-M3", "MiniMax"),
  modelEntry("uuid-glm", "glm-5.3", "GLM-5.3", "Zhipu"),
  modelEntry("uuid-flash", "GLM-5.3-Flash", "GLM-5.3-Flash", "Zhipu"),
  modelEntry("uuid-ds", "deepseek-v4-flash", "DeepSeek V4", "DeepSeek"),
];

/** 只有参与者模型、没有 preset 主持人(MiniMax-M3)的目录:锁定
 *  「主持人解析失败 → 提示条 + 提交禁用;手动改选可恢复」链路。 */
const GC_MODEL_LIST_NO_MINI = [
  modelEntry("m1", "gpt-4", "GPT-4", "OpenAI"),
  modelEntry("uuid-glm", "glm-5.3", "GLM-5.3", "Zhipu"),
  modelEntry("uuid-flash", "GLM-5.3-Flash", "GLM-5.3-Flash", "Zhipu"),
  modelEntry("uuid-ds", "deepseek-v4-flash", "DeepSeek V4", "DeepSeek"),
];

function mountModal(
  props: Partial<InstanceType<typeof GroupChatConfigModal>["$props"]> = {},
  models: unknown[] = MODEL_LIST,
) {
  const pinia = createPinia();
  setActivePinia(pinia);
  const modelsStore = useModelsStore();
  modelsStore.models = models as never;
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

function flush() {
  return new Promise((r) => setTimeout(r, 0));
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
    await flush();
    const rows = document.querySelectorAll<HTMLElement>(".gcfg-row");
    expect(rows.length).toBe(2);
    const submit = byTestId("gcfg-submit") as HTMLButtonElement | null;
    expect(submit).toBeTruthy();
    expect(submit!.disabled).toBe(true);
  });

  it("hides '+' button when 3 participants reached (D5 max)", async () => {
    mountModal();
    await flush();
    const addBtn = byTestId("gcfg-add") as HTMLButtonElement | null;
    expect(addBtn).toBeTruthy();
    addBtn!.click();
    addBtn!.click();
    await flush();
    const rows = document.querySelectorAll<HTMLElement>(".gcfg-row");
    expect(rows.length).toBe(3);
    expect(byTestId("gcfg-add")).toBeNull();
  });

  it("delete (-remove) buttons are disabled when only 2 participants remain", async () => {
    mountModal();
    await flush();
    const removes = allByTestIdPrefix("gcfg-remove-");
    expect(removes.length).toBe(2);
    for (const r of removes) {
      expect((r as HTMLButtonElement).disabled).toBe(true);
    }
  });

  it("emits update:open with false when cancel is clicked", async () => {
    const wrapper = mountModal();
    await flush();
    const cancel = byTestId("gcfg-cancel") as HTMLButtonElement | null;
    expect(cancel).toBeTruthy();
    cancel!.click();
    await flush();
    const events = wrapper.emitted("update:open");
    expect(events).toBeTruthy();
    expect(events![0]).toEqual([false]);
  });
});

// =====================================================================
// gce-m4c (09-08) — edit 成本区:`group_chat_token_usage`(新)与
// `group_chat_cache_rates`(既有)两次查询,per-speaker「tokens · 缓存率」
// 合并行 + 预算进度条;失败降级「—」不阻塞编辑。per-row 缓存率行已并入
// 成本区(design §4.2),旧的 `gcfg-cache-rate-*` 行内 testid 随之退役。
// =====================================================================

describe("GroupChatConfigModal — edit cost zone (gce-m4c)", () => {
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

  function stubCostPayload(
    cacheRates: unknown,
    tokenUsage: unknown,
  ) {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "group_chat_cache_rates") return cacheRates;
      if (cmd === "group_chat_token_usage") return tokenUsage;
      return null;
    });
  }

  afterEach(() => {
    invokeMock.mockReset();
    document
      .querySelectorAll(".gcfg-content, .gcfg-overlay")
      .forEach((el) => el.remove());
  });

  it("renders merged per-speaker rows (tokens 万单位 + 缓存率) + budget progress from both IPC payloads", async () => {
    stubCostPayload(
      [
        { speaker: "Alice", cache_read: 50, context_input: 200 }, // 25%
        { speaker: "moderator", cache_read: 40, context_input: 100 }, // 40%
      ],
      {
        total: 265000,
        by_speaker: [
          { speaker: "Alice", tokens: 150000 },
          { speaker: "moderator", tokens: 115000 },
        ],
      },
    );
    const wrapper = mountModal({
      mode: "edit",
      sessionId: "sess-1",
      initialParticipants: roster,
      initialTokenBudget: 400000,
    });
    seedSession();
    await flush();

    // Both queries fire per open, with the session id.
    expect(invokeMock).toHaveBeenCalledWith("group_chat_cache_rates", {
      sessionId: "sess-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("group_chat_token_usage", {
      sessionId: "sess-1",
    });

    // Budget progress: 26.5万 / 40万 (66%).
    const progress = byTestId("gcfg-budget-progress");
    expect(progress).toBeTruthy();
    const text = byTestId("gcfg-budget-text")?.textContent ?? "";
    expect(text).toContain("26.5万");
    expect(text).toContain("40万");
    expect(text).toContain("66%");

    // Merged rows: Alice → 15万 · 缓存 25%;Bob → 无数据「—」;
    // moderator row last → 主持人 11.5万 · 缓存 40%。
    const row0 = byTestId("gcfg-cost-row-0")?.textContent ?? "";
    expect(row0).toContain("Alice");
    expect(row0).toContain("15万");
    expect(row0).toContain("缓存 25%");
    const row1 = byTestId("gcfg-cost-row-1")?.textContent ?? "";
    expect(row1).toContain("Bob");
    expect(row1).toContain("tokens —");
    expect(row1).toContain("缓存 —");
    const row2 = byTestId("gcfg-cost-row-2")?.textContent ?? "";
    expect(row2).toContain("主持人");
    expect(row2).toContain("11.5万");
    expect(row2).toContain("缓存 40%");
    wrapper.unmount();
  });

  it("over-budget → full bar + error-text styling", async () => {
    stubCostPayload(
      [],
      { total: 500000, by_speaker: [{ speaker: "Alice", tokens: 500000 }] },
    );
    const wrapper = mountModal({
      mode: "edit",
      sessionId: "sess-1",
      initialParticipants: roster,
      initialTokenBudget: 400000,
    });
    seedSession();
    await flush();
    const text = byTestId("gcfg-budget-text")?.textContent ?? "";
    expect(text).toContain("125%");
    const fill = document.querySelector<HTMLElement>(".gcfg-cost__budget-fill");
    expect(fill?.classList.contains("gcfg-cost__budget-fill--over")).toBe(true);
    expect(
      byTestId("gcfg-budget-text")!.classList.contains("gcfg-cost__budget-text--over"),
    ).toBe(true);
    wrapper.unmount();
  });

  it("shows '—' placeholders in the cost rows when both payloads carry no rows", async () => {
    stubCostPayload([], { total: 0, by_speaker: [] });
    const wrapper = mountModal({
      mode: "edit",
      sessionId: "sess-1",
      initialParticipants: roster,
    });
    seedSession();
    await flush();
    expect(byTestId("gcfg-budget-progress")).toBeNull(); // 无预算不渲染进度条
    const body = document.body.textContent ?? "";
    expect(body).toContain("tokens —");
    expect(body).toContain("缓存 —");
    wrapper.unmount();
  });

  it("degrades to '—' and stays usable when both queries reject", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    invokeMock.mockRejectedValue(new Error("boom"));
    const wrapper = mountModal({
      mode: "edit",
      sessionId: "sess-1",
      initialParticipants: roster,
      initialTokenBudget: 400000,
    });
    seedSession();
    await flush();
    errorSpy.mockRestore();

    // Silent degradation — rows render placeholders, no progress bar
    // (usage unknown), and the edit flow itself stays usable: the
    // seeded roster is valid, so submit remains enabled.
    const body = document.body.textContent ?? "";
    expect(body).toContain("tokens —");
    expect(byTestId("gcfg-budget-progress")).toBeNull();
    const submit = byTestId("gcfg-submit") as HTMLButtonElement | null;
    expect(submit).toBeTruthy();
    expect(submit!.disabled).toBe(false);
    const names = allByTestIdPrefix("gcfg-name-");
    expect(names.length).toBe(2);
    wrapper.unmount();
  });

  it("keeps cost rows keyed to their speaker when a participant is removed", async () => {
    stubCostPayload(
      [
        { speaker: "Alice", cache_read: 50, context_input: 200 }, // 25%
        { speaker: "Bob", cache_read: 40, context_input: 100 }, // 40%
        { speaker: "Carol", cache_read: 60, context_input: 100 }, // 60%
      ],
      {
        total: 1000,
        by_speaker: [
          { speaker: "Alice", tokens: 100 },
          { speaker: "Bob", tokens: 200 },
          { speaker: "Carol", tokens: 300 },
          { speaker: "moderator", tokens: 400 },
        ],
      },
    );
    const wrapper = mountModal({
      mode: "edit",
      sessionId: "sess-1",
      initialParticipants: [...roster, { name: "Carol", model: "m2" }],
    });
    seedSession();
    await flush();

    expect(byTestId("gcfg-cost-row-0")?.textContent).toContain("Alice");

    // Remove Alice (draft row 0). The cost rows reflect the persisted
    // `messages.speaker` history — they must NOT shift with the draft.
    const remove0 = byTestId("gcfg-remove-0") as HTMLButtonElement | null;
    expect(remove0).toBeTruthy();
    remove0!.click();
    await flush();

    expect(byTestId("gcfg-cost-row-0")?.textContent).toContain("Alice");
    expect(byTestId("gcfg-cost-row-1")?.textContent).toContain("Bob");
    expect(byTestId("gcfg-cost-row-2")?.textContent).toContain("Carol");
    expect(byTestId("gcfg-cost-row-3")?.textContent).toContain("主持人");
    wrapper.unmount();
  });

  it("never loads cache rates or token usage in create mode", async () => {
    mountModal(); // mode = "create" (default)
    await flush();

    expect(invokeMock).not.toHaveBeenCalled();
    expect(byTestId("gcfg-cost-zone")).toBeNull();
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
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "group_chat_cache_rates") return [];
      if (cmd === "group_chat_token_usage") return { total: 0, by_speaker: [] };
      return null;
    });
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

  it("预算量级提示(D4 静态文案)渲染在预算输入下", async () => {
    const wrapper = mountModal({ mode: "create" });
    await flush();
    const hints = Array.from(document.querySelectorAll(".gcfg-field__hint")).map(
      (h) => h.textContent ?? "",
    );
    expect(hints.some((t) => t.includes("留空 = 不限"))).toBe(true);
    expect(hints.some((t) => t.includes("20-60 万 token"))).toBe(true);
    wrapper.unmount();
  });
});

// =====================================================================
// gce-m4c (09-08) — create:preset 单选卡 + 主持人 Select。
// =====================================================================

describe("GroupChatConfigModal — preset cards + moderator (create, gce-m4c)", () => {
  afterEach(() => {
    invokeMock.mockReset();
    document
      .querySelectorAll(".gcfg-content, .gcfg-overlay")
      .forEach((el) => el.remove());
  });

  function stubCreateSession() {
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "create_session"
        ? {
            id: "new-gc",
            title: "新讨论",
            created_at: "",
            updated_at: "",
            model: "uuid-mini",
            project_id: "p1",
            current_cwd: "",
          }
        : cmd === "list_sessions"
          ? []
          : null,
    );
  }

  /** 选中 preset 卡(经 RadioGroupRoot 的 update:modelValue 接线)。 */
  async function pickPreset(
    wrapper: ReturnType<typeof mount>,
    key: string,
  ) {
    wrapper
      .getComponent(RadioGroupRoot)
      .vm.$emit("update:modelValue", key);
    await flush();
  }

  /** reka SelectRoot 泛型复杂,VTU 的 VueWrapper.findAllComponents 重载
   *  解析退化为 DOMWrapper[](vue-tsc TS2339);按组件实例形状断言取
   *  props/vm(ScheduledTasksTab.test.ts 的 DOMWrapper 版同用法)。 */
  function selectRootsOf(wrapper: ReturnType<typeof mount>) {
    return wrapper.findAllComponents(SelectRoot) as unknown as Array<{
      props: (k: string) => unknown;
      vm: { $emit: (event: string, ...args: unknown[]) => void };
    }>;
  }

  it("preset 卡渲染:自定义卡 + JSON 声明序四预设;默认自定义选中", async () => {
    const wrapper = mountModal({ mode: "create" }, GC_MODEL_LIST);
    await flush();
    const keys = Object.keys(GC_PRESETS.presets);
    expect(keys).toEqual(["review", "fe_review", "arch", "retro"]);
    // 09-09:首卡「自定义」= 无预设初始态的显式入口(RadioGroup 选了
    // 退不回的补路),默认选中;preset 卡不预选(用户点卡才展开预填)。
    expect(byTestId("gcfg-preset-custom")).toBeTruthy();
    expect(byTestId("gcfg-preset-custom")?.textContent).toContain("自定义");
    const active = document.querySelector<HTMLElement>(".gcfg-preset-card--active");
    expect(active?.dataset.testid).toBe("gcfg-preset-custom");
    for (const key of keys) {
      const card = byTestId(`gcfg-preset-${key}`);
      expect(card).toBeTruthy();
      expect(card?.textContent).toContain(GC_PRESETS.presets[key]!.description);
    }
    wrapper.unmount();
  });

  it("选中 review → 预填三行阵容;persona_md = 边界文本 + \\n\\n + persona_common(共享模块同形)", async () => {
    const wrapper = mountModal({ mode: "create" }, GC_MODEL_LIST);
    await flush();
    await pickPreset(wrapper, "review");

    const def = GC_PRESETS.presets.review!;
    const names = allByTestIdPrefix("gcfg-name-");
    expect(names.length).toBe(def.participants.length);
    def.participants.forEach((p, i) => {
      expect((names[i] as HTMLInputElement).value).toBe(p.name);
    });
    // persona 逐字同形断言:边界 + "\n\n" + 公共纪律(M1 composePresets
    // / 定时表单 gcPersonaMd 同一拼接)。
    const personas = allByTestIdPrefix("gcfg-persona-");
    def.participants.forEach((p, i) => {
      const expected = `${GC_PRESETS.personas[p.persona]}\n\n${GC_PRESETS.persona_common}`;
      expect((personas[i] as HTMLTextAreaElement).value).toBe(expected);
      expect((personas[i] as HTMLTextAreaElement).value).toContain("\n\n");
    });
    // 主持人默认 = preset 的 moderator_model 解析结果(uuid-mini)。
    // SelectRoot 按序:三行参与者各一 + 主持人一个(索引 3)。
    const selectRoots = selectRootsOf(wrapper);
    expect(selectRoots.length).toBe(4);
    expect(selectRoots[3].props("modelValue")).toBe("uuid-mini");
    // 目录齐全 → 无模型缺失提示条,提交可用。
    expect(byTestId("gcfg-preset-error")).toBeNull();
    expect((byTestId("gcfg-submit") as HTMLButtonElement).disabled).toBe(false);
    wrapper.unmount();
  });

  it("选中 preset 后点「自定义」→ 恢复无预设初始态(09-09 退路)", async () => {
    const wrapper = mountModal({ mode: "create" }, GC_MODEL_LIST);
    await flush();
    await pickPreset(wrapper, "review");
    expect(document.querySelectorAll(".gcfg-row").length).toBe(3);

    await pickPreset(wrapper, "__custom__");

    // 阵容回到 create 种子:2 行空名 + 首个启用模型;主持人未选。
    const names = allByTestIdPrefix("gcfg-name-");
    expect(names.length).toBe(2);
    expect((names[0] as HTMLInputElement).value).toBe("");
    const roots = selectRootsOf(wrapper);
    expect(roots.length).toBe(3); // 2 行 + 主持人
    expect(roots[0].props("modelValue")).toBe("uuid-mini"); // 首个启用模型
    expect(roots[2].props("modelValue")).toBeUndefined(); // 主持人空
    // 自定义卡高亮、preset 卡去高亮;警示条消隐;空名 → 提交禁用。
    expect(
      byTestId("gcfg-preset-custom")?.classList.contains("gcfg-preset-card--active"),
    ).toBe(true);
    expect(
      byTestId("gcfg-preset-review")?.classList.contains("gcfg-preset-card--active"),
    ).toBe(false);
    expect(byTestId("gcfg-preset-error")).toBeNull();
    expect((byTestId("gcfg-submit") as HTMLButtonElement).disabled).toBe(true);
    wrapper.unmount();
  });

  it("主持人改选 → create_session 的 model 参数跟随;未改选(preset 默认)也随提交", async () => {
    stubCreateSession();
    const pinia = createPinia();
    setActivePinia(pinia);
    const modelsStore = useModelsStore();
    modelsStore.models = GC_MODEL_LIST as never;
    useProjectsStore().currentProjectId = "p1";
    const wrapper = mount(GroupChatConfigModal, {
      props: { open: true, mode: "create" },
      global: { plugins: [pinia] },
      attachTo: document.body,
    });
    await flush();
    await pickPreset(wrapper, "review");

    // preset 默认主持人直接提交 → model = uuid-mini。
    (byTestId("gcfg-submit") as HTMLButtonElement).click();
    await flush();
    let call = invokeMock.mock.calls.find((c) => c[0] === "create_session");
    expect(call?.[1].model).toBe("uuid-mini");
    expect(call?.[1].metadata.participants.map((p: { name: string }) => p.name)).toEqual([
      "架构",
      "产品",
      "后端",
    ]);

    // 手动改选主持人(SelectRoot 序:0-2 = 参与行,3 = 主持人)→ model 跟随改选。
    invokeMock.mockClear();
    stubCreateSession();
    selectRootsOf(wrapper)[3].vm.$emit("update:modelValue", "uuid-ds");
    await flush();
    (byTestId("gcfg-submit") as HTMLButtonElement).click();
    await flush();
    call = invokeMock.mock.calls.find((c) => c[0] === "create_session");
    expect(call?.[1].model).toBe("uuid-ds");
    wrapper.unmount();
  });

  it("preset 主持人解析失败 → 错误条(preset 原名)+ 提交禁用;手动改选可恢复", async () => {
    const wrapper = mountModal({ mode: "create" }, GC_MODEL_LIST_NO_MINI);
    await flush();
    await pickPreset(wrapper, "review");

    const bar = byTestId("gcfg-preset-error");
    expect(bar).toBeTruthy();
    expect(bar?.textContent).toContain("MiniMax-M3");
    expect(bar?.textContent).toContain("不在模型目录中,请先在「模型」页添加");
    // 三行参与者模型都能解析(uuid-glm 等),唯一缺失 = 主持人。
    expect((byTestId("gcfg-submit") as HTMLButtonElement).disabled).toBe(true);

    // 手动改选主持人(m1 = GPT-4;SelectRoot 索引 3)→ 提示条消隐,提交恢复。
    selectRootsOf(wrapper)[3].vm.$emit("update:modelValue", "m1");
    await flush();
    expect(byTestId("gcfg-preset-error")).toBeNull();
    expect((byTestId("gcfg-submit") as HTMLButtonElement).disabled).toBe(false);
    wrapper.unmount();
  });

  it("未选 preset 的 create 不带 model 参数(全局默认,零回归)", async () => {
    stubCreateSession();
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
    // 手填两行(不经 preset)。
    allByTestIdPrefix("gcfg-name-").forEach((el, i) => {
      const input = el as HTMLInputElement;
      input.value = `P${i + 1}`;
      input.dispatchEvent(new Event("input"));
    });
    await flush();
    (byTestId("gcfg-submit") as HTMLButtonElement).click();
    await flush();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_session");
    expect(call?.[1]).not.toHaveProperty("model");
    wrapper.unmount();
  });
});

// ---------------------------------------------------------------------------
// provider/model 禁用过滤(09-09-gc-disabled-model-leak)。
//
// 契约(09-07 provider-model-disable PRD R2 + 09-09 修正):
//   - create 流禁用模型不可被选/不可做默认/preset 不可静默预填;
//   - preset 引用禁用模型 → 留空 + 「已被禁用」两态警示 + 提交禁用;
//   - 主持人 Select 守卫拒选禁用 id;
//   - edit 回显:本行指向禁用模型仍可见可切走(提交放行),但不出现在
//     其他行的下拉里(原全局 pinned Set 的跨行泄漏)。
// ---------------------------------------------------------------------------
describe("GroupChatConfigModal — provider/model 禁用过滤(09-09)", () => {
  beforeAll(() => {
    // jsdom 未实现 Pointer Capture API(reka SelectTrigger 的 pointerdown
    // handler 会抛);stub 后打开路径走 keydown Enter。ScheduledTasksTab
    // .test.ts 同款。
    Element.prototype.hasPointerCapture = () => false;
    Element.prototype.setPointerCapture = () => {};
    Element.prototype.releasePointerCapture = () => {};
  });

  afterEach(() => {
    document
      .querySelectorAll(".gcfg-content, .gcfg-overlay")
      .forEach((el) => el.remove());
  });

  /** 键盘打开行/主持人下拉,快照弹层 option 文本(select 关闭态不渲染
   *  SelectContent,选项断言必须真实打开;portal teleport 到 body)。 */
  async function openOptions(triggerTestId: string): Promise<string[]> {
    byTestId(triggerTestId)?.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
    await flush();
    await flush();
    return Array.from(document.querySelectorAll('[role="option"]')).map(
      (el) => el.textContent?.trim() ?? "",
    );
  }

  /** 选中 preset 卡(经 RadioGroupRoot 的 update:modelValue 接线;上一
   *  describe 的同名 helper 是块内私有,此处本地复刻)。 */
  async function pickPreset(
    wrapper: ReturnType<typeof mount>,
    key: string,
  ) {
    wrapper
      .getComponent(RadioGroupRoot)
      .vm.$emit("update:modelValue", key);
    await flush();
  }

  /** SelectRoot 泛型复杂,VTU 的 findAllComponents 重载解析退化(见上一
   *  describe 同名 helper 注);本地复刻取 props/vm。 */
  function selectRootsOf(wrapper: ReturnType<typeof mount>) {
    return wrapper.findAllComponents(SelectRoot) as unknown as Array<{
      props: (k: string) => unknown;
      vm: { $emit: (event: string, ...args: unknown[]) => void };
    }>;
  }

  it("create 种子默认 = 首个启用模型(禁用模型不做默认)", async () => {
    const wrapper = mountModal(
      { mode: "create" },
      [
        { ...MODEL_LIST[0], id: "m1", disabled: true },
        MODEL_LIST[1],
      ],
    );
    await flush();
    const roots = selectRootsOf(wrapper);
    expect(roots.length).toBe(3); // 2 行 + 主持人
    expect(roots[0].props("modelValue")).toBe("m2");
    expect(roots[1].props("modelValue")).toBe("m2");
    wrapper.unmount();
  });

  it("行/主持人下拉不提供禁用模型(键盘打开弹层断言)", async () => {
    const wrapper = mountModal(
      { mode: "create" },
      [
        { ...MODEL_LIST[0], id: "m1", disabled: true },
        MODEL_LIST[1],
      ],
    );
    await flush();
    const rowOptions = await openOptions("gcfg-model-0");
    expect(rowOptions).toEqual(["Claude 3.5 (Anthropic)"]); // m1 被滤
    wrapper.unmount();

    const wrapper2 = mountModal(
      { mode: "create" },
      [
        { ...MODEL_LIST[0], id: "m1", disabled: true },
        MODEL_LIST[1],
      ],
    );
    await flush();
    const moderatorOptions = await openOptions("gcfg-moderator-select");
    expect(moderatorOptions).toEqual(["Claude 3.5 (Anthropic)"]);
    wrapper2.unmount();
  });

  it("preset 主持人被禁用 → 不预填 + 「已被禁用」警示 + 提交禁用;守卫拒选禁用 id", async () => {
    const catalog = GC_MODEL_LIST.map((m) =>
      m.id === "uuid-mini" ? { ...m, disabled: true } : m,
    );
    const wrapper = mountModal({ mode: "create" }, catalog);
    await flush();
    await pickPreset(wrapper, "review");

    // 主持人未预填(启用目录解析失败);参与者三行照常解析。
    const roots = selectRootsOf(wrapper);
    expect(roots[3].props("modelValue")).toBeUndefined();
    const bar = byTestId("gcfg-preset-error");
    expect(bar).toBeTruthy();
    expect(bar?.textContent).toContain("已被禁用,请先在「模型」页启用");
    expect(bar?.textContent).toContain("MiniMax-M3");
    expect((byTestId("gcfg-submit") as HTMLButtonElement).disabled).toBe(true);

    // 守卫:emit 禁用 id(uuid-mini)被拒;emit 启用 id(uuid-glm)放行。
    roots[3].vm.$emit("update:modelValue", "uuid-mini");
    await flush();
    expect(selectRootsOf(wrapper)[3].props("modelValue")).toBeUndefined();
    roots[3].vm.$emit("update:modelValue", "uuid-glm");
    await flush();
    expect(selectRootsOf(wrapper)[3].props("modelValue")).toBe("uuid-glm");
    // 手动改选后警示消隐、提交恢复。
    expect(byTestId("gcfg-preset-error")).toBeNull();
    expect((byTestId("gcfg-submit") as HTMLButtonElement).disabled).toBe(false);
    wrapper.unmount();
  });

  it("preset 参与者被禁用 → 该行留空 + 行级「已被禁用」警示 + 提交禁用", async () => {
    const catalog = GC_MODEL_LIST.map((m) =>
      m.id === "uuid-glm" ? { ...m, providerDisabled: true } : m,
    );
    const wrapper = mountModal({ mode: "create" }, catalog);
    await flush();
    await pickPreset(wrapper, "review");

    const roots = selectRootsOf(wrapper);
    expect(roots[0].props("modelValue")).toBe(""); // 架构行 glm 被禁 → 留空
    expect(roots[1].props("modelValue")).toBe("uuid-flash");
    const bar = byTestId("gcfg-preset-error");
    expect(bar?.textContent).toContain("参与者「架构」的模型「glm-5.3」已被禁用");
    expect((byTestId("gcfg-submit") as HTMLButtonElement).disabled).toBe(true);
    wrapper.unmount();
  });

  it("edit 回显:本行禁用模型可见可切走,别行下拉不提供(跨行泄漏修复)", async () => {
    const catalog = [
      { ...MODEL_LIST[0], id: "m1", disabled: true },
      MODEL_LIST[1],
    ];
    const initial = [
      { name: "A", model: "m1" },
      { name: "B", model: "m2" },
    ];
    // 行 0(指向禁用 m1):本行下拉回显 m1。
    const wrapper = mountModal(
      {
        mode: "edit",
        sessionId: "s-edit",
        initialParticipants: initial as never,
      },
      catalog,
    );
    await flush();
    const row0Options = await openOptions("gcfg-model-0");
    expect(row0Options).toContain("GPT-4 (OpenAI)"); // 回显:可见可切走
    expect(row0Options).toContain("Claude 3.5 (Anthropic)");
    // 编辑态 roster 行指向禁用模型不阻塞提交(已在用的会话不受影响)。
    expect((byTestId("gcfg-submit") as HTMLButtonElement).disabled).toBe(false);
    wrapper.unmount();

    // 行 1(启用 m2):下拉不提供 m1 —— 原全局 pinned Set 会把 m1 带进
    // 每一行的选项里(本用例锁死的回归点)。
    const wrapper2 = mountModal(
      {
        mode: "edit",
        sessionId: "s-edit",
        initialParticipants: initial as never,
      },
      catalog,
    );
    await flush();
    const row1Options = await openOptions("gcfg-model-1");
    expect(row1Options).toEqual(["Claude 3.5 (Anthropic)"]); // m1 不在
    wrapper2.unmount();
  });
});
