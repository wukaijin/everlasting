// useQuestionCardsStore — Pinia store for the `ask_user_question`
// blocking reverse-question tool (Phase C of
// `06-30-ask-user-question-tool`, 2026-06-30).
//
// The backend `agent::question_store::QuestionStore` (Phase A) is
// the **single source of truth** for pending questions — it lives
// in `AppState` (NOT in the LRU-bounded `messagesBySession`) so
// it survives session-switch reloads intact (PRD R9-R11:
// session-switch does NOT cancel pending questions; the user can
// switch to another session, work there, switch back, and the
// pending card is still answerable).
//
// This frontend store is a **CACHE** of that backend state:
//   1. Live events: `tool:question` IPC events push fresh
//      payloads here (Phase C3 listener).
//   2. Reload: on session switch / `ensureLoaded`, the
//      streamController invokes `get_pending_question(session_id)`
//      and **overwrites** this cache with the authoritative
//      backend state (design §5.4 source-of-truth). The cache can
//      drift if a session's in-memory messages get LRU-evicted
//      while a pending question for that session still lives in
//      the backend — `ensureLoaded` corrects this on reload.
//
// ⚠️ Cache invariant: NEVER read this cache without first asking
// the backend (via `get_pending_question`). The store's `getPending`
// is a fast read; `streamController.ensureLoaded` is the
// authoritative correction. Phase D's card component reads
// `getPending` for rendering but the card itself is only mounted
// AFTER `ensureLoaded` has reconciled the cache.
//
// Single-pending-mutex (PRD R12): the backend's QuestionStore
// enforces one pending question per session (the second `register`
// call returns `AlreadyPending` and the tool_result becomes
// `{"error": "已有 pending question,等当前回答完成"}`). The frontend
// store mirrors this — `addPending` overwrites any existing
// entry for the same session_id (the new event wins; the old
// pending was either resolved or cancelled in the meantime).

import { defineStore } from "pinia";
import { reactive } from "vue";

import type { PendingQuestion } from "./questionCards.types";

export const useQuestionCardsStore = defineStore("questionCards", () => {
  // ---------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------

  /** Per-session pending question. Keyed by `sessionId` so switching
   *  sessions shows that session's pending card (or none).
   *
   *  Cache semantics — see file header. The backend
   *  `get_pending_question` IPC is the authoritative source of
   *  truth; this Map is a frontend mirror that:
   *    - gets fresh data from the `tool:question` event listener
   *      (push side, optimistic),
   *    - gets overwritten by `ensureLoaded` via
   *      `get_pending_question` (pull side, authoritative).
   *
   *  `reactive(new Map())` (NOT a plain Map) so component computeds
   *  that read `pendingBySession.get(sessionId)` re-evaluate on
   *  mutations. Mutation sites: `addPending` (live event or
   *  ensureLoaded pull), `removePending` (correction when backend
   *  reports `null`). */
  const pendingBySession = reactive(new Map<string, PendingQuestion>());

  // ---------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------

  /** Record a pending question for a session. Called from:
   *   - the `tool:question` IPC event listener in streamController
   *     (live push — the backend just registered a question),
   *   - `streamController.ensureLoaded` after a
   *     `get_pending_question` pull that returned `Some(payload)`
   *     (authoritative overwrite).
   *
   *  Overwrite semantics: if there's already a pending question
   *  for this session, the new entry replaces it. The backend
   *  single-pending mutex guarantees we never have two live
   *  pending questions for the same session, so the "replace"
   *  branch is reached only when:
   *    (a) the user resolved the previous one AND a new one
   *        arrived (race window — the new one wins),
   *    (b) the cache had stale data from before a previous
   *        resolve + the ensureLoaded pull corrects it.
   *
   *  Idempotent — calling twice with the same payload produces
   *  the same state. */
  function addPending(p: PendingQuestion): void {
    pendingBySession.set(p.sessionId, p);
  }

  /** Clear the pending question for a session. Called from:
   *   - the `tool:question` event listener on a resolve event
   *     (no — there is no resolve event; the listener only fires
   *     on a fresh pending question),
   *   - `streamController.ensureLoaded` after a
   *     `get_pending_question` pull that returned `null` (the
   *     backend says "no pending" — correct any stale frontend
   *     cache),
   *   - session-delete path (deleteSession handler).
   *
   *  Safe to call for a session with no entry (no-op). */
  function removePending(sessionId: string): void {
    pendingBySession.delete(sessionId);
  }

  /** Read the pending question for a session. Returns `undefined`
   *  when no pending question exists. **Cache-only read** —
   *  callers needing authoritative state should invoke
   *  `get_pending_question` first (the streamController does
   *  this on every `ensureLoaded`). */
  function getPending(sessionId: string): PendingQuestion | undefined {
    return pendingBySession.get(sessionId);
  }

  /** List all pending questions. Used by debug surfaces + future
   *  "X questions pending across all sessions" badges. Returns
   *  a snapshot array (NOT a reactive view) — callers wanting
   *  reactivity should read `pendingBySession` directly. */
  function list(): PendingQuestion[] {
    return Array.from(pendingBySession.values());
  }

  /** Clear every pending question. Used by app shutdown hooks
   *  and unit tests; production code rarely calls this (the
   *  pending state outlives session-switches per PRD R9-R11).
   *  Future: a `beforeunload` handler could flush the in-flight
   *  pending question to the backend's persist path (out of
   *  scope for v1 — process death loses pending state, accepted
   *  per PRD AC7b). */
  function clearAll(): void {
    pendingBySession.clear();
  }

  return {
    // State (exposed as a reactive Map; consumers read via .get / .has)
    pendingBySession,
    // Actions
    addPending,
    removePending,
    getPending,
    list,
    clearAll,
  };
});