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

/** True when the user is within `threshold` pixels of the bottom of
 *  the scroll container. Used to decide whether a content change
 *  should yank the user to the bottom or leave them in place to
 *  keep reading older messages. */
function isNearBottom(el: HTMLElement, threshold = 80): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight < threshold;
}

async function scrollToBottom() {
  await nextTick();
  if (messagesEl.value) {
    messagesEl.value.scrollTop = messagesEl.value.scrollHeight;
  }
}

// Auto-scroll on any new content — but only when the user is
// already near the bottom. If they've scrolled up to read older
// messages, the new streaming content should appear in place
// below them, not yank them to the bottom.
//
// `flush: "pre"` is required so the watch callback runs BEFORE
// Vue's DOM update; that's the only moment when `scrollHeight`
// still reflects the pre-change geometry, and therefore the
// only moment when `isNearBottom` returns a meaningful answer
// (after the DOM flush, the new content has grown the
// scrollHeight and the predicate would always be true).
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
  () => {
    if (!messagesEl.value) return;
    const wasNearBottom = isNearBottom(messagesEl.value);
    void nextTick().then(() => {
      if (wasNearBottom) scrollToBottom();
    });
  },
  { flush: "pre" },
);

// When the user switches sessions, jump to the bottom of the new
// session. Session-switch is an explicit user action, so the
// "don't interrupt reading" guard above does not apply — the
// user has chosen to leave the old context.
//
// The 100ms retry handles the case where `controller.ensureLoaded`
// populates messages across multiple frames (DB load + rehydrate
// step). If the first scroll lands on an empty list, the second
// will land on the populated one.
watch(
  () => store.currentSessionId,
  async (newId, oldId) => {
    if (newId === oldId) return;
    await scrollToBottom();
    setTimeout(scrollToBottom, 100);
  },
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
