import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { useProjectsStore } from "./projects";
import { useConfigStore } from "./config";

type Role = "user" | "assistant";
type ErrorCategory =
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

/** Message loaded from DB on session switch. Snake_case fields
 *  match PR1's Rust serialization. */
interface LoadedMessage {
  id: number;
  session_id: string;
  role: Role;
  content: unknown; // Vec<ContentBlock> as JSON
  text: string;
  has_tool_calls: boolean;
  has_tool_results: boolean;
  created_at: string;
  seq: number;
}

/** One content block as serialized by Rust (snake_case tag + fields). */
interface ContentBlockFromDb {
  type: "text" | "thinking" | "redacted_thinking" | "tool_use" | "tool_result";
  text?: string;
  /** thinking block: summary text */
  thinking?: string;
  /** thinking block: opaque signature */
  signature?: string;
  /** redacted_thinking block: opaque data */
  data?: string;
  id?: string;
  name?: string;
  input?: Record<string, unknown>;
  tool_use_id?: string;
  content?: string;
  is_error?: boolean;
}

interface LoadedSession {
  session: {
    id: string;
    title: string;
    created_at: string;
    updated_at: string;
    model: string;
    project_id: string;
    current_cwd: string;
  };
  messages: LoadedMessage[];
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

/** High-frequency event (chat-event channel). */
interface ChatEventPayload {
  request_id: string;
  kind:
    | "start"
    | "delta"
    | "thinking_delta"
    | "signature_delta"
    | "redacted_thinking_delta"
    | "done"
    | "error";
  text?: string;
  signature?: string;
  data?: string;
  stop_reason?: string;
  message?: string;
  category?: ErrorCategory;
}

/** Low-frequency event (tool:call channel). */
interface ToolCallPayload {
  request_id: string;
  id: string;
  name: string;
  input: Record<string, unknown>;
}

/** Low-frequency event (tool:result channel). */
interface ToolResultPayload {
  request_id: string;
  tool_use_id: string;
  content: string;
  is_error: boolean;
}

let unlistenChat: UnlistenFn | null = null;
let unlistenTC: UnlistenFn | null = null;
let unlistenTR: UnlistenFn | null = null;

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
  // State
  // -----------------------------------------------------------------------

  const messages = ref<ChatMessage[]>([]);
  const sending = ref(false);
  const currentRequestId = ref<string | null>(null);
  const listenerReady = ref(false);

  // Session state — scoped to the current project (set by
  // `projectsStore.currentProjectId` and reloaded by the watcher
  // below). `currentCwd` mirrors the session's `current_cwd` for the
  // header display (Q5).
  const sessions = ref<SessionSummary[]>([]);
  const currentSessionId = ref<string | null>(null);
  const currentCwd = ref<string>("");

  // Streaming tracking (Q3 / PROPOSAL §4.4 "turn 结束一次性写"). When
  // a stream is in flight, `streamingSessionId` records which session
  // owns it (used by `shouldApplyEvent` to drop events for
  // non-current sessions), and `streamingProjectIds` is the set of
  // project IDs the UI uses to draw a red dot on the tab. We track
  // `lastStreamedProjectId` separately so `clearStreamingSession` can
  // remove the right project from the Set even if the user has
  // switched projects mid-stream (where `currentProjectId` no longer
  // points at the streaming project).
  const streamingSessionId = ref<string | null>(null);
  const lastStreamedProjectId = ref<string | null>(null);
  const streamingProjectIds = ref<Set<string>>(new Set());

  // -----------------------------------------------------------------------
  // Cross-store coordination: react to project changes
  // -----------------------------------------------------------------------

  // We grab the cross-store handles at the top of the setup function
  // so the watcher is registered against stable references. The
  // `useProjectsStore()` and `useConfigStore()` calls below are
  // idempotent (Pinia dedupes), and the `watch` we register is the
  // single source of truth for project-change side effects.
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

  async function onProjectChange(newId: string | null): Promise<void> {
    if (newId === null) {
      sessions.value = [];
      currentSessionId.value = null;
      currentCwd.value = "";
      messages.value = [];
      return;
    }
    await loadSessions(newId);
    // Default to the most-recently-updated session if any exist;
    // otherwise leave the chat area in its empty state.
    if (sessions.value.length > 0) {
      const first = sessions.value[0];
      currentSessionId.value = first.id;
      currentCwd.value = first.current_cwd ?? "";
      // Load messages for that session. We do this inline (not via
      // `switchSession`) so the initial-load path is a single
      // `await` and avoids a redundant `sending` guard check.
      try {
        const loaded = await invoke<LoadedSession | null>("load_session", {
          sessionId: first.id,
        });
        if (loaded) {
          messages.value = rehydrateMessages(loaded.messages);
        }
      } catch (e) {
        console.error("initial load_session failed:", e);
      }
    } else {
      currentSessionId.value = null;
      currentCwd.value = "";
      messages.value = [];
    }
  }

  // -----------------------------------------------------------------------
  // Listeners
  // -----------------------------------------------------------------------

  async function ensureListener() {
    if (unlistenChat) return;

    unlistenChat = await listen<ChatEventPayload>("chat-event", (e) => {
      handleChatEvent(e.payload);
    });
    unlistenTC = await listen<ToolCallPayload>("tool:call", (e) => {
      handleToolCall(e.payload);
    });
    unlistenTR = await listen<ToolResultPayload>("tool:result", (e) => {
      handleToolResult(e.payload);
    });

    listenerReady.value = true;
  }

  // -----------------------------------------------------------------------
  // Event handlers
  // -----------------------------------------------------------------------

  /** Drop events for a stream that no longer matches the session
   *  currently shown in the UI. Returns true if the event should be
   *  applied, false if it should be discarded. */
  function shouldApplyEvent(requestId: string): boolean {
    if (requestId !== currentRequestId.value) return false;
    // If a stream is in flight and the user has navigated to a
    // different session, drop the events. The user is no longer
    // looking at the streaming session; the in-flight events would
    // otherwise clobber the new session's `messages.value` last
    // entry. When the user returns to the streaming session, the
    // DB state is up-to-date up to the last persisted turn, and
    // the stream's `done` event will reset `currentRequestId` to
    // null on the next event tick.
    if (
      streamingSessionId.value !== null &&
      streamingSessionId.value !== currentSessionId.value
    ) {
      return false;
    }
    return true;
  }

  /** Get-or-create the in-flight thinking block (the one currently being
   *  streamed for this assistant message). There is at most one open
   *  block at a time; signature_delta on a new event after a text /
   *  tool_use boundary starts a fresh one. */
  function currentThinkingBlock(
    m: ChatMessage,
  ): ThinkingBlockInfo {
    if (!m.thinkingBlocks || m.thinkingBlocks.length === 0) {
      m.thinkingBlocks = [{ text: "", signature: "" }];
    } else {
      const last = m.thinkingBlocks[m.thinkingBlocks.length - 1];
      // If the last block already has a signature, the model has moved
      // on to a new thinking block (interleaved thinking). Open one.
      if (last.signature) {
        m.thinkingBlocks.push({ text: "", signature: "" });
      }
    }
    return m.thinkingBlocks[m.thinkingBlocks.length - 1];
  }

  function handleChatEvent(event: ChatEventPayload) {
    if (!shouldApplyEvent(event.request_id)) return;
    const last = messages.value[messages.value.length - 1];
    if (!last || last.role !== "assistant") return;

    switch (event.kind) {
      case "start":
        last.streaming = true;
        last.error = undefined;
        break;
      case "delta":
        if (event.text) last.content += event.text;
        break;
      case "thinking_delta":
        if (event.text) {
          const blk = currentThinkingBlock(last);
          blk.text += event.text;
        }
        break;
      case "signature_delta":
        if (event.signature) {
          const blk = currentThinkingBlock(last);
          blk.signature += event.signature;
        }
        break;
      case "redacted_thinking_delta":
        if (event.data) {
          if (!last.redactedThinkingData) last.redactedThinkingData = [];
          last.redactedThinkingData.push(event.data);
        }
        break;
      case "done":
        last.streaming = false;
        sending.value = false;
        currentRequestId.value = null;
        clearStreamingSession();
        // Refresh sidebar so updated_at / title reflect the new turn.
        if (projectsStore.currentProjectId) {
          void loadSessions(projectsStore.currentProjectId);
        }
        break;
      case "error":
        last.streaming = false;
        last.error = {
          message: event.message ?? "未知错误",
          category: event.category ?? "server",
        };
        sending.value = false;
        currentRequestId.value = null;
        clearStreamingSession();
        break;
    }
  }

  function handleToolCall(payload: ToolCallPayload) {
    if (!shouldApplyEvent(payload.request_id)) return;
    const last = messages.value[messages.value.length - 1];
    if (!last || last.role !== "assistant") return;

    if (!last.toolCalls) last.toolCalls = [];
    last.toolCalls.push({
      id: payload.id,
      name: payload.name,
      input: payload.input,
    });
  }

  function handleToolResult(payload: ToolResultPayload) {
    if (!shouldApplyEvent(payload.request_id)) return;
    const last = messages.value[messages.value.length - 1];
    if (!last || last.role !== "assistant") return;

    if (!last.toolResults) last.toolResults = [];
    last.toolResults.push({
      toolUseId: payload.tool_use_id,
      content: payload.content,
      isError: payload.is_error,
    });
  }

  function clearStreamingSession(): void {
    if (lastStreamedProjectId.value) {
      streamingProjectIds.value.delete(lastStreamedProjectId.value);
      // Runtime invariant (PR2 fix): after a stream ends, the
      // project's red dot must be gone. Catches future regressions
      // where the Set is keyed on session IDs again by mistake.
      console.assert(
        !streamingProjectIds.value.has(lastStreamedProjectId.value),
        "streamingProjectIds should not contain the streamed project ID after clear",
      );
    }
    lastStreamedProjectId.value = null;
    streamingSessionId.value = null;
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

  /** Convert DB-loaded messages into frontend ChatMessage objects.
   *
   *  Pass 1: parse each row's `content` blocks into `toolCalls` /
   *          `toolResults` / `thinkingBlocks` / `redactedThinkingData`.
   *  Pass 2: DB stores `tool_use` (in assistant) and the matching
   *          `tool_result` (in the next user message — Anthropic API
   *          requirement) as separate rows, but the in-memory model
   *          expects both on the same assistant message so the UI's
   *          "done / running" status lookup works. Merge the user
   *          message's tool_results into the previous assistant message. */
  function rehydrateMessages(loaded: LoadedMessage[]): ChatMessage[] {
    const messages: ChatMessage[] = loaded.map((m) => {
      const blocks: ContentBlockFromDb[] = Array.isArray(m.content)
        ? (m.content as ContentBlockFromDb[])
        : [];
      const toolCalls: ToolCallInfo[] = [];
      const toolResults: ToolResultInfo[] = [];
      const thinkingBlocks: ThinkingBlockInfo[] = [];
      const redactedThinkingData: string[] = [];
      for (const b of blocks) {
        if (!b || typeof b.type !== "string") continue;
        if (b.type === "thinking") {
          thinkingBlocks.push({
            text: b.thinking ?? "",
            signature: b.signature ?? "",
          });
        } else if (b.type === "redacted_thinking") {
          if (typeof b.data === "string") redactedThinkingData.push(b.data);
        } else if (
          b.type === "tool_use" &&
          typeof b.id === "string" &&
          typeof b.name === "string"
        ) {
          toolCalls.push({
            id: b.id,
            name: b.name,
            input: b.input ?? {},
          });
        } else if (b.type === "tool_result" && typeof b.tool_use_id === "string") {
          toolResults.push({
            toolUseId: b.tool_use_id,
            content: b.content ?? "",
            isError: !!b.is_error,
          });
        }
      }
      const msg: ChatMessage = {
        id: `${m.session_id}-${m.seq}`,
        role: m.role,
        content: m.text,
      };
      if (toolCalls.length) msg.toolCalls = toolCalls;
      if (toolResults.length) msg.toolResults = toolResults;
      if (thinkingBlocks.length) msg.thinkingBlocks = thinkingBlocks;
      if (redactedThinkingData.length) msg.redactedThinkingData = redactedThinkingData;
      return msg;
    });

    // Attach user-message tool_results to the previous assistant message so
    // the UI's per-message "done / running" lookup hits. The user message
    // itself becomes a UI "ghost": it stays in the array (so its
    // tool_results still flow to the LLM via toPayloadContent) but the
    // visible-messages filter hides it because it has no text and no
    // tool_calls.
    for (let i = 0; i < messages.length; i++) {
      const m = messages[i];
      const trs = m.toolResults;
      if (m.role !== "user" || !trs?.length) continue;
      for (let j = i - 1; j >= 0; j--) {
        if (messages[j].role === "assistant") {
          if (!messages[j].toolResults) messages[j].toolResults = [];
          messages[j].toolResults!.push(...trs);
          break;
        }
      }
    }

    return messages;
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
    messages.value = [];
    await loadSessions(projectId);
    return session.id;
  }

  async function switchSession(sessionId: string) {
    // Q3: switching sessions mid-stream is allowed. The in-flight
    // request keeps running on the backend; events for it are
    // dropped for non-current sessions via `shouldApplyEvent`, so
    // the new session's `messages.value` is not clobbered. When the
    // user returns to the streaming session, the DB state is
    // up-to-date up to the last persisted turn, and the stream's
    // `done` / `error` event will reset `sending` and
    // `currentRequestId` on the next event tick.
    const loaded = await invoke<LoadedSession | null>("load_session", {
      sessionId,
    });
    if (!loaded) return;
    currentSessionId.value = sessionId;
    currentCwd.value = loaded.session.current_cwd ?? "";
    messages.value = rehydrateMessages(loaded.messages);
  }

  async function deleteSession(sessionId: string) {
    await invoke("delete_session", { sessionId });
    if (currentSessionId.value === sessionId) {
      currentSessionId.value = null;
      currentCwd.value = "";
      messages.value = [];
    }
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
  }

  // -----------------------------------------------------------------------
  // Send
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
    // (Anthropic Messages API contract). `rehydrateMessages` attaches
    // the following user message's tool_results onto the assistant
    // message *for UI grouping* (per-message "done / running" lookup);
    // here we MUST NOT echo them onto the wire when role=assistant or
    // Anthropic returns 2013 ("tool result's tool id ... not found")
    // because the assistant message itself isn't allowed to contain
    // tool_result blocks. Same for `content` text emitted onto a
    // ghost user message: only the assistant's text counts.
    if (m.role === "assistant") {
      const hasTools = !!m.toolCalls?.length;
      const hasThinking =
        !!m.thinkingBlocks?.length || !!m.redactedThinkingData?.length;
      if (!hasTools && !hasThinking) {
        return m.content;
      }
      const blocks: ContentBlockPayload[] = [];
      // Thinking blocks come first (Anthropic convention: reasoning before
      // any visible text in the same turn).
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
      // the wire. The matching user-role message in `messages.value`
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
    if (!trimmed || sending.value) return;
    await ensureListener();

    // Lazily create a session if there isn't one yet. `createNewSession`
    // throws if no project is active, so the chat area is expected
    // to be visible only when a project is selected (Q2 in dispatch
    // prompt: the empty state hides the input, so send/create is
    // unreachable from the UI).
    if (!currentSessionId.value) {
      await createNewSession();
    }

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
    messages.value.push(userMsg, assistantMsg);

    // Build history — keep tool_use / tool_result / thinking /
    // redacted_thinking blocks intact so the LLM has full context
    // across turns and across session switches. The agent loop also
    // constructs a matching assistant message from the streaming
    // events and persists it before the next LLM call, so the
    // history we send here will line up with what's in the DB.
    const history: ChatMessagePayload[] = messages.value
      .filter((m) => m.id !== assistantMsg.id)
      .map((m) => ({ role: m.role, content: toPayloadContent(m) }));

    const requestId = genId();
    currentRequestId.value = requestId;
    sending.value = true;
    // Mark the current session as streaming so a tab switch can
    // (a) drop events for this stream (via `shouldApplyEvent`) and
    // (b) show the red dot on the originating tab. The project ID
    // is recorded in `lastStreamedProjectId` so we can clean the
    // Set when the stream ends, even if the user has switched
    // projects mid-stream.
    if (currentSessionId.value && projectsStore.currentProjectId) {
      streamingSessionId.value = currentSessionId.value;
      lastStreamedProjectId.value = projectsStore.currentProjectId;
      streamingProjectIds.value.add(projectsStore.currentProjectId);
      // Runtime invariant (PR2 fix): when a stream starts, the
      // originating project's ID must be in the Set so the tab
      // can render the red dot.
      console.assert(
        streamingProjectIds.value.has(projectsStore.currentProjectId),
        "streamingProjectIds should contain the current project ID after send()",
      );
    }

    try {
      await invoke("chat", {
        requestId,
        sessionId: currentSessionId.value,
        messages: history,
      });
    } catch (e) {
      assistantMsg.error = {
        message: String(e),
        category: "server",
      };
      assistantMsg.streaming = false;
      sending.value = false;
      currentRequestId.value = null;
      clearStreamingSession();
    }
  }

  /** PR5: cancel an in-flight chat request. The backend's agent
   *  loop notices on the next event boundary, bails out, persists
   *  whatever it has, and emits a `done` event with
   *  `stop_reason: "cancelled"`. The existing `handleChatEvent` for
   *  `done` then resets `sending` / `currentRequestId` and clears
   *  the streaming session — so this call only needs to fire the
   *  IPC; it should NOT clear local state synchronously, or the
   *  follow-up `done` event would be ignored as "stale" (see
   *  `shouldApplyEvent`). */
  async function cancel() {
    const rid = currentRequestId.value;
    if (!rid) return;
    try {
      await invoke("cancel_chat", { requestId: rid });
    } catch (e) {
      // A failed cancel is logged but not user-facing — the user
      // already saw the Stop button and clicked it. The natural
      // fallback is: the stream finishes on its own (or the next
      // event errors out), and the existing `done` / `error` path
      // resets state.
      console.error("cancel_chat failed:", e);
    }
  }

  /** Cleanup all listeners (for future teardown). */
  function cleanup() {
    unlistenChat?.();
    unlistenTC?.();
    unlistenTR?.();
    unlistenChat = null;
    unlistenTC = null;
    unlistenTR = null;
  }

  return {
    messages,
    sending,
    listenerReady,
    sessions,
    currentSessionId,
    currentCwd,
    streamingProjectIds,
    send,
    cancel,
    loadSessions,
    createNewSession,
    switchSession,
    deleteSession,
    cleanup,
  };
});
