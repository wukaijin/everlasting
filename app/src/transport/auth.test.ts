// auth — 多 token 存储(08-26-multi-node-pairing)。核心断言对象:
//   1. map 读写/覆盖/删除/清空
//   2. `currentDeviceToken` 的解析优先级(selected > 单条 > legacy > null)
//   3. `dropCurrentNodeToken` 只删"当前用的那条"
//   4. legacy 单 token 的迁移源语义(setNodeToken 即完成迁移)
//   5. localStorage 不可用(私密模式)/ JSON 损坏时不抛错
//
// jsdom 提供 localStorage;每个用例前清三个键避免串扰。

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

import {
  getNodeTokens,
  getTokenForNode,
  setNodeToken,
  removeNodeToken,
  clearAllNodeTokens,
  hasPairedNode,
  currentDeviceToken,
  dropCurrentNodeToken,
} from "./auth";

const TOKENS_KEY = "everlasting_node_tokens";
const LEGACY_KEY = "everlasting_device_token";
const SELECTED_KEY = "everlasting_selected_node";

function setStored(key: string, value: string | null) {
  if (value === null) localStorage.removeItem(key);
  else localStorage.setItem(key, value);
}

beforeEach(() => {
  [TOKENS_KEY, LEGACY_KEY, SELECTED_KEY].forEach((k) =>
    localStorage.removeItem(k),
  );
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("node token map", () => {
  it("setNodeToken 记录条目,getNodeTokens/getTokenForNode 可读回", () => {
    setNodeToken("pc-1", "t1");
    setNodeToken("pc-2", "t2");
    expect(getNodeTokens()).toEqual({ "pc-1": "t1", "pc-2": "t2" });
    expect(getTokenForNode("pc-2")).toBe("t2");
    expect(getTokenForNode("nope")).toBeNull();
  });

  it("重复配对同一节点覆盖旧 token(换码场景)", () => {
    setNodeToken("pc-1", "old");
    setNodeToken("pc-1", "new");
    expect(getTokenForNode("pc-1")).toBe("new");
    expect(Object.keys(getNodeTokens())).toEqual(["pc-1"]);
  });

  it("setNodeToken 完成迁移:删除 legacy 单值 key", () => {
    localStorage.setItem(LEGACY_KEY, "legacy-tok");
    setNodeToken("pc-1", "t1");
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull();
    // legacy 已清后 currentDeviceToken 不会退回它
    expect(currentDeviceToken()).toBe("t1");
  });

  it("removeNodeToken 只删目标条目", () => {
    setNodeToken("pc-1", "t1");
    setNodeToken("pc-2", "t2");
    removeNodeToken("pc-1");
    expect(getNodeTokens()).toEqual({ "pc-2": "t2" });
  });

  it("clearAllNodeTokens 清 map + legacy(登出)", () => {
    setNodeToken("pc-1", "t1");
    localStorage.setItem(LEGACY_KEY, "legacy-tok");
    clearAllNodeTokens();
    expect(getNodeTokens()).toEqual({});
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull();
    expect(hasPairedNode()).toBe(false);
  });

  it("损坏的 map JSON 当作空(回到未配对态,不抛错)", () => {
    localStorage.setItem(TOKENS_KEY, "{not json");
    expect(getNodeTokens()).toEqual({});
    expect(hasPairedNode()).toBe(false);
  });

  it("map JSON 里非字符串值被过滤", () => {
    localStorage.setItem(TOKENS_KEY, JSON.stringify({ a: "t", b: 3, c: "" }));
    expect(getNodeTokens()).toEqual({ a: "t" });
  });
});

describe("hasPairedNode", () => {
  it("map 有条目 → true;legacy 存在 → true;全空 → false", () => {
    expect(hasPairedNode()).toBe(false);
    localStorage.setItem(LEGACY_KEY, "legacy-tok");
    expect(hasPairedNode()).toBe(true);
    setNodeToken("pc-1", "t1"); // 顺带清 legacy
    expect(hasPairedNode()).toBe(true);
  });
});

describe("currentDeviceToken 优先级", () => {
  it("选中节点的条目优先", () => {
    setNodeToken("pc-1", "t1");
    setNodeToken("pc-2", "t2");
    localStorage.setItem(SELECTED_KEY, "pc-2");
    expect(currentDeviceToken()).toBe("t2");
  });

  it("未选择且仅一条 → 该条(配对后未进选择页)", () => {
    setNodeToken("pc-1", "t1");
    expect(currentDeviceToken()).toBe("t1");
  });

  it("未选择且多条 → null(歧义;路由 guard 保证此态不 invoke)", () => {
    setNodeToken("pc-1", "t1");
    setNodeToken("pc-2", "t2");
    expect(currentDeviceToken()).toBeNull();
  });

  it("map 空 + legacy 存在 → legacy(迁移前兜底)", () => {
    localStorage.setItem(LEGACY_KEY, "legacy-tok");
    expect(currentDeviceToken()).toBe("legacy-tok");
  });

  it("选中节点已被修剪(无条目)时退回单条 map", () => {
    setNodeToken("pc-1", "t1");
    setNodeToken("pc-2", "t2");
    removeNodeToken("pc-2");
    localStorage.setItem(SELECTED_KEY, "pc-2");
    expect(currentDeviceToken()).toBe("t1");
  });

  it("全空 → null", () => {
    expect(currentDeviceToken()).toBeNull();
  });
});

describe("dropCurrentNodeToken(401 修剪)", () => {
  it("删选中节点条目,其余配对不受影响", () => {
    setNodeToken("pc-1", "t1");
    setNodeToken("pc-2", "t2");
    localStorage.setItem(SELECTED_KEY, "pc-2");
    dropCurrentNodeToken();
    expect(getNodeTokens()).toEqual({ "pc-1": "t1" });
  });

  it("未选择 + 单条 → 删该条", () => {
    setNodeToken("pc-1", "t1");
    dropCurrentNodeToken();
    expect(getNodeTokens()).toEqual({});
  });

  it("legacy 兜底态 → 清 legacy", () => {
    localStorage.setItem(LEGACY_KEY, "legacy-tok");
    dropCurrentNodeToken();
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull();
  });

  it("map 选中条目 + 残留 legacy(脏态)→ 两者都清,不产生双源", () => {
    // setNodeToken 本身会清 legacy,这个态只在"手改 localStorage"时出现;
    // 行为取保守:drop 顺手清掉,不产生双源。
    setNodeToken("pc-1", "t1");
    localStorage.setItem(SELECTED_KEY, "pc-1");
    // 手工塞回 legacy 模拟脏态
    setStored(LEGACY_KEY, "stale");
    dropCurrentNodeToken();
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull();
    expect(getNodeTokens()).toEqual({});
  });
});

describe("localStorage 不可用(私密模式)", () => {
  it("所有读路径回退空/null,写路径静默", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("SecurityError");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("SecurityError");
    });
    vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => {
      throw new Error("SecurityError");
    });
    expect(getNodeTokens()).toEqual({});
    expect(hasPairedNode()).toBe(false);
    expect(currentDeviceToken()).toBeNull();
    expect(() => setNodeToken("pc-1", "t1")).not.toThrow();
    expect(() => clearAllNodeTokens()).not.toThrow();
  });
});
