import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

type Role = "user" | "assistant";
type ErrorCategory = "auth" | "rate_limit" | "invalid_request" | "server" | "network";

interface ChatMessage {
  id: string;
  role: Role;
  content: string;
  streaming?: boolean;
  error?: { message: string; category: ErrorCategory };
}

interface ChatMessagePayload {
  role: Role;
  content: string;
}

interface ChatEventPayload {
  request_id: string;
  kind: "start" | "delta" | "done" | "error";
  text?: string;
  stop_reason?: string;
  message?: string;
  category?: ErrorCategory;
}

let unlisten: UnlistenFn | null = null;

const genId = () =>
  Math.random().toString(36).slice(2) + Date.now().toString(36);

export const useChatStore = defineStore("chat", () => {
  const messages = ref<ChatMessage[]>([]);
  const sending = ref(false);
  const currentRequestId = ref<string | null>(null);
  const listenerReady = ref(false);

  async function ensureListener() {
    if (unlisten) return;
    unlisten = await listen<ChatEventPayload>("chat-event", (e) => {
      handleEvent(e.payload);
    });
    listenerReady.value = true;
  }

  function handleEvent(event: ChatEventPayload) {
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

  async function send(text: string) {
    const trimmed = text.trim();
    if (!trimmed || sending.value) return;
    await ensureListener();

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

    const history: ChatMessagePayload[] = messages.value
      .filter((m) => m.id !== assistantMsg.id)
      .map((m) => ({ role: m.role, content: m.content }));

    const requestId = genId();
    currentRequestId.value = requestId;
    sending.value = true;

    try {
      await invoke("chat", { requestId, messages: history });
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

  return { messages, sending, listenerReady, send };
});
