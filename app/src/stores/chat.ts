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

/** Payload sent to Rust (always plain text — frontend doesn't send ContentBlock). */
interface ChatMessagePayload {
  role: Role;
  content: string;
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

  /** Convert DB-loaded messages into frontend ChatMessage objects. */
  function rehydrateMessages(loaded: LoadedMessage[]): ChatMessage[] {
    return loaded.map((m) => ({
      id: `${m.sessionId}-${m.seq}`,
      role: m.role,
      content: m.text, // already-extracted text (denormalized in DB)
    }));
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

    // Build history — always plain text (backend handles MessageContent).
    const history: ChatMessagePayload[] = messages.value
      .filter((m) => m.id !== assistantMsg.id)
      .map((m) => ({ role: m.role, content: m.content }));

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
