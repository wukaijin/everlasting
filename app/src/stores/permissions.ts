// usePermissionsStore — Pinia store for the ⑨ 关 ↔ `permission:ask`
// IPC bridge.
//
// 2026-06-16 (inline approval card): the store now routes pending
// asks PER SESSION (`pendingBySession: Map<sessionId, PermissionAsk>`)
// instead of a single global slot. This fixes the multi-session
// concurrency bug where a new `permission:ask` from session B
// silently overwrote session A's pending ask — leaving A's agent
// loop blocked for 120s until a timeout deny. Each session now has
// its own slot, and each ask arms an independent 120s timer keyed
// by `rid`.
//
// The backend `agent/permissions::check` emits a `permission:ask`
// event with `{ rid, sessionId, toolUseId, toolName, toolInput,
// risk, reason?, path? }` when a tool_use lands on Tier 4 (no
// grant + outside-repo / shell Ask / web_fetch). This store:
//
//   1. On app boot, `start()` registers a global listener on the
//      `permission:ask` Tauri event and routes the payload into
//      `pendingBySession` keyed by `sessionId`.
//   2. The inline `<ToolCallCard>` approval UI reads
//      `getPending(currentSessionId)` and matches
//      `ask.toolUseId === call.id` to render the pending state on
//      the right card. The buttons invoke `respond(rid, decision,
//      reason?)` — `reason` is the optional "拒绝并说明" feedback.
//   3. Each ask arms an independent 120s client-side timer (keyed
//      by rid) mirroring the backend's `ASK_TIMEOUT` — if the user
//      doesn't respond in time, we fire `permission_response({decision:
//      "deny"})` ourselves + surface a toast. The backend's own 120s
//      timer also fires; both converge to a deny.
//
// IPC wire shape (matches `agent::permissions::PermissionAskPayload`,
// which uses `#[serde(rename_all = "camelCase")]`):
//
//   Server → Client: emit("permission:ask", payload)
//   Client → Server: invoke("permission_response", { rid, decision, reason? })
//
// `decision` is one of `"allow_once"`, `"allow_always"`, `"deny"`.
// `reason` is the user's optional "拒绝并说明" feedback (only
// meaningful for `"deny"`); the backend surfaces it as the
// `tool_result(is_error)` content so the LLM learns why it was denied.

import { defineStore } from "pinia";
import { reactive, computed } from "vue";
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
  /** Session this ask belongs to (per-session routing, 2026-06-16).
   *  The store keys pending asks by `sessionId` so multi-session
   *  concurrency no longer collides on a single slot. */
  sessionId: string;
  /** The `tool_use_id` of the tool_use that triggered this ask.
   *  The inline `<ToolCallCard>` matches `call.id === toolUseId`
   *  to render the approval state on the right card. */
  toolUseId: string;
  toolName: string;
  toolInput: Record<string, unknown>;
  risk: Risk;
  /** Optional human-readable reason (e.g. "matches denylist:
   *  rm -rf /"). Populated by the backend when the Tier 4
   *  prompt is emitted; the card renders it under the
   *  command preview when present. */
  reason?: string;
  /** Path scope row (re-grill 2026-06-13, Q10). Only set for path
   *  tools (read_file / write_file / edit_file / list_dir / grep /
   *  glob); `undefined` for shell / web_fetch — the card hides the
   *  path range row entirely when this is absent. The backend
   *  serializes with `#[serde(skip_serializing_if =
   *  "Option::is_none")]`. */
  path?: string;
}

/** Three-button response vocabulary. Matches the backend
 *  `commands::permissions::permission_response` mapping
 *  (string → `PermissionResponse` enum). */
export type PermissionDecision = "allow_once" | "allow_always" | "deny";

/** Mirror of the backend's 120s ask timeout. Each ask arms its own
 *  timer (keyed by rid) — they no longer share a single slot. The
 *  backend has its own `tokio::time::sleep(ASK_TIMEOUT)` that
 *  auto-denies; we duplicate on the frontend so we can (a) surface a
 *  "已超时,自动拒绝" toast and (b) clear the local pending without
 *  waiting on the backend's resolution. */
export const ASK_TIMEOUT_MS = 120_000;

/** Decision → Chinese label (mirror of the `Risk.label_cn` mapping
 *  on the backend). Kept here so the card doesn't need to reach
 *  into the Rust crate. */
export const RISK_LABEL_CN: Record<Risk, string> = {
  low: "低",
  medium: "中",
  high: "高",
  critical: "极高",
};

/** Title + icon-name mapping per risk level. Drives the card
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
  // Per-session pending asks
  // -----------------------------------------------------------------------

  /** Pending asks keyed by `sessionId`. One slot per session — the
   *  agent loop's serial `check()` means a single session has at
   *  most one pending ask at a time, but multiple sessions can
   *  each have their own (multi-session concurrency). Replaces the
   *  old single-slot `pendingPermission` which silently dropped a
   *  session's ask when another session's ask arrived. */
  const pendingBySession = reactive(new Map<string, PermissionAsk>());

  /** Active 120s timers, keyed by `rid`. Each ask times out
   *  independently (the old store shared one `timerRid` slot, so a
   *  new ask's timer silently orphaned the prior ask's timer). */
  const timersByRid = new Map<string, ReturnType<typeof setTimeout>>();

  /** SessionIds that currently have a pending ask. Drives the
   *  SessionList / session-tab "有待审批" badge so the user can see
   *  which sessions are blocked even after switching away. */
  const pendingSessionIds = computed<string[]>(() => [
    ...pendingBySession.keys(),
  ]);

  /** Unlisten handle for the `permission:ask` listener. Set on
   *  `start()` and torn down on `stop()`. */
  let unlisten: UnlistenFn | null = null;

  /** Optional toast surface — the store doesn't own the toast
   *  system (that lives in `useProjectsStore`), but we accept a
   *  callback at `start()` so the 120s-timeout path can show
   *  "权限询问已超时,已自动拒绝". */
  let showToast: ((msg: string, level: "info" | "warn" | "error") => void) | null =
    null;

  // -----------------------------------------------------------------------
  // Ask timer (per-rid 120s timeout — auto-deny + toast)
  // -----------------------------------------------------------------------

  function clearTimer(rid: string): void {
    const t = timersByRid.get(rid);
    if (t !== undefined) {
      clearTimeout(t);
      timersByRid.delete(rid);
    }
  }

  /** Arm an independent 120s timer for `rid`. When it fires we
   *  best-effort deny (the backend auto-denies at 120s too;
   *  `respond()` no-ops on a stale rid) + surface a toast + the
   *  matching pending is cleared by `respond()`. */
  function startAskTimer(rid: string): void {
    const t = window.setTimeout(() => {
      timersByRid.delete(rid);
      void respond(rid, "deny").catch(() => {
        // Swallow — best-effort; the backend already auto-denied.
      });
      showToast?.("权限询问已超时,已自动拒绝", "warn");
    }, ASK_TIMEOUT_MS);
    timersByRid.set(rid, t);
  }

  // -----------------------------------------------------------------------
  // Store actions
  // -----------------------------------------------------------------------

  /** Mount the global `permission:ask` listener. Idempotent —
   *  calling twice replaces the prior unlisten. The `toast`
   *  callback is optional; it's used by the 120s-timeout path. */
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

  /** Tear down the listener + clear ALL pending asks + timers. */
  function stop(): void {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    showToast = null;
    for (const rid of [...timersByRid.keys()]) {
      clearTimer(rid);
    }
    pendingBySession.clear();
  }

  /** Route a new ask into its session's slot. If the same session
   *  already has a pending ask (same-session replace — the agent
   *  loop is serial so this is the normal next-tool_use path),
   *  clear the prior timer first so it can't fire into the new
   *  ask. */
  function setPending(ask: PermissionAsk): void {
    const prev = pendingBySession.get(ask.sessionId);
    if (prev) {
      clearTimer(prev.rid);
    }
    pendingBySession.set(ask.sessionId, ask);
    startAskTimer(ask.rid);
  }

  /** Read the pending ask for a session (or `undefined`). The
   *  inline `<ToolCallCard>` calls this with `currentSessionId`
   *  and matches `ask.toolUseId === call.id`. */
  function getPending(sessionId: string): PermissionAsk | undefined {
    return pendingBySession.get(sessionId);
  }

  /** Whether a session currently has a pending ask. Drives the
   *  SessionList badge. */
  function hasPending(sessionId: string): boolean {
    return pendingBySession.has(sessionId);
  }

  /** Clear the pending ask for a session (+ its timer). Called
   *  after the card emits a decision; `respond()` also clears
   *  internally, so this is mostly for explicit teardown / tests. */
  function clearPending(sessionId: string): void {
    const ask = pendingBySession.get(sessionId);
    if (ask) {
      clearTimer(ask.rid);
    }
    pendingBySession.delete(sessionId);
  }

  /** Send the user's decision to the backend + clear the matching
   *  pending (looked up by `rid`) and its timer. `reason` is the
   *  optional "拒绝并说明" feedback — only sent for `"deny"`; the
   *  backend surfaces it as the `tool_result(is_error)` content. */
  async function respond(
    rid: string,
    decision: PermissionDecision,
    reason?: string,
  ): Promise<void> {
    try {
      await invoke("permission_response", {
        rid,
        decision,
        // Only deny carries a reason (the user's feedback text).
        reason: decision === "deny" ? reason : undefined,
      });
    } catch (e) {
      console.error("usePermissionsStore.respond failed:", e);
    }
    // Clear the matching pending (looked up by rid) + its timer.
    for (const [sid, ask] of pendingBySession) {
      if (ask.rid === rid) {
        clearTimer(rid);
        pendingBySession.delete(sid);
        break;
      }
    }
  }

  return {
    pendingBySession,
    pendingSessionIds,
    // start/stop manage the listener lifecycle (App.vue mount/unmount)
    start,
    stop,
    setPending,
    getPending,
    hasPending,
    clearPending,
    respond,
  };
});
