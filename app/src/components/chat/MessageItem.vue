<script setup lang="ts">
// MessageItem — single chat message bubble. Renders (in order):
//   1. Thinking block (if any) — violet left bar, collapsed by default
//   2. Redacted-thinking notice (rare, opaque data preserved for LLM)
//   3. Tool call cards (one per tool_use, with the matching result
//      looked up from the same message's `toolResults`)
//   4. The visible text bubble (with the blinking streaming cursor)
//   5. The error footer (if the turn failed)
//
// The "is there a bubble" predicate mirrors the original ChatWindow
// logic: any of {content, toolCalls, toolResults, thinkingBlocks,
// redactedThinkingData} → no bubble. The bubble is the fallback for
// the plain-text-only case.
//
// Markdown rendering (PR6):
//   The bubble text is now `v-html`'d through a debounced marked +
//   DOMPurify pipeline. See `utils/markdown.ts` for the XSS story.
//   The 50ms debounce collapses bursts of SSE deltas into a single
//   re-render; on stream end we flush so the final frame doesn't
//   wait out the timer.

import { computed, watch, onUnmounted } from "vue";
import type { ChatMessage } from "../../stores/chat";
import { getToolResult } from "../../utils/messageFormat";
import { createDebouncedRenderer } from "../../utils/markdown";
import ThinkingBlock from "./ThinkingBlock.vue";
import ToolCallCard from "./ToolCallCard.vue";
import Icon from "../Icon.vue";

const props = defineProps<{
  message: ChatMessage;
}>();

const hasVisibleBubble = computed<boolean>(() => {
  const m = props.message;
  return (
    !!m.content ||
    !!(m.toolCalls && m.toolCalls.length) ||
    !!(m.toolResults && m.toolResults.length) ||
    !!(m.thinkingBlocks && m.thinkingBlocks.length) ||
    !!(m.redactedThinkingData && m.redactedThinkingData.length)
  );
});

const showBubble = computed<boolean>(
  () =>
    !!props.message.content ||
    (!props.message.toolCalls?.length &&
      !props.message.toolResults?.length &&
      !props.message.thinkingBlocks?.length &&
      !props.message.redactedThinkingData?.length),
);

const showStreamingHint = computed<boolean>(
  () => !!props.message.streaming && !props.message.content,
);

// --- Markdown pipeline ----------------------------------------------------
// `displayContent` is a thin pass-through to `message.content`. The
// leading-whitespace trim that PR7 put here now lives inside
// `renderMarkdown()` (see `app/src/utils/markdown.ts`) so the rendering
// layer owns its own input policy. We keep the named computed because
// (a) the watch below needs a stable dependency reference, and (b) it
// documents intent at the call site ("this is the text we render").
const displayContent = computed<string>(() => props.message.content);

const { rendered, schedule, flush, dispose } = createDebouncedRenderer(50);

watch(
  displayContent,
  (next) => {
    schedule(next);
  },
  { immediate: true },
);

// When the stream ends, render the final frame immediately so the user
// doesn't see a 50ms gap between the last delta and the rendered
// terminal state. `streaming` is `true` only while SSE is active.
watch(
  () => props.message.streaming,
  (isStreaming) => {
    if (!isStreaming) flush();
  },
);

onUnmounted(() => {
  dispose();
});
</script>

<template>
  <li :class="['msg', `msg--${message.role}`, { 'msg--err': message.error }]">
    <ThinkingBlock
      v-if="
        message.role === 'assistant' &&
        message.thinkingBlocks &&
        message.thinkingBlocks.length
      "
      :blocks="message.thinkingBlocks"
      :streaming="message.streaming"
      :show-streaming-hint="showStreamingHint"
    />

    <div
      v-if="message.redactedThinkingData && message.redactedThinkingData.length"
      class="msg__redacted"
      :title="`${message.redactedThinkingData.length} redacted thinking block(s); preserved verbatim for the LLM but not displayable`"
    >
      <Icon name="lock" :size="12" icon-class="msg__redacted-icon" />
      {{ message.redactedThinkingData.length }} redacted thinking block{{
        message.redactedThinkingData.length === 1 ? "" : "s"
      }}
      (preserved for LLM)
    </div>

    <div
      v-if="message.toolCalls && message.toolCalls.length"
      class="msg__tools"
    >
      <ToolCallCard
        v-for="tc in message.toolCalls"
        :key="tc.id"
        :call="tc"
        :result="getToolResult(message, tc.id)"
      />
    </div>

    <div v-if="showBubble" class="msg__bubble">
      <span
        v-if="hasVisibleBubble || message.content"
        class="msg__markdown"
        v-html="rendered"
      />
      <span v-if="message.streaming" class="msg__cursor" aria-hidden="true"
        >▍</span
      >
    </div>

    <div v-if="message.error" class="msg__error">
      <Icon name="warn" :size="12" icon-class="msg__error-icon" />
      {{ message.error.message }}
    </div>
  </li>
</template>

<style scoped>
.msg {
  display: flex;
  flex-direction: column;
  max-width: 75%;
}

.msg--user {
  align-self: flex-end;
  margin-right: 16px;
}

.msg--assistant {
  align-self: flex-start;
}

.msg__redacted {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
  padding: 4px 10px;
  background: var(--color-bg-elevated);
  border: 1px dashed var(--color-bg-border);
  border-radius: 6px;
  font-size: 11px;
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.msg__redacted-icon {
  flex-shrink: 0;
  color: var(--color-text-secondary);
}

.msg__tools {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 4px;
  max-width: 100%;
}

.msg__bubble {
  padding: 10px 14px;
  border-radius: 6px;
  /* `white-space: pre-wrap` removed in PR6 — markdown handles its own
     line breaks via `breaks: true` in the marked options, and
     pre-wrap would mangle <pre> code blocks (the leading whitespace
     on each line of code would be preserved literally, fighting the
     monospace font's own rendering). */
  word-break: break-word;
  line-height: 1.6;
  border: 1px solid var(--color-bg-border);

  margin-top: 4px;
  margin-bottom: 4px;
}

.msg--user .msg__bubble {
  background: var(--color-accent);
  color: #ffffff;
  border-color: var(--color-accent);
  border-bottom-right-radius: 2px;
}

.msg--assistant .msg__bubble {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  border-bottom-left-radius: 2px;
}

.msg--err .msg__bubble {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}

.msg__cursor {
  display: inline-block;
  margin-left: 2px;
  animation: blink 1s steps(1) infinite;
  color: var(--color-text-muted);
}

@keyframes blink {
  50% {
    opacity: 0;
  }
}

.msg__error {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-top: 4px;
  padding: 0 14px;
  font-size: 12px;
  color: var(--color-tool-error);
}

.msg__error-icon {
  flex-shrink: 0;
}

/* Markdown content (v-html). The HTML lives in a child tree without
   scoped classes, so every selector below uses :deep() to reach into
   the rendered output. Keep the list focused on elements marked
   actually produces — avoid hypothetical selectors that will never
   match and just become dead code. */
.msg__markdown {
  display: block;
}

.msg__markdown :deep(p) {
  margin: 0 0 8px 0;
}

.msg__markdown :deep(p:last-child) {
  margin-bottom: 0;
}

.msg__markdown :deep(h1),
.msg__markdown :deep(h2),
.msg__markdown :deep(h3),
.msg__markdown :deep(h4),
.msg__markdown :deep(h5),
.msg__markdown :deep(h6) {
  margin: 12px 0 6px 0;
  font-weight: 600;
  line-height: 1.3;
}

.msg__markdown :deep(h1) {
  font-size: 1.4em;
}
.msg__markdown :deep(h2) {
  font-size: 1.25em;
}
.msg__markdown :deep(h3) {
  font-size: 1.1em;
}
.msg__markdown :deep(h4) {
  font-size: 1em;
}

.msg__markdown :deep(h1:first-child),
.msg__markdown :deep(h2:first-child),
.msg__markdown :deep(h3:first-child),
.msg__markdown :deep(h4:first-child) {
  margin-top: 0;
}

.msg__markdown :deep(ul),
.msg__markdown :deep(ol) {
  margin: 6px 0;
  padding-left: 24px;
}

.msg__markdown :deep(li) {
  margin: 2px 0;
}

.msg__markdown :deep(strong) {
  font-weight: 600;
}

.msg__markdown :deep(em) {
  font-style: italic;
}

.msg__markdown :deep(code) {
  font-family: var(--font-mono);
  font-size: 0.9em;
  padding: 1px 5px;
  border-radius: 3px;
  background: rgba(255, 255, 255, 0.08);
  border: 1px solid var(--color-bg-border-strong);
}

.msg__markdown :deep(pre) {
  margin: 8px 0;
  padding: 10px 12px;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid var(--color-bg-border-strong);
  border-radius: 6px;
  overflow-x: auto;
  line-height: 1.45;
}

.msg__markdown :deep(pre code) {
  padding: 0;
  background: transparent;
  border: 0;
  font-size: 0.9em;
  white-space: pre;
}

.msg__markdown :deep(a) {
  color: var(--color-accent);
  text-decoration: underline;
  text-underline-offset: 2px;
}

.msg__markdown :deep(blockquote) {
  margin: 8px 0;
  padding: 4px 12px;
  border-left: 3px solid var(--color-bg-border);
  color: var(--color-text-secondary);
  font-style: italic;
}

.msg__markdown :deep(hr) {
  border: 0;
  border-top: 1px solid var(--color-bg-border);
  margin: 12px 0;
}

.msg__markdown :deep(table) {
  border-collapse: collapse;
  margin: 8px 0;
  font-size: 0.95em;
}

.msg__markdown :deep(th),
.msg__markdown :deep(td) {
  /* Stronger border color than --color-bg-border because table cells
     sit on --color-bg-elevated (the bubble) and the regular border
     reads as invisible (only 4 luminance units of separation). */
  border: 1px solid var(--color-bg-border-strong);
  padding: 4px 8px;
  text-align: left;
}

.msg__markdown :deep(th) {
  background: var(--color-bg);
  font-weight: 600;
}
</style>
