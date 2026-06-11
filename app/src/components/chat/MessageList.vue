<script setup lang="ts">
// MessageList — the <ul> of visible messages. Watches the store's
// `messages` ref for both length changes (new message arrives) and
// content churn (text/thinking streaming) and auto-scrolls to keep
// the latest line in view.
//
// F2: "force follow" mode — after sending, auto-scroll tracks every
// delta regardless of user position. The user can opt out by
// scrolling up >80px. The mode resets when the stream finishes
// (streamController sets store.forceFollowActive = false on done/error).

import { ref, watch, nextTick, computed, onMounted, onUnmounted } from "vue";
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

function isNearBottom(el: HTMLElement, threshold = 80): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight < threshold;
}

async function scrollToBottom() {
  await nextTick();
  if (messagesEl.value) {
    messagesEl.value.scrollTop = messagesEl.value.scrollHeight;
  }
}

// F2: detect user manual scroll-up. When forceFollowActive is true
// and the user scrolls up >80px, cancel the follow mode.
function onScroll() {
  if (!messagesEl.value || !store.forceFollowActive) return;
  if (!isNearBottom(messagesEl.value, 80)) {
    store.forceFollowActive = false;
  }
}

// Auto-scroll on any content change. During force-follow mode, always
// scroll; otherwise only scroll when user is near the bottom.
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
    const shouldFollow =
      store.forceFollowActive || isNearBottom(messagesEl.value);
    if (!shouldFollow) return;
    void nextTick().then(() => scrollToBottom());
  },
  { flush: "pre" },
);

// When the user switches sessions, jump to the bottom of the new
// session. Session-switch is an explicit user action, so the
// "don't interrupt reading" guard does not apply.
watch(
  () => store.currentSessionId,
  async (newId, oldId) => {
    if (newId === oldId) return;
    await scrollToBottom();
    setTimeout(scrollToBottom, 100);
  },
);

// F4: after reloadAfterFinalize replaces the streaming buffer with
// DB messages, re-scroll to bottom to avoid position jitter.
watch(() => store.scrollAfterReload, () => {
  void scrollToBottom();
});

onMounted(() => {
  messagesEl.value?.addEventListener("scroll", onScroll, { passive: true });
});
onUnmounted(() => {
  messagesEl.value?.removeEventListener("scroll", onScroll);
});
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
