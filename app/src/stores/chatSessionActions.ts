// chatSessionActions — session CRUD + worktree actions(拆分自 chat.ts,
// 08-10-chat-store-split)。
//
// 13 个 action 原样搬迁为 `createSessionActions(ctx)` 工厂;ctx 注入共享
// state(refs / controller / configStore)。拆分契约见
// `.trellis/spec/frontend/state-management.md` §Stream Controller Pattern。
import type { ComputedRef, Ref } from "vue";
import { transport } from "../transport";
import { extractErrorMessage } from "../utils/useErrorBus";
import { useChecklistStore } from "./checklist";
import type { useStreamControllerStore } from "./streamController";
import type { useProjectsStore } from "./projects";
import type { useConfigStore } from "./config";
import type { DiffResult, ParticipantConfig, SessionSummary } from "./chat.types";

export interface SessionActionsContext {
  sessions: Ref<SessionSummary[]>;
  currentSessionId: Ref<string | null>;
  currentCwd: Ref<string>;
  sessionLoading: Ref<boolean>;
  diffCache: Ref<Map<string, DiffResult>>;
  isCurrentSessionStreaming: ComputedRef<boolean>;
  controller: ReturnType<typeof useStreamControllerStore>;
  projectsStore: ReturnType<typeof useProjectsStore>;
  configStore: ReturnType<typeof useConfigStore>;
  cancel: () => Promise<void>;
}

export function createSessionActions(ctx: SessionActionsContext) {
  const {
    sessions,
    currentSessionId,
    currentCwd,
    sessionLoading,
    diffCache,
    isCurrentSessionStreaming,
    controller,
    projectsStore,
    configStore,
    cancel,
  } = ctx;

  async function loadSessions(projectId: string | null): Promise<void> {
    if (!projectId) {
      sessions.value = [];
      return;
    }
    sessions.value = await transport.invoke<SessionSummary[]>("list_sessions", {
      projectId: projectId,
    });
  }

  /** Create a new session under the current project. Throws if no
   *  project is active — the caller (the chat area) is expected to
   *  be visible only when a project is selected (Q2 in dispatch
   *  prompt: the empty state hides the input, so send/create is
   *  unreachable from the UI).
   *
   *  Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E1):
   *  optional `opts` carries the session-type discriminator + the
   *  participant roster for group-chat sessions. Server-side:
   *  - `session_type`: `"chat"` (default) or `"group_chat"`
   *  - `metadata`: JSON blob with `{participants: [...]}` for
   *    group-chat sessions; `null` for classic chat.
   *  Both are optional — the existing callers (the "new session"
   *  button + the auto-create flow) get classic chat semantics
   *  (no behavior change). The `GroupChatConfigModal` is the only
   *  caller that sets `sessionType: "group_chat"` + `participants`. */
  async function createNewSession(
    opts: {
      sessionType?: "chat" | "group_chat";
      participants?: ParticipantConfig[];
    } = {},
  ): Promise<string> {
    const projectId = projectsStore.currentProjectId;
    if (!projectId) {
      throw new Error("createNewSession: no current project");
    }
    const project = projectsStore.projectById(projectId);
    const initialCwd = project?.path ?? "";
    // Group chat (Phase 4 Step 3): attach the session_type +
    // metadata only when the caller opted in. Classic chat uses
    // the same wire shape but with both fields undefined →
    // server-side defaults to `chat` + NULL metadata.
    const sessionType = opts.sessionType;
    const metadata =
      opts.sessionType === "group_chat" && opts.participants
        ? { participants: opts.participants }
        : null;
    const session = await transport.invoke<{
      id: string;
      title: string;
      created_at: string;
      updated_at: string;
      model: string;
      project_id: string;
      current_cwd: string;
    }>("create_session", {
      projectId: projectId,
      initialCwd: initialCwd,
      sessionType,
      metadata,
    });
    currentSessionId.value = session.id;
    currentCwd.value = session.current_cwd ?? "";
    // Seed the controller's cache with an empty buffer for the new
    // session. `ensureLoaded` will do an IPC `load_session` call
    // (returning an empty message list for a fresh session) — the
    // only public way to put a value into the controller's LRU.
    await controller.ensureLoaded(session.id);
    await loadSessions(projectId);
    return session.id;
  }

  /** Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E5):
   *  overwrite the participant roster on an existing group-chat
   *  session. Calls `update_session_metadata` IPC (REST mirror
   *  `PATCH /api/v1/sessions/:id/metadata`). The caller
   *  (`GroupChatConfigModal` edit mode) is responsible for
   *  validating the new roster before invoking this. The
   *  session_list is refreshed so the SessionList re-renders
   *  with the new metadata.
   *  Refuses to call on a non-group-chat session (the server
   *  accepts any JSON, but the result would be meaningless
   *  if applied to a classic chat session). */
  async function updateGroupChatConfig(
    sessionId: string,
    participants: ParticipantConfig[],
  ): Promise<void> {
    const summary = sessions.value.find((s) => s.id === sessionId);
    if (summary && summary.session_type !== "group_chat") {
      throw new Error(
        `updateGroupChatConfig: session ${sessionId} is not a group_chat session`,
      );
    }
    await transport.invoke("update_session_metadata", {
      sessionId,
      metadata: { participants },
    });
    // Refresh the session list so the SessionList re-renders
    // with the new metadata. `loadSessions` is the existing
    // façade; the controller's LRU is untouched (the messages
    // themselves are unchanged).
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
  }

  async function switchSession(sessionId: string) {
    // Per-session independence (PR3 / bug 6 fix): switching
    // sessions mid-stream is now a first-class operation. The
    // in-flight request keeps running on the backend; the
    // controller's listener routes events to the matching
    // `request_id` regardless of the user's current view. When
    // the user returns to the streaming session, the
    // `messages` computed re-evaluates and the in-flight
    // message is right there — no DB reload, no `done`-event
    // loss.
    //
    // F4: set loading state for spinner display. Cleared after
    // ensureLoaded completes.
    sessionLoading.value = true;
    try {
      await controller.ensureLoaded(sessionId);
      currentSessionId.value = sessionId;
      // F1: persist per-project last active session.
      if (projectsStore.currentProjectId) {
        configStore.writeLastSession(
          projectsStore.currentProjectId,
          sessionId,
        );
      }
      // Pull cwd from the session summary (the controller doesn't
      // expose session metadata; `list_sessions` already has the
    // value in memory). Avoids a redundant `load_session` IPC.
      const summary = sessions.value.find((s) => s.id === sessionId);
      currentCwd.value = summary?.current_cwd ?? "";
    } finally {
      sessionLoading.value = false;
    }
  }

  async function deleteSession(sessionId: string) {
    await transport.invoke("delete_session", { sessionId });
    // Evict from the controller's cache (and unpin, just in case)
    // so the in-memory buffer doesn't keep a stale entry alive
    // past the DB row's deletion.
    controller.evict(sessionId);
    // Drop any cached diff for this session — the worktree it
    // referenced is now gone, so the diff is meaningless.
    diffCache.value.delete(sessionId);
    // B12 Checklist (PR2 frontend, 2026-06-19): drop the
    // session's checklist state too. The store's per-session
    // map would otherwise retain the entry past the DB row.
    useChecklistStore().clearSession(sessionId);
    if (currentSessionId.value === sessionId) {
      currentSessionId.value = null;
      currentCwd.value = "";
    }
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
  }

  /** B3 `/clear` (PR2): clear all messages from the active session
   *  **but keep the session row** (title / color / mode / model /
   *  project / created_at all survive). Mirrors the backend's
   *  `clear_session_messages` Tauri command — `DELETE FROM messages
   *  WHERE session_id = ?` + audit log. The session continues to
   *  be the current session (no `switchSession` churn).
   *
   *  Side effects (in order):
   *  1. If a stream is in-flight on this session, cancel it first —
   *     otherwise the in-flight turn would re-persist a message
   *     *after* we wiped the table, undoing the clear.
   *  2. Fire the IPC. The DB rows are gone; the audit row records
   *     the clear.
   *  3. Evict the controller's in-memory buffer + re-seed an empty
   *     one via `ensureLoaded` so the UI re-renders blank without a
   *     flash of stale content. We use `evict` + `ensureLoaded`
   *     (NOT `refresh`) because the worktree baseline is unchanged —
   *     no system event was injected.
   *  4. Drop the diff cache (the cleared messages had ToolCallCards
   *     that may have referenced a now-irrelevant diff).
   *
   *  No-op when no session is active. Throws surface to the caller
   *  (the caller currently logs to console; a future toast hook
   *  could surface IPC failures). */
  async function clearSessionMessages(sessionId: string): Promise<void> {
    // Cancel any in-flight stream first. `cancel` is fire-and-forget
    // IPC (the `done` event does the state reset); we await a short
    // tick so the backend has flushed the cancel before we wipe the
    // DB. The `done` event for the cancelled request will arrive
    // after our evict, but `evict` already removed the session from
    // `activeRequests`'s pinning, and the controller's
    // `finalizeRequest` is a no-op on an evicted session.
    if (sessionId === currentSessionId.value && isCurrentSessionStreaming.value) {
      await cancel();
    }
    await transport.invoke("clear_session_messages", { sessionId });
    controller.evict(sessionId);
    diffCache.value.delete(sessionId);
    // B12 Checklist: the cleared session has no history → no
    // committed checklist. Drop the live state so the card
    // hides until the next update_checklist fires.
    useChecklistStore().clearSession(sessionId);
    // Re-seed an empty buffer so the UI re-renders immediately.
    // `ensureLoaded` will hit the (now empty) DB and produce `[]`.
    if (sessionId === currentSessionId.value) {
      await controller.ensureLoaded(sessionId);
    }
  }

  // D1: rename + color tag
  async function renameSession(sessionId: string, newTitle: string) {
    await transport.invoke("rename_session", { sessionId, newTitle });
    const s = sessions.value.find((x) => x.id === sessionId);
    if (s) s.title = newTitle.slice(0, 80);
  }

  async function setSessionColor(sessionId: string, colorTag: number | null) {
    await transport.invoke("set_session_color", { sessionId, colorTag: colorTag });
    const s = sessions.value.find((x) => x.id === sessionId);
    if (s) s.color_tag = colorTag;
  }

  // -----------------------------------------------------------------------
  // Step 4 follow-up: opt-in worktree actions
  //
  // Three Tauri commands, three Pinia actions. Each one (a) calls
  // the backend, (b) invalidates the local diff cache for the
  // session (the on-disk state has changed), and (c) refreshes the
  // sessions list so the sidebar chip updates. Errors are surfaced
  // via `projectsStore.showToast` so the user sees a single
  // consistent error path.
  // -----------------------------------------------------------------------

  async function attachWorktree(sessionId: string): Promise<void> {
    try {
      await transport.invoke("attach_worktree", { sessionId });
    } catch (e) {
      projectsStore.showToast(`attach worktree 失败: ${extractErrorMessage(e)}`, "error");
      throw e;
    }
    // Invalidate cached diff (the on-disk worktree is now
    // different from the session baseline) and refresh the list.
    diffCache.value.delete(sessionId);
    if (currentSessionId.value === sessionId) {
      // Re-load messages from the DB so the system event the
      // backend just inserted (REQ-17) is in the cache. The
      // next `send()` builds history from the cache; without
      // this refresh the LLM would not see the worktree
      // transition event.
      await controller.refresh(sessionId);
    }
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
  }

  // D (2026-06-30): publish the session's `session/<id>` branch into
  // `main` (local only — never pushes). Surfaces the chat-header
  // "Publish → main" button. On success main advances; the session
  // worktree stays bound so the user can keep working.
  async function publishSessionToMain(sessionId: string): Promise<void> {
    try {
      const result = await transport.invoke<string>("publish_session_to_main", { sessionId });
      projectsStore.showToast(result, "info");
    } catch (e) {
      projectsStore.showToast(`publish 到 main 失败: ${extractErrorMessage(e)}`, "error");
      throw e;
    }
  }

  async function detachWorktree(sessionId: string): Promise<void> {
    try {
      await transport.invoke("detach_worktree", { sessionId });
    } catch (e) {
      projectsStore.showToast(`detach worktree 失败: ${extractErrorMessage(e)}`, "error");
      throw e;
    }
    diffCache.value.delete(sessionId);
    if (currentSessionId.value === sessionId) {
      // Re-fetch the session metadata + messages so currentCwd,
      // the session's new state, and the system event the
      // backend just injected are all visible immediately. Use
      // `refresh` (not `ensureLoaded`) so the cache picks up
      // the new system event row.
      await controller.refresh(sessionId);
    }
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
  }

  async function deleteWorktree(sessionId: string): Promise<void> {
    try {
      await transport.invoke("delete_worktree", { sessionId });
    } catch (e) {
      projectsStore.showToast(`delete worktree 失败: ${extractErrorMessage(e)}`, "error");
      throw e;
    }
    diffCache.value.delete(sessionId);
    if (currentSessionId.value === sessionId) {
      await controller.refresh(sessionId);
    }
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
  }

  return {
    loadSessions,
    createNewSession,
    updateGroupChatConfig,
    switchSession,
    deleteSession,
    clearSessionMessages,
    renameSession,
    setSessionColor,
    attachWorktree,
    detachWorktree,
    publishSessionToMain,
    deleteWorktree,
  };
}
