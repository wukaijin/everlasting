<script setup lang="ts">
// MessageList — the virtualized message stream (N4 PR1,
// 09-19-n4-render-virtualization). Renders @tanstack/vue-virtual rows:
// DOM count = visible window + overscan, decoupled from session length.
//
// ALL anchoring semantics live in `useVirtualizedMessages`
// (composables/useVirtualizedMessages.ts — the single assembly point,
// including the PR0-spike handwritten force-follow and the per-render
// `_willUpdate` compensation). This component is deliberately thin:
// scroll container + virtual rows + back-to-bottom button.
//
// Retired with this rewrite (design §2 migration table):
//   - TransitionGroup (ul/li → div; PR1 shipped WITHOUT enter animation —
//     D4 rebuilt it in PR3 via the composable's enterRow phases + the
//     container fade-in below);
//   - setListEl $el hack (plain ref on a real div now);
//   - stickToBottomUntilStable + `stabilizing`/`data-stabilizing`
//     test signal (virtualization removes mount churn — a single
//     scrollToEnd lands);
//   - the O(n) fingerprint watch (library append/resize anchoring);
//   - manual scrollToBottom / scrollTop writes (spike constraint 3:
//     programmatic scrolls MUST go through library APIs).
import { computed, ref } from "vue";
import type { ComponentPublicInstance } from "vue";
import MessageItem from "./MessageItem.vue";
import Icon from "../Icon.vue";
import { useVirtualizedMessages } from "../../composables/useVirtualizedMessages";

const messagesEl = ref<HTMLElement | null>(null);

const { flatItems, virtualItems, virtualizer, isAtBottom, flashKey, enterRow, onScroll, jumpToBottom } =
  useVirtualizedMessages(messagesEl);

const totalSize = computed(() => virtualizer.value.getTotalSize());

// measureElement ref callback (spike constraint 3 adjacency: measurement
// is library-owned). Wrapper is the measured element — its padding-top
// (run spacing, D3) is inside the border box and therefore counted.
function measureRef(el: Element | ComponentPublicInstance | null): void {
  virtualizer.value?.measureElement(el as HTMLElement | null);
}

// D3 spacing classes: inter-run 12px / intra-run 6px via item-internal
// padding-top (measureElement reads border box — margin would overlap
// neighboring rows). First rendered row is exempt by INDEX (item.index
// === 0), not by the run-first flag — immune to flatten boundary drift
// (评审 D3 补强①).
function rowClass(index: number): Record<string, boolean> {
  if (index === 0) return {};
  return flatItems.value[index]?.runFirst
    ? { "run-first": true }
    : { "run-rest": true };
}

// N4 PR3 动画/flash 类(wrapper 级;动画目标 = .msg 子根,见 CSS 注):
// - search-hit:pendingScrollSeq 命中行高亮(flashKey 驱动,
//   SEARCH_FLASH_MS 后自动摘除);
// - run-enter-from/-active:D4 新 run 划入(composable 相位机驱动:
//   from+active 同挂 → 双 rAF 后仅 active → 过渡窗结束全摘)。
function stateClasses(key: string): Record<string, boolean> {
  const cls: Record<string, boolean> = {};
  if (flashKey.value === key) cls["search-hit"] = true;
  if (enterRow.value?.key === key) {
    cls["run-enter-active"] = true;
    if (enterRow.value.phase === "from") cls["run-enter-from"] = true;
  }
  return cls;
}
</script>

<template>
  <div class="messages-wrap">
    <div ref="messagesEl" class="messages" @scroll.passive="onScroll">
      <!-- Library-maintained spacer: height = getTotalSize(). Rows are
           absolutely positioned inside it (translateY). The flex column
           + gap of the old ul is GONE — padding-top on rows is the ONLY
           spacing source (D3). -->
      <div class="messages-spacer" :style="{ height: `${totalSize}px` }">
        <div
          v-for="vi in virtualItems"
          :key="String(vi.key)"
          :data-index="vi.index"
          :ref="measureRef"
          class="vrow"
          :class="[rowClass(vi.index), stateClasses(String(vi.key))]"
          :style="{ transform: `translateY(${vi.start}px)` }"
        >
          <MessageItem
            :message="flatItems[vi.index]!.message"
            :data-seq="flatItems[vi.index]!.message.seq ?? undefined"
          />
          <!-- data-seq (BUGLIST CH12-1b): fallthrough attr lands on the
               MessageItem root — the search modal's "在主窗口打开"
               hands its seq to the store command pendingScrollSeq
               (N4 PR1); the attr remains the row's identity hook for
               tests. The PR3 flash is class-driven (search-hit on the
               wrapper above), not attr-driven. Queued placeholders
               have no seq and render without the attribute. -->
        </div>
      </div>
    </div>
    <button
      v-if="!isAtBottom"
      class="scroll-to-bottom btn btn--muted btn--circle"
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
   fixed in the corner while the stream scrolls underneath. The wrap
   takes over the flex:1 + min-height:0 role as a direct child of
   .chat-panel__main. */
.messages-wrap {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.messages {
  flex: 1;
  overflow-y: auto;
  /* overflow-x: hidden — N4 PR1 保留(原注释):气泡位移类效果若回落,
     外侧偏移被裁剪而不冒水平滚动条。 */
  overflow-x: hidden;
  /* PR4 (2026-06-27): reserve stable space for the (vertical) scrollbar
     so message width doesn't jump when it appears/disappears. */
  scrollbar-gutter: stable;
  /* N4 PR1(D3):旧 `display:flex; flex-direction:column; gap:12px`
     删净 —— 间距唯一来源 = 虚拟项 wrapper 的项内 padding-top;flex
     容器会吞掉绝对定位子项的正常流,spacer 自身承担高度。 */
  /* N4 PR3(D5 appear 降级):session 切换 / 首挂载的一次性容器
     fade-in,挂载触发(ChatPanel 的 spinner v-if 使会话切换走重挂
     路径)。仅 opacity —— 白名单合规;reduced-motion 由 style.css
     顶层 @media 兜底(时长压 0.01ms → 即时呈现)。 */
  animation: messages-fade-in var(--duration-slow) var(--ease-out);
}

@keyframes messages-fade-in {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

/* The library-owned spacer: full scrollable height = getTotalSize().
   Rows position against it. */
.messages-spacer {
  position: relative;
}

/* One virtual row: absolutely positioned by translateY(vi.start), full
   width, and a flex column so MessageItem's `.msg` align-self
   (user → flex-end / assistant → flex-start) keeps the same alignment
   context the old run-group li provided.
   D3: spacing is padding-top INSIDE the row (border box → counted by
   measureElement; margin would make neighboring rows visually overlap).
     .run-first → 12px inter-run gap (old ul gap)
     .run-rest  → 6px intra-run gap (old run-group gap)
   The very first row (index 0) gets neither — no dead space above the
   stream top. */
.vrow {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  display: flex;
  flex-direction: column;
}

.vrow.run-first {
  padding-top: 12px;
}

.vrow.run-rest {
  padding-top: 6px;
}

/* ── N4 PR3 动画 D4 + data-seq flash ─────────────────────────────────
   两类动画的**目标元素都是 .msg 子根**(MessageItem 根,携本组件 scope
   id —— 子组件单根继承父 scope attr 的既有机制,spec §1 同款):wrapper
   自身带 inline translateY 定位,transform 属性不可被动画占用。
   白名单(design §4 评审补):只许 opacity + translateX,禁 scale /
   height —— 动画中间帧的测量值经 measureElement 按 getItemKey 写入
   持久缓存,几何属性会把中间尺寸固化成 session 内永久空隙。
   reduced-motion:style.css 顶层 @media 把 animation/transition 时长压
   到 0.01ms,两类动画即时呈现(契约保留,无需本组件处理)。 */

/* run-enter 相位(from+active 同挂 → 双 rAF 后仅 active → RUN_ENTER_
   ACTIVE_MS 后全摘,由 composable enterRow 驱动):active 态带过渡声明,
   from 态释放(类摘除)时从 0 / 24px 过渡回自然态 —— 与 Vue
   TransitionGroup 内部同式。参数沿用旧 TransitionGroup(--duration-slow
   240ms / --ease-out / +24px 用户侧词汇)。active 选择器特异性高于
   .msg 自身的 background-color hover 过渡,240ms 窗内覆盖之(旧实现
   需 !important 争同元素,现动画元素一致故不需)。 */
.vrow.run-enter-active > .msg {
  transition: opacity var(--duration-slow) var(--ease-out),
    transform var(--duration-slow) var(--ease-out);
}

.vrow.run-enter-from > .msg {
  opacity: 0;
  transform: translateX(24px);
}

/* data-seq flash(AC4 后半):主窗口形态复刻 CH12-1b 的 WAAPI 视觉
   (accent 22% → transparent,1400ms ease-out 单次;SearchPreviewBody
   的 14%×3 是弹层内形态,不采用)。类由 flashKey 驱动、SEARCH_FLASH_MS
   (1500ms)后摘除;background-color 无几何效应,不进测量缓存,不在
   D4 白名单管辖面。 */
.vrow.search-hit > .msg {
  animation: msg-hit-flash 1400ms var(--ease-out) 1;
}

@keyframes msg-hit-flash {
  from {
    background-color: color-mix(in srgb, var(--color-accent) 22%, transparent);
  }
  to {
    background-color: transparent;
  }
}

/* Floating "back to bottom" button — appears only when the user has
   scrolled away from the bottom (isAtEnd threshold 80, library-side).
   Confined to .messages-wrap, so it floats above the message list
   without touching the input box below. 08-24 btn-family:本体由
   muted·circle 家族承载;本地保留定位/32px 几何/FAB 阴影 + :active
   按压。 */
.scroll-to-bottom {
  position: absolute;
  right: 16px;
  bottom: 14px;
  /* 局部层:盖消息流;被 ChecklistCard 浮动面板(50)盖——其注释有契约 */
  z-index: 10;
  width: 32px;
  height: 32px;
  padding: 0;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.18);
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out), transform var(--duration-fast) var(--ease-out);
}

.scroll-to-bottom:active {
  transform: scale(0.94);
}

/* S6a 悬浮 ↓ 移动端避让(08-13-mobile-chat-view)。prd C2:与滚动条区域重叠
   易误触 → 右 8px / 下 64px 显式避让滚动条 + 输入区;触摸目标 44px
   (项目 HIG 约定,见 responsive-mobile.md §6)。桌面块零改动。 */
@media (max-width: 767px) {
  .scroll-to-bottom {
    right: 8px;
    bottom: 64px;
    width: 44px;
    height: 44px;
  }
}
</style>
