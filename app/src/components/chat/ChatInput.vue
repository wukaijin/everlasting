<script setup lang="ts">
// ChatInput — chat composer. Single-line textarea (auto-grows up to
// ~200px) + a circular Prussian-blue send button on the right, with
// a small hint row below. Matches the spike-003 reference layout
// (ui-A.png).
//
// IME-safe Enter-to-send: during composition (中文输入法 candidate
// selection) Enter must NOT submit, otherwise typing "你好" can blast
// an unfinished candidate into the model. Same composition gate as
// before.
//
// The component is "dumb" with respect to the chat model — it emits
// `send` with the trimmed text and lets the parent (ChatPanel) decide
// whether to actually call `store.send` (e.g. guard on `sending`,
// project, etc.).

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
const textareaEl = ref<HTMLTextAreaElement | null>(null);

/** Auto-grow: reset height so the field shrinks when content is
 *  deleted, then size to scrollHeight (capped via CSS max-height). */
function autosize() {
  const el = textareaEl.value;
  if (!el) return;
  el.style.height = "auto";
  el.style.height = `${el.scrollHeight}px`;
}

function onTextareaInput(e: Event) {
  if (isComposing.value) return;
  input.value = (e.target as HTMLTextAreaElement).value;
  autosize();
}

function onCompositionStart() {
  isComposing.value = true;
}

function onCompositionEnd(e: CompositionEvent) {
  isComposing.value = false;
  input.value = (e.target as HTMLTextAreaElement).value;
  autosize();
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Enter" && !e.shiftKey && !isComposing.value) {
    e.preventDefault();
    submit();
  }
}

function onSubmit() {
  submit();
}

function submit() {
  const text = input.value;
  if (!text.trim() || props.sending) return;
  input.value = "";
  // Reset height on send so an emptied field collapses to a single
  // line immediately rather than snapping to 0 on the next input.
  const el = textareaEl.value;
  if (el) el.style.height = "auto";
  emit("send", text);
}

const disabled = (): boolean => props.sending || !input.value.trim();
</script>

<template>
  <footer class="chat-input">
    <div class="chat-input__row">
      <textarea
        ref="textareaEl"
        :value="input"
        class="chat-input__field"
        rows="1"
        :placeholder="placeholder ?? '问点什么,或输入 / 调出命令…'"
        :disabled="sending"
        @input="onTextareaInput"
        @compositionstart="onCompositionStart"
        @compositionend="onCompositionEnd"
        @keydown="onKeydown"
      />
      <button
        class="chat-input__send"
        :disabled="disabled()"
        :aria-label="sending ? '生成中' : '发送'"
        @click="onSubmit"
      >
        <span v-if="sending" class="chat-input__spinner" aria-hidden="true">·</span>
        <span v-else aria-hidden="true">↑</span>
      </button>
    </div>
    <div class="chat-input__hint">
      ⏎ 发送 · ⇧⏎ 换行 · @ 引用文件 · / 命令
    </div>
  </footer>
</template>

<style scoped>
.chat-input {
  padding: 12px 20px 16px;
  background: var(--color-bg-app);
  flex-shrink: 0;
}

.chat-input__row {
  display: flex;
  align-items: flex-end;
  gap: 8px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 12px;
  padding: 6px 6px 6px 14px;
  transition: border-color 0.15s, box-shadow 0.15s;
}

.chat-input__row:focus-within {
  border-color: var(--color-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent);
}

.chat-input__field {
  flex: 1;
  resize: none;
  border: none;
  background: transparent;
  color: var(--color-text-primary);
  font-family: var(--font-sans);
  font-size: 14px;
  line-height: 1.5;
  outline: none;
  padding: 6px 0;
  min-height: 28px;
  max-height: 200px;
  overflow-y: auto;
}

.chat-input__field::placeholder {
  color: var(--color-text-muted);
}

.chat-input__field:disabled {
  color: var(--color-text-muted);
  cursor: not-allowed;
}

.chat-input__send {
  flex-shrink: 0;
  width: 32px;
  height: 32px;
  border-radius: 50%;
  border: none;
  background: var(--color-accent);
  color: #ffffff;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  font-size: 16px;
  font-family: inherit;
  padding: 0;
  transition: background 0.15s, opacity 0.15s;
}

.chat-input__send:hover:not(:disabled) {
  background: var(--color-accent-hover);
}

.chat-input__send:disabled {
  background: var(--color-bg-elevated);
  color: var(--color-text-muted);
  cursor: not-allowed;
  opacity: 0.6;
}

.chat-input__spinner {
  display: inline-block;
  animation: chat-input-spin 1s linear infinite;
  font-size: 20px;
  line-height: 1;
}

@keyframes chat-input-spin {
  to {
    transform: rotate(360deg);
  }
}

.chat-input__hint {
  margin-top: 8px;
  padding: 0 6px;
  font-size: 11px;
  color: var(--color-text-muted);
  user-select: none;
}
</style>
