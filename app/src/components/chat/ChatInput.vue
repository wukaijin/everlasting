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
//
// A4 (Token Usage Tracking): the hint row is split into two regions.
// Left: the original keyboard shortcuts ("⏎ 发送 · ⇧⏎ 换行 · @ 引用文件 · / 命令").
// Center: the per-session token usage chip
// ("14.2K · 7% / 200K") with color thresholds (green < 50%, yellow
// 50-74%, red >= 75%) and a reka-ui Tooltip on hover that breaks down
// the four counters (input / cache_read / cache_creation / output).
// Right: the PR5 ModelSelect popover (unchanged).
//
// Pre-A4 sessions (the four columns are NULL) render as "—" with the
// tooltip "升级前未统计". Brand-new sessions before their first LLM
// turn also render as "—". A session that has accumulated 0 tokens
// after at least one turn (e.g. a network-error turn) still renders
// the number; the ChatInput doesn't special-case zero.

import { computed, ref } from "vue";
import { TooltipProvider, TooltipRoot, TooltipTrigger, TooltipPortal, TooltipContent, TooltipArrow } from "reka-ui";
import Icon from "../Icon.vue";
import ModelSelect from "./ModelSelect.vue";
import { useChatStore } from "../../stores/chat";
import { useModelsStore } from "../../stores/models";
import { abbreviateTokens, tokenUsageLevel, type TokenUsageLevel } from "../../utils/tokenUsage";

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

// A4: per-session token usage — read from the chat store's
// reactive `currentSessionTokenUsage`. The model store provides
// the context window for the percentage denominator. We only
// need the default model's context_window; the model picker
// popover already exposes the selected model and updates this
// value on switch.
const chatStore = useChatStore();
const modelsStore = useModelsStore();

/** The model row backing the current session, or `null` for
 *  sessions that haven't resolved to a model yet (very
 *  early in the app lifecycle, before the catalog loads). The
 *  percentage denominator is `defaultModel.contextWindow` —
 *  the chat command always uses the default model for
 *  resolve-default fallback; a per-session override is also
 *  possible but the user explicitly picks that, and the
 *  percentage uses the same `defaultModel` for visual
 *  stability (a session mid-stream with a per-session override
 *  would still see "X% / 200K" of the default's window). */
const currentModelContextWindow = computed<number>(() => {
  const m = modelsStore.defaultModel;
  return m?.contextWindow ?? 200_000;
});

/** Color threshold for the percentage bar. Matches the
 *  PRD §Q4 decision 6 (50% yellow, 75% red):
 *  - 0-49% → green
 *  - 50-74% → yellow
 *  - 75%+ → red.
 *
 *  The actual band lookup lives in `utils/tokenUsage.ts` so the
 *  boundaries (49/50/74/75) can be unit-tested without spinning
 *  up a Vue renderer + Pinia store. */
const usageLevel = computed<TokenUsageLevel | null>(() => {
  const u = chatStore.currentSessionTokenUsage;
  if (!u) return null;
  const pct = u.input_tokens / currentModelContextWindow.value;
  return tokenUsageLevel(pct);
});

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
      <!-- A4: token usage chip. Render-mode depends on
           whether the session has accumulated any usage:
           - null → "—" with the "升级前未统计" tooltip
           - non-null → the percentage line; tooltip breaks
             the four counters down.
           Color thresholds are 50% (yellow) and 75% (red);
           see `usageLevel` computed above. -->
      <TooltipProvider>
        <TooltipRoot>
          <TooltipTrigger
            as-child
          >
            <span
              class="chat-input__token-usage"
              :class="{
                [`chat-input__token-usage--${usageLevel}`]: usageLevel,
              }"
            >
              <template v-if="chatStore.currentSessionTokenUsage">
                {{ abbreviateTokens(chatStore.currentSessionTokenUsage.input_tokens) }}
                ·
                {{
                  Math.min(
                    100,
                    Math.round(
                      (chatStore.currentSessionTokenUsage.input_tokens /
                        currentModelContextWindow) *
                        100,
                    ),
                  )
                }}% / {{ abbreviateTokens(currentModelContextWindow) }}
              </template>
              <template v-else>—</template>
            </span>
          </TooltipTrigger>
          <TooltipPortal>
            <TooltipContent class="chat-input__token-tooltip" :side-offset="6">
              <template v-if="chatStore.currentSessionTokenUsage">
                <div class="chat-input__token-tooltip-row">
                  <span>input</span>
                  <span>{{ abbreviateTokens(chatStore.currentSessionTokenUsage.input_tokens) }}</span>
                </div>
                <div class="chat-input__token-tooltip-row">
                  <span>cache_read</span>
                  <span>{{ abbreviateTokens(chatStore.currentSessionTokenUsage.cache_read_input_tokens) }}</span>
                </div>
                <div class="chat-input__token-tooltip-row">
                  <span>cache_creation</span>
                  <span>{{ abbreviateTokens(chatStore.currentSessionTokenUsage.cache_creation_input_tokens) }}</span>
                </div>
                <div class="chat-input__token-tooltip-row">
                  <span>output</span>
                  <span>{{ abbreviateTokens(chatStore.currentSessionTokenUsage.output_tokens) }}</span>
                </div>
              </template>
              <template v-else>
                <div class="chat-input__token-tooltip-empty">升级前未统计</div>
              </template>
              <TooltipArrow class="chat-input__token-tooltip-arrow" :size="6" />
            </TooltipContent>
          </TooltipPortal>
        </TooltipRoot>
      </TooltipProvider>
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

/* A4 (Token Usage Tracking): the per-session token usage
   chip in the hint row. The chip is a TooltipTrigger
   (reka-ui); the trigger itself has no role, the span is
   the visual target. The three color states map to the
   threshold ladder:
   - ok (0-49%): subtle green tint, still readable on dark
   - warn (50-74%): amber, calls attention
   - alert (75%+): red, stops the eye */
.chat-input__token-usage {
  display: inline-flex;
  align-items: center;
  padding: 0 6px;
  font-size: 11px;
  font-family: var(--font-mono);
  white-space: nowrap;
  cursor: help;
  border-radius: 4px;
  color: var(--color-text-muted);
  transition: color 0.15s;
  user-select: none;
}

.chat-input__token-usage--ok {
  color: #4ade80; /* green-400 — readable on dark, doesn't shout */
}

.chat-input__token-usage--warn {
  color: #fbbf24; /* amber-400 — matches --color-tool-shell family */
}

.chat-input__token-usage--alert {
  color: var(--color-tool-error);
}

/* Tooltip content (reka-ui `TooltipContent` portal to body
   — must use :deep() per `.trellis/spec/frontend/reka-ui-usage.md`
   gotcha). The popover floats above the trigger (default
   side is "top" since the chat input is at the bottom of the
   viewport). */
:deep(.chat-input__token-tooltip) {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  padding: 8px 10px;
  min-width: 180px;
  z-index: 3000;
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  animation: chat-input-tooltip-enter 150ms ease-out;
}

:deep(.chat-input__token-tooltip-row) {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  padding: 2px 0;
}

:deep(.chat-input__token-tooltip-row span:first-child) {
  color: var(--color-text-secondary);
}

:deep(.chat-input__token-tooltip-empty) {
  color: var(--color-text-muted);
  text-align: center;
  padding: 2px 0;
}

:deep(.chat-input__token-tooltip-arrow) {
  fill: var(--color-bg-surface);
  stroke: var(--color-bg-border);
}

@keyframes chat-input-tooltip-enter {
  from {
    opacity: 0;
    transform: translateY(2px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}
</style>
