// 2026-09-07 (provider-model-disable) — useModelsStore 禁用面测试。
// 覆盖:isModelEffectivelyDisabled 三态判定(模型级 / provider 级连坐 /
// 旧 daemon 无字段)、enabledModels + enabledModelsGroupedByProvider 过滤
// (空组剔除)、setDisabled 的 IPC payload + 整表刷新。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import {
  useModelsStore,
  isModelEffectivelyDisabled,
  type ModelWithProvider,
} from "./models";

function makeModel(overrides: Partial<ModelWithProvider>): ModelWithProvider {
  return {
    id: "m-1",
    providerId: "p-1",
    modelName: "model-a",
    displayName: "Model A",
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: false,
    supportsImages: false,
    contextWindow: 128_000,
    createdAt: "",
    updatedAt: "",
    providerDisplayName: "Provider One",
    providerProtocol: "anthropic",
    ...overrides,
  };
}

describe("isModelEffectivelyDisabled", () => {
  it("treats missing flags (old daemon) as enabled", () => {
    const m = makeModel({});
    expect(isModelEffectivelyDisabled(m)).toBe(false);
  });

  it("honors model-level disable", () => {
    expect(isModelEffectivelyDisabled(makeModel({ disabled: true }))).toBe(true);
  });

  it("honors provider-level disable (denormalized join)", () => {
    expect(isModelEffectivelyDisabled(makeModel({ providerDisabled: true }))).toBe(
      true,
    );
  });
});

describe("useModelsStore — disable-aware selection lists", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  function seed() {
    const store = useModelsStore();
    store.models = [
      makeModel({ id: "m-on", displayName: "On", providerId: "p-1" }),
      makeModel({
        id: "m-off",
        displayName: "Off",
        providerId: "p-1",
        disabled: true,
      }),
      makeModel({
        id: "m-provider-off",
        displayName: "ProviderOff",
        // 整个 provider 被禁用 —— 连坐,且组内没有其他模型 → 组剔除。
        providerId: "p-2",
        providerDisabled: true,
        providerDisplayName: "Provider Two",
      }),
    ];
    return store;
  }

  it("enabledModels filters model-level and provider-level disables", () => {
    const store = seed();
    expect(store.enabledModels.map((m) => m.id)).toEqual(["m-on"]);
  });

  it("enabledModelsGroupedByProvider drops groups left empty", () => {
    const store = seed();
    const groups = store.enabledModelsGroupedByProvider;
    expect(groups).toHaveLength(1);
    expect(groups[0].provider.id).toBe("p-1");
    // 全量分组不受影响(Settings Models 列表仍要展示禁用行)。
    expect(store.modelsGroupedByProvider).toHaveLength(2);
  });

  it("setDisabled sends the toggle IPC and reloads the list", async () => {
    const store = seed();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_models") {
        return [
          makeModel({ id: "m-off", disabled: false }),
          makeModel({ id: "m-on" }),
        ];
      }
      if (cmd === "get_default_model") return null;
      return null;
    });
    await store.setDisabled("m-off", false);
    // 参数名必须是 id(后端 Tauri 参数/daemon 字段都叫 id;modelId 会在
    // HTTP 路径转成 model_id 触发 422 —— 2026-09-07 实测翻过车)。
    expect(invokeMock).toHaveBeenCalledWith("set_model_disabled", {
      id: "m-off",
      disabled: false,
    });
    expect(store.models.map((m) => m.id)).toEqual(["m-off", "m-on"]);
    expect(store.enabledModels.map((m) => m.id)).toEqual(["m-off", "m-on"]);
  });
});
