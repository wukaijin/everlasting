<script setup lang="ts">
import { ref, nextTick, watch, computed, onMounted } from "vue";
import { useChatStore } from "../stores/chat";
import { useConfigStore } from "../stores/config";

const store = useChatStore();
const config = useConfigStore();
const input = ref("");
const messagesEl = ref<HTMLElement | null>(null);

onMounted(() => {
  config.load();
});

const isComposing = ref(false);

// IME-safe textarea binding.
// v-model would update `input` on every `input` event, including the
// intermediate pinyin state during composition. That re-renders the
// textarea, clobbering the IME's candidate window and cursor. Instead:
//   - During composition: ignore `input` events, let the browser/IME own
//     the textarea's value.
//   - On `compositionend`: sync the committed text into `input`.
//   - Outside composition: behave like a normal v-model.
function onTextareaInput(e: Event) {
  if (isComposing.value) return;
  input.value = (e.target as HTMLTextAreaElement).value;
}

function onCompositionStart() {
  isComposing.value = true;
}

function onCompositionEnd(e: CompositionEvent) {
  isComposing.value = false;
  input.value = (e.target as HTMLTextAreaElement).value;
}

async function scrollToBottom() {
  await nextTick();
  if (messagesEl.value) {
    messagesEl.value.scrollTop = messagesEl.value.scrollHeight;
  }
}

watch(
  () => store.messages.map((m) => m.content).join("|"),
  () => scrollToBottom(),
);

watch(
  () => store.messages.length,
  () => scrollToBottom(),
);

async function onSubmit() {
  const text = input.value;
  if (!text.trim() || store.sending) return;
  input.value = "";
  await store.send(text);
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Enter" && !e.shiftKey && !isComposing.value) {
    e.preventDefault();
    onSubmit();
  }
}

const hasMessages = computed(() => store.messages.length > 0);
</script>

<template>
  <div class="app">
    <header class="app__header">
      <h1 class="app__title">Everlasting</h1>
      <span class="app__subtitle">vibe coding workbench · step 1</span>
    </header>

    <main ref="messagesEl" class="app__main">
      <div v-if="!hasMessages" class="empty">
        <p>输入一句话,跟 LLM 聊聊看 👋</p>
        <p class="empty__hint">中文输入测试 + 流式响应</p>
      </div>

      <ul v-else class="messages">
        <li
          v-for="m in store.messages"
          :key="m.id"
          :class="['msg', `msg--${m.role}`, { 'msg--err': m.error }]"
        >
          <div class="msg__bubble">
            <span class="msg__text">{{ m.content }}</span>
            <span v-if="m.streaming" class="msg__cursor">▍</span>
          </div>
          <div v-if="m.error" class="msg__error">
            ⚠ {{ m.error.message }}
          </div>
        </li>
      </ul>
    </main>

    <footer class="app__footer">
      <textarea
        :value="input"
        class="input"
        rows="2"
        placeholder="输入消息,Enter 发送,Shift+Enter 换行"
        :disabled="store.sending"
        @input="onTextareaInput"
        @compositionstart="onCompositionStart"
        @compositionend="onCompositionEnd"
        @keydown="onKeydown"
      />
      <button
        class="send"
        :disabled="store.sending || !input.trim()"
        @click="onSubmit"
      >
        {{ store.sending ? "生成中…" : "发送" }}
      </button>
    </footer>

    <div v-if="config.loaded" class="statusbar" :class="{ 'statusbar--warn': !config.configured }">
      <span class="statusbar__dot" />
      <span class="statusbar__model">{{ config.model || "(no model)" }}</span>
      <span class="statusbar__sep">·</span>
      <span class="statusbar__url">{{ config.baseUrl || "(no base_url)" }}</span>
      <span v-if="!config.configured" class="statusbar__hint">ANTHROPIC_API_KEY 未设置</span>
    </div>
  </div>
</template>

<style scoped>
.app {
  display: flex;
  flex-direction: column;
  height: 100vh;
  background: #fafbfc;
}

.app__header {
  display: flex;
  align-items: baseline;
  gap: 12px;
  padding: 14px 20px;
  border-bottom: 1px solid #e5e7eb;
  background: #ffffff;
}

.app__title {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
  color: #111827;
}

.app__subtitle {
  font-size: 12px;
  color: #6b7280;
}

.app__main {
  flex: 1;
  overflow-y: auto;
  padding: 20px;
}

.empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: #6b7280;
  text-align: center;
}

.empty p {
  margin: 4px 0;
}

.empty__hint {
  font-size: 12px;
  color: #9ca3af;
}

.messages {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.msg {
  display: flex;
  flex-direction: column;
  max-width: 75%;
}

.msg--user {
  align-self: flex-end;
}

.msg--assistant {
  align-self: flex-start;
}

.msg__bubble {
  padding: 10px 14px;
  border-radius: 12px;
  white-space: pre-wrap;
  word-break: break-word;
  line-height: 1.6;
}

.msg--user .msg__bubble {
  background: #2563eb;
  color: #ffffff;
  border-bottom-right-radius: 2px;
}

.msg--assistant .msg__bubble {
  background: #ffffff;
  color: #1f2328;
  border: 1px solid #e5e7eb;
  border-bottom-left-radius: 2px;
}

.msg--err .msg__bubble {
  border-color: #fca5a5;
  background: #fef2f2;
}

.msg__cursor {
  display: inline-block;
  margin-left: 2px;
  animation: blink 1s steps(1) infinite;
  color: #6b7280;
}

@keyframes blink {
  50% {
    opacity: 0;
  }
}

.msg__error {
  margin-top: 4px;
  padding: 0 14px;
  font-size: 12px;
  color: #b91c1c;
}

.app__footer {
  display: flex;
  gap: 8px;
  padding: 12px 20px 16px;
  border-top: 1px solid #e5e7eb;
  background: #ffffff;
}

.input {
  flex: 1;
  resize: none;
  padding: 10px 12px;
  border: 1px solid #d1d5db;
  border-radius: 8px;
  background: #ffffff;
  outline: none;
  font-family: inherit;
  font-size: 14px;
  line-height: 1.5;
}

.input:focus {
  border-color: #2563eb;
  box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.15);
}

.input:disabled {
  background: #f3f4f6;
  color: #9ca3af;
}

.send {
  padding: 0 18px;
  border: none;
  border-radius: 8px;
  background: #2563eb;
  color: #ffffff;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s;
}

.send:hover:not(:disabled) {
  background: #1d4ed8;
}

.send:disabled {
  background: #93c5fd;
  cursor: not-allowed;
}

.statusbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 16px;
  background: #f3f4f6;
  border-top: 1px solid #e5e7eb;
  font-size: 11px;
  color: #6b7280;
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace;
}

.statusbar--warn {
  background: #fef3c7;
  color: #92400e;
}

.statusbar__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #10b981;
}

.statusbar--warn .statusbar__dot {
  background: #f59e0b;
}

.statusbar__model {
  font-weight: 500;
}

.statusbar__sep {
  color: #9ca3af;
}

.statusbar__url {
  color: inherit;
  opacity: 0.85;
}

.statusbar__hint {
  margin-left: auto;
  color: #b45309;
  font-weight: 500;
}
</style>
