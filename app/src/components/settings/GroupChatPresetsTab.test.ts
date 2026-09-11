// GroupChatPresetsTab — 群聊预设管理页组件测试(GCE-P1, task
// `09-12-gc-preset-settings`,design §6 前端组件行)。
//
// 契约:
//   1. 建预设流:表单 → create_group_chat_preset(顶层 camelCase + 嵌套
//      participants camelCase `{name, modelId, persona}` —— wire 契约,
//      嵌套不经 transport 转换)→ 列表重拉 → 表单关闭。
//   2. 校验提示:空名 / 撞内置 key → 内联错误,不发起 IPC(服务端仍是
//      事实源,这里只锁前端预校验闸口)。
//   3. 内置四档只读:渲染 + 「内置」徽标,无编辑/删除按钮。
//   4. 用户列表:渲染摘要;编辑回填;删除走 ConfirmDialog 确认后才调
//      delete_group_chat_preset。
//
// transport / projects store mock(SubagentsTabModelOptions.test.ts 同款;
// pointer capture stub 见 beforeAll)。模型下拉接线走 SelectRoot 的
// update:modelValue emit(jsdom 点 reka 弹层不可靠,ScheduledTasksTab
// 同款);内置只读等静态断言走键盘打开弹层查 [role=option](需要
// pointer capture stub 的只有「打开」,本文件静态断言为主,保留 stub
// 以便后续加弹层用例)。

import { describe, it, expect, beforeAll, afterEach, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";
import { SelectRoot } from "reka-ui";

const invokeMock = vi.fn();

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

const showToastMock = vi.fn();
vi.mock("../../stores/projects", () => ({
  useProjectsStore: () => ({
    showToast: showToastMock,
  }),
}));

import GroupChatPresetsTab from "./GroupChatPresetsTab.vue";
import type { GcPresetRow } from "../../stores/groupChatPresets";

/** 目录:m1 / m2 启用,m3 禁用(禁用模型回显语义的锁死用)。 */
const CATALOG = [
  modelEntry("m1", "Model One", "OpenAI", false),
  modelEntry("m2", "Model Two", "Zhipu", false),
  modelEntry("m3", "Model Old", "OpenAI", true),
];

function modelEntry(
  id: string,
  displayName: string,
  provider: string,
  disabled: boolean,
) {
  return {
    id,
    providerId: `prov-${provider}`,
    modelName: id,
    displayName,
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: false,
    supportsImages: false,
    contextWindow: 128000,
    disabled,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    providerDisplayName: provider,
    providerProtocol: "openai",
  };
}

function presetRow(overrides: Partial<GcPresetRow> = {}): GcPresetRow {
  return {
    id: "row-1",
    name: "我的评审团",
    description: "自定义阵容",
    moderatorModelId: "m1",
    participants: [
      { name: "架构", modelId: "m2", persona: "arch" },
      { name: "后端", modelId: "m1", persona: "backend" },
    ],
    createdAt: "2026-09-12T00:00:00Z",
    updatedAt: "2026-09-12T00:00:00Z",
    ...overrides,
  };
}

/** 内存 mini backend:list/create/update/delete 维护同一份行数组
 *  (test-environment gotcha 6:onMounted 拉取会覆盖直接 seed,所以
 *  用 production-shaped 的 IPC 应答而不是预填 store)。 */
let userRows: GcPresetRow[] = [];

function stubBackend() {
  invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "list_models") return CATALOG;
    if (cmd === "get_default_model") return null;
    if (cmd === "list_group_chat_presets") return userRows;
    if (cmd === "create_group_chat_preset") {
      const row = presetRow({
        id: `row-${userRows.length + 1}`,
        name: String(args?.name),
        description: String(args?.description),
        moderatorModelId: String(args?.moderatorModelId),
        participants: args?.participants as GcPresetRow["participants"],
      });
      userRows = [...userRows, row];
      return row;
    }
    if (cmd === "delete_group_chat_preset") {
      userRows = userRows.filter((r) => r.id !== args?.id);
      return { ok: true };
    }
    return null;
  });
}

async function mountTab() {
  const w = mount(GroupChatPresetsTab, {
    global: { plugins: [createPinia()] },
  });
  await flushPromises();
  return w;
}

/** 表单内的 SelectRoot 按序:0=主持人,1=参与者0模型,2=参与者0人设,
 *  3=参与者1模型,4=参与者1人设。reka SelectRoot 泛型复杂,VTU 的
 *  findAllComponents 重载解析退化(GroupChatConfigModal.test.ts 同款
 *  断言取 props/vm)。 */
function selectRootsOf(wrapper: ReturnType<typeof mount>) {
  return wrapper.findAllComponents(SelectRoot) as unknown as Array<{
    vm: { $emit: (event: string, ...args: unknown[]) => void };
  }>;
}

async function pickSelect(
  w: ReturnType<typeof mount>,
  index: number,
  value: string,
) {
  selectRootsOf(w)[index].vm.$emit("update:modelValue", value);
  await flushPromises();
}

beforeAll(() => {
  // jsdom 未实现 Pointer Capture API(reka trigger pointerdown 链需要)。
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
});

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
  userRows = [];
  showToastMock.mockClear();
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("GroupChatPresetsTab 内置只读区", () => {
  it("内置四档渲染 + 「内置」徽标;无编辑/删除按钮", async () => {
    stubBackend();
    const w = await mountTab();
    const section = w.get('[data-testid="gcp-builtin-list"]');
    for (const key of ["review", "fe_review", "arch", "retro"]) {
      const row = section.get(`[data-testid="gcp-builtin-${key}"]`);
      expect(row.text()).toContain("内置");
      expect(row.text()).toContain(key);
    }
    // 内置区没有任何操作按钮。
    expect(section.findAll("button")).toHaveLength(0);
    w.unmount();
  });
});

describe("GroupChatPresetsTab 建预设流", () => {
  it("新增:提交 create_group_chat_preset(camelCase 嵌套 participants)→ 重拉 + 表单关闭", async () => {
    stubBackend();
    const w = await mountTab();
    await w.get('[data-testid="gcp-create-btn"]').trigger("click");
    const form = w.get('[data-testid="gcp-form"]');
    await form.get('[data-testid="gcp-name"]').setValue("我的评审团");
    await form.get('[data-testid="gcp-desc"]').setValue("自定义阵容");
    // 主持人 SelectRoot 序 0;默认已是首个启用模型 m1,改选 m2。
    await pickSelect(w, 0, "m2");
    // 参与者名字(model 默认 m1/persona 默认 arch 与 product)。
    await form.get('[data-testid="gcp-p-name-0"]').setValue("架构");
    await form.get('[data-testid="gcp-p-name-1"]').setValue("后端");
    // 参与者 1 人设切 backend(SelectRoot 序 4:p1 persona)。
    await pickSelect(w, 4, "backend");

    await w.get('[data-testid="gcp-submit"]').trigger("click");
    await flushPromises();

    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "create_group_chat_preset",
    );
    expect(call).toBeTruthy();
    // wire 契约:顶层 camelCase(transport 扳 snake),嵌套 participants
    // 保持 camelCase `{name, modelId, persona}`(Rust 按 camelCase 反序列化)。
    expect(call?.[1]).toEqual({
      name: "我的评审团",
      description: "自定义阵容",
      moderatorModelId: "m2",
      participants: [
        { name: "架构", modelId: "m1", persona: "arch" },
        { name: "后端", modelId: "m1", persona: "backend" },
      ],
    });
    // 创建后重拉列表 + 新行出现 + 表单关闭 + 成功 toast。
    expect(invokeMock).toHaveBeenCalledWith("list_group_chat_presets", {});
    expect(w.find('[data-testid="gcp-form"]').exists()).toBe(false);
    expect(w.find('[data-testid="gcp-row-row-1"]').exists()).toBe(true);
    expect(showToastMock).toHaveBeenCalledWith("预设已创建", "info");
    w.unmount();
  });

  it("校验:空名 / 撞内置 key → 内联错误,不发起 create", async () => {
    stubBackend();
    const w = await mountTab();
    await w.get('[data-testid="gcp-create-btn"]').trigger("click");

    // 空名。
    await w.get('[data-testid="gcp-submit"]').trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="gcp-form-error"]').text()).toContain("预设名称不能为空");

    // 撞内置 key(大小写变体也拦)。
    await w.get('[data-testid="gcp-name"]').setValue("Review");
    await w.get('[data-testid="gcp-submit"]').trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="gcp-form-error"]').text()).toContain("与内置预设冲突");

    expect(invokeMock).not.toHaveBeenCalledWith(
      "create_group_chat_preset",
      expect.anything(),
    );
    w.unmount();
  });

  it("校验:参与者重名 → 内联错误,不发起 create", async () => {
    stubBackend();
    const w = await mountTab();
    await w.get('[data-testid="gcp-create-btn"]').trigger("click");
    await w.get('[data-testid="gcp-name"]').setValue("重名预设");
    await w.get('[data-testid="gcp-p-name-0"]').setValue("架构");
    await w.get('[data-testid="gcp-p-name-1"]').setValue("架构");
    await w.get('[data-testid="gcp-submit"]').trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="gcp-form-error"]').text()).toContain("参与者重名");
    expect(invokeMock).not.toHaveBeenCalledWith(
      "create_group_chat_preset",
      expect.anything(),
    );
    w.unmount();
  });

  it("加减参与者:2 下限无删除按钮,3 上限隐藏加号", async () => {
    stubBackend();
    const w = await mountTab();
    await w.get('[data-testid="gcp-create-btn"]').trigger("click");
    expect(w.find('[data-testid="gcp-p-remove-0"]').exists()).toBe(false);
    await w.get('[data-testid="gcp-add-participant"]').trigger("click");
    await flushPromises();
    expect(w.find('[data-testid="gcp-participant-2"]').exists()).toBe(true);
    expect(w.find('[data-testid="gcp-add-participant"]').exists()).toBe(false);
    await w.get('[data-testid="gcp-p-remove-2"]').trigger("click");
    await flushPromises();
    expect(w.find('[data-testid="gcp-participant-2"]').exists()).toBe(false);
    w.unmount();
  });
});

describe("GroupChatPresetsTab 用户列表", () => {
  it("渲染名称/描述/主持人+参与者摘要;行带「自定义」徽标", async () => {
    userRows = [presetRow()];
    stubBackend();
    const w = await mountTab();
    const rowEl = w.get('[data-testid="gcp-row-row-1"]');
    expect(rowEl.text()).toContain("我的评审团");
    expect(rowEl.text()).toContain("自定义阵容");
    expect(rowEl.text()).toContain("自定义");
    // 摘要:主持人 + 参与者(模型显示名)。
    expect(rowEl.text()).toContain("主持人 OpenAI · Model One");
    expect(rowEl.text()).toContain("架构(Zhipu · Model Two)");
    w.unmount();
  });

  it("编辑:表单回填行数据;提交走 update_group_chat_preset(带 id)", async () => {
    userRows = [presetRow()];
    stubBackend();
    const w = await mountTab();
    await w.get('[data-testid="gcp-edit-row-1"]').trigger("click");
    const form = w.get('[data-testid="gcp-form"]');
    expect((form.get('[data-testid="gcp-name"]').element as HTMLInputElement).value).toBe(
      "我的评审团",
    );
    await form.get('[data-testid="gcp-name"]').setValue("改名评审团");
    await w.get('[data-testid="gcp-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "update_group_chat_preset",
    );
    expect(call?.[1]).toMatchObject({ id: "row-1", name: "改名评审团" });
    expect(showToastMock).toHaveBeenCalledWith("预设已更新", "info");
    w.unmount();
  });

  it("删除:ConfirmDialog 确认后才调 delete_group_chat_preset", async () => {
    userRows = [presetRow()];
    stubBackend();
    const w = await mountTab();
    await w.get('[data-testid="gcp-delete-row-1"]').trigger("click");
    await flushPromises();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "delete_group_chat_preset",
      expect.anything(),
    );
    const confirmBtn = w
      .findAll("button")
      .find((b) => b.text() === "删除" && b.classes().includes("btn--danger"));
    expect(confirmBtn).toBeTruthy();
    await confirmBtn!.trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("delete_group_chat_preset", { id: "row-1" });
    expect(w.find('[data-testid="gcp-row-row-1"]').exists()).toBe(false);
    expect(showToastMock).toHaveBeenCalledWith("预设已删除", "info");
    w.unmount();
  });
});
