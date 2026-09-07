// 2026-09-07 (provider-model-disable) — ProvidersTab 测试。
// 覆盖:provider 级「默认模型拦截」—— 拥有当前默认模型的 provider
// 禁用开关被禁用;不含默认模型的 provider 正常可点;已禁用 provider
// 的「启用」方向不误伤。

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
    // (providerDisabled)由 Models 页徽标呈现,这里只验证开关可点。
    expect(btn.attributes("disabled")).toBeUndefined();
    expect(btn.attributes("title")).toContain("启用");
  });
});
