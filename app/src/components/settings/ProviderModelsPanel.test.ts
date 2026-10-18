// 2026-10-19 (task 10-19-merge-provider-model-settings) —
// ProviderModelsPanel 测试:合并设置后每个供应商卡片内的模型管理区。
// 覆盖:
//   1. 探测:点「探测模型」调 `probe_provider_models`,列出供应商返回的
//      模型 + 目录能力徽标;已添加的行标「已添加」且不可勾选。
//   2. 导入:勾选后点「导入」逐条 `add_model`,能力字段来自目录匹配。
//   3. 自动匹配:表单里点按钮调 `lookup_model_metadata`,把上下文窗口 /
//      最大输出 / vision / thinking 填进表单字段。
//   4. 探测失败:错误内联展示,不炸整个面板。

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

import ProviderModelsPanel from "./ProviderModelsPanel.vue";
import { useModelsStore, type ModelWithProvider } from "../../stores/models";
import type { ProviderRow } from "../../stores/providers";
import type {
  ProbeModelsResult,
  LookupMetadataResult,
  ModelSpec,
} from "../../utils/modelMetadata";

function provider(overrides: Partial<ProviderRow> = {}): ProviderRow {
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

function model(overrides: Partial<ModelWithProvider> = {}): ModelWithProvider {
  return {
    id: "m-1",
    providerId: "p-1",
    modelName: "claude-sonnet-4-5",
    displayName: "Claude Sonnet 4.5",
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: true,
    supportsImages: true,
    contextWindow: 200_000,
    createdAt: "",
    updatedAt: "",
    providerDisplayName: "BigModel",
    providerProtocol: "anthropic",
    ...overrides,
  };
}

function probePayload(): ProbeModelsResult {
  return {
    success: true,
    latencyMs: 120,
    truncated: false,
    error: null,
    metadataWarning: null,
    metadataSource: "models.dev",
    models: [
      {
        modelName: "claude-sonnet-4-5",
        displayName: "claude-sonnet-4-5",
        spec: {
          providerKey: "anthropic",
          providerName: "Anthropic",
          modelId: "claude-sonnet-4-5",
          name: "Claude Sonnet 4.5",
          contextWindow: 200_000,
          maxOutputTokens: 64_000,
          supportsVision: true,
          supportsThinking: true,
          thinkingLevels: ["low", "medium", "high"],
          releaseDate: "2025-09-29",
        },
      },
      {
        modelName: "claude-haiku-4-5",
        displayName: "claude-haiku-4-5",
        spec: {
          providerKey: "anthropic",
          providerName: "Anthropic",
          modelId: "claude-haiku-4-5",
          name: "Claude Haiku 4.5",
          contextWindow: 200_000,
          maxOutputTokens: 64_000,
          supportsVision: true,
          supportsThinking: true,
          thinkingLevels: ["low", "high"],
          releaseDate: "2025-10-15",
        },
      },
      {
        modelName: "mystery-model",
        displayName: "mystery-model",
        spec: null,
      },
    ],
  };
}

function lookupPayload(): LookupMetadataResult {
  return {
    success: true,
    source: "models.dev",
    cached: false,
    stale: false,
    error: null,
    matches: [
      {
        providerKey: "anthropic",
        providerName: "Anthropic",
        modelId: "claude-opus-5-5",
        name: "Claude Opus 5.5",
        contextWindow: 500_000,
        maxOutputTokens: 128_000,
        supportsVision: true,
        supportsThinking: true,
        thinkingLevels: ["low", "high"],
        releaseDate: "2026-05-01",
      },
    ],
  };
}

function mountPanel(p: ProviderRow = provider()) {
  return mount(ProviderModelsPanel, { props: { provider: p } });
}

describe("ProviderModelsPanel — 模型探测 + 导入", () => {
  let pinia: Pinia;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);
    useModelsStore(pinia).models = [model()];
  });

  it("探测后列出供应商模型,已添加的行不可勾选", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "probe_provider_models") return Promise.resolve(probePayload());
      return Promise.resolve([]);
    });
    const wrapper = mountPanel();
    await wrapper.find('[data-testid="probe-models-p-1"]').trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("probe_provider_models", { providerId: "p-1" });
    const items = wrapper.findAll(".provider-models__probe-item");
    expect(items).toHaveLength(3);
    // 已存在的模型标「已添加」+ checkbox disabled。
    const existing = items[0]!;
    expect(existing.classes()).toContain("provider-models__probe-item--added");
    expect(existing.find("input[type='checkbox']").attributes("disabled")).toBeDefined();
    // 新模型展示目录能力徽标。
    expect(items[1]!.text()).toContain("ctx 200k");
    expect(items[1]!.text()).toContain("thinking");
    // 目录无记录的仍可勾选,只是没有能力徽标。
    expect(items[2]!.text()).toContain("目录无记录");
  });

  it("勾选并导入:逐条 add_model,能力字段来自目录", async () => {
    const modelsStore = useModelsStore(pinia);
    const addSpy = vi.spyOn(modelsStore, "add").mockResolvedValue(undefined);
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "probe_provider_models") return Promise.resolve(probePayload());
      return Promise.resolve([]);
    });
    const wrapper = mountPanel();
    await wrapper.find('[data-testid="probe-models-p-1"]').trigger("click");
    await flushPromises();

    const items = wrapper.findAll(".provider-models__probe-item");
    await items[1]!.find("input[type='checkbox']").trigger("change");
    await items[2]!.find("input[type='checkbox']").trigger("change");
    await wrapper.find('[data-testid="import-models-p-1"]').trigger("click");
    await flushPromises();

    expect(addSpy).toHaveBeenCalledTimes(2);
    // 有目录记录:context / maxTokens / thinking / vision 全来自目录。
    expect(addSpy).toHaveBeenCalledWith("p-1", "claude-haiku-4-5", "claude-haiku-4-5", {
      supportsThinking: true,
      supportsImages: true,
      contextWindow: 200_000,
      maxTokens: 64_000,
      thinkingEffort: "high",
    });
    // 无目录记录:默认能力值(200k / 无 vision / 不覆盖 max_tokens)。
    expect(addSpy).toHaveBeenCalledWith("p-1", "mystery-model", "mystery-model", {
      supportsThinking: false,
      supportsImages: false,
      contextWindow: 200_000,
    });
  });

  it("探测失败:错误内联展示,列表清空", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "probe_provider_models") {
        return Promise.resolve({
          success: false,
          latencyMs: 15,
          models: [],
          truncated: false,
          error: "HTTP 401: invalid api key",
          metadataWarning: null,
        });
      }
      return Promise.resolve([]);
    });
    const wrapper = mountPanel();
    await wrapper.find('[data-testid="probe-models-p-1"]').trigger("click");
    await flushPromises();
    const err = wrapper.find('[data-testid="probe-error-p-1"]');
    expect(err.exists()).toBe(true);
    expect(err.text()).toContain("401");
  });
});

describe("ProviderModelsPanel — 按名称自动匹配能力", () => {
  let pinia: Pinia;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);
    useModelsStore(pinia).models = [];
  });

  it("匹配成功:上下文窗口 / 最大输出 / vision / thinking 填入表单", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "lookup_model_metadata") return Promise.resolve(lookupPayload());
      return Promise.resolve([]);
    });
    const wrapper = mountPanel();
    await wrapper.find('[data-testid="add-model-p-1"]').trigger("click");
    await flushPromises();

    // 填模型名(自动匹配按钮在此之前是禁用的)。
    const nameInput = wrapper.findAll("input[type='text']")[0]!;
    await nameInput.setValue("claude-opus-5-5");
    const button = wrapper.find('[data-testid="model-auto-match"]');
    expect(button.attributes("disabled")).toBeUndefined();
    await button.trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("lookup_model_metadata", {
      modelName: "claude-opus-5-5",
      providerId: "p-1",
    });
    // 表单字段(mm:ss:Context Window 是 number input)。
    const contextInput = wrapper.find("input[type='number']");
    expect((contextInput.element as HTMLInputElement).value).toBe("500000");
    const textValues = wrapper
      .findAll("input[type='text']")
      .map((i) => (i.element as HTMLInputElement).value);
    // Max Tokens 与 Display Name 被填入。
    expect(textValues).toContain("128000");
    expect(textValues).toContain("Claude Opus 5.5");
    // 成功提示内联。
    expect(wrapper.find(".model-form__match-hint--ok").text()).toContain("已匹配");
  });

  it("多个候选:表单内列出「其它候选」,点一下换用该条能力", async () => {
    const second: ModelSpec = {
      providerKey: "openrouter",
      providerName: "OpenRouter",
      modelId: "anthropic/claude-opus-5-5",
      name: "Claude Opus 5.5 (gateway)",
      contextWindow: 1_000_000,
      maxOutputTokens: 32_000,
      supportsVision: false,
      supportsThinking: true,
      thinkingLevels: ["low"],
      releaseDate: "2026-05-01",
    };
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "lookup_model_metadata") {
        return Promise.resolve({ ...lookupPayload(), matches: [lookupPayload().matches[0]!, second] });
      }
      return Promise.resolve([]);
    });
    const wrapper = mountPanel();
    await wrapper.find('[data-testid="add-model-p-1"]').trigger("click");
    await flushPromises();
    await wrapper.findAll("input[type='text']")[0]!.setValue("claude-opus-5-5");
    await wrapper.find('[data-testid="model-auto-match"]').trigger("click");
    await flushPromises();

    const alts = wrapper.findAll(".model-form__alt");
    expect(alts).toHaveLength(1);
    expect(alts[0]!.text()).toContain("OpenRouter");
    // 最佳匹配口径先落表。
    expect((wrapper.find("input[type='number']").element as HTMLInputElement).value).toBe("500000");
    await alts[0]!.trigger("click");
    await flushPromises();
    // 换用网关条目:窗口 1M / 输出 32000 / vision=false。
    expect((wrapper.find("input[type='number']").element as HTMLInputElement).value).toBe("1000000");
    const texts = wrapper.findAll("input[type='text']").map((i) => (i.element as HTMLInputElement).value);
    expect(texts).toContain("32000");
  });

  it("匹配失败:提示可手填,不改动已有字段", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "lookup_model_metadata") {
        return Promise.resolve({
          success: false,
          matches: [],
          error: "request failed: offline",
        });
      }
      return Promise.resolve([]);
    });
    const wrapper = mountPanel();
    await wrapper.find('[data-testid="add-model-p-1"]').trigger("click");
    await flushPromises();
    await wrapper.findAll("input[type='text']")[0]!.setValue("unknown-model");
    await wrapper.find('[data-testid="model-auto-match"]').trigger("click");
    await flushPromises();
    const hint = wrapper.find(".model-form__match-hint--fail");
    expect(hint.exists()).toBe(true);
    expect(hint.text()).toContain("查询失败");
    expect(hint.text()).toContain("offline");
  });
});
