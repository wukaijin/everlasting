// Per-node device token storage for pwa-remote mode (08-26-multi-node-pairing).
//
// The PWA (mobile / remote-served browser) authenticates to the remote
// daemon with per-node `device_token`s obtained by redeeming pairing
// codes (`remote/routes/pairing.rs`). Each redeem binds ONE token to
// ONE PC node (`devices.token → node_id`); supporting multiple paired
// PCs therefore means keeping a `{ nodeId → token }` map, not a single
// token (the pre-08-26 single-token store silently dropped every node
// but the last-paired one — see task research/code-findings.md).
//
// Key insight (why transport never needs the nodeId): the remote's
// `require_device_token` middleware resolves token → node itself.
// `currentDeviceToken()` answers the only question transport asks —
// "which token is in play right now" — from the node selection the
// user made on /nodes (`everlasting_selected_node`, written by the
// nodes store):
//
//   1. selected node's map entry → it
//   2. exactly one map entry → it (just redeemed, picker not yet shown)
//   3. legacy single-token key → it (pre-multi-node data, pre-migration)
//   4. null → browser-local (direct) behavior
//
// localStorage keys:
//
//   everlasting_node_tokens    JSON Record<nodeId, token>  (this task)
//   everlasting_device_token   legacy single value — READ-ONLY migration
//                              source; never written again. Migrated
//                              lazily by nodes.loadNodes() (it has to
//                              call /nodes with the token anyway to
//                              learn which nodeId it binds).
//   everlasting_selected_node  selected node id (nodes store owns it;
//                              read here only to resolve the token)
//
// localStorage access is wrapped in try/catch (private mode / disabled
// cookies throw `SecurityError`); callers fall back to `null` / no-op so
// the app never crashes on storage — aligned with the existing
// `stores/config.ts` localStorage pattern. A corrupt tokens JSON is
// treated as empty (same failure mode as a lost token: re-pair).

const TOKENS_KEY = "everlasting_node_tokens";
/** Pre-multi-node single-token key (08-26 之前唯一存储)。只读迁移源。 */
const LEGACY_TOKEN_KEY = "everlasting_device_token";
/** Selected node id — written by the nodes store, read here (and there)
 *  to resolve the current token. Exported so both modules share ONE
 *  definition of the storage key. */
export const SELECTED_NODE_KEY = "everlasting_selected_node";

function readSelectedNodeId(): string | null {
  try {
    return localStorage.getItem(SELECTED_NODE_KEY);
  } catch {
    return null;
  }
}

function readLegacyToken(): string | null {
  try {
    return localStorage.getItem(LEGACY_TOKEN_KEY);
  } catch {
    return null;
  }
}

function clearLegacyToken(): void {
  try {
    localStorage.removeItem(LEGACY_TOKEN_KEY);
  } catch {
    // fail silently
  }
}

/** Read the `{ nodeId → token }` map. Corrupt/absent JSON → `{}`. */
export function getNodeTokens(): Record<string, string> {
  try {
    const raw = localStorage.getItem(TOKENS_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      return {};
    }
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(parsed)) {
      if (typeof v === "string" && v.length > 0) out[k] = v;
    }
    return out;
  } catch {
    return {};
  }
}

function writeNodeTokens(map: Record<string, string>): void {
  try {
    localStorage.setItem(TOKENS_KEY, JSON.stringify(map));
  } catch {
    // localStorage may be unavailable — fail silently; pwa-remote mode
    // simply won't activate.
  }
}

/** Token bound to one node, or `null`. */
export function getTokenForNode(nodeId: string): string | null {
  return getNodeTokens()[nodeId] ?? null;
}

/** Record (or overwrite — re-pairing the same PC rotates its token) a
 *  node's token. Also completes the legacy migration: once ANY node is
 *  in the map, the old single-value key is meaningless and removed. */
export function setNodeToken(nodeId: string, token: string): void {
  const map = getNodeTokens();
  map[nodeId] = token;
  writeNodeTokens(map);
  clearLegacyToken();
}

/** Drop one node's token (e.g. revoked server-side, pruned on 401). */
export function removeNodeToken(nodeId: string): void {
  const map = getNodeTokens();
  if (!(nodeId in map)) return;
  delete map[nodeId];
  writeNodeTokens(map);
}

/** Drop everything (logout). */
export function clearAllNodeTokens(): void {
  try {
    localStorage.removeItem(TOKENS_KEY);
  } catch {
    // fail silently
  }
  clearLegacyToken();
}

/** True when at least one pairing exists (map entry OR legacy token).
 *  Navigation-gating signal for the router guard / PairingView — the
 *  successor of the old `hasDeviceToken()`. */
export function hasPairedNode(): boolean {
  if (Object.keys(getNodeTokens()).length > 0) return true;
  return readLegacyToken() !== null;
}

/** The token transport should attach right now. Synchronous, no
 *  network. Priority: selected node → sole map entry → legacy → null.
 *  Ambiguous (2+ entries, no selection) → null: the router guard keeps
 *  the user on /nodes until a node is picked, so no app command runs
 *  in this state. */
export function currentDeviceToken(): string | null {
  const map = getNodeTokens();
  const selected = readSelectedNodeId();
  if (selected !== null && map[selected]) return map[selected];
  const entries = Object.values(map);
  if (entries.length === 1) return entries[0];
  if (entries.length > 1) return null;
  return readLegacyToken();
}

/** Drop the token that `currentDeviceToken()` just 401'd with (invoked
 *  by the transport's choke point; App's onAuthFailed then routes to
 *  /nodes while other pairings remain). Mirrors the resolution order:
 *  selected entry → sole entry → whatever legacy value is left. */
export function dropCurrentNodeToken(): void {
  const map = getNodeTokens();
  const selected = readSelectedNodeId();
  if (selected !== null && map[selected]) {
    delete map[selected];
    writeNodeTokens(map);
  } else {
    const nodeIds = Object.keys(map);
    if (nodeIds.length === 1) {
      delete map[nodeIds[0]];
      writeNodeTokens(map);
    }
  }
  // Legacy fallback case (and defensive partial-migration states):
  // clearing the old key is always safe — map entries above survive.
  clearLegacyToken();
}
