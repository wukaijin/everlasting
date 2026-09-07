// 2026-09-07 (provider-model-disable) — ModelRow 测试。
// 覆盖:默认模型禁用开关被禁用(isDefault 拦截)+ 常规行点击发出
// toggle-disabled + 两级禁用徽标(模型级 / provider 级连坐)。

import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";

import ModelRow from "./ModelRow.vue";
import type { ModelWithProvider } from "../../stores/models";

function makeModel(overrides: Partial<ModelWithProvider> = {}): ModelWithProvider {
  return {
    id: "m-1",
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

function mountRow(model: ModelWithProvider, isDefault = false) {
  return mount(ModelRow, {
    props: { model, test: undefined, isStreaming: false, isDefault },
  });
}

describe("ModelRow — default model disable guard", () => {
  it("disables the power toggle on the current default model", () => {
    const wrapper = mountRow(makeModel(), true);
    const btn = wrapper.find('[data-testid="model-toggle-disabled-m-1"]');
    expect(btn.attributes("disabled")).toBeDefined();
    expect(btn.attributes("title")).toContain("默认模型不能禁用");
  });

  it("emits toggle-disabled on click for non-default rows", async () => {
    const wrapper = mountRow(makeModel(), false);
    const btn = wrapper.find('[data-testid="model-toggle-disabled-m-1"]');
    expect(btn.attributes("disabled")).toBeUndefined();
    await btn.trigger("click");
    expect(wrapper.emitted("toggle-disabled")).toHaveLength(1);
  });

  it("still allows re-enabling a disabled default (enable direction is safe)", async () => {
    // 默认 + 已禁用的组合只在旧数据/竞态下出现(开关已被拦),启用
    // 方向不该被误伤 —— 按钮仍可点。
    const wrapper = mountRow(makeModel({ disabled: true }), true);
    const btn = wrapper.find('[data-testid="model-toggle-disabled-m-1"]');
    await btn.trigger("click");
    expect(wrapper.emitted("toggle-disabled")).toHaveLength(1);
  });
});

describe("ModelRow — disabled badges", () => {
  it("shows the 已禁用 tag for model-level disable", () => {
    const wrapper = mountRow(makeModel({ disabled: true }));
    expect(wrapper.find(".model-row__tag--disabled").text()).toBe("已禁用");
  });

  it("shows the provider 已禁用 tag when only the parent provider is disabled", () => {
    const wrapper = mountRow(makeModel({ providerDisabled: true }));
    expect(wrapper.text()).toContain("provider 已禁用");
    // 「已禁用」可能作为「provider 已禁用」的子串出现,断言用 tag
    // 元素本身:provider 连坐时不应存在模型级徽标节点。
    expect(wrapper.find(".model-row__tag--disabled").text()).toBe("provider 已禁用");
  });
});
