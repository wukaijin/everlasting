// toolQuestion.ts — Tauri invoke wrappers for the `ask_user_question`
// blocking reverse-question tool (Phase C4 of
// `06-30-ask-user-question-tool`, 2026-06-30).
//
// Thin layer over `@tauri-apps/api/core`'s `invoke` for the two
// frontend-initiated IPC commands (`resolve_tool_question`,
// `get_pending_question`). The third wire — `tool:question` — is
// server-pushed via `listen<>` in streamController, not a Tauri
// command, so it doesn't live here.
//
// Why a thin wrapper (rather than inline `invoke(...)` calls in
// the consuming components)? Two reasons:
//   1. Single source of truth for the command names — if the Rust
//      command ever gets renamed (e.g. `resolve_tool_question` →
//      `submit_question_answer`), one edit here vs. every call
//      site.
//   2. Field-name discipline — the snake_case → camelCase mapping
//      (Rust `session_id` ↔ JS `sessionId`) lives in one place.
//      `resolveToolQuestion` translates `ToolQuestionResolvePayload`'s
//      `session_id` → `sessionId` at the IPC boundary so the
//      consuming card never has to think about it.
//
// `getPendingQuestion` is not re-exported here — it's only used
// inside `streamController.reconcilePendingQuestionFromBackend`
// which lives in the controller file directly. The wrapper would
// add no value (single call site, no field-name translation
// needed beyond what `invoke` already does).

import { invoke } from "@tauri-apps/api/core";

import {
  RESOLVE_TOOL_QUESTION_CMD,
  type ToolQuestionResolvePayload,
} from "../stores/questionCards.types";

/** Resolve a pending question. Frontend → backend.
 *
 *  Routes to `commands::question::resolve_tool_question` →
 *  `QuestionStore.resolve(session_id, response)`. The backend
 *  looks up the pending oneshot by session_id, sends the
 *  response, and returns Ok(()). A second call for the same
 *  session returns Err("question already resolved") because the
 *  oneshot can only be `send`-ed once — the caller should
 *  ignore the error (the card already transitioned to the
 *  answered/cancelled state on its first successful invoke).
 *
 *  Tauri auto-translates the JS camelCase args to Rust
 *  snake_case at the IPC boundary, so we send `sessionId` /
 *  `toolUseId` and the Rust `serde::Deserialize` reads
 *  `session_id` / `tool_use_id`. The `answer` and `cancelled`
 *  fields on `ToolQuestionResolvePayload` are optional + use
 *  the same JS keys, so the translation is mechanical.
 *
 *  Returns the backend's `Ok(())` payload on success; rejects
 *  with `String` (Tauri's `Result<T, String>` convention) on
 *  backend error (e.g. "question already resolved", unknown
 *  session). The caller should `.catch(console.error)` and
 *  NOT surface the error to the user — the card already
 *  flipped to the answered/cancelled state optimistically.
 */
export async function resolveToolQuestion(
  payload: ToolQuestionResolvePayload,
): Promise<void> {
  await invoke<void>(RESOLVE_TOOL_QUESTION_CMD, {
    sessionId: payload.session_id,
    toolUseId: payload.tool_use_id,
    // Tauri auto-converts `null`/`undefined` to `Option::None`
    // for Rust. The Rust side uses `Option<Vec<QuestionAnswer>>`
    // for `answer` and `Option<bool>` for `cancelled`, so we
    // pass them through unchanged (omit the field entirely
    // when not set — Tauri's arg-binder drops `undefined`
    // keys, so the Rust deserializer reads `None`).
    answer: payload.answer,
    cancelled: payload.cancelled,
  });
}