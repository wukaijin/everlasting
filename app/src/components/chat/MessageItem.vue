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

import { computed } from "vue";
import type { ChatMessage } from "../../stores/chat";
import { getToolResult } from "../../utils/messageFormat";
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

// Strip leading whitespace from the rendered text. Anthropic's SSE
// stream commonly emits a leading "\n" right after the role marker
// (the model's first content_block_start is the text block, and the
// delta often begins with "\n\n"). Combined with `white-space:
// pre-wrap` in `.msg__bubble` that would render as a visible blank
// first line. We trim at the display layer so the DB / rehydration
// / wire format all keep the raw LLM text untouched (markdown tools
// in a future PR can decide their own leading-whitespace policy).
// Idempotent on already-trimmed strings, so streaming deltas are
// safe (the first delta gets trimmed, subsequent append-only deltas
// are no-ops).
const displayContent = computed<string>(() =>
  props.message.content.replace(/^\s+/, ""),
);
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
      v-if="
        message.redactedThinkingData && message.redactedThinkingData.length
      "
      class="msg__redacted"
      :title="`${message.redactedThinkingData.length} redacted thinking block(s); preserved verbatim for the LLM but not displayable`"
    >
      <Icon name="lock" :size="12" icon-class="msg__redacted-icon" />
      {{ message.redactedThinkingData.length }} redacted thinking
      block{{ message.redactedThinkingData.length === 1 ? "" : "s" }}
      (preserved for LLM)
    </div>

    <div v-if="message.toolCalls && message.toolCalls.length" class="msg__tools">
      <ToolCallCard
        v-for="tc in message.toolCalls"
        :key="tc.id"
        :call="tc"
        :result="getToolResult(message, tc.id)"
      />
    </div>

    <div v-if="showBubble" class="msg__bubble">
      <span v-if="hasVisibleBubble || message.content" class="msg__text">
        {{ displayContent }}
      </span>
      <span v-if="message.streaming" class="msg__cursor" aria-hidden="true">▍</span>
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
}

.msg--assistant {
  align-self: flex-start;
}

.msg__redacted {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 6px;
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
  margin-top: 8px;
  max-width: 100%;
}

.msg__bubble {
  padding: 10px 14px;
  border-radius: 12px;
  white-space: pre-wrap;
  word-break: break-word;
  line-height: 1.6;
  border: 1px solid var(--color-bg-border);
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
</style>
