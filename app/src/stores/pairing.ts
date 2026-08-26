import { defineStore } from "pinia";
import { daemonBase, resetEventSource } from "../transport/http";
import { setNodeToken } from "../transport/auth";

// Pairing store (S4 Step 4, design §2.4 / §6.1).
//
// Mobile / remote-served browser redeems a 6-digit code against the
// remote daemon's own endpoint `POST /api/v1/pairing/redeem`. This is
// NOT routed through `transport.invoke` (D4): pairing/nodes are
// remote-native REST endpoints with a shape different from the daemon
// `/api/v1/{domain}/{cmd}` commands. Hardcoding them into transport
// would pollute CMD_TO_DOMAIN + the URL builder. A plain fetch is
// clearer, and this store is new (doesn't touch existing stores).
//
// Wire contract (read directly from the Rust source, not assumed):
//
//   request body  — snake_case (RedeemRequest, pairing.rs:52, default serde):
//       { code: string, device_name: string }
//   success 200   — camelCase (RedeemedResponse, pairing.rs:63,
//       rename_all="camelCase"):
//       { deviceToken, nodeId, nodeDisplayName }
//   failure 400   — invalid / expired / already-used code
//   failure 429   — per-IP rate limit (10/min, pairing.rs:37)
//
// P2-2 (review): the response is destructured camelCase, NOT snake_case.
// On success the node's device token is persisted — 08-26 多节点:按
// nodeId 累积进 `{ nodeId → token }` map(setNodeToken),不再覆盖唯一
// token,之前配对的 PC 保留 —— /nodes 才能显示多张卡片。SSE 流也重
// 建(resetEventSource,design §6.1)。
export const usePairingStore = defineStore("pairing", () => {
  /** Redeem a pairing code. On success: persists the device token +
   *  resets the SSE stream, then returns the bound `nodeId` (the caller
   *  navigates to /nodes or /chat). On failure: throws an Error whose
   *  `message` is a user-facing Chinese string (400 / 429 / network each
   *  get a distinct message). */
  async function redeem(code: string, deviceName: string): Promise<string> {
    let resp: Response;
    try {
      resp = await fetch(`${daemonBase()}/api/v1/pairing/redeem`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        // Request body is snake_case (RedeemRequest default serde).
        body: JSON.stringify({ code, device_name: deviceName }),
      });
    } catch {
      // fetch throws TypeError on network failure / DNS / CORS — the
      // remote is unreachable. Surface a friendly message rather than
      // the raw "Failed to fetch".
      throw new Error("无法连接到服务器，请检查网络后重试。");
    }

    if (!resp.ok) {
      // Map the two expected failure statuses to user-facing hints.
      // (The remote also returns an AppError body, but the status code
      // alone is enough to pick the right message — avoids depending
      // on the exact error body shape here.)
      if (resp.status === 400) {
        throw new Error("配对码无效或已过期。");
      }
      if (resp.status === 429) {
        throw new Error("尝试过于频繁，请稍后再试。");
      }
      throw new Error(`配对失败 (HTTP ${resp.status})。`);
    }

    // Response wire is camelCase (RedeemedResponse rename_all="camelCase").
    // P2-2: destructure camelCase, NOT snake_case.
    const { deviceToken, nodeId } = (await resp.json()) as {
      deviceToken: string;
      nodeId: string;
      nodeDisplayName: string;
    };
    setNodeToken(nodeId, deviceToken);
    // Rebuild the SSE stream with auth (the pre-pairing EventSource, if
    // any, was a token-less direct connection to the daemon base).
    resetEventSource();
    return nodeId;
  }

  return { redeem };
});
