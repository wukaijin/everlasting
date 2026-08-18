// chatModeActions — mode / yolo / workflow actions(拆分自 chat.ts,
// 08-10-chat-store-split)。
//
// 2 个 ref + 6 个 action 原样搬迁为 `createModeActions(ctx)` 工厂;ctx 注入
// 共享 state(sessions / currentSessionId)。拆分契约见
// `.trellis/spec/frontend/state-management.md` §Stream Controller Pattern。
import { ref, type Ref } from "vue";
import { transport } from "../transport";
import type { SessionMode, SessionSummary } from "./chat.types";

export interface ModeActionsContext {
  sessions: Ref<SessionSummary[]>;
  currentSessionId: Ref<string | null>;
}

export function createModeActions(ctx: ModeActionsContext) {
  const { sessions, currentSessionId } = ctx;

  // -----------------------------------------------------------------------
  // A2 + B7 (PR2 front-end): per-session Mode changes via the
  // `set_session_mode` Tauri command. Both the popover entry
  // (`ModeSelect.vue`) and the keyboard entry (`Shift+Tab` in
  // `ChatInput.vue` via `useKeyboard`) call this so the Yolo
  // confirm modal flow can live in exactly one place. The
  // component-side handlers (`ModeSelect.onModePick`,
  // `ChatInput.cycleMode`) just route here.
  //
  // We deliberately do NOT ship the Yolo confirm modal as a
  // store-managed thing — the modal is visual chrome and a
  // store shouldn't own a `<Teleport>` target. Instead, the
  // store exposes:
  //   - `pendingYoloConfirm`: a reactive boolean the modal
  //     mounts against (`v-if`).
  //   - `requestSetMode(sessionId, mode)`: the orchestrator
  //     that flips the Yolo gate for non-Chat modes and
  //     short-circuits when the gate is already open.
  //   - `confirmYolo()` / `cancelYolo()`: confirm / cancel the
  //     pending modal (the modal calls these on its buttons).
  //
  // `ModeSelect` reads `pendingYoloConfirm` to render the modal
  // (it owns the modal mount today; the store only holds the
  // boolean). `ChatInput`'s `cycleMode` calls `requestSetMode`
  // — the Yolo transition will surface in `ModeSelect`'s
  // mounted modal because both UIs share the same store state.
  // -----------------------------------------------------------------------

  /** True while the Yolo confirm modal should be mounted. Both
   *  UI entry points (`ModeSelect` popover + `ChatInput`
   *  Shift+Tab) flip this through `requestSetMode`. The modal
   *  is unmounted via `v-if` when this flips false. */
  const pendingYoloConfirm = ref(false);

  /** 2026-07-07 (`request_mode_change` task): when the
   *  inline `<RequestModeChangeCard>` opens the Yolo modal,
   *  it writes this ref BEFORE calling `requestSetMode`. The
   *  extended `confirmYolo(pendingResolveRequest)` /
   *  `cancelYolo(pendingResolveRequest)` read this ref after
   *  the Yolo IPC resolves and fire `resolveModeChange` for
   *  the agent loop's oneshot. `null` when no request_mode_change
   *  flow is in flight (the user-initiated Shift+Tab /
   *  ModeSelect popover paths don't set this — they have no
   *  agent-loop oneshot to unblock). */
  const pendingResolveRequest = ref<{
    sessionId: string;
    toolUseId: string;
    targetMode: "edit" | "plan" | "yolo";
  } | null>(null);

  /** Orchestrator for a mode change. The caller passes the
   *  target mode; this method handles the Yolo gate. Returns
   *  `true` if the mode was applied (or already current),
   *  `false` if the call was deferred to the modal. Errors
   *  propagate to the caller via the `invoke` throw.
   *
   *  No streaming guard — mode changes are accepted at any
   *  time and the backend persists them. The turn-boundary
   *  semantics ("applies on the next turn") live in
   *  `chat_loop.rs:396`, not here. Toast feedback for the
   *  "next-turn" UX hint is the caller's responsibility
   *  (see `ModeSelect.vue`). */
  async function requestSetMode(
    sessionId: string,
    mode: SessionMode,
  ): Promise<boolean> {
    if (!sessionId) return false;

    // No-op when the mode is already current. The optimistic
    // local update below is also a no-op, but we skip the IPC
    // round-trip to keep Shift+Tab snappy.
    const summary = sessions.value.find((s) => s.id === sessionId);
    if (summary && summary.mode === mode) return true;

    // Yolo always requires the confirm ceremony. We stage the
    // modal mount and let `confirmYolo` fire the IPC.
    if (mode === "yolo") {
      pendingYoloConfirm.value = true;
      return false;
    }

    // Non-Yolo mode: apply directly.
    try {
      await transport.invoke("set_session_mode", { sessionId, mode });
      if (summary) {
        (summary as { mode: string }).mode = mode;
      }
      return true;
    } catch (e) {
      console.error("Failed to update session mode:", e);
      return false;
    }
  }

  /** Called by `YoloConfirmModal`'s confirm button. Fires the
   *  pending IPC, optimistic-updates the session row, and
   *  closes the modal. Returns `true` on successful IPC + DB
   *  write, `false` on no-op (no session) or IPC failure.
   *  No streaming guard — matches `requestSetMode`'s contract
   *  that mode changes pass through unconditionally.
   *
   *  2026-07-07 (`request_mode_change` task): when invoked from
   *  the inline `<RequestModeChangeCard>` via
   *  `pendingResolveRequest` (set by the card before opening
   *  the modal), this method ALSO fires `resolveModeChange`
   *  after the Yolo IPC succeeds — the agent loop's oneshot is
   *  unblocked with `allow=true` so the LLM sees the
   *  `mode_change_allowed` audit + the `allowed: true` tool
   *  result. Without this hook the user-initiated Yolo path
   *  would update the DB but leave the agent loop frozen on
   *  the request_mode_change tool's oneshot — the LLM would
   *  never see the resolution. On IPC failure (Yolo root guard,
   *  DB error), the resolveModeChange is fired with `allow=false`
   *  so the user gets a `cancelled_by_user` tool result + a
   *  `mode_change_denied` audit (the cleaner outcome — the user
   *  was the one who tried to enable Yolo, the failure means
   *  Yolo can't be applied). */
  async function confirmYolo(
    pendingResolve?: {
      sessionId: string;
      toolUseId: string;
      targetMode: "edit" | "plan" | "yolo";
    },
  ): Promise<boolean> {
    pendingYoloConfirm.value = false;
    const sid = pendingResolve?.sessionId ?? currentSessionId.value;
    if (!sid) return false;
    // Clear the store ref up-front (before the IPC) — if the
    // resolve itself somehow surfaces the Yolo modal again
    // (it shouldn't, but defensive), we don't want a stale
    // hook to recurse. The `pendingResolve` parameter still
    // carries the data for the resolve call below.
    if (pendingResolve) {
      pendingResolveRequest.value = null;
    }
    try {
      await transport.invoke("set_session_mode", { sessionId: sid, mode: "yolo" });
      const summary = sessions.value.find((s) => s.id === sid);
      if (summary) {
        (summary as { mode: string }).mode = "yolo";
      }
    } catch (e) {
      console.error("Failed to confirm Yolo:", e);
      // 2026-08-18 (5df29977 问题4): surface the backend rejection.
      // Pre-fix this was console-only — the root guard's
      // "Cannot enable Yolo as root" left the user with a closed
      // modal, an unchanged chip, and zero feedback ("无法切换" with
      // no explanation). Lazy imports match questionCards.ts's
      // pattern (avoids a static store cycle at module-eval time).
      const { extractErrorMessage } = await import("../utils/useErrorBus");
      const { useProjectsStore } = await import("./projects");
      useProjectsStore().showToast(
        `Yolo 切换失败：${extractErrorMessage(e)}`,
        "error",
        5000,
      );
      // 2026-07-07: even on failure, unblock the agent loop
      // oneshot (with allow=false) so the LLM doesn't freeze
      // waiting. Only do this when we have a pending resolve
      // hook (the user-initiated path doesn't need it).
      if (pendingResolve) {
        try {
          // Lazy import to avoid a circular dependency with
          // `questionCards.ts` (which already imports
          // `useChatStore` lazily for the same reason — see
          // `resolveModeChange` in that file).
          const { useQuestionCardsStore } = await import(
            "./questionCards"
          );
          await useQuestionCardsStore().resolveModeChange(
            pendingResolve.sessionId,
            pendingResolve.toolUseId,
            pendingResolve.targetMode,
            false,
          );
        } catch (resolveErr) {
          // The resolve is best-effort: the DB write already
          // failed; surfacing a second error here would only
          // add noise. Log and move on.
          console.error(
            "Failed to resolve mode_change after Yolo IPC error:",
            resolveErr,
          );
        }
      }
      return false;
    }
    // Success path — fire resolveModeChange with allow=true to
    // unblock the agent loop oneshot. Only when we have the
    // pendingResolve (the user-initiated Shift+Tab path has
    // no request_mode_change oneshot to unblock).
    if (pendingResolve) {
      try {
        const { useQuestionCardsStore } = await import(
          "./questionCards"
        );
        await useQuestionCardsStore().resolveModeChange(
          pendingResolve.sessionId,
          pendingResolve.toolUseId,
          pendingResolve.targetMode,
          true,
        );
      } catch (e) {
        console.error(
          "Failed to resolve mode_change after Yolo IPC success:",
          e,
        );
        // Don't surface as toast — the Yolo IPC succeeded; the
        // resolve failure means the LLM will see the pending
        // interaction (eventually timed out or rejected). Log
        // for now; a follow-up can surface via the existing
        // pending-card re-mount path.
      }
    }
    return true;
  }

  /** Cancel the pending Yolo confirm — no mode change.
   *
   *  2026-07-07: when invoked from the inline
   *  `<RequestModeChangeCard>` via `pendingResolve`,
   *  fires `resolveModeChange(allow=false)` so the agent loop
   *  sees `cancelled_by_user` and the user sees
   *  `mode_change_denied` audit. The user-initiated path
   *  (`pendingResolve === undefined`) just closes the
   *  modal — there's no agent-loop oneshot to unblock. */
  async function cancelYolo(
    pendingResolve?: {
      sessionId: string;
      toolUseId: string;
      targetMode: "edit" | "plan" | "yolo";
    },
  ): Promise<void> {
    pendingYoloConfirm.value = false;
    if (!pendingResolve) return;
    // Clear the in-store ref so a subsequent user-initiated
    // Yolo switch (Shift+Tab / popover) doesn't see a stale
    // request_mode_change hook.
    pendingResolveRequest.value = null;
    try {
      const { useQuestionCardsStore } = await import("./questionCards");
      await useQuestionCardsStore().resolveModeChange(
        pendingResolve.sessionId,
        pendingResolve.toolUseId,
        pendingResolve.targetMode,
        false,
      );
    } catch (e) {
      console.error(
        "Failed to resolve mode_change after Yolo cancel:",
        e,
      );
    }
  }

  /** W1 (Workflow integration, Step 0.2 — 2026-07-08):
   *  per-session workflow opt-in toggle. Mirrors
   *  `requestSetMode`'s optimistic-update + IPC pattern:
   *
   *  1. **No-op fast path**: if the requested state matches the
   *     current `SessionSummary.workflow_enabled`, return
   *     `true` immediately without an IPC round-trip. Keeps the
   *     toggle snappy when a re-render accidentally fires a
   *     duplicate click (RULE-Front-Mode-001 symmetrical fix for
   *     the workflow chip).
   *
   *  2. **Optimistic update**: flip the local `SessionSummary`
   *     BEFORE awaiting the IPC so the chip lights up
   *     instantly. Same trade-off as `setSessionColor` and the
   *     pre-PR2 `setMode` path: a backend failure (DB locked /
   *     network blip) leaves the local state out-of-sync; we
   *     restore the prior value in the catch block below.
   *
   *  3. **No streaming guard**: matches `set_session_mode`'s
   *     contract — the flip applies on the next turn boundary
   *     (see `agent/chat_loop.rs:396`), so mid-stream calls
   *     are safe. The next turn's `build_instructions_blocks`
   *     rehydrates the workflow state from
   *     `SessionRow.workflow_enabled` (Phase 0 Step 0.5).
   *
   *  4. **No Yolo / root gate**: workflow toggling is a UI
   *     preference (NOT a privileged operation like `yolo`).
   *     Mirrors `setSessionColor`'s no-gate contract.
   *
   *  Returns `true` on success (or no-op), `false` on IPC
   *  failure. The caller (`PluginSelect.vue`, since the
   *  2026-07-09 chip-merge) does not surface a toast on
   *  `false` — the local state was already reverted in the
   *  catch block, so the chip + DB converge back to the
   *  prior state silently (matching the color-tag toggle
   *  UX). */
  async function requestSetWorkflowEnabled(
    sessionId: string,
    enabled: boolean,
  ): Promise<boolean> {
    if (!sessionId) return false;
    const summary = sessions.value.find((s) => s.id === sessionId);
    if (!summary) return false;
    if (summary.workflow_enabled === enabled) return true;

    // Snapshot prior value for rollback on IPC failure.
    const prior = summary.workflow_enabled;
    (summary as { workflow_enabled: boolean }).workflow_enabled = enabled;
    try {
      await transport.invoke("set_session_workflow_enabled", {
        sessionId,
        enabled,
      });
      return true;
    } catch (e) {
      // Restore the local state so the chip matches the DB.
      // We log + return false but don't toast — the
      // PluginSelect caller's UI feedback is the chip
      // returning to its prior color.
      console.error("Failed to update session workflow_enabled:", e);
      (summary as { workflow_enabled: boolean }).workflow_enabled = prior;
      return false;
    }
  }

  /** W1 (Workflow integration, Step 2.2 — 2026-07-08):
   *  per-session workflow plugin name flip. Mirrors
   *  `requestSetWorkflowEnabled`'s optimistic-update + IPC
   *  pattern:
   *
   *  1. **No-op fast path**: if the requested name matches
   *     the current `SessionSummary.plugin_name`, return
   *     `true` immediately (handles duplicate clicks).
   *
   *  2. **Optimistic update**: write the local
   *     `SessionSummary.plugin_name` BEFORE awaiting the
   *     IPC so the chip label flips instantly. On IPC
   *     failure (DB locked / network blip), restore the
   *     prior value in the catch block.
   *
   *  3. **No streaming guard**: matches
   *     `set_session_workflow_enabled`'s contract — the
   *     name flip applies on the next turn boundary (the
   *     next `build_workflow_ctx` call reads the persisted
   *     name from `SessionRow.plugin_name`). Mid-stream
   *     flips are safe; the new breadcrumb surfaces on the
   *     next turn.
   *
   *  4. **No validation here**: the backend
   *     `set_session_plugin_name` IPC rejects empty
   *     strings with an `AppCommandError` — we surface the
   *     rejection via `console.error` (matching
   *     `requestSetWorkflowEnabled`'s no-toast policy; the
   *     chip's UI feedback IS the rollback).
   *
   *  Returns `true` on success (or no-op), `false` on IPC
   *  failure. */
  async function requestSetPluginName(
    sessionId: string,
    name: string,
  ): Promise<boolean> {
    if (!sessionId) return false;
    const summary = sessions.value.find((s) => s.id === sessionId);
    if (!summary) return false;
    const trimmed = name.trim();
    if (!trimmed) return false;
    if (summary.plugin_name === trimmed) return true;

    const prior = summary.plugin_name;
    summary.plugin_name = trimmed;
    try {
      await transport.invoke("set_session_plugin_name", {
        sessionId,
        name: trimmed,
      });
      return true;
    } catch (e) {
      console.error("Failed to update session plugin_name:", e);
      summary.plugin_name = prior;
      return false;
    }
  }

  /** W1 (Workflow integration, Step 2.2 — 2026-07-08):
   *  discover available workflow plugins under
   *  `<project>/.everlasting/workflow/<dir>/workflow.json`.
   *  Thin wrapper around the `list_workflow_plugins` IPC.
   *
   *  Returns `[]` on IPC failure (frontend logs + falls
   *  back to the empty list — the `PluginSelect` chip
   *  still works with just the active plugin showing in
   *  the trigger; the popover simply has no entries to
   *  offer).
   *
   *  Callers should memoize on `project_id` (the project
   *  root only changes when the user switches active
   *  project, which is rare). The `PluginSelect.vue`
   *  component does the caching. */
  async function listWorkflowPlugins(projectPath: string): Promise<string[]> {
    try {
      const names = await transport.invoke<string[]>("list_workflow_plugins", {
        projectPath,
      });
      return Array.isArray(names) ? names : [];
    } catch (e) {
      console.error("list_workflow_plugins failed:", e);
      return [];
    }
  }

  return {
    pendingYoloConfirm,
    pendingResolveRequest,
    requestSetMode,
    confirmYolo,
    cancelYolo,
    requestSetWorkflowEnabled,
    requestSetPluginName,
    listWorkflowPlugins,
  };
}
