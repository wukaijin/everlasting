<script setup lang="ts">
// ChatInputHintRow — A4 + F5 + PR5: the bottom hint row of the
// ChatInput composer.
//
// Layout (locked, see ChatInput.vue PR5 header comment):
// - LEFT: LLM cumulative latency chip (clock icon + "Σ 1.2s" /
//   "—") with clickable popover breaking the running total into a
//   per-turn TTFB / Gen / Total list. Rendered via
//   `<ChatInputLatencyPopover>` (0 store import — props-only).
//   **Hidden when `currentSessionId` is null** (matches the A4
//   token chip's "no session → don't render" rule).
// - CENTER: token usage chip. 2026-08-21: the old reka-ui hover
//   `Tooltip` (4-counter breakdown) was replaced by a clickable
//   large popover (`<ChatInputTokenUsage>`) that also absorbed the
//   AppHeader QuotaChip panel (context progress bar + last-turn
//   detail + cache-hit rate + rolling-window aggregates +
//   settings). Session data still flows props-only; the window
//   aggregate lives in `useQuotaStore` inside the child.
// - RIGHT: model picker popover (`<ModelSelect>` — opens UP).
//
// Extracted from `ChatInput.vue` (split refactor 2026-06-23).
//
// **0 store import** — props only (tokenUsage / contextWindow /
// usageLevel / currentSessionId + totalMs / turns for the latency
// sub-component). Parent computes usageLevel from `tokenUsage`
// and `contextWindow` so this component is testable with synthetic
// props.

import ChatInputLatencyPopover from "./ChatInputLatencyPopover.vue";
import ChatInputTokenUsage from "./ChatInputTokenUsage.vue";
import ModelSelect from "./ModelSelect.vue";
import type { TokenUsageLevel } from "../../utils/tokenUsage";
import type { LatencyInfo, SessionTokenUsage } from "../../stores/chat.types";

defineProps<{
  /** Per-session cumulative token usage. `null` = pre-A4 session or
   *  no usage recorded yet → render "—" + "升级前未统计" tooltip.
   *  Mirrors the Rust `SessionTokenUsage` shape exactly. */
  tokenUsage: SessionTokenUsage | null;
  /** The denominator for the percentage chip ("X% / 200K"). Pulled
   *  from the current model's `contextWindow` field in the catalog;
   *  the parent computes this from `modelsStore.defaultModel`. */
  contextWindow: number;
  /** Pre-computed color band derived from
   *  `tokenUsage.input_tokens / contextWindow`. The parent runs
   *  `tokenUsageLevel(pct)` so the boundary rules (49/50/74/75) live
   *  in `utils/tokenUsage.ts` and stay unit-testable. `null` for
   *  sessions without usage (renders plain "—", no color). */
  usageLevel: TokenUsageLevel | null;
  /** Active session id. When `null` the latency chip is hidden
   *  (no session = nothing to time); the token chip renders in
   *  empty state; the ModelSelect always renders. */
  currentSessionId: string | null;
  /** Cumulative Σ totalMs across all recorded turns. `null` for
   *  pre-F5 sessions / no recorded turns → latency chip renders "—".
   *  Forwarded to `<ChatInputLatencyPopover>`. */
  totalMs: number | null;
  /** Per-turn list for the latency popover. `null` = no session,
   *  `[]` = active session but no turns recorded yet. Forwarded
   *  to `<ChatInputLatencyPopover>`. */
  turns: LatencyInfo[] | null;
}>();
</script>

<template>
  <div class="chat-input__hint">
    <!-- F5 follow-up: LLM cumulative latency chip (LEFT).
         Rendered via the dedicated sub-component. Hidden when
         no session is active (matches the A4 token chip's
         "no session → don't render" rule). -->
    <ChatInputLatencyPopover
      v-if="currentSessionId"
      :total-ms="totalMs"
      :turns="turns"
    />
    <!-- A4 → 2026-08-21: token usage chip. Click opens the large
         usage-detail popover (context bar + last-turn breakdown +
         cache-hit rate + rolling-window aggregates). All rendering
         logic lives in `<ChatInputTokenUsage>`; this file only
         passes the props through. -->
    <ChatInputTokenUsage
      :token-usage="tokenUsage"
      :context-window="contextWindow"
      :usage-level="usageLevel"
    />
    <!-- PR5: model picker popover (upward-opening) attached to
         the right edge of the hint row. Replaces the
         bottom-of-content `StatusBar` from PR4. -->
    <ModelSelect />
  </div>
</template>

<style scoped>
.chat-input__hint {
  margin-top: 8px;
  padding: 0 6px;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  user-select: none;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

/* S6a 状态条折叠(08-13-mobile-chat-view)。prd A7/D7 + DEC-3:LLM 延迟
   chip + token 用量 chip 是开发者调试信息,手机端隐藏;ModelSelect 保留
   (模型选择是普通用户主操作)。桌面块零改动。
   两个 chip 的根节点都是子组件根(.chat-input__latency /
   .chat-input__token,含 2026-08-21 并入的用量弹层),scoped 下用
   :deep() 命中;隐藏后整行只剩 ModelSelect,justify-content 从
   space-between 改 flex-end 让它靠右,margin-top 4px 收紧行高
   (design §3.3)。 */
@media (max-width: 767px) {
  :deep(.chat-input__latency),
  :deep(.chat-input__token) {
    display: none;
  }
  .chat-input__hint {
    margin-top: 4px;
    justify-content: flex-end;
    gap: 4px;
  }
}
</style>
