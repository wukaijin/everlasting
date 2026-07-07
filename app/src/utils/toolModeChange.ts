// toolModeChange.ts — Tauri invoke wrappers for the
// `request_mode_change` blocking tool (07-07-07-07,
// 2026-07-07).
//
// Thin layer over `@tauri-apps/api/core`'s `invoke` for the two
// frontend-initiated IPC commands (`resolve_mode_change`,
// `get_pending_interaction`). The third wire —
// `mode:change:request` — is server-pushed via `listen<>`
// in streamController, not a Tauri command, so it doesn't live
// here.
//
// Why a thin wrapper (rather than inline `invoke(...)` calls in
// the consuming components)? Same two reasons as
// `toolQuestion.ts`:
//   1. Single source of truth for the command names.
//   2. Field-name discipline — the snake_case → camelCase
//      mapping (Rust `session_id` ↔ JS `sessionId`) lives in one
//      place.
//
// `resolveModeChange` returns the freshly-loaded `SessionRow`
// (snake_case — the Rust `SessionRow` has no `rename_all`, so it
// serializes as snake_case verbatim). The frontend uses this
// to refresh `currentSession`'s mode field via the chat store's
// session-list mutation (mirroring `requestSetMode`'s optimistic
// `(summary as { mode: string }).mode = mode;` pattern).

import { invoke } from "@tauri-apps/api/core";

import {
  GET_PENDING_INTERACTION_CMD,
  RESOLVE_MODE_CHANGE_CMD,
  type ModeChangeResolvePayload,
  type PendingInteraction,
} from "../stores/questionCards.types";

/** Subset of the Rust `db::SessionRow` that the frontend
 *  actually uses after a `resolve_mode_change` IPC. The backend
 *  serializes `SessionRow` as snake_case (no `rename_all`), so
 *  the wire shape matches this interface verbatim. We only
 *  declare the fields the chat store's session-list patch path
 *  touches (`mode` + `id`) — the rest of the row is unused for
 *  this flow. A future broader SessionRow type lives in
 *  `chat.types.ts` (the chat store's `SessionSummary` covers the
 *  list view; the full row is internal to the backend). */
export interface SessionRowUpdate {
  id: string;
  mode: "edit" | "plan" | "yolo" | "background";
}

/** Resolve a pending mode change. Frontend → backend.
 *
 *  Routes to `commands::question::resolve_mode_change`, which:
 *    1. parses + validates `target_mode` (lenient → `Edit` on
 *       unknown),
 *    2. if `allow === false` → records `mode_change_denied`
 *       audit + resolves the oneshot as `Cancelled` + returns
 *       the freshly-loaded row (mode unchanged),
 *    3. if `allow === true` → calls
 *       `set_session_mode_internal` (the single source of truth
 *       for mode application; also writes `mode_changed` +
 *       Yolo-transition audits), records `mode_change_allowed`
 *       audit, resolves the oneshot as `Answered(true)`, and
 *       returns the row with the new `mode`.
 *
 *  Returns the SessionRow payload from the backend; rejects
 *  with `String` (Tauri's `Result<T, String>` convention) on
 *  backend error (Yolo root guard, DB error, unknown session).
 *  On the allow-but-apply-failed path the backend resolves the
 *  oneshot as `Cancelled` AND surfaces the error so the
 *  frontend can toast it.
 *
 *  Tauri auto-translates the JS camelCase args to Rust
 *  snake_case at the IPC boundary. */
export async function resolveModeChange(
  payload: ModeChangeResolvePayload,
): Promise<SessionRowUpdate> {
  return await invoke<SessionRowUpdate>(RESOLVE_MODE_CHANGE_CMD, {
    sessionId: payload.sessionId,
    toolUseId: payload.toolUseId,
    targetMode: payload.targetMode,
    allow: payload.allow,
  });
}

/** Fetch the authoritative pending interaction state for a
 *  session from the backend's QuestionStore. Returns the tagged
 *  `PendingInteraction` payload (question OR mode_change) for
 *  the session, or `null` when no interaction is pending.
 *
 *  Routes to `commands::question::get_pending_interaction` →
 *  `QuestionStore.get_payload(session_id)`. The IPC return is
 *  `PendingInteractionEntry { kind, payload }`; we unwrap to
 *  `payload` here so the caller can dispatch on
 *  `entry.kind` without a second wrapper layer. The backend
 *  emits the tagged enum directly (snake_case — same shared-
 *  struct exemption as the question payload).
 *
 *  The streamController calls this on `ensureLoaded` to
 *  overwrite the optimistic Pinia cache with the
 *  authoritative backend state (the QuestionStore lives in
 *  `AppState`, NOT in the LRU-bounded `messagesBySession`, so
 *  it survives session-switch reloads intact — see the
 *  `ask_user_question` design §5.4 source-of-truth rationale). */
export async function getPendingInteraction(
  sessionId: string,
): Promise<PendingInteraction | null> {
  return await invoke<PendingInteraction | null>(
    GET_PENDING_INTERACTION_CMD,
    { sessionId },
  );
}
