// Tests for the ChatPanel empty-state four-way split (N1 onboarding,
// R1.1 / AC5, 2026-09-15).
//
// Coverage:
//   1. Gate: `config.loaded === false` → the original 开始对话 empty
//      state renders (no onboarding card flash while providers/models
//      are still loading).
//   2. No providers → card A (3-step guide + CTA) and the CTA opens
//      Settings landing on the "providers" category.
//   3. Providers but no models → card B; CTA lands on "models".
//   4. Models but no default → card C; CTA lands on "models".
//   5. Fully configured → the original empty state, byte-for-byte
//      behavior (no card).
//
// Heavy children (ChatInput / modals / MessageList) are stubbed — the
// states under test only involve the `chat-panel__empty` branch.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import ChatPanel from "./ChatPanel.vue";
import { useConfigStore } from "../../stores/config";
import { useProvidersStore, type ProviderRow } from "../../stores/providers";
import {
  useModelsStore,
  type ModelWithProvider,
} from "../../stores/models";
import { useSettingsModalStore } from "../../stores/settingsModal";

function makeProvider(id: string): ProviderRow {
  return {
    id,
    protocol: "anthropic",
    displayName: "Test Provider",
    baseUrl: "https://example.com",
    hasKey: true,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
  };
}

function makeModel(id: string, providerId: string): ModelWithProvider {
  return {
    id,
    providerId,
    modelName: "test-model",
    displayName: "Test Model",
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: false,
    supportsImages: false,
    contextWindow: 128000,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    providerDisplayName: "Test Provider",
    providerProtocol: "anthropic",
  };
}

describe("ChatPanel — empty state four-way split (N1)", () => {
  let wrapper: VueWrapper | null = null;

  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(null);
    wrapper = null;
  });

  function mountPanel(): VueWrapper {
    return mount(ChatPanel, {
      global: {
        stubs: {
          ChatInput: true,
          MessageList: true,
          DiffModal: true,
          DeleteWorktreeConfirm: true,
          MemoryModal: true,
          RuntimeMemoryModal: true,
          ActivityPanel: true,
          WorkerAskBanner: true,
          AskUserQuestionCard: true,
        },
      },
    });
  }

  it("gate:config.loaded=false 时渲染原空状态,不闪引导卡", () => {
    // stores 初始 loaded=false,模拟启动加载窗口。
    wrapper = mountPanel();
    expect(wrapper.text()).toContain("开始对话");
    expect(wrapper.find("[data-testid='chat-empty-no-providers']").exists()).toBe(false);
    expect(wrapper.find("[data-testid='chat-empty-no-models']").exists()).toBe(false);
    expect(wrapper.find("[data-testid='chat-empty-no-default']").exists()).toBe(false);
  });

  it("无 provider → 卡 A(3 步引导),CTA 打开设置落 providers", async () => {
    const config = useConfigStore();
    config.loaded = true;
    wrapper = mountPanel();
    const card = wrapper.find("[data-testid='chat-empty-no-providers']");
    expect(card.exists()).toBe(true);
    expect(card.text()).toContain("还没有可用的模型");
    expect(card.findAll(".chat-panel__onboard-steps li")).toHaveLength(3);
    expect(card.text()).toContain("添加 provider");
    expect(card.text()).toContain("粘贴 API key");
    expect(card.text()).toContain("添加模型并测试");
    expect(card.text()).toContain("/llm-setup");
    expect(card.text()).toContain("/doctor");
    await card.get("[data-testid='chat-empty-open-settings']").trigger("click");
    const settings = useSettingsModalStore();
    expect(settings.open).toBe(true);
    expect(settings.initialCategory).toBe("providers");
  });

  it("有 provider 无模型 → 卡 B,CTA 落 models", async () => {
    const config = useConfigStore();
    const providers = useProvidersStore();
    const models = useModelsStore();
    config.loaded = true;
    providers.providers = [makeProvider("p1")];
    models.models = [];
    models.defaultModelId = null;
    wrapper = mountPanel();
    const card = wrapper.find("[data-testid='chat-empty-no-models']");
    expect(card.exists()).toBe(true);
    expect(card.text()).toContain("provider 已就绪");
    await card.get("[data-testid='chat-empty-open-settings']").trigger("click");
    const settings = useSettingsModalStore();
    expect(settings.open).toBe(true);
    expect(settings.initialCategory).toBe("models");
  });

  it("有模型未设默认 → 卡 C,CTA 落 models", async () => {
    const config = useConfigStore();
    const providers = useProvidersStore();
    const models = useModelsStore();
    config.loaded = true;
    providers.providers = [makeProvider("p1")];
    models.models = [makeModel("m1", "p1")];
    models.defaultModelId = null;
    wrapper = mountPanel();
    const card = wrapper.find("[data-testid='chat-empty-no-default']");
    expect(card.exists()).toBe(true);
    expect(card.text()).toContain("选择默认模型");
    await card.get("[data-testid='chat-empty-open-settings']").trigger("click");
    const settings = useSettingsModalStore();
    expect(settings.open).toBe(true);
    expect(settings.initialCategory).toBe("models");
  });

  it("配置齐 → 原「开始对话」空状态,无引导卡", () => {
    const config = useConfigStore();
    const providers = useProvidersStore();
    const models = useModelsStore();
    config.loaded = true;
    providers.providers = [makeProvider("p1")];
    models.models = [makeModel("m1", "p1")];
    models.defaultModelId = "m1";
    wrapper = mountPanel();
    expect(wrapper.text()).toContain("开始对话");
    expect(wrapper.find("[data-testid='chat-empty-no-providers']").exists()).toBe(false);
    expect(wrapper.find("[data-testid='chat-empty-no-models']").exists()).toBe(false);
    expect(wrapper.find("[data-testid='chat-empty-no-default']").exists()).toBe(false);
  });
});
