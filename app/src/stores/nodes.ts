import { defineStore } from "pinia";
import { ref } from "vue";
import { daemonBase, resetEventSource } from "../transport/http";
import {
  getNodeTokens,
  setNodeToken,
  removeNodeToken,
  clearAllNodeTokens,
  currentDeviceToken,
  SELECTED_NODE_KEY,
} from "../transport/auth";

// Nodes store (S4 Step 5, design §5.4 / §6.2; 08-26-multi-node-pairing).
//
// Lists the PCs bound to the STORED device tokens (one per paired PC,
// `transport/auth.ts` map). Like pairing, this hits a remote-native
// REST endpoint (`GET /api/v1/nodes`) directly via fetch — NOT through
// `transport.invoke` (D4: pairing/nodes are remote-owned endpoints with
// a shape distinct from daemon `/api/v1/{domain}/{cmd}` commands).
//
// Wire contract (read directly from the Rust source, nodes.rs:27):
//
//   NodeInfo (rename_all="camelCase"):
//     { nodeId, displayName, status: "online"|"offline", lastSeenAt }
//   lastSeenAt is unix epoch ms (last heartbeat Pong).
//   Each token returns exactly the ONE node it is bound to (devices
//   table: token → node_id); multi-card lists come from querying once
//   per stored token and merging by nodeId.
//
// P2-2 (review): destructure camelCase, NOT snake_case.
//
// selectNode does NOT switch the transport baseURL (D4 correction): the
// baseURL is always `location.origin` (the remote). The device token is
// already bound to a node_id at pairing time (remote `devices` table);
// every proxied request is routed by the remote to that node. selectNode
// only records the user's choice locally (transport resolves the token
// via `currentDeviceToken()`); the NODE SWITCH itself (full SPA reload
// to reset stores/SSE) is driven by NodeListView.

/** One bound PC card. Wire shape is camelCase (nodes.rs:27). */
export interface NodeInfo {
  nodeId: string;
  displayName: string;
  status: "online" | "offline";
  /** Unix epoch ms of the last heartbeat Pong. */
  lastSeenAt: number;
}

/** Fetch `/api/v1/nodes` once with `token` (one HTTP call per token).
 *  Throws on network failure; returns null on 401 (caller prunes). */
async function fetchNodesForToken(token: string): Promise<NodeInfo[] | null> {
  let resp: Response;
  try {
    resp = await fetch(`${daemonBase()}/api/v1/nodes`, {
      headers: { Authorization: `Bearer ${token}` },
    });
  } catch {
    // fetch throws TypeError on network failure / DNS / CORS — surface a
    // friendly message (same wording as the pairing store).
    throw new Error("无法连接到服务器，请检查网络后重试。");
  }
  if (resp.status === 401) return null;
  if (!resp.ok) {
    throw new Error(`加载节点失败 (HTTP ${resp.status})。`);
  }
  return (await resp.json()) as NodeInfo[];
}

export const useNodesStore = defineStore("nodes", () => {
  const nodes = ref<NodeInfo[]>([]);
  const loaded = ref(false);
  const loading = ref(false);

  /** Restore the last-selected node from localStorage so the user
   *  doesn't re-pick on every app launch. try/catch mirrors the auth.ts
   *  localStorage pattern (private mode / disabled storage). */
  const selectedNodeId = ref<string | null>(readSelectedNodeId());

  /** 08-26 迁移:旧版单 token(map 空 + legacy key 存在)无法本地反解
   *  nodeId —— 正好借一次 /nodes 查询(响应含 nodeId)把 legacy token
   *  归位进 map(setNodeToken 内部删 legacy key)。失败留待下次重试,
   *  不阻塞正常加载(currentDeviceToken 有 legacy 兜底)。 */
  async function migrateLegacyToken(): Promise<void> {
    if (Object.keys(getNodeTokens()).length > 0) return;
    const token = currentDeviceToken(); // map 空 → legacy(或 null)
    if (!token) return;
    try {
      const list = await fetchNodesForToken(token);
      const nodeId = list?.[0]?.nodeId;
      if (nodeId) setNodeToken(nodeId, token);
    } catch {
      // 网络失败:不迁移,下次 loadNodes 再试
    }
  }

  /** Load every paired node: one `/api/v1/nodes` call per stored token,
   *  merged by nodeId. A 401 for one token means that pairing was
   *  revoked server-side (devices row deleted) — prune it and keep the
   *  rest; other failures throw (NodeListView shows loadError + retry).
   *  A 401 does NOT route to /pairing while other pairings remain
   *  (that routing lives in App's onAuthFailed via hasPairedNode). */
  async function loadNodes() {
    loading.value = true;
    try {
      await migrateLegacyToken();
      const collected: NodeInfo[] = [];
      for (const [nodeId, token] of Object.entries(getNodeTokens())) {
        const list = await fetchNodesForToken(token);
        if (list === null) {
          removeNodeToken(nodeId);
          continue;
        }
        collected.push(...list);
      }
      // 防御性按 nodeId 去重(map 同 id 覆盖本不会产生重复;服务器端
      // 也不会 —— 一码一 token 一节点;此处兜底合并语义)。
      const byId = new Map(collected.map((n) => [n.nodeId, n]));
      nodes.value = [...byId.values()];
      loaded.value = true;
    } finally {
      loading.value = false;
    }
  }

  /** Record the user's node choice locally (state + localStorage). Does
   *  not hit the network — the token is already bound to the node. */
  function selectNode(nodeId: string) {
    selectedNodeId.value = nodeId;
    writeSelectedNodeId(nodeId);
  }

  /** Logout: clear ALL node tokens (→ browser-local behavior), tear
   *  down the SSE stream (it was authenticated), drop the selected node,
   *  and clear its localStorage entry. The caller navigates to /pairing
   *  afterwards (the guard would do it anyway). */
  function logout() {
    clearAllNodeTokens();
    resetEventSource();
    selectedNodeId.value = null;
    clearSelectedNodeId();
    // Drop the cached list too — it was scoped to the now-cleared tokens.
    nodes.value = [];
    loaded.value = false;
  }

  return { nodes, loaded, loading, selectedNodeId, loadNodes, selectNode, logout };
});

function readSelectedNodeId(): string | null {
  try {
    return localStorage.getItem(SELECTED_NODE_KEY);
  } catch {
    return null;
  }
}

function writeSelectedNodeId(nodeId: string) {
  try {
    localStorage.setItem(SELECTED_NODE_KEY, nodeId);
  } catch {
    // localStorage unavailable (private mode) — selection only lives in
    // memory for this session.
  }
}

function clearSelectedNodeId() {
  try {
    localStorage.removeItem(SELECTED_NODE_KEY);
  } catch {
    // fail silently
  }
}
