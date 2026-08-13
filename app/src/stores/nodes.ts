import { defineStore } from "pinia";
import { ref } from "vue";
import { daemonBase, resetEventSource } from "../transport/http";
import { getDeviceToken, clearDeviceToken } from "../transport/auth";

// Nodes store (S4 Step 5, design §5.4 / §6.2).
//
// Lists the PCs bound to the current device token (mobile / remote-served
// browser home view). Like pairing, this hits a remote-native REST
// endpoint (`GET /api/v1/nodes`) directly via fetch — NOT through
// `transport.invoke` (D4: pairing/nodes are remote-owned endpoints with
// a shape distinct from daemon `/api/v1/{domain}/{cmd}` commands).
//
// Wire contract (read directly from the Rust source, nodes.rs:27):
//
//   NodeInfo (rename_all="camelCase"):
//     { nodeId, displayName, status: "online"|"offline", lastSeenAt }
//   lastSeenAt is unix epoch ms (last heartbeat Pong).
//
// P2-2 (review): destructure camelCase, NOT snake_case.
//
// selectNode does NOT switch the transport baseURL (D4 correction): the
// baseURL is always `location.origin` (the remote). The device token is
// already bound to a node_id at pairing time (remote `devices` table);
// every proxied request is routed by the remote to that node. selectNode
// only records the user's choice locally so the router guard +
// downstream views know which node the user picked.

const SELECTED_NODE_KEY = "everlasting_selected_node";

/** One bound PC card. Wire shape is camelCase (nodes.rs:27). */
export interface NodeInfo {
  nodeId: string;
  displayName: string;
  status: "online" | "offline";
  /** Unix epoch ms of the last heartbeat Pong. */
  lastSeenAt: number;
}

export const useNodesStore = defineStore("nodes", () => {
  const nodes = ref<NodeInfo[]>([]);
  const loaded = ref(false);
  const loading = ref(false);

  /** Restore the last-selected node from localStorage so the user
   *  doesn't re-pick on every app launch. try/catch mirrors the auth.ts
   *  localStorage pattern (private mode / disabled storage). */
  const selectedNodeId = ref<string | null>(readSelectedNodeId());

  /** Fetch the bound nodes for the current device token. Throws on
   *  non-2xx (the caller surfaces a toast); a 401 here is also caught
   *  by the transport-layer onAuthFailed (setOnAuthFailed in App.vue),
   *  which clears the token + routes to /pairing — so the NodeListView
   *  only needs to handle the "show the error" half. */
  async function loadNodes() {
    loading.value = true;
    try {
      const token = getDeviceToken();
      const resp = await fetch(`${daemonBase()}/api/v1/nodes`, {
        headers: token ? { Authorization: `Bearer ${token}` } : {},
      });
      if (!resp.ok) {
        throw new Error(
          resp.status === 401
            ? "认证已失效，请重新配对。"
            : `加载节点失败 (HTTP ${resp.status})。`,
        );
      }
      nodes.value = (await resp.json()) as NodeInfo[];
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

  /** Logout: clear the device token (→ browser-local behavior), tear
   *  down the SSE stream (it was authenticated), drop the selected node,
   *  and clear its localStorage entry. The caller navigates to /pairing
   *  afterwards (the guard would do it anyway). */
  function logout() {
    clearDeviceToken();
    resetEventSource();
    selectedNodeId.value = null;
    clearSelectedNodeId();
    // Drop the cached list too — it was scoped to the now-cleared token.
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
