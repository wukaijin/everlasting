// N4 PR1(09-19-n4-render-virtualization)— composable 内纯函数单测。
//
// jsdom 无布局:这里只测**决策**(followModeOf 的选项值 / estimateSize
// 的三态组合),不断言滚动结果(那是指令级假绿,design §7);锚定行为
// 的结果断言在 e2e(virtualized-list.spec.ts,真 Chromium)。composable
// 本体经 MessageList.test.ts 的假布局 harness(固定尺寸假 virtualizer)
// 间接覆盖 —— 本文件 import 该模块,store 依赖按 test-environment §4
// 的 canonical 形态 mock 掉(纯函数路径不会真正触碰 store)。

import { describe, it, expect, vi } from "vitest";

const invokeMock = vi.fn(async (): Promise<unknown> => null);

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...(args as Parameters<typeof invokeMock>)),
    listen: async () => () => {},
  },
}));

import { followModeOf, estimateMessageHeight, isVisible } from "./useVirtualizedMessages";
import type { ChatMessage } from "../stores/chat.types";

describe("isVisible(完整可见性 = 单调核心 ∨ error;缓存链语义对照钉)", () => {
  const msg = (extra: Partial<ChatMessage>): ChatMessage =>
    ({ id: "m1", role: "assistant", content: "", ...extra }) as ChatMessage;

  it("核心可见(content / toolCalls / thinking 任一)", () => {
    expect(isVisible(msg({ content: "x" }))).toBe(true);
    expect(
      isVisible(msg({ toolCalls: [{ id: "t" } as never] })),
    ).toBe(true);
    expect(
      isVisible(msg({ thinkingBlocks: [{ text: "t" } as never] })),
    ).toBe(true);
  });

  it("error 单独构成可见;清除后(仅 error 路径)回到不可见", () => {
    expect(isVisible(msg({ error: { message: "x" } as never }))).toBe(true);
    expect(isVisible(msg({}))).toBe(false);
  });
});

describe("followModeOf(design §2 锚定决策,PR0 勘误后形态)", () => {
  it("非流式:库不跟、无手写 force", () => {
    expect(followModeOf({ isStreaming: false, forceFollow: false })).toEqual({
      followOnAppend: false,
      forceFollow: false,
    });
    // 非流式时 force 残留(异常态)也不跟 —— 发送路径才会置 force。
    expect(followModeOf({ isStreaming: false, forceFollow: true })).toEqual({
      followOnAppend: false,
      forceFollow: false,
    });
  });

  it("流式非 force:库 'auto'(isAtEnd 门内跟 append),无手写 force", () => {
    expect(followModeOf({ isStreaming: true, forceFollow: false })).toEqual({
      followOnAppend: "auto",
      forceFollow: false,
    });
  });

  it("流式且 force:库 'auto' + 手写 force 路径(spike 结论 2:true ≡ 'auto',强制形态不存在)", () => {
    expect(followModeOf({ isStreaming: true, forceFollow: true })).toEqual({
      followOnAppend: "auto",
      forceFollow: true,
    });
  });

  it("决策值永不包含 true —— 库不存在强制跟滚取值(spike 实证回归钉)", () => {
    for (const isStreaming of [false, true]) {
      for (const forceFollow of [false, true]) {
        const d = followModeOf({ isStreaming, forceFollow });
        expect(d.followOnAppend).not.toBe(true);
      }
    }
  });
});

describe("estimateMessageHeight(PR2 实测回归式:22/行(88 字符/行)+ 座 28 + 卡 26 + 折叠思考 28)", () => {
  const msg = (extra: Partial<ChatMessage>): ChatMessage =>
    ({ id: "m1", role: "assistant", content: "", ...extra }) as ChatMessage;

  it("纯文本:折行 + 座(61c→52 / 401c→142 锚点)", () => {
    expect(estimateMessageHeight(msg({ content: "x".repeat(61) }))).toBe(22 + 28);
    expect(estimateMessageHeight(msg({ content: "x".repeat(401) }))).toBe(
      5 * 22 + 28,
    );
  });

  it("user 气泡 +16(实测 154 vs assistant 142 @401c)", () => {
    const u = { ...msg({ content: "x".repeat(401) }), role: "user" } as ChatMessage;
    expect(estimateMessageHeight(u)).toBe(5 * 22 + 28 + 16);
  });

  it("工具卡 26/张;assistant 的 toolResults 是合并副本不另计卡", () => {
    const m = msg({
      toolCalls: [{ id: "t1", name: "shell", input: {} } as never],
    });
    expect(estimateMessageHeight(m)).toBe(26);
    const m2 = msg({
      content: "结果如下",
      toolCalls: [{ id: "t1", name: "shell", input: {} } as never],
      toolResults: [{ toolUseId: "t1", content: "ok" } as never],
    });
    // 合并副本渲染在 assistant 的卡里 —— 不再按卡叠加(卡行实测 78px)。
    expect(estimateMessageHeight(m2)).toBe(22 + 28 + 26);
  });

  it("ghost user 行:tool_result 残根 6px(实测 6px)", () => {
    const ghost = {
      id: "g1",
      role: "user" as const,
      content: "",
      toolResults: [{ toolUseId: "t1", content: "ok" } as never],
    } as ChatMessage;
    expect(estimateMessageHeight(ghost)).toBe(6);
  });

  it("折叠思考块 28/块,redacted 同级", () => {
    const m = msg({
      thinkingBlocks: [
        { text: "a" } as never,
        { text: "b" } as never,
        { text: "c" } as never,
      ],
      redactedThinkingData: ["x"],
    });
    expect(estimateMessageHeight(m)).toBe(4 * 28);
  });

  it("空消息兜底下限(估 0 会让 spacer 塌掉)", () => {
    expect(estimateMessageHeight(msg({}))).toBe(6);
  });
});
