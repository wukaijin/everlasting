// 2026-10-19 (task 10-19-merge-provider-model-settings) —
// `utils/modelMetadata` 纯函数测试:目录条目 → 表单能力字段的折算规则。
// 网络面(Rust `probe_provider_models` / `lookup_model_metadata`)在
// 后端单测 + e2e 路由 smoke 里覆盖,前端只测确定性逻辑。

import { describe, it, expect } from "vitest";
import {
  capabilitiesFromSpec,
  formatTokens,
  pickThinkingEffort,
  specTags,
  type ModelSpec,
} from "./modelMetadata";

function spec(overrides: Partial<ModelSpec> = {}): ModelSpec {
  return {
    providerKey: "anthropic",
    providerName: "Anthropic",
    modelId: "claude-sonnet-4-5",
    name: "Claude Sonnet 4.5 (latest)",
    contextWindow: 200_000,
    maxOutputTokens: 64_000,
    supportsVision: true,
    supportsThinking: true,
    thinkingLevels: [],
    releaseDate: "2025-09-29",
    ...overrides,
  };
}

describe("capabilitiesFromSpec — 目录条目折算为模型能力", () => {
  it("fills all four capability fields from the catalog entry", () => {
    const caps = capabilitiesFromSpec(spec(), "anthropic");
    expect(caps.contextWindow).toBe(200_000);
    expect(caps.maxTokens).toBe(64_000);
    expect(caps.supportsThinking).toBe(true);
    expect(caps.supportsImages).toBe(true);
    // budget_tokens 型思考没有 effort 词表 → 不显式发送挡位。
    expect(caps.thinkingEffort).toBeNull();
  });

  it("keeps the fallback context window when the catalog has none", () => {
    const caps = capabilitiesFromSpec(spec({ contextWindow: null }), "anthropic", 128_000);
    expect(caps.contextWindow).toBe(128_000);
    // 默认 fallback 是 200k(与表单一贯缺省口径一致)。
    expect(capabilitiesFromSpec(spec({ contextWindow: null }), "openai").contextWindow).toBe(
      200_000,
    );
  });

  it("leaves max tokens empty when the catalog has no output limit", () => {
    expect(capabilitiesFromSpec(spec({ maxOutputTokens: null }), "openai").maxTokens).toBeNull();
  });

  it("thinking=false drops the effort even if levels exist", () => {
    const caps = capabilitiesFromSpec(
      spec({ supportsThinking: false, thinkingLevels: ["low", "high"] }),
      "anthropic",
    );
    expect(caps.supportsThinking).toBe(false);
    expect(caps.thinkingEffort).toBeNull();
  });

  it("vision=false maps to supportsImages=false", () => {
    expect(capabilitiesFromSpec(spec({ supportsVision: false }), "openai").supportsImages).toBe(
      false,
    );
  });

  it("picks an effort level from the catalog vocabulary", () => {
    const caps = capabilitiesFromSpec(
      spec({ supportsThinking: true, thinkingLevels: ["low", "medium", "high"] }),
      "anthropic",
    );
    expect(caps.thinkingEffort).toBe("high");
  });
});

describe("pickThinkingEffort — 协议词表过滤", () => {
  it("prefers high, then medium, then low", () => {
    expect(pickThinkingEffort(spec({ thinkingLevels: ["low", "medium"] }), "anthropic")).toBe(
      "medium",
    );
    expect(pickThinkingEffort(spec({ thinkingLevels: ["low"] }), "anthropic")).toBe("low");
    expect(pickThinkingEffort(spec({ thinkingLevels: [] }), "anthropic")).toBeNull();
  });

  it("drops levels the protocol cannot send", () => {
    // xhigh/max 在 Responses 协议是非法值(responses.rs 归一化到 high)。
    expect(
      pickThinkingEffort(spec({ thinkingLevels: ["xhigh", "max"] }), "openai_responses"),
    ).toBeNull();
    // 同一目录条目在 Anthropic 协议下可以选 xhigh。
    expect(pickThinkingEffort(spec({ thinkingLevels: ["xhigh"] }), "anthropic")).toBe("xhigh");
  });

  it("keeps the current value when it is still valid", () => {
    expect(
      pickThinkingEffort(spec({ thinkingLevels: ["low", "high"] }), "anthropic", "low"),
    ).toBe("low");
    // 当前值已不在目录词表里 → 回退偏好顺序。
    expect(
      pickThinkingEffort(spec({ thinkingLevels: ["low", "high"] }), "anthropic", "max"),
    ).toBe("high");
  });

  it("only offers minimal on Responses when the catalog says minimal", () => {
    // minimal 不在偏好列表里,但它是该协议的唯一合法值 → 兜底取首个。
    expect(
      pickThinkingEffort(spec({ thinkingLevels: ["minimal"] }), "openai_responses"),
    ).toBe("minimal");
    // 同一值在 Anthropic 协议下非法 → 无挡位。
    expect(pickThinkingEffort(spec({ thinkingLevels: ["minimal"] }), "anthropic")).toBeNull();
  });
});

describe("formatTokens / specTags — 展示辅助", () => {
  it("formats thousands and millions compactly", () => {
    expect(formatTokens(200_000)).toBe("200k");
    expect(formatTokens(1_000_000)).toBe("1M");
    expect(formatTokens(1_050_000)).toBe("1.1M");
    expect(formatTokens(128_000)).toBe("128k");
    expect(formatTokens(null)).toBe("—");
    expect(formatTokens(0)).toBe("—");
  });

  it("builds the capability badge list", () => {
    expect(specTags(spec())).toEqual(["ctx 200k", "out 64k", "vision", "thinking"]);
    expect(
      specTags(
        spec({
          contextWindow: null,
          maxOutputTokens: null,
          supportsVision: false,
          supportsThinking: false,
        }),
      ),
    ).toEqual([]);
  });
});
