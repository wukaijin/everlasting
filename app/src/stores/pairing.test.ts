// pairing store — 08-26-multi-node-pairing redeem 语义:
//   1. 成功:token 按 nodeId 累积进 auth map(不覆盖既有配对)+ SSE 重置
//   2. 两次 redeem(两台 PC)→ map 两条
//   3. 400/429/网络失败 → 用户可读中文错误
//   4. wire:请求 body snake_case(device_name),响应解构 camelCase
//
// `../transport/http` mock 掉 daemonBase/resetEventSource;auth 真模块 +
// jsdom localStorage;fetch stub 全局。

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { setActivePinia, createPinia } from "pinia";

vi.mock("../transport/http", () => ({
  daemonBase: () => "https://remote.example.com",
  resetEventSource: vi.fn(),
}));

import { usePairingStore } from "./pairing";
import { getNodeTokens } from "../transport/auth";
import { resetEventSource } from "../transport/http";

const TOKENS_KEY = "everlasting_node_tokens";
const LEGACY_KEY = "everlasting_device_token";

const fetchMock = vi.fn();
const resetMock = vi.mocked(resetEventSource);

function ok(body: Record<string, unknown>) {
  return {
    ok: true,
    status: 200,
    json: async () => body,
  } as Response;
}

beforeEach(() => {
  setActivePinia(createPinia());
  [TOKENS_KEY, LEGACY_KEY].forEach((k) => localStorage.removeItem(k));
  fetchMock.mockReset();
  resetMock.mockClear();
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("pairing.redeem", () => {
  it("成功:token 按 nodeId 入 map,重置 SSE,返回 nodeId;body 是 snake_case", async () => {
    fetchMock.mockResolvedValueOnce(
      ok({ deviceToken: "tok-a", nodeId: "pc-1", nodeDisplayName: "公司 PC" }),
    );

    const store = usePairingStore();
    const nodeId = await store.redeem("123456", "我的手机");

    expect(nodeId).toBe("pc-1");
    expect(getNodeTokens()).toEqual({ "pc-1": "tok-a" });
    expect(resetMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [
      string,
      RequestInit,
    ];
    expect(url).toBe("https://remote.example.com/api/v1/pairing/redeem");
    expect(JSON.parse(init.body as string)).toEqual({
      code: "123456",
      device_name: "我的手机",
    });
  });

  it("第二次 redeem(另一台 PC)累积,不覆盖既有配对", async () => {
    fetchMock
      .mockResolvedValueOnce(
        ok({ deviceToken: "tok-a", nodeId: "pc-1", nodeDisplayName: "公司 PC" }),
      )
      .mockResolvedValueOnce(
        ok({ deviceToken: "tok-b", nodeId: "pc-2", nodeDisplayName: "家里 PC" }),
      );

    const store = usePairingStore();
    await store.redeem("111111", "我的手机");
    await store.redeem("222222", "我的手机");

    expect(getNodeTokens()).toEqual({ "pc-1": "tok-a", "pc-2": "tok-b" });
  });

  it("400 → 配对码无效或已过期", async () => {
    fetchMock.mockResolvedValueOnce({ ok: false, status: 400 } as Response);
    const store = usePairingStore();
    await expect(store.redeem("000000", "d")).rejects.toThrow(
      "配对码无效或已过期。",
    );
  });

  it("429 → 尝试过于频繁", async () => {
    fetchMock.mockResolvedValueOnce({ ok: false, status: 429 } as Response);
    const store = usePairingStore();
    await expect(store.redeem("000000", "d")).rejects.toThrow(
      "尝试过于频繁，请稍后再试。",
    );
  });

  it("网络失败 → 无法连接到服务器", async () => {
    fetchMock.mockRejectedValueOnce(new TypeError("Failed to fetch"));
    const store = usePairingStore();
    await expect(store.redeem("000000", "d")).rejects.toThrow(
      "无法连接到服务器",
    );
  });
});
