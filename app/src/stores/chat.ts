// chat.ts — UI-facing chat store.
//
// PR3 of `06-07-6-ui-bug-markdown-sse`: this file is now a thin
// facade over `streamController.ts`. The controller is the single
// source of truth for in-flight streams and per-session message
// buffers (see that file's top-of-file comment for the rationale).
// What remains here is:
//
//   - Type definitions re-exported for the rest of the app
//     (`ChatMessage`, `ErrorCategory`, `ThinkingBlockInfo`, ...).
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
//
// External API surface (consumed by components) is unchanged for
// `sessions`, `currentSessionId`, `currentCwd`, `simplifiedCwd`,
// `send`, `cancel`, `switchSession`, `createNewSession`,
// `loadSessions`, `deleteSession`. The old global `sending` is
// replaced by `isCurrentSessionStreaming` (per-session); callers
// were updated in the same PR.

import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

import { useProjectsStore } from "./projects";
import { useConfigStore } from "./config";
import { useStreamControllerStore } from "./streamController";
import { simplifyPath } from "../utils/path";

type Role = "user" | "assistant";
export type ErrorCategory =
  | "auth"
  | "rate_limit"
  | "invalid_request"
  | "server"
  | "network";

/** Tool call info displayed in the UI. */
export interface ToolCallInfo {
  id: string;
  name: string;
  input: Record<string, unknown>;
}

/** Tool result info displayed in the UI. */
export interface ToolResultInfo {
  toolUseId: string;
  content: string;
  isError: boolean;
}

/** One thinking content block. The model can produce multiple blocks per
 *  turn (interleaved thinking with tool calls); each must be preserved
 *  in order and round-tripped back to the LLM verbatim, otherwise the
 *  next turn 400s. `text` is the streamed summary (or empty under
 *  `display: "omitted"`); `signature` is the opaque, encrypted blob. */
export interface ThinkingBlockInfo {
  text: string;
  signature: string;
}

/** Chat message with optional tool call/result/thinking metadata. */
export interface ChatMessage {
  id: string;
  role: Role;
  content: string; // accumulated text content
  streaming?: boolean;
  error?: { message: string; category: ErrorCategory };
  toolCalls?: ToolCallInfo[];
  toolResults?: ToolResultInfo[];
  /** All thinking blocks emitted by the model for this message, in
   *  streaming order. Empty/missing for messages without thinking. */
  thinkingBlocks?: ThinkingBlockInfo[];
  /** Each entry is the opaque `data` payload of a `redacted_thinking`
   *  block — preserved verbatim for round-trip, never displayed. */
  redactedThinkingData?: string[];
}

/** Session summary shown in the sidebar. Snake_case to match PR1's
 *  Rust serialization (no `#[serde(rename_all = "camelCase")]`). */
export interface SessionSummary {
  id: string;
  title: string;
  updated_at: string;
  preview: string;
  project_id: string;
  current_cwd: string;
}

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
  // Stream controller — single source of truth for messages + active
  // requests. Owned by a separate Pinia store; this file only projects
  // the controller's state into the shape the components expect.
  // -----------------------------------------------------------------------
  const controller = useStreamControllerStore();

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
    // Default to the most-recently-updated session if any exist;
    // otherwise leave the chat area in its empty state.
    if (sessions.value.length > 0) {
      const first = sessions.value[0];
      currentSessionId.value = first.id;
      currentCwd.value = first.current_cwd ?? "";
      // Seed the controller's cache for the new active session so
      // the `messages` computed and the controller's per-session
      // event routing have something to look at on first render.
      await controller.ensureLoaded(first.id);
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
    await controller.ensureLoaded(sessionId);
    currentSessionId.value = sessionId;
    // Pull cwd from the session summary (the controller doesn't
    // expose session metadata; `list_sessions` already has the
    // value in memory). Avoids a redundant `load_session` IPC.
    const summary = sessions.value.find((s) => s.id === sessionId);
    currentCwd.value = summary?.current_cwd ?? "";
  }

  async function deleteSession(sessionId: string) {
    await invoke("delete_session", { sessionId });
    // Evict from the controller's cache (and unpin, just in case)
    // so the in-memory buffer doesn't keep a stale entry alive
    // past the DB row's deletion.
    controller.evict(sessionId);
    if (currentSessionId.value === sessionId) {
      currentSessionId.value = null;
      currentCwd.value = "";
    }
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
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

    const userMsg: ChatMessage = {
      id: genId(),
      role: "user",
      content: trimmed,
    };
    const assistantMsg: ChatMessage = {
      id: genId(),
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

  return {
    // Reactive state (computed projections)
    messages,
    isCurrentSessionStreaming,
    currentRequestId,
    // UI-side state (refs)
    sessions,
    currentSessionId,
    currentCwd,
    simplifiedCwd,
    // Methods
    send,
    cancel,
    loadSessions,
    createNewSession,
    switchSession,
    deleteSession,
  };
});
