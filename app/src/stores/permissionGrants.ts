// usePermissionGrantsStore — Pinia store for the permission-grant
// management UI (task 07-01-permission-grant-list-ui).
//
// Backend exposes two Tauri commands:
//   - `list_session_tool_permissions(sessionId)` → all "always
//     allow" rows for the session, across the tool / prefix / path
//     match_kind dimensions, sorted `granted_at DESC, rowid DESC`.
//   - `revoke_tool_permission(sessionId, toolName, matchKind,
//     matchValue)` → delete ONE row by its full PK.
//
// This store is the frontend reactive wrapper:
//   1. `loadForSession(sessionId)` on modal-open.
//   2. `revoke(row)` deletes one row by PK and removes it locally
//      (no full re-fetch) — the four-tuple (toolName, matchKind,
//      matchValue) is the row identity.
//   3. `refresh()` for the modal's manual refresh button. MVP has
//      no live push — same edge-case policy as the audit store
//      (the agent loop can write a new grant while the modal is
//      open; refresh / reopen re-fetches).
//
// Failure policy: any IPC failure is caught into `error`; `grants`
// keeps its previous value so the modal renders the stale state
// with an error banner instead of clearing. `revoke` failure leaves
// the row in place (in both UI and DB) so the user can retry.
//
// Design D1 (immediate effect): the check path reads the DB on
// every tool_use, so a successful revoke takes effect on the next
// tool call without any cache-invalidation signal here.

import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/** Wire shape of a `session_tool_permissions` row. camelCase to
 *  match the Rust `PermissionGrantRow`'s
 *  `#[serde(rename_all = "camelCase")]`. `matchValue` is `null`
 *  for `matchKind = "tool"` (whole-tool grants); a glob like
 *  `"src/*"` for `path`; a token like `"git"` for `prefix`. */
export interface PermissionGrantRow {
  sessionId: string;
  toolName: string;
  matchKind: "tool" | "prefix" | "path";
  matchValue: string | null;
  grantedAt: string;
}

export const usePermissionGrantsStore = defineStore("permissionGrants", () => {
  const grants = ref<PermissionGrantRow[]>([]);
  const loading = ref<boolean>(false);
  const error = ref<string | null>(null);
  const lastSessionId = ref<string | null>(null);

  /** Identity key for a grant row — the PK four-tuple joined with
   *  NUL (a separator that can't appear in a tool name / kind /
   *  value). Used to remove a revoked row from `grants` locally
   *  without a full re-fetch. */
  function rowKey(g: {
    toolName: string;
    matchKind: string;
    matchValue: string | null;
  }): string {
    return `${g.toolName}\u0000${g.matchKind}\u0000${g.matchValue ?? ""}`;
  }

  /** Load all "always allow" rows for a session. Replaces `grants`
   *  on success; on failure, sets `error` and leaves `grants` at
   *  the previous value (defensive). The backend already sorts by
   *  `granted_at DESC, rowid DESC`, so no client-side re-sort. */
  async function loadForSession(sessionId: string): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      const rows = await invoke<PermissionGrantRow[]>(
        "list_session_tool_permissions",
        { sessionId },
      );
      grants.value = rows;
      lastSessionId.value = sessionId;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  /** Revoke one row by PK. On success, removes the row locally
   *  (matched by the four-tuple key) — no full re-fetch. On
   *  failure, sets `error` and leaves `grants` unchanged. */
  async function revoke(row: PermissionGrantRow): Promise<void> {
    try {
      await invoke("revoke_tool_permission", {
        sessionId: row.sessionId,
        toolName: row.toolName,
        matchKind: row.matchKind,
        // `null` (not undefined) — serde must see the key as JSON
        // null to reach the Rust `Option<String>` as None; passing
        // undefined drops the key and the backend read of a missing
        // arg fails (Tauri v2 does NOT treat a missing arg as None
        // for a non-Option, and for Option it still wants the key).
        matchValue: row.matchValue,
      });
      const key = rowKey(row);
      grants.value = grants.value.filter((g) => rowKey(g) !== key);
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }

  /** Re-fetch the last-loaded session. Used by the modal's manual
   *  refresh button. */
  async function refresh(): Promise<void> {
    if (!lastSessionId.value) return;
    await loadForSession(lastSessionId.value);
  }

  return {
    grants,
    loading,
    error,
    lastSessionId,
    loadForSession,
    revoke,
    refresh,
  };
});
