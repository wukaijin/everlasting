// useQuestionCardsStore — Pinia store for both blocking
// reverse-question tools:
//
//   1. `ask_user_question` (Phase C of
//      `06-30-ask-user-question-tool`, 2026-06-30) — initial
//      design, still active.
//   2. `request_mode_change` (Phase B of
//      `07-07-07-07-request-mode-change-tool`, 2026-07-07) —
//      added as a second variant on the same single-pending
//      gate, sharing the same per-session mutex (the backend's
//      `QuestionStore` enforces one pending interaction per
//      session regardless of kind).
//
// The backend `agent::question_store::QuestionStore` (Phase A)
// is the **single source of truth** for pending interactions —
// it lives in `AppState` (NOT in the LRU-bounded
// `messagesBySession`) so it survives session-switch reloads
// intact (PRD R9-R11: session-switch does NOT cancel pending
// interactions; the user can switch to another session, work
// there, switch back, and the pending card is still answerable).
//
// This frontend store is a **CACHE** of that backend state:
//   1. Live events: `tool:question` / `mode:change:request` IPC
//      events push fresh payloads here (Phase C3 listener).
//   2. Reload: on session switch / `ensureLoaded`, the
//      streamController invokes `get_pending_interaction` and
//      **overwrites** this cache with the authoritative
//      backend state. The cache can drift if a session's
//      in-memory messages get LRU-evicted while a pending
//      interaction for that session still lives in the backend
//      — `ensureLoaded` corrects this on reload.
//
// ⚠️ Cache invariant: NEVER read this cache without first asking
// the backend (via `get_pending_interaction`). The store's
// `getPending` is a fast read; `streamController.ensureLoaded`
// is the authoritative correction. Phase C/D's card components
// read `getPending` for rendering but the card itself is only
// mounted AFTER `ensureLoaded` has reconciled the cache.
//
// Single-pending-mutex (PRD R12): the backend's QuestionStore
// enforces one pending interaction per session (the second
// `register` call returns `AlreadyPending` and the tool_result
// becomes `{"error": "已有 pending interaction,等当前完成"}`).
// The frontend store mirrors this — `addPending` overwrites any
// existing entry for the same session_id (the new event wins;
// the old pending was either resolved or cancelled in the
// meantime).

import { defineStore } from "pinia";
import { reactive } from "vue";

import type { PendingInteraction } from "./questionCards.types";
import type { SessionMode } from "./chat.types";

export const useQuestionCardsStore = defineStore("questionCards", () => {
  // ---------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------

  /** Per-session pending interaction. Keyed by `sessionId` so
   *  switching sessions shows that session's pending card (or
   *  none). Value is the tagged `PendingInteraction` union — the
   *  caller dispatches on `entry.kind` to pick the right card
   *  component (`<AskUserQuestionCard>` vs
   *  `<RequestModeChangeCard>`).
   *
   *  Cache semantics — see file header. The backend
   *  `get_pending_interaction` IPC is the authoritative source
   *  of truth; this Map is a frontend mirror that:
   *    - gets fresh data from the `tool:question` /
   *      `mode:change:request` event listeners (push side,
   *      optimistic),
   *    - gets overwritten by `ensureLoaded` via
   *      `get_pending_interaction` (pull side, authoritative).
   *
   *  `reactive(new Map())` (NOT a plain Map) so component
   *  computeds that read `pendingBySession.get(sessionId)`
   *  re-evaluate on mutations. Mutation sites: `addPending`
   *  (live event or ensureLoaded pull), `removePending`
   *  (correction when backend reports `null`). */
  const pendingBySession = reactive(new Map<string, PendingInteraction>());

  /** Per-session current mode. Populated by
   *  `resolveModeChange` (after the backend applies the new
   *  mode) so the UI can render "当前 mode" without an extra
   *  IPC. Future-proof for cases where the backend pushes mode
   *  updates via a separate event (not in scope for v1). */
  const currentModeBySession = reactive(new Map<string, SessionMode>());

  // ---------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------

  /** Record a pending interaction for a session. Called from:
   *   - the `tool:question` / `mode:change:request` IPC event
   *     listeners in streamController (live push — the backend
   *     just registered an interaction),
   *   - `streamController.ensureLoaded` after a
   *     `get_pending_interaction` pull that returned a payload
   *     (authoritative overwrite).
   *
   *  Overwrite semantics: if there's already a pending
   *  interaction for this session, the new entry replaces it.
   *  The backend single-pending mutex guarantees we never have
   *  two live pending interactions for the same session, so
   *  the "replace" branch is reached only when:
   *    (a) the user resolved the previous one AND a new one
   *        arrived (race window — the new one wins),
   *    (b) the cache had stale data from before a previous
   *        resolve + the ensureLoaded pull corrects it.
   *
   *  Idempotent — calling twice with the same payload produces
   *  the same state. */
  function addPending(sessionId: string, payload: PendingInteraction): void {
    pendingBySession.set(sessionId, payload);
  }

  /** Clear the pending interaction for a session. Called from:
   *   - the live event listeners (no — there is no resolve
   *     event; listeners only fire on fresh pending events),
   *   - `streamController.ensureLoaded` after a
   *     `get_pending_interaction` pull that returned `null` (the
   *     backend says "no pending" — correct any stale frontend
   *     cache),
   *   - `resolveModeChange` after a successful backend resolve
   *     (the user acted, the card's IPC just succeeded, so the
   *     cache entry is obsolete),
   *   - session-delete path (deleteSession handler).
   *
   *  Safe to call for a session with no entry (no-op). */
  function removePending(sessionId: string): void {
    pendingBySession.delete(sessionId);
  }

  /** Read the pending interaction for a session. Returns
   *  `undefined` when no pending interaction exists.
   *  **Cache-only read** — callers needing authoritative state
   *  should invoke `get_pending_interaction` first (the
   *  streamController does this on every `ensureLoaded`). */
  function getPending(sessionId: string): PendingInteraction | undefined {
    return pendingBySession.get(sessionId);
  }

  /** List all pending interactions. Used by debug surfaces +
   *  future "X pending across all sessions" badges. Returns a
   *  snapshot array (NOT a reactive view) — callers wanting
   *  reactivity should read `pendingBySession` directly. */
  function list(): PendingInteraction[] {
    return Array.from(pendingBySession.values());
  }

  /** Clear every pending interaction. Used by app shutdown
   *  hooks and unit tests; production code rarely calls this
   *  (the pending state outlives session-switches per PRD
   *  R9-R11). Future: a `beforeunload` handler could flush the
   *  in-flight pending interaction to the backend's persist
   *  path (out of scope for v1 — process death loses pending
   *  state, accepted per PRD AC7b). */
  function clearAll(): void {
    pendingBySession.clear();
    currentModeBySession.clear();
  }

  // ---------------------------------------------------------------------
  // `request_mode_change` actions (Phase B3, 2026-07-07)
  // ---------------------------------------------------------------------

  /** Resolve a pending mode change. The frontend's "允许" /
   *  "拒绝" click handler calls this with `allow = true |
   *  false`; the backend's `resolve_mode_change` IPC applies
   *  the new mode (on allow) + resolves the QuestionStore
   *  oneshot in one round-trip.
   *
   *  On success:
   *    - updates `currentModeBySession[sid]` with the freshly-
   *      loaded `SessionRow.mode` so UI components reading
   *      "当前 mode" see the new value without a re-fetch,
   *    - patches the in-memory session summary in the chat
   *      store (`sessions.value`) so the session list's mode
   *      chip + the active session's mode chip update
   *      optimistically (mirrors `requestSetMode`'s
   *      `(summary as { mode: string }).mode = mode;` pattern),
   *    - removes the cache entry from `pendingBySession` so
   *      the card flips off,
   *    - returns the updated SessionRow.
   *
   *  On error: throws (so the caller can toast via
   *  `useErrorBus`); the cache entry is left intact so the
   *  user can retry. The backend's error is the canonical
   *  user-facing message (Yolo root guard → "Cannot enable
   *  Yolo as root"; DB error → server error category). */
  async function resolveModeChange(
    sessionId: string,
    toolUseId: string,
    targetMode: SessionMode,
    allow: boolean,
  ): Promise<void> {
    const { resolveModeChange: invokeResolveModeChange } = await import(
      "../utils/toolModeChange"
    );
    const row = await invokeResolveModeChange({
      sessionId,
      toolUseId,
      targetMode,
      allow,
    });
    // Backend already wrote the mode + audit + resolved the
    // oneshot. Update local caches + remove the card.
    currentModeBySession.set(sessionId, row.mode as SessionMode);
    // Patch the session summary in the chat store so the
    // session-list mode chip reflects the new mode
    // immediately. The chat store's `sessions` ref is the
    // canonical list — we mutate in place to mirror the
    // `requestSetMode` optimistic-update pattern.
    const { useChatStore } = await import("./chat");
    const chatStore = useChatStore();
    const summary = chatStore.sessions.find((s) => s.id === sessionId);
    if (summary) {
      (summary as { mode: string }).mode = row.mode;
    }
    removePending(sessionId);
  }

  /** Fetch the authoritative pending interaction from the
   *  backend's QuestionStore via the `get_pending_interaction`
   *  IPC and merge into the `pendingBySession` cache. Returns
   *  the fetched entry (or `null`) so callers can dispatch on
   *  `kind` without a second cache read.
   *
   *  Cache semantics: the returned entry is the source of
   *  truth; `pendingBySession` is overwritten to match (the
   *  pull side of the optimistic-push-pull reconciliation
   *  pattern, see file header). A `null` return removes the
   *  cache entry. */
  async function getPendingInteractionAction(
    sessionId: string,
  ): Promise<PendingInteraction | null> {
    const { getPendingInteraction } = await import("../utils/toolModeChange");
    const entry = await getPendingInteraction(sessionId);
    if (entry) {
      addPending(sessionId, entry);
    } else {
      removePending(sessionId);
    }
    return entry;
  }

  return {
    // State (exposed as reactive Maps; consumers read via .get / .has)
    pendingBySession,
    currentModeBySession,
    // Actions
    addPending,
    removePending,
    getPending,
    list,
    clearAll,
    resolveModeChange,
    getPendingInteractionAction,
  };
});
