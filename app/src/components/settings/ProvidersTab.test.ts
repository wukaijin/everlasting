// ProvidersTab 测试。
//
// 2026-09-07 (provider-model-disable):provider 级「默认模型拦截」——
// 拥有当前默认模型的 provider 禁用开关被禁用;不含默认模型的 provider
// 正常可点;已禁用 provider 的「启用」方向不误伤。
//
// 2026-10-19 (task 10-19-merge-provider-model-settings):供应商与模型
// 合并为一处 —— 每个供应商卡片下内嵌模型管理区(默认展开,chevron 可
// 折叠),卡片头部常驻模型数徽标;原独立 Models 分类已删除。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia, type Pinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import ProvidersTab from "./ProvidersTab.vue";
import { useProvidersStore, type ProviderRow } from "../../stores/providers";
import { useModelsStore, type ModelWithProvider } from "../../stores/models";

function makeProvider(overrides: Partial<ProviderRow> = {}): ProviderRow {
  return {
    id: "p-1",
    protocol: "anthropic",
    displayName: "BigModel",
    baseUrl: "https://api.example.com",
    hasKey: true,
    disabled: false,
    createdAt: "",
    updatedAt: "",
    ...overrides,
  };
}

function makeModel(overrides: Partial<ModelWithProvider> = {}): ModelWithProvider {
  return {
    id: "m-def",
    providerId: "p-1",
    modelName: "glm-5.3",
    displayName: "GLM-5.3",
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: false,
    supportsImages: false,
    contextWindow: 200_000,
    createdAt: "",
    updatedAt: "",
    providerDisplayName: "BigModel",
    providerProtocol: "anthropic",
    ...overrides,
  };
}

/** 种子:p-1 持有默认模型 m-def,p-2 不持有任何默认。 */
function seed(pinia: Pinia) {
  const providersStore = useProvidersStore(pinia);
  const modelsStore = useModelsStore(pinia);
  providersStore.providers = [
    makeProvider(),
    makeProvider({ id: "p-2", displayName: "Other" }),
  ];
  modelsStore.models = [
    makeModel(),
    makeModel({ id: "m-other", providerId: "p-2", displayName: "Other Model" }),
  ];
  modelsStore.defaultModelId = "m-def";
  return { providersStore, modelsStore };
}

describe("ProvidersTab — default model disable guard", () => {
  let pinia: Pinia;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);
  });

  it("disables the power toggle on the provider owning the default model", async () => {
    seed(pinia);
    const wrapper = mount(ProvidersTab, { global: { plugins: [pinia] } });
    await flushPromises();
    const btn = wrapper.find('[data-testid="providers-toggle-disabled-p-1"]');
    expect(btn.attributes("disabled")).toBeDefined();
    expect(btn.attributes("title")).toContain("不能禁用");
  });

  it("keeps the toggle enabled for providers without the default model", async () => {
    seed(pinia);
    const wrapper = mount(ProvidersTab, { global: { plugins: [pinia] } });
    await flushPromises();
    const btn = wrapper.find('[data-testid="providers-toggle-disabled-p-2"]');
    expect(btn.attributes("disabled")).toBeUndefined();
  });

  it("still allows enabling an already-disabled provider that owns the default", async () => {
    seed(pinia);
    const providersStore = useProvidersStore(pinia);
    providersStore.providers = [makeProvider({ disabled: true })];
    const wrapper = mount(ProvidersTab, { global: { plugins: [pinia] } });
    await flushPromises();
    const btn = wrapper.find('[data-testid="providers-toggle-disabled-p-1"]');
    // 启用方向不锁(恢复正常态);连 disabled provider 的连带禁用态
    // (providerDisabled)由 ModelRow 徽标呈现,这里只验证开关可点。
    expect(btn.attributes("disabled")).toBeUndefined();
    expect(btn.attributes("title")).toContain("启用");
  });
});

// 2026-10-19 (merged settings): 卡片内嵌模型管理区。
describe("ProvidersTab — 卡片内嵌模型管理(2026-10-19 合并)", () => {
  let pinia: Pinia;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);
  });

  it("每个供应商卡片下渲染模型面板,并显示模型数徽标", async () => {
    seed(pinia);
    const wrapper = mount(ProvidersTab, { global: { plugins: [pinia] } });
    await flushPromises();
    expect(wrapper.findAll(".provider-models")).toHaveLength(2);
    // 模型数徽标:p-1 有 1 个,p-2 有 1 个(seed 两条)。
    expect(wrapper.find('[data-testid="providers-model-count-p-1"]').text()).toBe("1 模型");
    // 卡片内的模型行来自 ProviderModelsPanel(ModelRow 渲染 displayName)。
    expect(wrapper.text()).toContain("GLM-5.3");
  });

  it("chevron 折叠该供应商的模型面板(aria-expanded 同步)", async () => {
    seed(pinia);
    const wrapper = mount(ProvidersTab, { global: { plugins: [pinia] } });
    await flushPromises();
    const toggle = wrapper.find('[data-testid="providers-expand-p-1"]');
    expect(toggle.attributes("aria-expanded")).toBe("true");
    const panel = wrapper.findAll(".provider-models")[0]!;
    expect((panel.element as HTMLElement).style.display).not.toBe("none");
    await toggle.trigger("click");
    expect(toggle.attributes("aria-expanded")).toBe("false");
    // v-show 折叠:面板仍在 DOM(保留探测/编辑状态),只是不可见。
    expect((panel.element as HTMLElement).style.display).toBe("none");
    await toggle.trigger("click");
    expect(toggle.attributes("aria-expanded")).toBe("true");
  });
});

// Task 09-18 (openai_responses): 第三协议的下拉项与徽标色。
describe("ProvidersTab — openai_responses protocol", () => {
  let pinia: Pinia;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);
  });

  it("renders the responses badge class for openai_responses providers", async () => {
    const { providersStore } = seed(pinia);
    providersStore.providers.push(
      makeProvider({ id: "p-3", displayName: "Resp", protocol: "openai_responses" }),
    );
    const wrapper = mount(ProvidersTab, { global: { plugins: [pinia] } });
    await flushPromises();
    const badge = wrapper
      .findAll(".providers-tab__badge")
      .find((b) => b.text() === "openai_responses");
    expect(badge).toBeDefined();
    expect(badge!.classes()).toContain("providers-tab__badge--responses");
    wrapper.unmount();
  });

  it("offers 'OpenAI Responses' as the third protocol option", async () => {
    seed(pinia);
    const wrapper = mount(ProvidersTab, { global: { plugins: [pinia] } });
    await flushPromises();
    await wrapper.find(".providers-tab__btn--primary").trigger("click");
    await flushPromises();
    // jsdom 无 Pointer Capture,键盘 Enter ∈ reka OPEN_KEYS 打开下拉;
    // SelectContent teleport 到 document.body(先例:ScheduledTasksTab)。
    await wrapper.find('[aria-label="Protocol"]').trigger("keydown", { key: "Enter" });
    await flushPromises();
    const options = Array.from(document.querySelectorAll('[role="option"]')).map(
      (el) => el.textContent?.trim() ?? "",
    );
    expect(options).toEqual([
      "Anthropic (Messages API)",
      "OpenAI (Chat Completions)",
      "OpenAI Responses",
    ]);
    // 卸载清掉 teleport 到 body 的弹层,避免污染后续用例的 DOM 查询。
    wrapper.unmount();
  });
});
