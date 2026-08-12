import { defineStore } from "pinia";
import { ref } from "vue";
import { transport } from "../transport";

// RemoteTab store (S4 Step 3, design §3.1).
//
// PC Settings → Remote tab is a pure-UI wrapper over the four daemon IPCs
// already wired in `commands::{config,pairing}.rs` + `CMD_TO_DOMAIN`
// (S2 task `08-11-tunnel-client`). The store only holds the three view
// states + four thin invoke wrappers; all wire shapes are camelCase
// (Rust payloads use `#[serde(rename_all = "camelCase")]`, confirmed by
// reading `commands/config.rs` RemoteConfigPayload / TunnelStatusPayload
// and `commands/pairing.rs` PairingCodePayload).
//
// `set_remote_config` args are passed camelCase; httpTransport's
// `transformArgsTopLevel` converts the top-level keys to snake_case for
// the daemon handler (see transport/http.ts). This matches the existing
// convention used by every other store (providers.ts, projects.ts, ...).

/** `get_remote_config` payload. `null` when remote is unconfigured
 *  (no remote_url key / empty). Wire: camelCase (config.rs:143). */
export interface RemoteConfig {
  remoteUrl: string;
  sharedSecret: string;
}

/** `get_tunnel_status` payload. `null` when remote is unconfigured.
 *  Wire: camelCase (config.rs:152). NOTE (P3-1): there is NO
 *  `displayName` field on this payload — the display name lives only
 *  in the remote nodes table, unreachable from the PC side without a
 *  Rust change (forbidden by S4 hard constraint). The RemoteTab node
 *  info area shows `nodeId` only. */
export interface TunnelStatus {
  connected: boolean;
  remoteUrl: string;
  nodeId: string;
  lastError: string | null;
}

/** Locally-stored view of an active pairing code. The daemon returns
 *  `{ code, expiresIn }` (seconds); we pre-compute the absolute expiry
 *  so the component's countdown just reads `Date.now()`. */
export interface PairingCode {
  code: string;
  expiresAt: number;
}

export const useRemoteConfigStore = defineStore("remoteConfig", () => {
  const config = ref<RemoteConfig | null>(null);
  const status = ref<TunnelStatus | null>(null);
  const pairingCode = ref<PairingCode | null>(null);

  /** Load the persisted remote config (or `null` if unset). */
  async function load() {
    config.value = await transport.invoke<RemoteConfig | null>(
      "get_remote_config",
    );
  }

  /** Persist remote_url + shared_secret, triggering a tunnel reconnect
   *  on the daemon side. Args are camelCase; httpTransport rewrites the
   *  top-level keys to snake_case for the daemon handler. */
  async function save(remoteUrl: string, sharedSecret: string) {
    await transport.invoke("set_remote_config", { remoteUrl, sharedSecret });
    // Reflect the saved values locally so the form stays in sync.
    config.value = { remoteUrl, sharedSecret };
  }

  /** Refresh the tunnel connection status snapshot. Polled by RemoteTab
   *  on a 2s interval while mounted. */
  async function refreshStatus() {
    status.value = await transport.invoke<TunnelStatus | null>(
      "get_tunnel_status",
    );
  }

  /** Generate a fresh 6-digit pairing code via the tunnel RPC. Stores
   *  it with an absolute expiry so the countdown UI is a pure
   *  `Date.now()` diff. */
  async function generateCode() {
    const r = await transport.invoke<{ code: string; expiresIn: number }>(
      "generate_pairing_code",
    );
    pairingCode.value = {
      code: r.code,
      expiresAt: Date.now() + r.expiresIn * 1000,
    };
  }

  return { config, status, pairingCode, load, save, refreshStatus, generateCode };
});
