<script setup lang="ts">
// MessageList — the <ul> of visible messages. Watches the store's
// `messages` ref for both length changes (new message arrives) and
// content churn (text/thinking streaming) and auto-scrolls to keep
// the latest line in view.
//
// "Ghost" messages (post-rehydrate user messages that exist only to
// carry tool_result blocks for the LLM) are filtered out here so
// the chat list stays clean. Pure-thinking assistant messages
// (no text, no tool calls) are kept visible so the user can see
// the model's reasoning even when it never produced a reply.

import { ref, watch, nextTick, computed } from "vue";
import { useChatStore } from "../../stores/chat";
import MessageItem from "./MessageItem.vue";

const store = useChatStore();
const messagesEl = ref<HTMLElement | null>(null);

const visibleMessages = computed(() =>
  store.messages.filter(
    (m) =>
      m.content ||
      m.toolCalls?.length ||
      m.error ||
      (m.thinkingBlocks && m.thinkingBlocks.length > 0) ||
      (m.redactedThinkingData && m.redactedThinkingData.length > 0),
  ),
);

async function scrollToBottom() {
  await nextTick();
  if (messagesEl.value) {
    messagesEl.value.scrollTop = messagesEl.value.scrollHeight;
  }
}

// Auto-scroll on any new content. The "length" key covers new
// messages arriving; the content hash covers streaming deltas.
watch(
  () =>
    store.messages
      .map(
        (m) =>
          m.content +
          (m.toolCalls?.length ?? 0) +
          (m.toolResults?.length ?? 0) +
          (m.thinkingBlocks?.reduce((n, b) => n + b.text.length, 0) ?? 0) +
          (m.redactedThinkingData?.length ?? 0),
      )
      .join("|"),
  () => scrollToBottom(),
);

watch(
  () => store.messages.length,
  () => scrollToBottom(),
);
</script>

<template>
  <ul ref="messagesEl" class="messages">
    <MessageItem
      v-for="m in visibleMessages"
      :key="m.id"
      :message="m"
    />
  </ul>
</template>

<style scoped>
.messages {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
  flex: 1;
  overflow-y: auto;
}
</style>
