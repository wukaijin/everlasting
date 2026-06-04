import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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

/** Chat message with optional tool call/result metadata. */
export interface ChatMessage {
  id: string;
  role: Role;
  content: string; // accumulated text content
  streaming?: boolean;
  error?: { message: string; category: ErrorCategory };
  toolCalls?: ToolCallInfo[];
  toolResults?: ToolResultInfo[];
}

/** Session summary shown in the sidebar. */
export interface SessionSummary {
  id: string;
  title: string;
  updatedAt: string;
  preview: string;
}

/** Message loaded from DB on session switch. */
interface LoadedMessage {
  id: number;
  sessionId: string;
  role: Role;
  content: unknown; // Vec<ContentBlock> as JSON
  text: string;
  hasToolCalls: boolean;
  hasToolResults: boolean;
  createdAt: string;
  seq: number;
}

/** One content block as serialized by Rust (snake_case tag + fields). */
interface ContentBlockFromDb {
  type: "text" | "tool_use" | "tool_result";
  text?: string;
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
    createdAt: string;
    updatedAt: string;
    model: string;
  };
  messages: LoadedMessage[];
}

/** Wire-format content sent to the Rust `chat` command. Mirrors
 *  Rust's `MessageContent`: a plain string for text-only messages,
 *  or an array of `ContentBlock` (snake_case tag + fields) when
 *  the message carries tool_use / tool_result blocks. */
type ContentBlockPayload =
  | { type: "text"; text: string }
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
  kind: "start" | "delta" | "done" | "error";
  text?: string;
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

export const useChatStore = defineStore("chat", () => {
  // -----------------------------------------------------------------------
  // State
  // -----------------------------------------------------------------------

  const messages = ref<ChatMessage[]>([]);
  const sending = ref(false);
  const currentRequestId = ref<string | null>(null);
  const listenerReady = ref(false);

  // Session state
  const sessions = ref<SessionSummary[]>([]);
  const currentSessionId = ref<string | null>(null);

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

  function handleChatEvent(event: ChatEventPayload) {
    if (event.request_id !== currentRequestId.value) return;
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
      case "done":
        last.streaming = false;
        sending.value = false;
        currentRequestId.value = null;
        // Refresh sidebar so updated_at / title reflect the new turn.
        void loadSessions();
        break;
      case "error":
        last.streaming = false;
        last.error = {
          message: event.message ?? "未知错误",
          category: event.category ?? "server",
        };
        sending.value = false;
        currentRequestId.value = null;
        break;
    }
  }

  function handleToolCall(payload: ToolCallPayload) {
    if (payload.request_id !== currentRequestId.value) return;
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
    if (payload.request_id !== currentRequestId.value) return;
    const last = messages.value[messages.value.length - 1];
    if (!last || last.role !== "assistant") return;

    if (!last.toolResults) last.toolResults = [];
    last.toolResults.push({
      toolUseId: payload.tool_use_id,
      content: payload.content,
      isError: payload.is_error,
    });
  }

  // -----------------------------------------------------------------------
  // Session management
  // -----------------------------------------------------------------------

  async function loadSessions() {
    sessions.value = await invoke<SessionSummary[]>("list_sessions");
  }

  /** Convert DB-loaded messages into frontend ChatMessage objects.
   *
   *  Two passes:
   *  1. Parse each row's `content` blocks into `toolCalls` / `toolResults`.
   *  2. DB stores `tool_use` (in assistant) and the matching `tool_result`
   *     (in the next user message — Anthropic API requirement) as separate
   *     rows, but the in-memory model expects both on the same assistant
   *     message so the UI's "done / running" status lookup works. Merge
   *     the user message's tool_results into the previous assistant message. */
  function rehydrateMessages(loaded: LoadedMessage[]): ChatMessage[] {
    const messages: ChatMessage[] = loaded.map((m) => {
      const blocks: ContentBlockFromDb[] = Array.isArray(m.content)
        ? (m.content as ContentBlockFromDb[])
        : [];
      const toolCalls: ToolCallInfo[] = [];
      const toolResults: ToolResultInfo[] = [];
      for (const b of blocks) {
        if (
          b?.type === "tool_use" &&
          typeof b.id === "string" &&
          typeof b.name === "string"
        ) {
          toolCalls.push({
            id: b.id,
            name: b.name,
            input: b.input ?? {},
          });
        } else if (b?.type === "tool_result" && typeof b.tool_use_id === "string") {
          toolResults.push({
            toolUseId: b.tool_use_id,
            content: b.content ?? "",
            isError: !!b.is_error,
          });
        }
      }
      const msg: ChatMessage = {
        id: `${m.sessionId}-${m.seq}`,
        role: m.role,
        content: m.text,
      };
      if (toolCalls.length) msg.toolCalls = toolCalls;
      if (toolResults.length) msg.toolResults = toolResults;
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

  async function createNewSession(): Promise<string> {
    const session = await invoke<{
      id: string;
      title: string;
      createdAt: string;
      updatedAt: string;
      model: string;
    }>("create_session", { model: null });
    currentSessionId.value = session.id;
    messages.value = [];
    await loadSessions();
    return session.id;
  }

  async function switchSession(sessionId: string) {
    if (sending.value) return; // don't switch mid-stream
    const loaded = await invoke<LoadedSession | null>("load_session", {
      sessionId,
    });
    if (!loaded) return;
    currentSessionId.value = sessionId;
    messages.value = rehydrateMessages(loaded.messages);
  }

  async function deleteSession(sessionId: string) {
    await invoke("delete_session", { sessionId });
    if (currentSessionId.value === sessionId) {
      currentSessionId.value = null;
      messages.value = [];
    }
    await loadSessions();
  }

  // -----------------------------------------------------------------------
  // Send
  // -----------------------------------------------------------------------

  /** Build the wire-format content for a history message: plain string
   *  for text-only turns, or an array of blocks when the turn carries
   *  tool_use / tool_result data. Backend's `MessageContent` deserializer
   *  accepts both shapes. */
  function toPayloadContent(m: ChatMessage): string | ContentBlockPayload[] {
    if (!m.toolCalls?.length && !m.toolResults?.length) {
      return m.content;
    }
    const blocks: ContentBlockPayload[] = [];
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
    for (const tr of m.toolResults ?? []) {
      blocks.push({
        type: "tool_result",
        tool_use_id: tr.toolUseId,
        content: tr.content,
        is_error: tr.isError,
      });
    }
    return blocks;
  }

  async function send(text: string) {
    const trimmed = text.trim();
    if (!trimmed || sending.value) return;
    await ensureListener();

    // Lazily create a session if there isn't one yet.
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

    // Build history — keep tool_use/tool_result blocks intact so the LLM
    // has full context across turns and across session switches.
    const history: ChatMessagePayload[] = messages.value
      .filter((m) => m.id !== assistantMsg.id)
      .map((m) => ({ role: m.role, content: toPayloadContent(m) }));

    const requestId = genId();
    currentRequestId.value = requestId;
    sending.value = true;

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
    send,
    loadSessions,
    createNewSession,
    switchSession,
    deleteSession,
    cleanup,
  };
});
