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
//
// PR5: when `sending` is true, the right-side send button morphs into
// a Stop button. Clicking it emits `stop`; the parent calls
// `chatStore.cancel()`. The disabled-while-streaming state of the
// input itself is unchanged — the user can still see what's being
// streamed; they just can't type a new message until the stream ends
// (or they hit Stop and the stream bails out).

import { ref } from "vue";
import Icon from "../Icon.vue";
import ModelSelect from "./ModelSelect.vue";

const props = defineProps<{
  /** True while the model is generating. Disables the input. */
  sending: boolean;
  /** Placeholder text shown when empty. */
  placeholder?: string;
}>();

const emit = defineEmits<{
  send: [text: string];
  stop: [];
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

function onStop() {
  emit("stop");
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

const sendDisabled = (): boolean => props.sending || !input.value.trim();
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
      <!-- PR5: morph the send button into a Stop button while
           `sending` is true. We use the same accent color for
           visual continuity; the stop glyph is a CSS-rendered
           square (no extra icon import — heroicons 2.x has no
           StopIcon). The button is always enabled (even when the
           input is empty) so the user can interrupt a long
           stream with no draft. -->
      <button
        v-if="sending"
        class="chat-input__action chat-input__stop"
        aria-label="停止生成"
        @click="onStop"
      >
        <span class="chat-input__stop-glyph" aria-hidden="true"></span>
      </button>
      <button
        v-else
        class="chat-input__action chat-input__send"
        :disabled="sendDisabled()"
        aria-label="发送"
        @click="onSubmit"
      >
        <Icon name="arrow-up" :size="16" />
      </button>
    </div>
    <div class="chat-input__hint">
      <span class="chat-input__hint-text">⏎ 发送 · ⇧⏎ 换行 · @ 引用文件 · / 命令</span>
      <!-- PR5: model picker popover (upward-opening) attached to
           the right edge of the hint row. Replaces the
           bottom-of-content `StatusBar` from PR4. -->
      <ModelSelect />
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

/* Shared shape for both the Send and Stop action buttons. PR5
   factored the common width/height/border-radius/padding out of
   the old `.chat-input__send` rule so the new Stop variant can
   reuse it without duplicating pixel values. */
.chat-input__action {
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

/* PR5 Stop button. Uses a different background so the visual cue
   "this will halt the stream" is unambiguous, and the square
   glyph differentiates it from the up-arrow Send icon. The
   `warn` tool-error color (a warm orange) reads as "danger,
   cancel" without being as harsh as the actual error red. */
.chat-input__stop {
  background: var(--color-tool-error);
}

.chat-input__stop:hover {
  background: color-mix(in srgb, var(--color-tool-error) 80%, #000 20%);
}

/* Tiny centered square — the universal "stop" pictogram. 10×10
   in a 32px button reads as a solid stop block on both standard
   and high-DPI displays. */
.chat-input__stop-glyph {
  display: block;
  width: 10px;
  height: 10px;
  background: #ffffff;
  border-radius: 2px;
}

.chat-input__spinner {
  animation: chat-input-spin 1s linear infinite;
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
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.chat-input__hint-text {
  flex: 1;
  min-width: 0;
}
</style>
