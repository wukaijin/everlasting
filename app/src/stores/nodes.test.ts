// nodes store — 08-26-multi-node-pairing 多节点加载语义:
//   1. 多 token 逐查 `/api/v1/nodes` 后按 nodeId 合并(多卡片)
//   2. 单 token 401(服务端吊销)→ 只修剪该配对,其余照常,不抛错
//   3. 全部 401 → 空列表 + loaded(map 全清)
//   4. legacy 单 token 惰性迁移(一次 /nodes 查询反解 nodeId 入 map)
//   5. 网络失败 → 抛友好错误(NodeListView 走 loadError + 重试)
//
// `../transport/http` mock 掉 daemonBase/resetEventSource;auth 用真模块
// + jsdom localStorage(与产线同路径);fetch 按 Authorization 路由到
// 各 token 的响应。

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { setActivePinia, createPinia } from "pinia";

vi.mock("../transport/http", () => ({
  daemonBase: () => "https://remote.example.com",
  resetEventSource: vi.fn(),
}));

import { useNodesStore, type NodeInfo } from "./nodes";
import { getNodeTokens, setNodeToken } from "../transport/auth";

const TOKENS_KEY = "everlasting_node_tokens";
const LEGACY_KEY = "everlasting_device_token";
const SELECTED_KEY = "everlasting_selected_node";

const fetchMock = vi.fn();

function jsonResponse(nodes: NodeInfo[], status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => nodes } as Response;
}

function nodeOf(nodeId: string, status: "online" | "offline" = "online"): NodeInfo {
  return { nodeId, displayName: nodeId, status, lastSeenAt: 1_700_000_000_000 };
}

/** fetch 按 Bearer token 路由:`{ "t1": Response | 401, ... }`。 */
function routeByToken(routes: Record<string, number | NodeInfo[]>) {
  fetchMock.mockImplementation((_url: string, init?: RequestInit) => {
    const h = (init?.headers as Record<string, string>) ?? {};
    const m = /Bearer (\S+)/.exec(h.Authorization ?? "");
    const token = m?.[1] ?? "";
    const target = routes[token];
    if (target === undefined) {
      return Promise.resolve(jsonResponse([], 401));
    }
    if (typeof target === "number") {
      return Promise.resolve(jsonResponse([], target));
    }
    return Promise.resolve(jsonResponse(target));
  });
}

beforeEach(() => {
  setActivePinia(createPinia());
  [TOKENS_KEY, LEGACY_KEY, SELECTED_KEY].forEach((k) => localStorage.removeItem(k));
  fetchMock.mockReset();
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("loadNodes 多节点", () => {
  it("多 token 各查一次,按 nodeId 合并成多卡片", async () => {
    setNodeToken("pc-1", "t1");
    setNodeToken("pc-2", "t2");
    routeByToken({
      t1: [nodeOf("pc-1", "online")],
      t2: [nodeOf("pc-2", "offline")],
    });

    const store = useNodesStore();
    await store.loadNodes();

    expect(fetchMock).toHaveBeenCalledTimes(2);
    const ids = store.nodes.map((n) => n.nodeId).sort();
    expect(ids).toEqual(["pc-1", "pc-2"]);
    expect(store.nodes.find((n) => n.nodeId === "pc-2")?.status).toBe("offline");
    expect(store.loaded).toBe(true);
  });

  it("单 token 401(吊销)→ 只修剪该配对,其余节点照常,不抛错", async () => {
    setNodeToken("pc-1", "t1");
    setNodeToken("pc-2", "t2");
    routeByToken({
      t1: [nodeOf("pc-1")],
      t2: 401,
    });

    const store = useNodesStore();
    await store.loadNodes();

    expect(store.nodes.map((n) => n.nodeId)).toEqual(["pc-1"]);
    expect(getNodeTokens()).toEqual({ "pc-1": "t1" });
  });

  it("全部 401 → 空列表 + loaded(用户走配对新设备)", async () => {
    setNodeToken("pc-1", "t1");
    routeByToken({ t1: 401 });

    const store = useNodesStore();
    await store.loadNodes();

    expect(store.nodes).toEqual([]);
    expect(store.loaded).toBe(true);
    expect(getNodeTokens()).toEqual({});
  });

  it("legacy 单 token:借一次查询反解 nodeId 入 map,legacy key 删除", async () => {
    localStorage.setItem(LEGACY_KEY, "legacy-tok");
    routeByToken({ "legacy-tok": [nodeOf("pc-old")] });

    const store = useNodesStore();
    await store.loadNodes();

    expect(getNodeTokens()).toEqual({ "pc-old": "legacy-tok" });
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull();
    expect(store.nodes.map((n) => n.nodeId)).toEqual(["pc-old"]);
  });

  it("网络失败 → 抛友好错误(不吞)", async () => {
    setNodeToken("pc-1", "t1");
    fetchMock.mockRejectedValueOnce(new TypeError("Failed to fetch"));

    const store = useNodesStore();
    await expect(store.loadNodes()).rejects.toThrow("无法连接到服务器");
  });
});

describe("selectNode / logout", () => {
  it("selectNode 记录选择(state + localStorage)", () => {
    const store = useNodesStore();
    store.selectNode("pc-1");
    expect(store.selectedNodeId).toBe("pc-1");
    expect(localStorage.getItem(SELECTED_KEY)).toBe("pc-1");
  });

  it("logout 清全部 token + 选择 + 列表", async () => {
    setNodeToken("pc-1", "t1");
    setNodeToken("pc-2", "t2");
    localStorage.setItem(SELECTED_KEY, "pc-1");
    routeByToken({
      t1: [nodeOf("pc-1")],
      t2: [nodeOf("pc-2")],
    });
    const store = useNodesStore();
    await store.loadNodes();
    expect(store.nodes.length).toBe(2);

    store.logout();

    expect(getNodeTokens()).toEqual({});
    expect(localStorage.getItem(SELECTED_KEY)).toBeNull();
    expect(store.selectedNodeId).toBeNull();
    expect(store.nodes).toEqual([]);
    expect(store.loaded).toBe(false);
  });
});
