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
//
// Scroll-to-bottom button: when the user scrolls away from the bottom
// (reading history, or after force-follow was cancelled), a floating
// `↓` button appears. Clicking it jumps back to the bottom (smooth)
// and re-engages force-follow if the stream is still active — fixing
// the old "easy to leave the bottom, hard to return" asymmetry.

import { ref, watch, nextTick, computed, onMounted, onUnmounted } from "vue";
import { useChatStore } from "../../stores/chat";
import MessageItem from "./MessageItem.vue";
import Icon from "../Icon.vue";

const store = useChatStore();
const messagesEl = ref<HTMLElement | null>(null);
// TransitionGroup (tag="ul") exposes its rendered <ul> via the
// component instance's $el, not via a plain ref (which would hand
// back the instance). A function ref captures the DOM node so the
// scroll logic below (scrollHeight / scrollTop / scrollTo) works
// unchanged.
function setListEl(instance: unknown) {
  messagesEl.value =
    (instance as { $el?: HTMLElement } | null)?.$el ?? null;
}

// Whether the viewport is currently pinned to (near) the bottom.
// Drives the scroll-to-bottom button's visibility. Updated on every
// scroll event; the ref de-dupes so the button only re-renders when
// the value actually flips.
const isAtBottom = ref(true);

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

// `smooth` is used only for user-initiated jumps (the scroll-to-bottom
// button) — streaming-delta follow stays instant, because smooth-scroll
// on every high-frequency delta would queue overlapping animations and
// stutter.
async function scrollToBottom(smooth = false) {
  await nextTick();
  const el = messagesEl.value;
  if (!el) return;
  if (smooth) {
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  } else {
    el.scrollTop = el.scrollHeight;
  }
}

// Scroll-to-bottom button handler. Pre-emptively set isAtBottom so the
// button hides immediately instead of waiting for the scroll to cross
// the threshold. While streaming, snap instantly and re-engage
// force-follow so the view keeps tracking live deltas — a smooth
// animation would lag behind new output. When idle, a smooth jump back
// to the bottom reads as less jarring.
async function jumpToBottom() {
  isAtBottom.value = true;
  if (store.isCurrentSessionStreaming) {
    store.forceFollowActive = true;
    await scrollToBottom(false);
  } else {
    await scrollToBottom(true);
  }
}

// F4 fix: after reloadAfterFinalize replaces the streaming buffer with
// DB messages, rehydrate assigns fresh ids so Vue unmounts+remounts the
// ENTIRE <MessageItem> list. N components mounting at once keeps the
// layout churning across several frames — and Vue's patch of a long
// list can itself take tens of ms, during which rAF reads a stale
// scrollHeight. A single scrollToBottom, or a "N stable frames"
// heuristic, nails scrollTop to that stale value mid-churn, so the
// viewport springs back to ~the top of the last turn (the thinking
// card). The two-frame version exited on a false positive while the
// patch was still running.
//
// Fix: pin to bottom every frame and only stop once scrollHeight has
// been QUIET for `quietMs` (render truly finished) or the hard deadline
// elapses. rAF callbacks run right before paint, so scrollHeight is the
// post-layout value when we read it.
function stickToBottomUntilStable(deadlineMs = 1000, quietMs = 150) {
  void nextTick().then(() => {
    const start = performance.now();
    let lastH = -1;
    let lastChangeAt = start;
    const tick = () => {
      const el = messagesEl.value;
      if (!el) return; // unmounted mid-loop — bail
      el.scrollTop = el.scrollHeight;
      const now = performance.now();
      if (el.scrollHeight !== lastH) {
        lastH = el.scrollHeight;
        lastChangeAt = now;
      }
      if (now - lastChangeAt >= quietMs) return;
      if (now - start > deadlineMs) return;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}

// F2: detect user manual scroll-up, and track isAtBottom for the
// scroll-to-bottom button. Runs on every scroll event; isNearBottom is
// cheap and the ref de-dupes, so high-frequency scroll only flips UI
// state at the threshold crossing.
function onScroll() {
  const el = messagesEl.value;
  if (!el) return;
  const near = isNearBottom(el, 80);
  isAtBottom.value = near;
  if (store.forceFollowActive && !near) {
    store.forceFollowActive = false;
  }
}

// Auto-scroll on any content change. During force-follow mode, always
// scroll; otherwise only scroll when user is near the bottom. The
// fingerprint includes latency + thinkingDurationMs so the F5
// post-stream latency IPC (which can change the last row's height via
// the latency badge) is also caught here instead of leaving the view
// one-badge-height above the bottom.
watch(
  () =>
    store.messages
      .map(
        (m) =>
          m.content +
          (m.toolCalls?.length ?? 0) +
          (m.toolResults?.length ?? 0) +
          (m.thinkingBlocks?.reduce((n, b) => n + b.text.length, 0) ?? 0) +
          (m.redactedThinkingData?.length ?? 0) +
          (m.latency?.totalMs ?? "") +
          (m.thinkingDurationMs ?? ""),
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
// session. Uses stickToBottomUntilStable so the post-switch DOM rebuild
// (fresh message ids) settles across frames without the same jitter the
// reload path used to have.
watch(
  () => store.currentSessionId,
  (newId, oldId) => {
    if (newId === oldId) return;
    isAtBottom.value = true;
    stickToBottomUntilStable();
  },
);

// F4: after reloadAfterFinalize replaces the streaming buffer with
// DB messages, re-scroll to bottom to avoid position jitter. See
// stickToBottomUntilStable for why this can't be a single scrollToBottom.
watch(() => store.scrollAfterReload, () => {
  stickToBottomUntilStable();
});

onMounted(() => {
  messagesEl.value?.addEventListener("scroll", onScroll, { passive: true });
  // Session switches REBUILD this component: ChatPanel swaps in the
  // loading spinner (v-if="sessionLoading") while switchSession's IPC
  // runs, then remounts MessageList once it resolves. By mount time
  // currentSessionId already matches, so watch(currentSessionId) on the
  // new instance never fires — and the fresh <ul> defaults to
  // scrollTop=0 (the top). Pin to bottom on mount so the user lands on
  // the latest message. stickToBottomUntilStable rides out the v-for
  // mount churn the same way it does for reload.
  stickToBottomUntilStable();
});
onUnmounted(() => {
  messagesEl.value?.removeEventListener("scroll", onScroll);
});
</script>

<template>
  <div class="messages-wrap">
    <TransitionGroup name="msg" tag="ul" :ref="setListEl" class="messages" appear>
      <MessageItem
        v-for="m in visibleMessages"
        :key="m.id"
        :message="m"
      />
    </TransitionGroup>
    <button
      v-if="!isAtBottom"
      class="scroll-to-bottom"
      type="button"
      title="回到底部"
      aria-label="回到底部"
      @click="jumpToBottom"
    >
      <Icon name="arrow-down" :size="16" />
    </button>
  </div>
</template>

<style scoped>
/* Wrapper gives the floating button a non-scrolling positioning
   context: the button is absolute against .messages-wrap, so it stays
   fixed in the corner while the <ul> scrolls underneath. The wrap
   takes over the flex:1 + min-height:0 role the <ul> used to play as a
   direct child of .chat-panel__main. */
.messages-wrap {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.messages {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
  flex: 1;
  overflow-y: auto;
  /* overflow-x: hidden — 消息气泡 enter 动画用 translateX 偏移到列表
     外侧；overflow-y:auto 会让 overflow-x 隐式变 auto，动画期间气泡
     出界会冒出水平滚动条（底部的"进度条"闪烁）。显式 hidden 让外侧
     偏移被裁剪而不显示滚动条。 */
  overflow-x: hidden;
  /* PR4 (2026-06-27): reserve stable space for the (vertical) scrollbar
     so message width doesn't jump when it appears/disappears, and so
     .chat-panel__main can use symmetric L/R padding instead of a 4px
     right-side gutter hack. */
  scrollbar-gutter: stable;
}

/* PR3 (2026-06-27): new-message enter animation. Only fires when an
   element is ADDED to the already-mounted list (a streaming new turn)
   — TransitionGroup's `appear` defaults to off, so the full-list
   remount on session switch does NOT animate (the list just appears).
   Uses `transform: translateY` (not a layout property) so it never
   perturbs scrollHeight and the stick-to-bottom loop stays correct.
   `prefers-reduced-motion` collapses this to ~instant via the
   top-level @media in style.css.

   :deep() is required because the `msg-enter-*` classes are added by
   TransitionGroup to MessageItem's root <li> (a CHILD component root);
   a scoped `.msg-enter-active` compiles to `.msg-enter-active[data-v-ML]`
   which doesn't reliably match the class on the child's root element.
   :deep() drops the attribute selector so the transition reaches the li. */
/* !important: MessageItem 的 `.msg:not(.msg--editing):not(.msg--err)` 特异性
   (0,4,0) 高于本 :deep (0,2,0)，其 `transition: background-color` 会整体
   覆盖这里的 opacity/transform transition（transition 是属性级覆盖，非按
   property 合并），导致 enter 无渐显 + 划入瞬间不可见。!important 强制
   enter 期间的 transition；enter 期间无 hover，丢掉 background-color
   transition 无影响。 */
:deep(.msg-enter-active) {
  transition: opacity var(--duration-slow) var(--ease-out),
    transform var(--duration-slow) var(--ease-out) !important;
}
/* PR3: 划入方向 —— user 从右划入（translateX +16→0）、assistant 从左
   划入（translateX -16→0），各从自己的对齐侧"外侧"出来。位移走外侧
   （user 右边界外、assistant 左边界外）：.messages 的 overflow-y:auto
   使 overflow-x 隐式 auto，外侧偏移的超出部分会被裁剪，但 transition
   工作时气泡主体的位移仍清晰可见，正是"从外侧划出"的观感。translateX
   不参与布局，不扰动 scrollHeight / stick-to-bottom；reduced-motion 由
   顶层 @media 兜底。 */
:deep(.msg--user.msg-enter-from) {
  opacity: 0;
  transform: translateX(24px);
}
:deep(.msg--assistant.msg-enter-from) {
  opacity: 0;
  transform: translateX(-24px);
}

/* Floating "back to bottom" button — appears only when the user has
   scrolled away from the bottom. Confined to .messages-wrap, so it
   floats above the message list without touching the input box below. */
.scroll-to-bottom {
  position: absolute;
  right: 16px;
  bottom: 14px;
  z-index: 10;
  width: 32px;
  height: 32px;
  padding: 0;
  border-radius: 50%;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  color: var(--color-text-secondary);
  cursor: pointer;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.18);
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out), transform var(--duration-fast) var(--ease-out);
}

.scroll-to-bottom:hover {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.scroll-to-bottom:active {
  transform: scale(0.94);
}
</style>
