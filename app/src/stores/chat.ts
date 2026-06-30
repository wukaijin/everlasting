// chat.ts — UI-facing chat store.
//
// PR3 of `06-07-6-ui-bug-markdown-sse`: this file is now a thin
// facade over `streamController.ts`. The controller is the single
// source of truth for in-flight streams and per-session message
// buffers (see that file's top-of-file comment for the rationale).
// What remains here is:
//
//   - UI-side session metadata: the sessions list (sidebar
//     summaries), the active session id / cwd / simplified cwd.
//   - The project-change watcher (cascades `loadSessions` and
//     `ensureLoaded` on tab switch).
//   - Session CRUD delegations: `loadSessions`, `createNewSession`,
//     `switchSession`, `deleteSession`.
//   - `send` / `cancel` thin wrappers that build the wire-format
//     history and forward to the controller's request lifecycle.
//   - Reactive projections over controller state: `messages`,
//     `isCurrentSessionStreaming`, `currentRequestId` — the UI
//     only reads these, never the controller's raw state.
//   - The `useChatStore` factory itself.
//
// Public type/interface declarations (`ChatMessage`,
// `ErrorCategory`, `ThinkingBlockInfo`, `SessionSummary`,
// `SessionMode`, `FileDiff`, etc.) live in `./chat.types` —
// this file imports them back. Splitting the public contract
// into its own module keeps `chat.ts` focused on the store
// body (see PRD 06-23-06-23-split-chat-types).
//
// External API surface (consumed by components) is unchanged for
// `sessions`, `currentSessionId`, `currentCwd`, `simplifiedCwd`,
// `send`, `cancel`, `switchSession`, `createNewSession`,
// `loadSessions`, `deleteSession`. The old global `sending` is
// replaced by `isCurrentSessionStreaming` (per-session); callers
// were updated in the same PR.

import { defineStore } from "pinia";
import { computed, reactive, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

import { useProjectsStore } from "./projects";
import { useConfigStore } from "./config";
import { useStreamControllerStore } from "./streamController";
import { useChecklistStore } from "./checklist";
import { simplifyPath } from "../utils/path";
import {
  type ChatMessage,
  type DiffResult,
  type FileDiff,
  type LatencyInfo,
  type SessionMode,
  type SessionSummary,
  type SessionTokenUsage,
  type ThinkingBlockInfo,
} from "./chat.types";

type Role = "user" | "assistant";

/** Wire-format content sent to the Rust `chat` command. Mirrors
 *  Rust's `MessageContent`: a plain string for text-only messages,
 *  or an array of `ContentBlock` (snake_case tag + fields) when
 *  the message carries tool_use / tool_result / thinking /
 *  redacted_thinking blocks. */
type ContentBlockPayload =
  | { type: "text"; text: string }
  | { type: "thinking"; thinking: string; signature: string }
  | { type: "redacted_thinking"; data: string }
  | {
      type: "tool_use";
      id: string;
      name: string;
      input: Record<string, unknown>;
    }
  | {
      type: "tool_result";
      tool_use_id: string;
      content: string;
      is_error: boolean;
    };

interface ChatMessagePayload {
  role: Role;
  content: string | ContentBlockPayload[];
}

const genId = () =>
  Math.random().toString(36).slice(2) + Date.now().toString(36);

/** Concatenate the streamed summary text of all thinking blocks for
 *  display in the UI's thinking section. Newlines separate blocks so
 *  multiple blocks (interleaved thinking) read coherently. */
export function thinkingBlocksToText(blocks: ThinkingBlockInfo[] | undefined): string {
  if (!blocks || blocks.length === 0) return "";
  return blocks.map((b) => b.text).join("\n\n");
}

export const useChatStore = defineStore("chat", () => {
  // -----------------------------------------------------------------------
  // UI-side state (sessions list + active session metadata)
  // -----------------------------------------------------------------------

  const sessions = ref<SessionSummary[]>([]);
  const currentSessionId = ref<string | null>(null);
  const currentCwd = ref<string>("");

  // -----------------------------------------------------------------------
  // A4 (Token Usage Tracking): per-session running totals.
  //
  // The Map is keyed by session id; the value is the cumulative
  // token usage as of the most recent LLM turn Done event. The
  // data flow is:
  //
  //   Anthropic / OpenAI stream ends
  //     → ChatEvent::Done { usage: Some(t) }
  //     → streamController.handleChatEvent("done")
  //     → useChatStore().accumulateTokenUsage(sid, t)
  //     → tokenUsageBySession.get(sid) gets t added in place
  //     → currentSessionTokenUsage computed re-evaluates
  //     → ChatInput.vue re-renders the hint area
  //
  // The Map is also seeded from the `SessionSummary` returned by
  // `list_sessions` / `load_session` so a fresh page reload
  // shows the totals from the DB (the user sees the cumulative
  // value, not "—" + reset). Subsequent per-turn increments are
  // additive on top of the seeded totals.
  //
  // `null` (not `0`) for sessions that have never sent a turn —
  // the ChatInput hint renders this as "—" with the
  // "升级前未统计" tooltip.
  // -----------------------------------------------------------------------
  const tokenUsageBySession = reactive(
    new Map<string, SessionTokenUsage | null>(),
  );

  /** Reactive getter for the current session's running token
   *  totals. `null` when no session is active, or when the
   *  active session has not yet sent its first turn (pre-A4
   *  data or brand-new session). The ChatInput.vue hint area
   *  reads this; the threshold coloring is computed inline in
   *  the component (keeps the store API single-purpose). */
  const currentSessionTokenUsage = computed<SessionTokenUsage | null>(
    () => {
      const sid = currentSessionId.value;
      if (!sid) return null;
      return tokenUsageBySession.get(sid) ?? null;
    },
  );

  /** 2026-06-26 (token-usage snapshot fix): OVERWRITE the
   *  per-session last-turn usage snapshot with this turn's
   *  `usage`. Called by `streamController.handleChatEvent` on
   *  every `done` event that carries a `usage` payload. Snapshot
   *  semantics: the value reflects the LLM's LAST request (not a
   *  running total) — matching Anthropic's statusline convention. */
  function setLastTurnUsage(
    sessionId: string,
    usage: SessionTokenUsage,
  ): void {
    tokenUsageBySession.set(sessionId, { ...usage });
  }

  // -----------------------------------------------------------------------
  // F5 (LLM Latency Tracking): per-session cumulative latency.
  //
  // The Map is keyed by session id; the value is the running
  // total of `total_ms` across all assistant turns in the
  // session, displayed in the ChatPanel footer ("本次 session
  // LLM 累计耗时"). The data flow mirrors A4's token usage:
  //
  //   streamController.handleChatEvent("done")
  //     → compute { ttfbMs, genMs, totalMs } for the assistant turn
  //     → update_message_latency IPC to persist per-message columns
  //     → sessionTotalLatencyMs map += totalMs (cumulative)
  //
  // The Map is also seeded from `load_session` so a fresh page
  // reload shows the cumulative value (not "—"). The seed
  // sums `Σ total_ms WHERE role = 'assistant' AND total_ms IS
  // NOT NULL` — the controller does the sum during rehydrate
  // and hands the value to `accumulateLatency` via
  // `add-latency` (the per-message increments then stack on
  // top).
  //
  // The sessionTotalLatencyMs is also exposed as a
  // `currentSessionLatencyTotal` computed (mirroring
  // `currentSessionTokenUsage`) for the ChatPanel footer to
  // read.
  // -----------------------------------------------------------------------
  const sessionTotalLatencyMs = reactive(
    new Map<string, number>(),
  );

  /** Reactive getter for the current session's running latency
   *  total. `null` when no session is active OR when the
   *  active session has not yet recorded a `total_ms` value
   *  (pre-F5 data or brand-new session). The ChatPanel footer
   *  reads this; "—" is rendered for `null`. */
  const currentSessionLatencyTotal = computed<number | null>(() => {
    const sid = currentSessionId.value;
    if (!sid) return null;
    return sessionTotalLatencyMs.get(sid) ?? null;
  });

  /** Add a per-turn latency report to the running session
   *  total. Called by `streamController.handleChatEvent` on
   *  every `done` event that resolved a `totalMs`. A first
   *  call seeds the map (overwriting any prior seed value
   *  from rehydrate). Subsequent calls add. The caller is
   *  responsible for NOT firing this on cancel / error paths
   *  that have no `totalMs`. */
  function accumulateLatency(sessionId: string, totalMs: number): void {
    const existing = sessionTotalLatencyMs.get(sessionId);
    if (existing === undefined) {
      sessionTotalLatencyMs.set(sessionId, totalMs);
    } else {
      sessionTotalLatencyMs.set(sessionId, existing + totalMs);
    }
  }

  /** F5 follow-up: per-turn latency list for the active
   *  session, in chronological order (oldest first). The
   *  ChatInput popover renders this as a row-by-row breakdown
   *  (TTFB / Gen / Total per turn). Derived purely from the
   *  controller's in-memory messages — no separate Map needed,
   *  because the streaming `done` / `error` handler writes
   *  `latency` onto the assistant message in place, and
   *  rehydrated rows carry the values from `messages.total_ms`
   *  via `rehydrateMessages`. Returns `null` when no session
   *  is active; empty array when the session has messages
   *  but none of them recorded a latency (pre-F5 data, or a
   *  fresh session before its first turn). The render layer
   *  distinguishes "no session" (`null` → "—") from "no
   *  latency yet" (`[]` → "0.0s · 0 turns" / similar) so the
   *  user gets a stable label across the three states. */
  const currentSessionLatencyTurns = computed<LatencyInfo[] | null>(() => {
    const sid = currentSessionId.value;
    if (!sid) return null;
    const msgs = controller.getMessages(sid);
    if (!msgs) return [];
    const out: LatencyInfo[] = [];
    for (const m of msgs) {
      if (m.role !== "assistant") continue;
      if (!m.latency) continue;
      out.push(m.latency);
    }
    return out;
  });

  // -----------------------------------------------------------------------
  // Stream controller — single source of truth for messages + active
  // requests. Owned by a separate Pinia store; this file only projects
  // the controller's state into the shape the components expect.
  // -----------------------------------------------------------------------
  const controller = useStreamControllerStore();

  // F2: when true, auto-scroll follows every delta regardless of
  // user position. Set on send(), cleared on stream-done or when
  // the user manually scrolls up.
  const forceFollowActive = ref(false);

  // F4: true while switchSession is loading messages (IPC pending).
  const sessionLoading = ref(false);

  // F4: incremented after reloadAfterFinalize replaces messages, so
  // MessageList can re-scroll to bottom. The value is a counter, not a
  // boolean, to guarantee Vue detects the change.
  const scrollAfterReload = ref(0);

  // D3 PR2 (2026-06-17): the message seq currently in inline edit
  // mode (`null` = no row is being edited). Stored on the chat store
  // rather than as a local ref in MessageItem because (a) MessageList
  // remounts on session switch and would lose a local ref, and
  // (b) only one row can be in edit mode at a time, so a single
  // nullable scalar is the right shape. The MessageItem reads it as
  // a computed and the parent flips it via the
  // `<MessageActionsMenu>`'s `edit` emit. Cleared on Save success
  // (the IPC + refresh has finished) and on Cancel.
  const editingMessageSeq = ref<number | null>(null);

  // -----------------------------------------------------------------------
  // Reactive projections over the controller's state. Components read
  // these and never touch the controller directly.
  // -----------------------------------------------------------------------

  /** Messages for the currently active session. Touches the
   *  controller's LRU on every read so the active session stays MRU
   *  (and therefore won't be evicted mid-view). Returns `[]` when
   *  no session is active. The LRU side effect is the intended
   *  behavior — see `streamController.getMessages`. */
  const messages = computed<ChatMessage[]>(() => {
    const sid = currentSessionId.value;
    if (!sid) return [];
    return controller.getMessages(sid) ?? [];
  });

  /** True if the CURRENT session has an in-flight stream.
   *  Per-session independence (PR3 / bug 6): a stream in session A
   *  does not make this true while the user is looking at session
   *  B. Use the controller's `streamingSessionIds` directly for
   *  the full picture (e.g. session card streaming indicators in
   *  PR4).
   *
   *  Note: Pinia auto-unwraps refs/computeds when you read them
   *  off a store proxy, so `controller.streamingSessionIds` is
   *  the `Set<string>` itself (no `.value`). The reactive Set
   *  triggers our computed to re-run when the controller's
   *  `activeRequests` map changes. */
  const isCurrentSessionStreaming = computed<boolean>(() => {
    const sid = currentSessionId.value;
    if (!sid) return false;
    return controller.streamingSessionIds.has(sid);
  });

  /** The request id of the current session's active stream, or
   *  `null` if it isn't streaming. Replaces the old chat-store
   *  `currentRequestId` writable ref — the controller owns the
   *  actual request state, this is just a per-session lookup. */
  const currentRequestId = computed<string | null>(() => {
    const sid = currentSessionId.value;
    if (!sid) return null;
    return controller.currentRequestId(sid);
  });

  // PR3 (BACKLOG §5.1): the chat panel header displays the cwd with
  // the user's home prefix shortened to `~`. The computed is reactive
  // so when the home-dir cache finishes loading after the chat store
  // is first read, the UI re-renders without extra wiring. The
  // `configStore` reference is captured lazily — the computed body
  // only runs on first `.value` access, by which time the line
  // below has been initialized.
  const simplifiedCwd = computed<string>(() =>
    simplifyPath(currentCwd.value, configStore.homeDir),
  );

  // -----------------------------------------------------------------------
  // Cross-store coordination: react to project changes
  // -----------------------------------------------------------------------

  const projectsStore = useProjectsStore();
  const configStore = useConfigStore();

  watch(
    () => projectsStore.currentProjectId,
    async (newId) => {
      // Persist last-active project to localStorage. The config
      // store's own watcher writes to localStorage; we just update
      // its ref. Done here (not in the projects store) so the
      // persistence lives next to the read path (config.load) for
      // cohesion.
      configStore.lastActiveProjectId = newId;
      await onProjectChange(newId);
    },
    { immediate: true },
  );

  // PR3 self-check fix: the old `done` handler in chat.ts ran
  // `loadSessions(currentProjectId)` after each turn so the sidebar
  // would reflect the new `updated_at` / auto-generated title. With
  // the listener owned by the controller, that side effect moved
  // out of the event handler — but we still need it. Watch the
  // controller's `activeRequests.size` for any shrink (a request
  // ended via done or error) and refresh sessions for the project
  // the user is currently viewing. Cross-project case (stream
  // finishes in project A while user views B) is naturally covered
  // by `onProjectChange` reloading on next switch.
  watch(
    () => controller.activeRequests.size,
    (newSize, oldSize) => {
      if (newSize < oldSize && projectsStore.currentProjectId) {
        void loadSessions(projectsStore.currentProjectId);
      }
    },
  );

  async function onProjectChange(newId: string | null): Promise<void> {
    if (newId === null) {
      sessions.value = [];
      currentSessionId.value = null;
      currentCwd.value = "";
      return;
    }
    await loadSessions(newId);
    // 2026-06-26 snapshot fix: seed the per-session token usage
    // map from the SessionSummary's LAST-TURN snapshot (NOT the
    // legacy cumulative `*_total`). The判定 field is
    // `last_context_input_tokens` (the cross-provider-normalized
    // numerator) — if it's NULL, the session has no snapshot
    // (pre-snapshot legacy row or fresh session before first
    // turn) and the ChatInput hint renders "—".
    for (const s of sessions.value) {
      if (s.last_context_input_tokens !== null) {
        tokenUsageBySession.set(s.id, {
          input_tokens: s.last_input_tokens ?? 0,
          output_tokens: s.last_output_tokens ?? 0,
          cache_creation_input_tokens: s.last_cache_creation ?? 0,
          cache_read_input_tokens: s.last_cache_read ?? 0,
          context_input_tokens: s.last_context_input_tokens,
        });
      }
    }
    // Default to the most-recently-updated session if any exist;
    // otherwise leave the chat area in its empty state.
    if (sessions.value.length > 0) {
      // F1: prefer per-project last active session over sessions[0].
      const lastId = configStore.readLastSession(newId);
      const target =
        lastId && sessions.value.some((s) => s.id === lastId)
          ? sessions.value.find((s) => s.id === lastId)!
          : sessions.value[0];
      currentSessionId.value = target.id;
      currentCwd.value = target.current_cwd ?? "";
      // F1: persist the selected session as last active for this project.
      configStore.writeLastSession(newId, target.id);
      // Seed the controller's cache for the new active session so
      // the `messages` computed and the controller's per-session
      // event routing have something to look at on first render.
      await controller.ensureLoaded(target.id);
    } else {
      currentSessionId.value = null;
      currentCwd.value = "";
    }
  }

  // -----------------------------------------------------------------------
  // Session management
  // -----------------------------------------------------------------------

  async function loadSessions(projectId: string | null): Promise<void> {
    if (!projectId) {
      sessions.value = [];
      return;
    }
    sessions.value = await invoke<SessionSummary[]>("list_sessions", {
      projectId: projectId,
    });
  }

  /** Create a new session under the current project. Throws if no
   *  project is active — the caller (the chat area) is expected to
   *  be visible only when a project is selected (Q2 in dispatch
   *  prompt: the empty state hides the input, so send/create is
   *  unreachable from the UI). */
  async function createNewSession(): Promise<string> {
    const projectId = projectsStore.currentProjectId;
    if (!projectId) {
      throw new Error("createNewSession: no current project");
    }
    const project = projectsStore.projectById(projectId);
    const initialCwd = project?.path ?? "";
    const session = await invoke<{
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
    await invoke("delete_session", { sessionId });
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
    await invoke("clear_session_messages", { sessionId });
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
    await invoke("rename_session", { sessionId, newTitle });
    const s = sessions.value.find((x) => x.id === sessionId);
    if (s) s.title = newTitle.slice(0, 80);
  }

  async function setSessionColor(sessionId: string, colorTag: number | null) {
    await invoke("set_session_color", { sessionId, colorTag: colorTag });
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
      await invoke("attach_worktree", { sessionId });
    } catch (e) {
      projectsStore.showToast(`attach worktree 失败: ${String(e)}`, "error");
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
      const result = await invoke<string>("publish_session_to_main", { sessionId });
      projectsStore.showToast(result, "info");
    } catch (e) {
      projectsStore.showToast(`publish 到 main 失败: ${String(e)}`, "error");
      throw e;
    }
  }

  async function detachWorktree(sessionId: string): Promise<void> {
    try {
      await invoke("detach_worktree", { sessionId });
    } catch (e) {
      projectsStore.showToast(`detach worktree 失败: ${String(e)}`, "error");
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
      await invoke("delete_worktree", { sessionId });
    } catch (e) {
      projectsStore.showToast(`delete worktree 失败: ${String(e)}`, "error");
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

  // -----------------------------------------------------------------------
  // Diff (step 4 / PR3) — fetch and cache the session's worktree
  // diff. The IPC call is read-only and cheap (libgit2 walks the
  // tree, no remote I/O), but we still cache to avoid recomputing
  // for repeated clicks on the same session. The cache is keyed by
  // session id and is invalidated on session switch (so a stale
  // "diff from a different session" can't bleed through) and on
  // session delete.
  // -----------------------------------------------------------------------

  const diffCache = ref<Map<string, DiffResult>>(new Map());

  /** Reactive getter: cached diff for a session, or `null` if
   *  not yet fetched. Vue consumers should call `fetchDiff`
   *  first; this is just the read-side of the cache. */
  function getDiff(sessionId: string): DiffResult | null {
    return diffCache.value.get(sessionId) ?? null;
  }

  /** Fetch the session's worktree diff. Cached after the first
   *  call until the session is deleted. Errors propagate to the
   *  caller (the UI surfaces them in the popover). */
  async function fetchDiff(sessionId: string): Promise<DiffResult> {
    const cached = diffCache.value.get(sessionId);
    if (cached) {
      return cached;
    }
    const result = await invoke<DiffResult>("diff_worktree", { sessionId });
    diffCache.value.set(sessionId, result);
    // Force reactivity for the new Map reference (Pinia tracks
    // Map.set on the proxy but consumers reading `.get` want a
    // fresh snapshot).
    diffCache.value = new Map(diffCache.value);
    return result;
  }

  /** BUG FIX (06-08-06-08 step-4 follow-up — 2013 wire invariant):
   *  drop a single session's entry from the diff cache so the next
   *  reader (the worktree chip in `ChatPanel.vue` or a
   *  `diffWorktree` modal open) takes the cache-miss path and
   *  re-invokes the backend `diff_worktree` IPC. Called from
   *  `streamController.finalizeRequest` right after a `chat`
   *  request ends, so the worktree chip reflects post-send state
   *  (e.g. a `git commit` run inside the worktree drops the
   *  "diff (N)" counter immediately) instead of staying on the
   *  pre-send snapshot. The map replacement (`new Map(...)`) is
   *  the same reactivity trick `fetchDiff` uses — Vue tracks
   *  Map.set on the proxy but downstream `computed` consumers
   *  want a fresh reference. No-op if the session isn't cached.
   *
   *  Note: this does NOT touch `loadedFromDb` or the in-memory
   *  message buffer — that's `streamController.evict`, called in
   *  the same `finalizeRequest` so the two stay paired. */
  function invalidateDiff(sessionId: string): void {
    if (diffCache.value.has(sessionId)) {
      diffCache.value.delete(sessionId);
      diffCache.value = new Map(diffCache.value);
    }
  }

  /** Filter a session's diff down to a single file path. Returns
   *  `null` if the file isn't in the diff (either not changed in
   *  this session, OR the session diff hasn't been fetched yet). */
  function getFileDiff(sessionId: string, filePath: string): FileDiff | null {
    const result = diffCache.value.get(sessionId);
    if (!result) return null;
    return result.files.find((f) => f.path === filePath) ?? null;
  }

  // -----------------------------------------------------------------------
  // Send / Cancel
  // -----------------------------------------------------------------------

  /** Build the wire-format content for a history message: plain string
   *  for text-only / thinking-only messages, or an array of blocks when
   *  the turn carries tool_use / tool_result data. Backend's
   *  `MessageContent` deserializer accepts both shapes.
   *
   *  CRITICAL: thinking blocks (incl. signatures) and redacted_thinking
   *  data are emitted verbatim in their original streaming order. The
   *  Anthropic API requires the exact signature blob on the next turn —
   *  omitting or rewriting it produces 400. */
  function toPayloadContent(m: ChatMessage): string | ContentBlockPayload[] {
    // CRITICAL: tool_result blocks belong ONLY on user-role messages
    // (Anthropic Messages API contract). `rehydrateMessages` (in the
    // controller) attaches the following user message's tool_results
    // onto the assistant message *for UI grouping* (per-message "done /
    // running" lookup); here we MUST NOT echo them onto the wire when
    // role=assistant or Anthropic returns 2013 ("tool result's tool id
    // ... not found") because the assistant message itself isn't
    // allowed to contain tool_result blocks. Same for `content` text
    // emitted onto a ghost user message: only the assistant's text
    // counts.
    if (m.role === "assistant") {
      const hasTools = !!m.toolCalls?.length;
      const hasThinking =
        !!m.thinkingBlocks?.length || !!m.redactedThinkingData?.length;
      if (!hasTools && !hasThinking) {
        return m.content;
      }
      const blocks: ContentBlockPayload[] = [];
      // Thinking blocks come first (Anthropic convention: reasoning
      // before any visible text in the same turn).
      for (const tb of m.thinkingBlocks ?? []) {
        blocks.push({
          type: "thinking",
          thinking: tb.text,
          signature: tb.signature,
        });
      }
      if (m.content) {
        blocks.push({ type: "text", text: m.content });
      }
      for (const tc of m.toolCalls ?? []) {
        blocks.push({
          type: "tool_use",
          id: tc.id,
          name: tc.name,
          input: tc.input,
        });
      }
      for (const data of m.redactedThinkingData ?? []) {
        blocks.push({ type: "redacted_thinking", data });
      }
      // Intentionally omit `m.toolResults` — they're for the UI, not
      // the wire. The matching user-role message in the array
      // carries the canonical tool_result blocks.
      return blocks;
    }

    // user role: emit tool_result blocks + any text/thinking/redacted.
    // The rehydrated user message (formerly tool_result-only "ghost")
    // and the live user-typed message both pass through here.
    const hasTools = !!m.toolResults?.length;
    const hasThinking =
      !!m.thinkingBlocks?.length || !!m.redactedThinkingData?.length;
    if (!hasTools && !hasThinking) {
      return m.content;
    }
    const blocks: ContentBlockPayload[] = [];
    for (const tb of m.thinkingBlocks ?? []) {
      blocks.push({
        type: "thinking",
        thinking: tb.text,
        signature: tb.signature,
      });
    }
    if (m.content) {
      blocks.push({ type: "text", text: m.content });
    }
    for (const tr of m.toolResults ?? []) {
      blocks.push({
        type: "tool_result",
        tool_use_id: tr.toolUseId,
        content: tr.content,
        is_error: tr.isError,
      });
    }
    for (const data of m.redactedThinkingData ?? []) {
      blocks.push({ type: "redacted_thinking", data });
    }
    return blocks;
  }

  async function send(text: string) {
    const trimmed = text.trim();
    // Bug 6 fix (PR3): the old guard was a single global `sending`
    // ref. The new guard is per-session: the user can have multiple
    // sessions streaming concurrently, but they can't fire a second
    // message into the SAME session while it's still streaming.
    if (!trimmed || isCurrentSessionStreaming.value) return;
    const projectId = projectsStore.currentProjectId;
    if (!projectId) {
      throw new Error("send: no current project");
    }

    // explicit-agent-dispatch (2026-06-30): detect a `@@<agent>
    // <task>` prefix. When present, strip it from the user message
    // body (the body becomes the task) and thread a `forcedDispatch`
    // payload through the `chat` IPC so the backend short-circuits
    // the LLM and dispatches the named subagent directly. An unknown
    // agent name is NOT rejected here — the backend's `run_subagent`
    // surfaces it as an error tool_result (cache.lookup miss). An
    // empty task after the prefix is rejected (no dispatch without a
    // brief). Only one leading `@@` prefix is honored.
    let forcedDispatch: { subagent: string; task: string } | undefined;
    let body = trimmed;
    const atAt = trimmed.match(/^@@([A-Za-z0-9_-]+)[ \t]+([\s\S]+)$/);
    if (atAt) {
      const task = atAt[2].trim();
      if (!task) return;
      forcedDispatch = { subagent: atAt[1], task };
      body = task;
    }

    // Lazily create a session if there isn't one yet. `createNewSession`
    // throws if no project is active, so the chat area is expected
    // to be visible only when a project is selected (Q2 in dispatch
    // prompt: the empty state hides the input, so send/create is
    // unreachable from the UI).
    if (!currentSessionId.value) {
      await createNewSession();
    }
    // After createNewSession, `currentSessionId` is set; we
    // re-read in case the project's `last_cwd` is different from
    // the previous session's, etc.
    const sessionId = currentSessionId.value!;

    // Make sure the controller's cache has an entry for this
    // session (in case the user hits send immediately after a
    // project switch before `ensureLoaded` has run, or after a
    // long-idle eviction). `ensureLoaded` is a no-op for cached
    // sessions and an IPC call for evicted ones.
    const msgs = await controller.ensureLoaded(sessionId);

    // B12 Checklist (PR2 frontend, 2026-06-19): per-request
    // lifetime — a new user message starts a fresh run with a
    // fresh empty checklist. Mirror the backend's fresh
    // `Vec<ChecklistItem>` in each `run_chat_loop` invocation.
    // The controller's `reloadAfterFinalize` at the end of THIS
    // run will re-derive from history if any update_checklist
    // fires; for the duration of the stream the card stays
    // hidden until the first update_checklist tool_use arrives.
    useChecklistStore().clearForNewRun(sessionId);

    // B2 PR3 (bug fix 2026-06-17): compute the seq the
    // backend's `chat_loop` will assign to the user row.
    // The agent loop's `next_seq` counter starts at
    // `max(messages.seq) + 1` from `load_session` — the
    // same value we read off the rehydrated `msgs` here.
    // We stamp the user placeholder with this seq (and the
    // assistant placeholder with `nextSeq + 1`) so the
    // `ChatEvent::FileInjections` handler in
    // `streamController.ts` can locate the user message by
    // `m.seq === event.message_seq`. The rehydrated
    // messages all carry `seq` (set in
    // `rehydrateMessages` from `MessageRow.seq`); the
    // pre-stamping matters for the live path because the
    // freshly-pushed user/assistant placeholders are
    // not yet in the DB and so have no `seq` to read back.
    // Without this stamp, the live path silently drops
    // every `FileInjections` event.
    const nextSeq = msgs.reduce(
      (acc, m) => (typeof m.seq === "number" && m.seq > acc ? m.seq : acc),
      -1,
    ) + 1;

    // F2: activate force-follow mode so the chat stays scrolled to
    // bottom for the entire duration of the stream.
    forceFollowActive.value = true;

    const userMsg: ChatMessage = {
      id: genId(),
      // B2 PR3 (bug fix 2026-06-17): stamp the user message
      // with the seq the backend's `chat_loop` will assign.
      // The agent loop computes `next_seq = max(messages.seq)
      // + 1` from `load_session` at startup, and that value
      // is the seq the user row gets on `persist_turn`
      // (line 295 of `app/src-tauri/src/agent/chat_loop.rs`).
      // Without this, the `ChatEvent::FileInjections` handler
      // in `streamController.ts` does `msgs.find(m => m.role
      // === "user" && m.seq === event.message_seq)` and
      // NEVER finds the user message (its `seq` is undefined),
      // so the hint row under the user bubble never appears
      // during live streaming. Reload-after-DB-persist works
      // because `rehydrateMessages` reads `seq` from
      // `MessageRow.seq` and stamps it on every rehydrated
      // message — but the live path needs an explicit stamp
      // here.
      seq: nextSeq,
      role: "user",
      content: body,
    };
    const assistantMsg: ChatMessage = {
      id: genId(),
      // Assistant placeholder takes the next seq so
      // `case "turn_complete"` and `case "file_injections"`
      // both have a stable seq to key on. The agent loop
      // bumps seq after each `persist_turn` (user row →
      // assistant row → tool_result row), so the assistant
      // row seq is `userSeq + 1`.
      seq: nextSeq + 1,
      role: "assistant",
      content: "",
    };
    // The controller's event handlers look up `last` on this
    // array, so the assistant placeholder MUST be the final
    // entry before the stream starts. Pushing in this order also
    // matches the order the UI renders (user message first,
    // assistant placeholder right after).
    msgs.push(userMsg, assistantMsg);

    // Build history — keep tool_use / tool_result / thinking /
    // redacted_thinking blocks intact so the LLM has full context
    // across turns and across session switches. The agent loop
    // also constructs a matching assistant message from the
    // streaming events and persists it before the next LLM call,
    // so the history we send here will line up with what's in the
    // DB.
    const history: ChatMessagePayload[] = msgs
      .filter((m) => m.id !== assistantMsg.id)
      .map((m) => ({ role: m.role, content: toPayloadContent(m) }));

    // `startRequest` registers the active request, pins the session
    // in the LRU, and invokes the backend `chat` IPC. The
    // controller owns the listener, the request state, the
    // message routing, and the cleanup on `done` / `error` /
    // cancel. This call returns once the IPC completes (the
    // backend stream continues independently; events route back
    // via the global listener).
    await controller.startRequest({
      sessionId,
      projectId,
      userMsg,
      assistantMsg,
      history,
      forcedDispatch,
    });
  }

  /** PR5: cancel an in-flight chat request. The backend's agent
   *  loop notices on the next event boundary, bails out, persists
   *  whatever it has, and emits a `done` event with
   *  `stop_reason: "cancelled"`. That `done` flows through the
   *  controller's `handleChatEvent` → `finalizeRequest`, which
   *  clears the active request and unpins the session — so this
   *  call is fire-and-forget IPC; the actual state reset happens
   *  via the `done` event. */
  async function cancel() {
    const rid = currentRequestId.value;
    if (!rid) return;
    await controller.cancel(rid);
  }

  // -----------------------------------------------------------------------
  // D3 PR2 (2026-06-17): user message edit + cascade delete
  //
  // Mirrors the backend `edit_user_message` Tauri command (PR1,
  // commit `308d277`): in-place update the row's content, cascade-
  // delete every strictly-later message in the session, append an
  // audit row. The frontend flow is:
  //
  //   1. Cancel any in-flight stream on the session — the backend
  //      `edit_user_message` command also cancels as a defense in
  //      depth (cancel_inflight_for_session + await_inflight_exit),
  //      but doing it on the frontend too means the in-memory
  //      `streaming` flag on the placeholder message clears via
  //      the same `done` event path, and the user sees the input
  //      row's send button re-enable in the same tick.
  //   2. Fire the IPC. The backend's `Result<(), String>` becomes
  //      a JS rejection on failure (Tauri's IPC contract) — we
  //      let it propagate to the caller, which surfaces it via a
  //      toast and keeps the parent in edit mode for retry.
  //   3. Refresh the controller's per-session message buffer
  //      from the DB. `refresh` evicts + re-loads, so the
  //      rehydrated buffer shows the new content (the new
  //      `content` / `text` columns + the bumped
  //      `metadata.edited_at`) AND the trimmed tail (the cascade
  //      DELETE). The Vue computed `messages` re-evaluates and
  //      the <MessageList> re-renders.
  //
  // The `Resend` half is intentionally NOT wired in PR2. The
  // backend doesn't have a `Resend` IPC yet (needs a new
  // `ChatEvent::Resend` variant + an audit kind + the spec for
  // the cancel-vs-resend race), and the dispatch prompt's "DoD"
  // lists it under "留 PR3". The UI menu item stays disabled with
  // a "PR3 待实施" tooltip.
  //
  // Multi-listener safety: this method only mutates the
  // controller's per-session buffer (via `controller.refresh`),
  // never the `sessions` list directly, and never the
  // `currentSessionId` ref. The SessionList / project tab
  // subscribers see the title's `updated_at` advance via the
  // existing `controller.activeRequests.size` watcher (which
  // fires `loadSessions` on any shrink). Pinia deep proxies
  // mean no listener sees a "reset" event — the new content
  // lands in place on the reactive array.
  // -----------------------------------------------------------------------
  async function editMessage(
    sessionId: string,
    messageSeq: number,
    newContent: string,
  ): Promise<void> {
    if (!sessionId) {
      throw new Error("editMessage: sessionId is required");
    }
    if (typeof messageSeq !== "number") {
      throw new Error("editMessage: messageSeq is required");
    }
    if (typeof newContent !== "string") {
      throw new Error("editMessage: newContent must be a string");
    }
    // 1. Stream race — cancel any in-flight stream on this
    // session. The chat store's `cancel` is per-current-session
    // (it reads `currentRequestId`); for cross-session edits
    // (e.g. user edits a message in session A while session B is
    // streaming) we use the controller's lower-level `cancel`
    // with the resolved requestId. The current session's case
    // is the common one and goes through the existing wrapper.
    if (sessionId === currentSessionId.value && isCurrentSessionStreaming.value) {
      await cancel();
    } else {
      const rid = controller.currentRequestId(sessionId);
      if (rid) {
        await controller.cancel(rid);
      }
    }
    // 2. Fire the IPC. The backend takes `newContent` as a
    // plain string and wraps it in `MessageContent::Text`
    // (mirrors the wire shape the `chat` command's
    // `toPayloadContent` emits for a plain text message). The
    // Rust side serializes the new content to `messages.content`
    // (JSON `Vec<ContentBlock>` form) and the `text` denormalized
    // column. On error, the backend's `Result::Err(String)`
    // surfaces here as a rejected promise — we let it propagate
    // so the caller (`MessageItem.vue`'s Save handler) can
    // toast and keep the edit mode active.
    await invoke<void>("edit_user_message", {
      sessionId,
      messageSeq,
      newContent,
    });
    // 3. Refresh the per-session message buffer. We always
    // refresh, even if the user is currently viewing a
    // different session — the rehydrated buffer lives in the
    // controller's LRU keyed by sessionId and will surface
    // correctly when the user navigates back. The `refresh`
    // helper does evict + ensureLoaded, so the new content +
    // trimmed tail are read from the DB and the in-memory
    // `messagesBySession` is replaced atomically (no blank
    // page flash — see the BUG FIX comment in
    // `finalizeRequest` for the same invariant).
    await controller.refresh(sessionId);
  }

  // -----------------------------------------------------------------------
  // D3 PR3 (2026-06-17): user message Resend — re-fire the
  // existing user prompt (no content mutation) by re-calling
  // `chat` with the same messages payload + a `resendSeq`
  // flag pointing at the original user message's seq. The
  // backend's agent loop detects the flag and writes a
  // `resend_message` audit row at the user-message persist
  // site (best-effort; see `app/src-tauri/src/agent/chat.rs`
  // `chat` command signature).
  //
  // Diff vs `editMessage`:
  // - No IPC `edit_user_message` call — content is unchanged.
  // - `chat` IPC receives an extra `resendSeq` parameter (the
  //   seq the user clicked Resend on). Backend audit fires at
  //   persist site; otherwise the request is identical to a
  //   normal send.
  // - No `controller.refresh` — the in-flight stream will
  //   stream into the same placeholder, and `finalizeRequest`
  //   will evict the buffer + `load_session` rehydrates
  //   including the (newly created) re-sent user message row.
  //
  // Stream race: same as `editMessage` — cancel any in-flight
  // stream first. The cancel order matters: the user clicks
  // Resend, we cancel the old stream, then we re-fire chat.
  // If the user clicks Resend twice in quick succession, the
  // second click cancels the first Resend's stream (which is
  // mid-flight) and starts yet another — the second
  // `resend_message` audit row will overwrite the first one's
  // role (the latest is the only one the user sees anyway).
  // -----------------------------------------------------------------------
  async function resendMessage(
    sessionId: string,
    messageSeq: number,
    contentText: string,
  ): Promise<void> {
    if (!sessionId) {
      throw new Error("resendMessage: sessionId is required");
    }
    if (typeof messageSeq !== "number") {
      throw new Error("resendMessage: messageSeq is required");
    }
    if (typeof contentText !== "string") {
      throw new Error("resendMessage: contentText must be a string");
    }
    // 1. Stream race — cancel any in-flight stream on this
    // session, mirroring `editMessage`'s defensive pattern.
    if (sessionId === currentSessionId.value && isCurrentSessionStreaming.value) {
      await cancel();
    } else {
      const rid = controller.currentRequestId(sessionId);
      if (rid) {
        await controller.cancel(rid);
      }
    }
    // 2. Re-fire `chat` with the same messages payload + a
    // `resendSeq` flag. We mirror `send()`'s placeholder
    // construction (push a fresh userMsg + assistantMsg so the
    // controller's `case "delta"` finds the assistant message
    // to mutate), but the `resendSeq` flag tells the backend
    // this is a re-fire (audit at the user-message persist
    // site). The user message content is identical to the
    // original — we're re-running the same prompt.
    const projectId = projectsStore.currentProjectId;
    if (!projectId) {
      throw new Error("resendMessage: no current project");
    }
    const msgs = await controller.ensureLoaded(sessionId);
    // B12 Checklist: per-request lifetime — resend starts a
    // fresh run; drop any prior checklist state. The new run's
    // first update_checklist will repopulate the card.
    useChecklistStore().clearForNewRun(sessionId);
    // Compute next seq for the new placeholders, same logic
    // as `send()`. The agent loop will use `max(loaded.seq)
    // + 1` for the actual persist, but we stamp the
    // in-memory placeholder so the controller's
    // `FileInjections` / `TurnComplete` events can key on it.
    const nextSeq = msgs.reduce(
      (acc: number, m: ChatMessage) =>
        typeof m.seq === "number" && m.seq > acc ? m.seq : acc,
      -1,
    ) + 1;
    forceFollowActive.value = true;
    const userMsg: ChatMessage = {
      id: genId(),
      seq: nextSeq,
      role: "user",
      content: contentText,
    };
    const assistantMsg: ChatMessage = {
      id: genId(),
      seq: nextSeq + 1,
      role: "assistant",
      content: "",
    };
    msgs.push(userMsg, assistantMsg);
    const history: ChatMessagePayload[] = msgs
      .filter((m) => m.id !== assistantMsg.id)
      .map((m) => ({ role: m.role, content: toPayloadContent(m) }));
    // 3. Start the request with the `resendSeq` flag. Backend
    // audit fires at user-message persist site; otherwise the
    // request is identical to a normal send.
    await controller.startRequest({
      sessionId,
      projectId,
      userMsg,
      assistantMsg,
      history,
      // D3 PR3 (2026-06-17): mark this request as a resend
      // of the original user message at `messageSeq`. The
      // backend's agent loop reads this and writes a
      // `resend_message` audit row at the user-message
      // persist site (best-effort).
      resendSeq: messageSeq,
    });
  }

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
      await invoke("set_session_mode", { sessionId, mode });
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
   *  that mode changes pass through unconditionally. */
  async function confirmYolo(): Promise<boolean> {
    pendingYoloConfirm.value = false;
    const sid = currentSessionId.value;
    if (!sid) return false;
    try {
      await invoke("set_session_mode", { sessionId: sid, mode: "yolo" });
      const summary = sessions.value.find((s) => s.id === sid);
      if (summary) {
        (summary as { mode: string }).mode = "yolo";
      }
      return true;
    } catch (e) {
      console.error("Failed to confirm Yolo:", e);
      return false;
    }
  }

  /** Cancel the pending Yolo confirm — no mode change. */
  function cancelYolo(): void {
    pendingYoloConfirm.value = false;
  }

  return {
    // Reactive state (computed projections)
    messages,
    isCurrentSessionStreaming,
    currentRequestId,
    // A4: per-session running token totals. The ChatInput
    // hint area reads `currentSessionTokenUsage`; the Map is
    // exposed for tests / future per-session UIs.
    currentSessionTokenUsage,
    tokenUsageBySession,
    // F5: per-session running latency total. The ChatPanel
    // footer reads `currentSessionLatencyTotal`; the Map is
    // exposed for tests.
    currentSessionLatencyTotal,
    sessionTotalLatencyMs,
    // F5 follow-up: per-turn latency list for the popover
    // breakdown. Derived from the controller's in-memory
    // messages (no separate Map — see the computed's doc
    // comment for the rationale). `null` when no session
    // is active; `[]` when the active session has no
    // latency data yet.
    currentSessionLatencyTurns,
    // UI-side state (refs)
    sessions,
    currentSessionId,
    currentCwd,
    simplifiedCwd,
    diffCache,
    // F2/F4: scroll follow mode + session loading
    forceFollowActive,
    sessionLoading,
    scrollAfterReload,
    // D3 PR2: the message seq currently in inline edit mode.
    // Written by `<MessageActionsMenu>`'s `edit` emit, cleared
    // on Save success / Cancel. UI consumers read it via
    // `chatStore.editingMessageSeq` (a `number | null`).
    editingMessageSeq,
    // Methods
    send,
    cancel,
    loadSessions,
    createNewSession,
    switchSession,
    deleteSession,
    // B3 (PR2): `/clear` — wipe messages, keep session row.
    clearSessionMessages,
    renameSession,
    setSessionColor,
    attachWorktree,
    detachWorktree,
    publishSessionToMain,
    deleteWorktree,
    fetchDiff,
    getDiff,
    getFileDiff,
    invalidateDiff,
    // 2026-06-26 snapshot fix: hook called by
    // streamController.handleChatEvent on every `done` event
    // that carries a usage payload. OVERWRITES the per-session
    // last-turn snapshot.
    setLastTurnUsage,
    // F5: hook called by streamController.handleChatEvent on
    // every `done` event that resolved a `totalMs`. Adds the
    // per-turn `totalMs` to the running session total.
    accumulateLatency,
    // A2 + B7 (PR2): per-session Mode setters. The Yolo gate
    // is held in `pendingYoloConfirm` and consumed by the
    // YoloConfirmModal mounted by `ModeSelect.vue`.
    pendingYoloConfirm,
    requestSetMode,
    confirmYolo,
    cancelYolo,
    // D3 PR2 (2026-06-17): user message edit + cascade delete
    // bridge to the backend `edit_user_message` IPC. Called by
    // `MessageItem.vue`'s Save handler; the parent catches
    // errors and keeps the edit mode active for retry.
    editMessage,
    // D3 PR3 (2026-06-17): re-fire an existing user message
    // (no content mutation). Called by `MessageActionsMenu`'s
    // `resend` emit; `MessageItem.vue` builds the
    // `contentText` from `message.content` and the chat
    // store fires the new stream with the `resendSeq` flag.
    resendMessage,
  };
});
