// usePermissionsStore — Pinia store for the ⑨ 关 ↔ `permission:ask`
// IPC bridge.
//
// PR3 (A2 + B7): the backend `agent/permissions::check` emits a
// `permission:ask` event with `{ rid, tool_name, tool_input,
// risk, reason? }` when a tool_use lands on Tier 3 of the 5-tier
// decision layer (no "始终允许" record for the tool in the active
// session). This store is the frontend-side counterpart:
//
//   1. On app boot, `start()` registers a global listener on the
//      `permission:ask` Tauri event and wires the payload into
//      `pendingPermission` (a single-slot ref so a new ask
//      replaces the old one — see §"Multi-tool_use 批处理" in the
//      spec).
//   2. `<PermissionModal>` reads `pendingPermission` and mounts
//      when it's non-null. The modal's 3 buttons each invoke
//      `respond(rid, decision)` which calls `permission_response`
//      IPC; the backend wakes the oneshot it had registered for
//      that `rid` and continues the agent loop.
//   3. The store also arms a 120s client-side timer that mirrors
//      the backend's `ASK_TIMEOUT` — if the user doesn't pick a
//      button in time, we fire `permission_response({decision:
//      "deny"})` ourselves and surface a toast saying "权限询问
//      已超时,已自动拒绝". The backend's own 120s timer also
//      fires (we can't cancel it; we just match its deadline).
//      Duplicated timeout is OK because both paths converge to a
//      deny — see the IPC 异常路径 table in the spec.
//
// IPC wire shape (matches `agent::permissions::PermissionAskPayload`,
// which uses `#[serde(rename_all = "camelCase")]`):
//
//   Server → Client: emit("permission:ask", payload)
//   Client → Server: invoke("permission_response", { rid, decision })
//
// The `decision` string is one of `"allow_once"`, `"allow_always"`,
// `"deny"` — exactly what the backend's `PermissionResponse` enum
// deserializes from (see `agent/permissions.rs::PermissionResponse`
// and `commands/permissions.rs::permission_response`).

import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Per-tool risk level. Serialized to/from the IPC payload —
 *  backend `permissions::Risk` uses `#[serde(rename_all =
 *  "lowercase")]`, producing `"low" | "medium" | "high" |
 *  "critical"` on the wire. The mapping is static (per-tool
 *  hard-coded in `risk_for_tool`); the frontend just renders
 *  the value. */
export type Risk = "low" | "medium" | "high" | "critical";

/** Wire shape of a single `permission:ask` event. Field names
 *  are camelCase to match the Rust `PermissionAskPayload`'s
 *  `#[serde(rename_all = "camelCase")]` directive. */
export interface PermissionAsk {
  rid: string;
  toolName: string;
  toolInput: Record<string, unknown>;
  risk: Risk;
  /** Optional human-readable reason (e.g. "matches denylist:
   *  rm -rf /"). Populated by the backend when the Tier 3
   *  prompt is emitted; the modal renders it under the
   *  command preview when present. */
  reason?: string;
}

/** Three-button response vocabulary. Matches the backend
 *  `commands::permissions::permission_response` mapping
 *  (string → `PermissionResponse` enum). */
export type PermissionDecision = "allow_once" | "allow_always" | "deny";

/** Mirror of the backend's 120s ask timeout. We don't actually
 *  NEED to fire a deny at 120s — the backend has its own
 *  `tokio::time::sleep(ASK_TIMEOUT)` that auto-denies. We
 *  duplicate the timeout on the frontend so we can (a) surface
 *  a "已超时,自动拒绝" toast and (b) close the modal without
 *  waiting on the backend's response. The duplication is
 *  intentional — see store doc comment. */
export const ASK_TIMEOUT_MS = 120_000;

/** Decision → Chinese label (mirror of the `Risk.label_cn` mapping
 *  on the backend). Kept here so the modal doesn't need to reach
 *  into the Rust crate. */
export const RISK_LABEL_CN: Record<Risk, string> = {
  low: "低",
  medium: "中",
  high: "高",
  critical: "极高",
};

/** Title + icon-name mapping per risk level. Drives the modal
 *  header's icon container + label. Full chinese labels
 *  ("全中文" per audit §6.2 feedback). */
export const RISK_META: Record<
  Risk,
  { label: string; iconName: string; iconColor: string; title: string }
> = {
  low: {
    label: "低",
    iconName: "info",
    iconColor: "var(--color-text-muted)",
    title: "需要权限:只读操作",
  },
  medium: {
    label: "中",
    iconName: "circle-dot",
    iconColor: "var(--color-tool-write)",
    title: "需要权限:写文件",
  },
  high: {
    label: "高",
    iconName: "shield-check",
    iconColor: "var(--color-tool-shell)",
    title: "需要权限:执行 Shell",
  },
  critical: {
    label: "极高",
    iconName: "shield-x",
    iconColor: "var(--color-tool-error)",
    title: "此命令匹配硬拒绝规则,默认拒绝",
  },
};

export const usePermissionsStore = defineStore("permissions", () => {
  // -----------------------------------------------------------------------
  // Single-slot pending ask
  // -----------------------------------------------------------------------

  /** The currently-visible `permission:ask` payload, or `null`
   *  when no modal is showing. Per spec Q7 (multi-tool_use
   *  批处理), the backend emits one ask per tool_use serially;
   *  the store replaces `pendingPermission` on each new event.
   *  The modal's `:key` is bound to `pendingPermission.rid` so
   *  it remounts on every new ask (resets focus, scrolls the
   *  preview to top, etc.). */
  const pendingPermission = ref<PermissionAsk | null>(null);

  /** The `rid` we currently have a 120s timer armed for. Stored
   *  as a ref so the timer's clear-on-replace logic can verify
   *  it's still the same ask (race-guard against the user
   *  clicking a button just as the timer fires). */
  const timerRid = ref<string | null>(null);

  /** Unlisten handle for the `permission:ask` listener. Set on
   *  `start()` and torn down on `stop()`. */
  let unlisten: UnlistenFn | null = null;

  /** Optional toast surface — the store doesn't own the toast
   *  system (that lives in `useProjectsStore`), but we accept a
   *  callback at `start()` so the 120s-timeout path can show
   *  "权限询问已超时,已自动拒绝". This keeps the store free of
   *  a hard dependency on the projects store. */
  let showToast: ((msg: string, level: "info" | "warn" | "error") => void) | null =
    null;

  // -----------------------------------------------------------------------
  // Ask timer (120s timeout — auto-deny + toast)
  // -----------------------------------------------------------------------

  function clearAskTimer(): void {
    timerRid.value = null;
  }

  /** Arm the 120s timer for `rid`. If a new ask arrives before
   *  the previous timer fires, the new ask replaces the ref and
   *  a new timer is armed; the previous timer is left to fire
   *  into a no-op (the `respond()` is keyed by rid and will
   *  gracefully no-op against a stale rid — see the IPC
   *  异常路径 table "重复 permission_response" in the spec).
   *
   *  Even though the backend has its own 120s timer, we duplicate
   *  it here so we can:
   *    (a) close the modal at the same moment (the backend only
   *        resolves the oneshot at its timeout; the modal stays
   *        mounted until the agent loop resumes and the next
   *        event arrives),
   *    (b) surface a toast "权限询问已超时,已自动拒绝" — without
   *        a client-side timer the user would see a frozen
   *        modal for the full 120s.
   *  See store doc comment for the rationale. */
  function startAskTimer(rid: string): void {
    timerRid.value = rid;
    window.setTimeout(() => {
      // Race-guard: if the user clicked a button between
      // schedule and fire, the store state has moved on and
      // we shouldn't re-deny. `timerRid.value === rid` is the
      // canonical check (the click path clears the ref).
      if (timerRid.value !== rid) return;
      // The backend will also auto-deny at 120s; calling
      // `respond("deny")` here is a no-op against the backend's
      // already-resolved oneshot. Safe — `permission_response`
      // returns `Ok(false)` for a stale rid (best-effort).
      void respond(rid, "deny").catch(() => {
        // Swallow — best-effort; the backend already auto-denied.
      });
      // Surface the user-facing explanation (the user has been
      // staring at a frozen modal for 120s; the toast confirms
      // the auto-deny happened).
      showToast?.("权限询问已超时,已自动拒绝", "warn");
    }, ASK_TIMEOUT_MS);
  }

  // -----------------------------------------------------------------------
  // Store actions
  // -----------------------------------------------------------------------

  /** Mount the global `permission:ask` listener. Idempotent —
   *  calling twice replaces the prior unlisten. The `toast`
   *  callback is optional; it's used by the 120s-timeout path
   *  to surface a "已超时" toast. Pass `null` to skip (e.g. in
   *  tests). Call from `App.vue`'s `onMounted`. */
  async function start(toast?: typeof showToast): Promise<void> {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    showToast = toast ?? null;
    unlisten = await listen<PermissionAsk>("permission:ask", (event) => {
      setPending(event.payload);
    });
  }

  /** Tear down the listener. Call from `App.vue`'s
   *  `onUnmounted` (defensive — for a single-window Tauri app
   *  this is mostly redundant with the process lifetime, but
   *  it makes the store cleanly testable). */
  function stop(): void {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    showToast = null;
    clearAskTimer();
    pendingPermission.value = null;
  }

  /** Wire a new ask into the store. Called by the listener
   *  AND exposed for tests. Replacing the prior pending ask
   *  (if any) is the canonical multi-tool_use path: the
   *  modal's `:key="pendingPermission.rid"` triggers a
   *  remount, so the new ask gets a fresh focus + scroll
   *  state. */
  function setPending(ask: PermissionAsk): void {
    pendingPermission.value = ask;
    startAskTimer(ask.rid);
  }

  /** Clear the pending ask (e.g. after the modal emits a
   *  decision). The backend's resolution is fire-and-forget
   *  via the oneshot; we don't wait for the agent loop to
   *  resume here. The next `permission:ask` event (for the
   *  next tool_use in the same turn) re-arms the modal. */
  function clearPending(): void {
    pendingPermission.value = null;
    clearAskTimer();
  }

  /** Send the user's decision to the backend. Best-effort —
   *  a throw is caught and logged; the modal closes either
   *  way (a stale rid returns `Ok(false)` from the backend,
   *  which is a benign no-op per the spec). */
  async function respond(
    rid: string,
    decision: PermissionDecision,
  ): Promise<void> {
    try {
      await invoke("permission_response", { rid, decision });
    } catch (e) {
      console.error("usePermissionsStore.respond failed:", e);
    }
    // Always clear locally so the modal closes regardless of
    // IPC outcome (the modal's `:key` will be `undefined` after
    // `clearPending`; the `v-if` unmounts the modal).
    if (pendingPermission.value?.rid === rid) {
      clearPending();
    }
  }

  return {
    pendingPermission,
    // start/stop manage the listener lifecycle (App.vue mount/unmount)
    start,
    stop,
    setPending,
    clearPending,
    respond,
  };
});