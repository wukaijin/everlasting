<script setup lang="ts">
// ChatInput — multi-line textarea + send button. Implements the
// IME-safe Enter-to-send pattern: during composition (中文输入法
// candidate selection) Enter must NOT submit, otherwise typing
// "你好" can blast an unfinished candidate into the model.
//
// The component is "dumb" with respect to the chat model — it
// emits `send` with the trimmed text and lets the parent (ChatWindow)
// decide whether to actually call `store.send` (e.g. guard on
// `sending`, project, etc.). The component also emits `change` so
// the parent can mirror the value if it needs to (currently unused
// but cheap to keep for future "draft restored on session switch").

import { ref } from "vue";

const props = defineProps<{
  /** True while the model is generating. Disables the input. */
  sending: boolean;
  /** Placeholder text shown when empty. */
  placeholder?: string;
}>();

const emit = defineEmits<{
  send: [text: string];
}>();

const input = ref("");
const isComposing = ref(false);

function onInput(e: Event) {
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

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Enter" && !e.shiftKey && !isComposing.value) {
    e.preventDefault();
    submit();
  }
}

function submit() {
  const text = input.value;
  if (!text.trim() || props.sending) return;
  input.value = "";
  emit("send", text);
}
</script>

<template>
  <footer class="chat-input">
    <textarea
      :value="input"
      class="chat-input__field"
      rows="2"
      :placeholder="placeholder ?? '输入消息,Enter 发送,Shift+Enter 换行'"
      :disabled="sending"
      @input="onInput"
      @compositionstart="onCompositionStart"
      @compositionend="onCompositionEnd"
      @keydown="onKeydown"
    />
    <button
      class="chat-input__send"
      :disabled="sending || !input.trim()"
      @click="submit"
    >
      {{ sending ? "生成中…" : "发送" }}
    </button>
  </footer>
</template>

<style scoped>
.chat-input {
  display: flex;
  gap: 8px;
  padding: 12px 20px 16px;
  border-top: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  flex-shrink: 0;
}

.chat-input__field {
  flex: 1;
  resize: none;
  padding: 10px 12px;
  border: 1px solid var(--color-bg-border);
  border-radius: 8px;
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  outline: none;
  font-family: inherit;
  font-size: 14px;
  line-height: 1.5;
  transition: border-color 0.1s, box-shadow 0.1s;
}

.chat-input__field:focus {
  border-color: var(--color-accent);
  box-shadow: 0 0 0 3px var(--color-accent-muted);
}

.chat-input__field:disabled {
  background: var(--color-bg-app);
  color: var(--color-text-muted);
  cursor: not-allowed;
}

.chat-input__send {
  padding: 0 18px;
  border: none;
  border-radius: 8px;
  background: var(--color-accent);
  color: #ffffff;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s;
  font-family: inherit;
}

.chat-input__send:hover:not(:disabled) {
  background: var(--color-accent-hover);
}

.chat-input__send:disabled {
  background: var(--color-accent-muted);
  color: var(--color-text-muted);
  cursor: not-allowed;
}
</style>
