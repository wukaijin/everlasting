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
    createdAt: string;
    updatedAt: string;
    model: string;
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
        id: `${m.sessionId}-${m.seq}`,
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
   *  for text-only / thinking-only messages, or an array of blocks when
   *  the turn carries tool_use / tool_result data. Backend's
   *  `MessageContent` deserializer accepts both shapes.
   *
   *  CRITICAL: thinking blocks (incl. signatures) and redacted_thinking
   *  data are emitted verbatim in their original streaming order. The
   *  Anthropic API requires the exact signature blob on the next turn —
   *  omitting or rewriting it produces 400. */
  function toPayloadContent(m: ChatMessage): string | ContentBlockPayload[] {
    const hasTools = !!m.toolCalls?.length || !!m.toolResults?.length;
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
