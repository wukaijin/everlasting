// toolTaskStateTransition.ts — Tauri invoke wrapper for the
// `request_task_state_transition` blocking tool
// (`07-09-workflow-transition-card`, 2026-07-09).
//
// Thin layer over the transport's `invoke` for the
// frontend-initiated `resolve_task_state_transition` IPC command.
// The server-pushed wire — `task:state:transition:request` — is a
// `listen<>` event handled in streamController, not a Tauri command,
// so it doesn't live here.
//
// Sibling of `toolModeChange.ts`. Field-name discipline + command-
// name single-source-of-truth live here (same rationale as
// `toolQuestion.ts` / `toolModeChange.ts`).
//
// Differs from `resolveModeChange` in ONE arg: `slug`. The
// `resolve_task_state_transition` IPC handler has no WorkflowCtx
// (it runs outside the chat loop), so it must locate
// `<project>/.everlasting/tasks/<slug>/task.json` itself to read the
// current `from` state off disk. There is no `fromState` arg — the
// backend reads it fresh; the card only echoes target + slug + allow.

import { transport } from "../transport";

import {
  RESOLVE_TASK_STATE_TRANSITION_CMD,
  type TaskStateTransitionResolvePayload,
} from "../stores/questionCards.types";
import type { SessionRowUpdate } from "./toolModeChange";

/** Resolve a pending workflow state transition. Frontend → backend.
 *
 *  Routes to `commands::question::resolve_task_state_transition`,
 *  which:
 *    1. parses + validates `target_state` (rejects unknown values),
 *    2. if `allow === false` → records `task_state_transition_denied`
 *       audit + resolves the oneshot as `Cancelled` (no
 *       `set_task_state` call; task.json untouched),
 *    3. if `allow === true` → calls `workflow::set_task_state`
 *       (writes task.json.status + bumps updated_at + dispatches the
 *       `from → to` Rust hook — e.g. Check→Done triggers spec
 *       distillation), records `task_state_transition_allowed` audit,
 *       resolves the oneshot as `Answered(true)`.
 *
 *  Returns the SessionRow (the established return shape for the
 *  resolve-* pattern). Unlike `resolveModeChange`, the row's `mode`
 *  is UNCHANGED — a workflow state transition does not touch the
 *  session's edit/plan/yolo mode. The store action therefore does
 *  NOT patch the session-summary mode; it only removes the pending
 *  card. Rejects with `String` (Tauri convention) on backend error.
 *
 *  Tauri auto-translates the JS camelCase args to Rust snake_case at
 *  the IPC boundary. */
export async function resolveTaskStateTransition(
  payload: TaskStateTransitionResolvePayload,
): Promise<SessionRowUpdate> {
  return await transport.invoke<SessionRowUpdate>(RESOLVE_TASK_STATE_TRANSITION_CMD, {
    sessionId: payload.sessionId,
    toolUseId: payload.toolUseId,
    targetState: payload.targetState,
    slug: payload.slug,
    allow: payload.allow,
  });
}
