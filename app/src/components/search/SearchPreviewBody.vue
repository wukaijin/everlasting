<script setup lang="ts">
// SearchPreviewBody — D2 (08-17-cross-session-search): read-only
// message-list rendering of an arbitrary session, mounted inside
// the SearchModal's preview state.
//
// Reuse granularity (design §4.3): NOT MessageList (it reads
// `store.messages` directly — coupling it to the current session —
// and its scroll logic is entangled with streaming force-follow).
// Instead this component reuses the two REAL rendering pieces:
//   - `MessageItem` (prop-driven; `readonly` structurally disables
//     the hover edit menu so preview actions can never fire against
//     the wrong session), and
//   - `buildRunGroups` (extracted from MessageList so the
//     interleaved-thinking run grouping is pixel-identical).
//
// Data path mirrors streamController's `ensureLoaded` shape
// (`load_session` IPC → `rehydrateMessages`) but WITHOUT the LRU,
// the currentSessionId swap, or the lastSession persistence — a
// pure snapshot. A session that is streaming right now simply shows
// its persisted-so-far rows.
//
// Positioning: each message's `data-seq` lands on MessageItem's
// root <li> via attribute fallthrough; after mount we scroll the
// hit into view (center) and flash a highlight. MessageList is not
// virtualized, so the whole list is in the DOM — scrollIntoView
// needs no virtual-anchor machinery.

import { computed, nextTick, onMounted, ref, watch } from "vue";
import { transport } from "../../transport";
import { rehydrateMessages, type LoadedSession } from "../../stores/streamRehydrate";
import type { ChatMessage } from "../../stores/chat.types";
import { buildRunGroups, type RunGroup } from "../../utils/messageFormat";
import MessageItem from "../chat/MessageItem.vue";

const props = defineProps<{
  /** Session to render (read-only snapshot). */
  sessionId: string;
  /** Message seq to scroll to + highlight. `null` → just render. */
  targetSeq: number | null;
}>();

const loading = ref(false);
const error = ref<string | null>(null);
const messages = ref<ChatMessage[]>([]);
const scrollEl = ref<HTMLElement | null>(null);

// Same visibility predicate as MessageList's `visibleMessages` —
// drop rows with nothing to show (pure tool_result carriers keep
// their bubbles via toolResults).
const visibleMessages = computed(() =>
  messages.value.filter(
    (m) =>
      m.content ||
      m.toolCalls?.length ||
      m.error ||
      (m.thinkingBlocks && m.thinkingBlocks.length > 0) ||
      (m.redactedThinkingData && m.redactedThinkingData.length > 0),
  ),
);
const renderGroups = computed<RunGroup<ChatMessage>[]>(() =>
  buildRunGroups(visibleMessages.value),
);

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  messages.value = [];
  try {
    const loaded = await transport.invoke<LoadedSession | null>("load_session", {
      sessionId: props.sessionId,
    });
    messages.value = loaded ? rehydrateMessages(loaded.messages) : [];
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

/** Scroll the target message to the viewport center + flash the
 *  `--search-hit` highlight. Scoped CSS reaches MessageItem's root
 *  <li> because a child component's root carries the parent's
 *  scope id. */
async function revealTarget(): Promise<void> {
  if (props.targetSeq === null) return;
  await nextTick();
  // A nextTick after data load isn't always enough for the browser
  // to lay out the freshly mounted list — requestAnimationFrame
  // before measuring/scrolling is the standard fix.
  requestAnimationFrame(() => {
    const el = scrollEl.value?.querySelector(
      `[data-seq="${props.targetSeq}"]`,
    ) as HTMLElement | null;
    if (!el) return;
    el.scrollIntoView({ block: "center" });
    el.classList.add("search-hit");
    window.setTimeout(() => el.classList.remove("search-hit"), 2000);
  });
}

onMounted(async () => {
  await load();
  await revealTarget();
});

// Session switch inside the preview (future: next/prev hit) —
// reload + re-reveal.
watch(
  () => [props.sessionId, props.targetSeq] as const,
  async () => {
    await load();
    await revealTarget();
  },
);
</script>

<template>
  <div class="search-preview">
    <div v-if="loading" class="search-preview__state">加载会话…</div>
    <div v-else-if="error" class="search-preview__state search-preview__state--error">
      加载失败:{{ error }}
    </div>
    <div v-else-if="messages.length === 0" class="search-preview__state">
      该会话没有消息
    </div>
    <div v-else ref="scrollEl" class="search-preview__scroll">
      <ul class="search-preview__list">
        <li v-for="g in renderGroups" :key="g.key" class="run-group">
          <MessageItem
            v-for="m in g.items"
            :key="m.id"
            :message="m"
            readonly
            :data-seq="m.seq ?? undefined"
          />
        </li>
      </ul>
    </div>
  </div>
</template>

<style scoped>
.search-preview {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.search-preview__state {
  padding: var(--space-6) var(--space-4);
  color: var(--color-text-secondary);
  font-size: var(--text-sm);
  text-align: center;
}

.search-preview__state--error {
  color: var(--color-tool-error);
}

.search-preview__scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-gutter: stable;
  background: var(--color-bg-app);
}

/* Mirrors MessageList's spacing contract: 12px between run groups,
   6px inside a run (interleaved-thinking "one flow" visual). */
.search-preview__list {
  list-style: none;
  margin: 0;
  padding: var(--space-4) var(--space-4) var(--space-6);
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.search-preview__list .run-group {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

/* Hit highlight — applies to MessageItem's root <li> (carries this
   component's scope id via single-root fallthrough). */
.search-preview__scroll :deep(.search-hit) {
  animation: search-hit-flash 0.6s var(--ease-out) 3;
  border-radius: var(--radius-md);
}

@keyframes search-hit-flash {
  0%,
  100% {
    background-color: transparent;
  }
  50% {
    background-color: color-mix(in srgb, var(--color-accent) 14%, transparent);
  }
}
</style>
