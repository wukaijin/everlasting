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
// Hint row layout (F5 follow-up):
// - LEFT: LLM cumulative chip (clock icon + "Σ 1.2s" / "—") backed
//   by a CLICKABLE popover that breaks the running total into a
//   per-turn TTFB / Gen / Total list. Replaces the old
//   "⏎ 发送 · ⇧⏎ 换行 · @ 引用文件 · / 命令" text — the keyboard
//   hints are still documented here in the comment block but the
//   the on-screen real estate now goes to the latency summary (which
//   is useful during streaming, whereas the keyboard hint never
//   changed and just ate horizontal space).
// - CENTER: per-session token usage chip (reka-ui Tooltip on hover,
//   "14.2K · 7% / 200K" with green/yellow/red thresholds and a 4-row
//   breakdown tooltip). Unchanged from A4.
// - RIGHT: model picker popover (ModelSelect, opens UP). Unchanged.
//
// A4 (Token Usage Tracking): the hint row's center token-usage chip
// keeps its 50%/75% color thresholds and the "升级前未统计" fallback
// for pre-A4 sessions (the four columns are NULL). Brand-new sessions
// before their first LLM turn render as "—".
//
// F5 (LLM Latency Tracking) follow-up: the left chip renders "—"
// for pre-F5 / brand-new sessions (currentSessionLatencyTotal ===
// null). For sessions with at least one recorded turn, it shows
// the cumulative Σ totalMs formatted via `abbreviateDuration`. The
// popover (click-triggered, NOT hover) shows the per-turn list
// (TTFB / Gen / Total per assistant message) plus a header with
// 累计 / 轮次 / 平均 three rows. Pre-F5 / no-records sessions
// show the three rows as "—" / 0 / "—" with the "本次 session
// 还没有 LLM 耗时数据" empty footer. Click-outside / Esc closes
// the popover. The popover is hand-written (ModelSelect style)
// instead of reka-ui's `PopoverRoot` because (a) we already have
// the hand-written pattern in the codebase, (b) the layout needs
// a scrollable list with a sticky header, and (c) the reka-ui
// `PopoverRoot` would require an extra import for one user.

import { computed, onUnmounted, ref } from "vue";
import { TooltipProvider, TooltipRoot, TooltipTrigger, TooltipPortal, TooltipContent, TooltipArrow } from "reka-ui";
import Icon from "../Icon.vue";
import ModelSelect from "./ModelSelect.vue";
import { useChatStore } from "../../stores/chat";
import { useModelsStore } from "../../stores/models";
import { abbreviateTokens, tokenUsageLevel, type TokenUsageLevel } from "../../utils/tokenUsage";
import { abbreviateDuration } from "../../utils/duration";
import { colorTagHex, hexToRgba } from "../../utils/colorTag";

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

// D1: conditional background tint on chat-input__row from session color tag.
const inputRowStyle = computed(() => {
  const s = chatStore.sessions.find((x) => x.id === chatStore.currentSessionId);
  if (!s || s.color_tag === null) return {};
  const hex = colorTagHex(s.color_tag);
  if (!hex) return {};
  return { backgroundColor: hexToRgba(hex, 0.2) };
});

// -----------------------------------------------------------------------
// F5 follow-up: LLM cumulative latency summary chip + clickable popover.
// Mirrors the ModelSelect hand-written popover pattern (open/close
// ref, click-outside + Esc handlers). The chip itself is just a
// clock icon + "Σ 1.2s" label; clicking it opens the popover with
// the per-turn breakdown. The trigger is hidden when no session is
// active (matches the A4 token-usage chip's "no session → don't
// render" rule).
// -----------------------------------------------------------------------

const latencyPopoverOpen = ref(false);
const latencyPopoverRoot = ref<HTMLElement | null>(null);

function toggleLatencyPopover() {
  latencyPopoverOpen.value = !latencyPopoverOpen.value;
}

/** Click outside the latency popover root closes it. Mirrors
 *  `ModelSelect.onDocumentClick` and the worktree dropdown's
 *  pattern. */
function onDocumentClick(e: MouseEvent) {
  if (!latencyPopoverOpen.value) return;
  const target = e.target as Node | null;
  if (
    latencyPopoverRoot.value &&
    target &&
    !latencyPopoverRoot.value.contains(target)
  ) {
    latencyPopoverOpen.value = false;
  }
}

/** Esc closes the latency popover. Bound on `window` because
 *  the trigger button may not have focus when the popover is
 *  open. Same pattern as ModelSelect. */
function onKeyDown(e: KeyboardEvent) {
  if (e.key === "Escape" && latencyPopoverOpen.value) {
    latencyPopoverOpen.value = false;
  }
}

if (typeof document !== "undefined") {
  document.addEventListener("click", onDocumentClick);
}
if (typeof window !== "undefined") {
  window.addEventListener("keydown", onKeyDown);
}
onUnmounted(() => {
  if (typeof document !== "undefined") {
    document.removeEventListener("click", onDocumentClick);
  }
  if (typeof window !== "undefined") {
    window.removeEventListener("keydown", onKeyDown);
  }
});

/** Per-turn latency list for the popover breakdown. `null` →
 *  no session active (chip hidden). `[]` → active session but
 *  no turns recorded yet (chip renders "—"). Non-empty → the
 *  popover renders a row per turn. */
const latencyTurns = computed(() => chatStore.currentSessionLatencyTurns);

/** Average totalMs across recorded turns. Computed live from
 *  the per-turn list (no separate counter needed). Returns
 *  `null` when no turns have been recorded. */
const latencyAverage = computed<number | null>(() => {
  const t = latencyTurns.value;
  if (!t || t.length === 0) return null;
  let sum = 0;
  let count = 0;
  for (const x of t) {
    if (typeof x.totalMs === "number") {
      sum += x.totalMs;
      count++;
    }
  }
  return count > 0 ? sum / count : null;
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

function onEscKeydown() {
  if (props.sending) {
    onStop();
  }
}
</script>

<template>
  <footer class="chat-input" @keydown.escape.prevent="onEscKeydown">
    <div class="chat-input__row" :style="inputRowStyle">
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
      <!-- F5 follow-up: LLM cumulative latency chip (LEFT).
           Renders the Σ totalMs of every recorded assistant turn
           in the active session. Clicking opens a popover with a
           per-turn breakdown (TTFB / Gen / Total). Pre-F5 / no
           session / no recorded turns → "—". -->
      <div
        v-if="chatStore.currentSessionId"
        ref="latencyPopoverRoot"
        class="chat-input__latency"
      >
        <button
          type="button"
          class="chat-input__latency-chip"
          :class="{
            'chat-input__latency-chip--open': latencyPopoverOpen,
          }"
          :aria-haspopup="'dialog'"
          :aria-expanded="latencyPopoverOpen"
          :title="
            chatStore.currentSessionLatencyTotal !== null
              ? '点击查看本次 session LLM 累计耗时明细'
              : '本次 session 还没有 LLM 耗时数据'
          "
          @click="toggleLatencyPopover"
        >
          <Icon name="clock" :size="11" />
          <span class="chat-input__latency-label">LLM</span>
          <span class="chat-input__latency-value">
            {{
              chatStore.currentSessionLatencyTotal !== null
                ? abbreviateDuration(chatStore.currentSessionLatencyTotal)
                : "—"
            }}
          </span>
        </button>
        <Transition name="chat-input-latency-popover">
          <div
            v-if="latencyPopoverOpen"
            class="chat-input__latency-popover"
            role="dialog"
            aria-label="LLM 累计耗时明细"
          >
            <div class="chat-input__latency-popover-header">
              <Icon name="clock" :size="11" />
              <span>本次 session LLM 累计耗时</span>
            </div>
            <div class="chat-input__latency-popover-summary">
              <div class="chat-input__latency-popover-row">
                <span>累计</span>
                <span class="chat-input__latency-popover-strong">
                  {{
                    chatStore.currentSessionLatencyTotal !== null
                      ? abbreviateDuration(chatStore.currentSessionLatencyTotal)
                      : "—"
                  }}
                </span>
              </div>
              <div class="chat-input__latency-popover-row">
                <span>轮次</span>
                <span>{{ latencyTurns?.length ?? 0 }}</span>
              </div>
              <div class="chat-input__latency-popover-row">
                <span>平均</span>
                <span>
                  {{ latencyAverage !== null ? abbreviateDuration(latencyAverage) : "—" }}
                </span>
              </div>
            </div>
            <div
              v-if="latencyTurns && latencyTurns.length > 0"
              class="chat-input__latency-popover-list"
            >
              <div
                v-for="(turn, i) in latencyTurns"
                :key="i"
                class="chat-input__latency-popover-turn"
              >
                <div class="chat-input__latency-popover-turn-head">
                  <span>turn {{ i + 1 }}</span>
                  <span class="chat-input__latency-popover-strong">
                    {{ turn.totalMs !== undefined ? abbreviateDuration(turn.totalMs) : "—" }}
                  </span>
                </div>
                <div class="chat-input__latency-popover-turn-detail">
                  <span>TTFB</span>
                  <span>{{ turn.ttfbMs !== undefined ? abbreviateDuration(turn.ttfbMs) : "—" }}</span>
                </div>
                <div class="chat-input__latency-popover-turn-detail">
                  <span>gen</span>
                  <span>{{ turn.genMs !== undefined ? abbreviateDuration(turn.genMs) : "—" }}</span>
                </div>
              </div>
            </div>
            <div v-else class="chat-input__latency-popover-empty">
              本次 session 还没有 LLM 耗时数据
            </div>
          </div>
        </Transition>
      </div>
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

/* F5 follow-up: LLM cumulative latency chip (LEFT of hint row).
   Shape matches the existing token-usage chip and the A4 color
   thresholds family, but it's a real clickable button (cursor
   pointer) that opens a popover. Uses the same `color-bg-elevated`
   base + `color-bg-border` outline as the worktree chip and the
   `ModelSelect` trigger, so the visual family is consistent. */
.chat-input__latency {
  position: relative;
  display: inline-flex;
  flex-shrink: 0;
}

.chat-input__latency-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--color-text-muted);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  cursor: pointer;
  user-select: none;
  font: inherit;
  font-family: var(--font-mono);
  font-size: 11px;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}

.chat-input__latency-chip:hover {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-text-primary);
}

.chat-input__latency-chip--open {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-text-primary);
}

.chat-input__latency-label {
  color: var(--color-text-secondary);
}

.chat-input__latency-value {
  color: var(--color-text-primary);
  font-weight: 600;
}

/* The latency popover (F5 follow-up). Hand-written like
   ModelSelect's `.model-select__menu` — opens UPWARD because the
   trigger sits at the bottom of the chat panel; opening down
   would clip under the next sibling. Width is enough to fit the
   longest "0.0s · 0.0s · 0.0s" line without overflow. The list
   area scrolls when there are too many turns (rare, but a 50-turn
   session shouldn't break the layout). */
.chat-input__latency-popover {
  position: absolute;
  bottom: calc(100% + 4px);
  top: auto;
  left: 0;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  min-width: 220px;
  max-width: 280px;
  max-height: 320px;
  z-index: 200;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  font-size: 11px;
  color: var(--color-text-primary);
  font-family: var(--font-mono);
}

.chat-input__latency-popover-header {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 10px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--color-text-muted);
  padding-bottom: 4px;
  border-bottom: 1px solid var(--color-bg-border);
}

.chat-input__latency-popover-summary {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.chat-input__latency-popover-row {
  display: flex;
  justify-content: space-between;
  gap: 16px;
}

.chat-input__latency-popover-row > span:first-child {
  color: var(--color-text-secondary);
}

.chat-input__latency-popover-strong {
  color: var(--color-text-primary);
  font-weight: 600;
}

.chat-input__latency-popover-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  overflow-y: auto;
  max-height: 200px;
  padding-top: 4px;
  border-top: 1px solid var(--color-bg-border);
}

.chat-input__latency-popover-turn {
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 4px 0;
}

.chat-input__latency-popover-turn + .chat-input__latency-popover-turn {
  border-top: 1px dashed var(--color-bg-border);
}

.chat-input__latency-popover-turn-head {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  font-weight: 500;
}

.chat-input__latency-popover-turn-detail {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  color: var(--color-text-secondary);
  padding-left: 8px;
}

.chat-input__latency-popover-empty {
  color: var(--color-text-muted);
  text-align: center;
  padding: 6px 0;
}

/* Open/close animation. The popover opens UP, so it slides
   from translateY(4px) (slightly below the final position) up
   into place. Exit reverses. Matches the ModelSelect pattern. */
.chat-input-latency-popover-enter-active,
.chat-input-latency-popover-leave-active {
  transition: opacity 150ms ease-out, transform 150ms ease-out;
  transform-origin: bottom left;
}

.chat-input-latency-popover-enter-from,
.chat-input-latency-popover-leave-to {
  opacity: 0;
  transform: translateY(4px);
}

.chat-input-latency-popover-leave-active {
  transition-duration: 100ms;
  transition-timing-function: ease-in;
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
