<script setup lang="ts">
// MessageItemFooter — the bottom-of-bubble footer for a chat
// message row. Renders (in order):
//   1. Error row (if `error` is set) — red text with a warn icon
//      + an OPTIONAL `↻ 重试` button (visible when
//      `categoryRetryable(category)` resolves true).
//   2. F5 latency chip (assistant only, when not streaming and
//      `latency.totalMs` is set) — hover surfaces the three-line
//      breakdown (TTFB / 生成 / 端到端) via reka-ui Tooltip.
//
// This is the second of the two children extracted from
// `MessageItem.vue` on 2026-06-23. Per the task's ADR-2
// decision, the (edited) label stays in the parent — it sits
// inside the bubble div, visually distinct from the error /
// latency chips that hang below the bubble. The footer
// therefore has four visual surfaces (error row, retry button,
// checkpoint badge + latency chip row).
//
// Why pure presentation (no store import for retry):
//   - Single source of truth: the parent (`MessageItem.vue`)
//     owns `chatStore` (provides `retryChat`) and decides when
//     the retry button is enabled.
//   - Testable in isolation: vitest can drive this component
//     with hand-built props and assert on the rendered DOM
//     without spinning up Pinia. See
//     `app/src/components/chat/MessageItemFooter.test.ts`.
//   - Mirrors the `<MessageActionsMenu>` and `<MessageItemEdit>`
//     conventions (parent orchestrates, child renders).
//
// A5 R2 (2026-07-17) retry button UX:
//   - Visible iff `categoryRetryable(error.category)` is true
//     (RateLimit / Server / Network). Auth / InvalidRequest /
//     no-category rows show no button.
//   - Click → `emit('retry', messageSeq)`; the parent
//     (`MessageItem.vue`) routes to `chatStore.retryChat`.
//   - Loading state: while the parent has a retry stream in
//     flight (`parent:streaming` reaches us via the
//     `retry-loading` prop), the button is disabled and shows
//     "重试中...". The parent toggles the prop via its own
//     watcher on the session's active stream.
//
// N1 首次引导(R1.2, 2026-09-15)错误行「测试连接」行动点:
//   - auth / network / server 类错误值得实测一次连接(key 失效 /
//     断网 / 服务端故障都可能,一测便知);invalid_request 是请求
//     构造问题,连接层测试给不了新信息;rate_limit 已有 retry。
//   - 仍然零 store:按钮只 emit("test-connection"),`test_model`
//     IPC 由父(MessageItem.vue)编排,testState 三态(running /
//     ok(latencyMs) / fail(error))经 prop 回传行内渲染 —— 与
//     retry 同款「父编排、子渲染」分工(照 ModelsTab runTest 形制)。

import { computed } from "vue";
import {
  TooltipProvider,
  TooltipRoot,
  TooltipTrigger,
  TooltipPortal,
  TooltipContent,
  TooltipArrow,
} from "reka-ui";
import { abbreviateDuration } from "../../utils/duration";
import { categoryRetryable } from "../../utils/error";
// 跨 settings/ 的 type-only 导入(单源:与 ModelsTab/ModelRow 的
// 行内测试结果同形);运行时零依赖(类型擦除)。
import type { TestState } from "../settings/ModelRow.vue";
import Icon from "../Icon.vue";

const props = withDefaults(
  defineProps<{
    /** The message's role. Latency is shown only for assistant
     *  rows (user rows have no measurable turn latency). The
     *  `error` row shows for either role. */
    role: "user" | "assistant";
    /** True while a chat stream is in flight. Hides the latency
     *  chip (the chip is in flux; the user is reading the
     *  bubble, not the footer). Does NOT hide the error row —
     *  a streaming turn can still surface an error before
     *  the latency lands. Also disables the retry button
     *  (defense against mid-stream double-click). */
    streaming: boolean;
    /** Per-message latency breakdown. Missing for pre-F5
     *  rows and for user-role / system-event rows. The chip
     *  renders only when `latency.totalMs` is set and the row
     *  is an assistant turn not currently streaming. */
    latency?: {
      ttfbMs?: number;
      genMs?: number;
      totalMs?: number;
    };
    /** Per-message error. Renders a small red row above the
     *  latency chip (or in place of it, if no latency is
     *  available). Missing for non-error rows. */
    error?: { message: string; category?: string };
    /** A5 R2 (2026-07-17): the message's seq. Used by the retry
     *  button's emit so the parent can call
     *  `chatStore.retryChat(sessionId, messageSeq)` without
     *  re-reading the message. Missing for tool-only rows
     *  that the row's footer wouldn't render for anyway. */
    messageSeq?: number;
    /** A5 R2: while a retry stream is in flight for THIS
     *  message, the parent sets this to true to flip the
     *  retry button into its loading state ("重试中..." +
     *  disabled). The parent uses its own `currentSessionId`
     *  watcher to clear it on `done` / `error`. */
    retryLoading?: boolean;
    /** N1 (R1.2, 2026-09-15): the model to test when the user
     *  clicks 「测试连接」, resolved by the parent (session.model_id
     *  → group-chat participant → default). Missing → the button
     *  does not render (nothing to test against). */
    modelId?: string;
    /** N1 (R1.2): the inline test result owned by the parent
     *  (same shape as ModelsTab's per-row TestState). Null /
     *  undefined = never tested; `running` disables the button
     *  and flips its label; `ok` / `fail` render the inline
     *  result text next to the error row. */
    testState?: TestState | null;
    /** N2 follow-up (2026-09-20): checkpoint 徽标载荷 —— seq 命中
     *  「有变更」快照行时父传 files_changed(≥1),否则不传 / null。
     *  渲染在耗时 chip 左侧,点击 emit `turn-diff`(父开「本轮
     *  diff」弹窗)。role / readonly / 行命中由父闸(store 的
     *  filesChangedAt,与「本轮 diff」入口同闸),此处仅防
     *  streaming 中渲染。 */
    checkpointFiles?: number | null;
  }>(),
  {
    streaming: false,
    checkpointFiles: null,
    latency: undefined,
    error: undefined,
    messageSeq: undefined,
    retryLoading: false,
    modelId: undefined,
    testState: undefined,
  },
);

const emit = defineEmits<{
  /** A5 R2: fired when the user clicks the `↻ 重试` button.
   *  Payload is the row's `messageSeq` so the parent can call
   *  `chatStore.retryChat(sessionId, messageSeq)` directly. */
  (e: "retry", messageSeq: number): void;
  /** N1 (R1.2): fired when the user clicks the 「测试连接」
   *  button. No payload — the parent already knows the
   *  resolved `modelId` (it passed it down). */
  (e: "test-connection"): void;
  /** N2 follow-up (2026-09-20): checkpoint 徽标点击 —— 父据此打开
   *  「本轮 diff」弹窗(与 MessageActionsMenu 的同名入口同路)。 */
  (e: "turn-diff"): void;
}>();

/** A5 R2: whether the retry button renders at all. True iff
 *  - the row carries an error, AND
 *  - the error's `category` is one of RateLimit/Server/Network
 *    (mirrors backend `AppError::retryable()` default), AND
 *  - the row is not mid-stream (defense: a stale error from
 *    before a retry that just landed should not be retry-able
 *    mid-stream). */
const canRetry = computed<boolean>(
  () =>
    !!props.error &&
    categoryRetryable(props.error.category) &&
    !props.streaming,
);

/** N1 (R1.2): 「测试连接」按钮的 category 白名单。auth(key 失效)/
 *  network(断网 / base_url 错)/ server(服务商故障)实测一次连接
 *  都有信息量;invalid_request 是请求构造问题,测连接给不了新信息;
 *  rate_limit 已有 retry。大小写两形态都收(wire 是 snake_case,但
 *  utils/error.ts 的双 case 约定保留统一处理)。 */
const TESTABLE_CATEGORIES = new Set([
  "auth",
  "Auth",
  "network",
  "Network",
  "server",
  "Server",
]);

/** N1 (R1.2): whether the test-connection button renders at all.
 *  True iff the row carries an error AND the parent resolved a
 *  modelId AND the category is one of auth/network/server. */
const canTestConnection = computed<boolean>(
  () =>
    !!props.error &&
    !!props.modelId &&
    TESTABLE_CATEGORIES.has(props.error.category ?? ""),
);

/** F5 chip visibility. Renders the bottom-right of the
 *  assistant bubble with a 1-decimal abbreviation of
 *  `totalMs` (e.g. "3.2s"). Hidden for:
 *  - user-role messages (only assistant turns have a latency)
 *  - messages without a `latency` object (pre-F5 rows)
 *  - messages mid-stream (`streaming` true; the chip is in
 *    flux and the user is reading the bubble, not the footer)
 *  - rows where `latency.totalMs` is not a number (a cancel
 *    path that left ttfbMs / genMs null while totalMs is
 *    also missing — show "—" in place of the chip) */
const showLatency = computed<boolean>(
  () =>
    props.role === "assistant" &&
    !props.streaming &&
    !!props.latency &&
    typeof props.latency.totalMs === "number",
);

/** checkpoint 徽标可见性(N2 follow-up,2026-09-20)。行命中 /
 *  role / readonly 已由父经 checkpointFiles prop 闸掉(非 null 即
 *  ≥1);此处只防 streaming 中渲染 + 数值防御。与 latency chip
 *  不同,徽标不依赖 latency 对象 —— pre-F5 老行只要有 diff 也显示。 */
const showCheckpointBadge = computed<boolean>(
  () =>
    !props.streaming &&
    typeof props.checkpointFiles === "number" &&
    props.checkpointFiles > 0,
);

/** The chip's visible label. Falls back to "—" when no
 *  `totalMs` is present (this branch is unreachable in
 *  practice because `showLatency` gates the render, but
 *  the function is exposed for any future caller that
 *  wants the formatted value directly). */
const latencyTotalLabel = computed<string>(() => {
  const t = props.latency?.totalMs;
  if (typeof t !== "number") return "—";
  return abbreviateDuration(t);
});

/** The three lines shown in the hover tooltip. Each is
 *  omitted (and the row hidden) when the value is
 *  undefined — the cancel / error path leaves ttfbMs /
 *  genMs null while totalMs is set, and the UI shows only
 *  the available rows. */
const latencyRows = computed<Array<{ label: string; value: string }>>(() => {
  const lat = props.latency;
  if (!lat) return [];
  const rows: Array<{ label: string; value: string }> = [];
  if (typeof lat.ttfbMs === "number") {
    rows.push({ label: "TTFB", value: abbreviateDuration(lat.ttfbMs) });
  }
  if (typeof lat.genMs === "number") {
    rows.push({ label: "生成", value: abbreviateDuration(lat.genMs) });
  }
  if (typeof lat.totalMs === "number") {
    rows.push({ label: "端到端", value: abbreviateDuration(lat.totalMs) });
  }
  return rows;
});

/** A5 R2: the retry button's text. Switches between the
 *  default label and the loading-state label when the parent
 *  flips `retryLoading`. */
const retryButtonLabel = computed<string>(() =>
  props.retryLoading ? "重试中..." : "↻ 重试",
);

/** A5 R2: the retry button's click handler. Emits the row's
 *  seq to the parent for orchestrating the
 *  `chatStore.retryChat` call. Defensive: if `messageSeq`
 *  is missing (which shouldn't happen for a row with an
 *  error, but defensively) we log + skip. */
function onRetryClick(): void {
  if (typeof props.messageSeq !== "number") return;
  if (props.retryLoading) return;
  emit("retry", props.messageSeq);
}
</script>

<template>
  <!--
    Error footer. Sits at the top of the footer block (above
    the latency chip when both are present) so the user sees
    the failure first, the latency second. The text is the
    `error.message` string from the ChatMessage.

    A5 R2 (2026-07-17): the `↻ 重试` button renders next to
    the error text when `canRetry` is true. The button uses
    inline-flex spacing; clicking it emits `retry(seq)` which
    the parent (MessageItem.vue) routes to
    `chatStore.retryChat(sessionId, messageSeq)`.
  -->
  <div
    v-if="error"
    class="msg__error"
    role="alert"
    data-testid="msg-error-row"
  >
    <Icon name="warn" :size="12" icon-class="msg__error-icon" />
    <span class="msg__error-text">{{ error.message }}</span>
    <button
      v-if="canRetry"
      type="button"
      class="msg__error-retry btn btn--danger-soft btn--sm"
      :disabled="!!retryLoading"
      :data-testid="'msg-retry-button'"
      @click="onRetryClick"
    >
      {{ retryButtonLabel }}
    </button>
    <!--
      N1 (R1.2): 「测试连接」行动点。挂 retry 旁,复用 btn btn--sm
      家族;点击只 emit(零 store),结果由父经 testState prop 回传
      行内渲染(running → 按钮 disabled + 测试中...,ok / fail →
      按钮后的行内结果文案)。
    -->
    <button
      v-if="canTestConnection"
      type="button"
      class="msg__error-test btn btn--muted btn--sm"
      :disabled="testState?.kind === 'running'"
      data-testid="msg-test-connection-button"
      @click="emit('test-connection')"
    >
      {{ testState?.kind === "running" ? "测试中..." : "测试连接" }}
    </button>
    <span
      v-if="testState?.kind === 'ok'"
      class="msg__test-result msg__test-result--ok"
      data-testid="msg-test-connection-ok"
    >连接正常 · {{ testState.latencyMs }} ms</span>
    <span
      v-else-if="testState?.kind === 'fail'"
      class="msg__test-result msg__test-result--fail"
      data-testid="msg-test-connection-fail"
    >{{ testState.error }}</span>
  </div>

  <!--
    F5 (LLM Latency Tracking): per-message latency chip. The
    chip is the TooltipTrigger; the tooltip content is the
    three-row breakdown (TTFB / 生成 / 端到端). The `delay-duration`
    of 150ms defers the open so quick mouse-passes don't
    trigger (matches the project-wide Tooltip convention
    documented in `.trellis/spec/frontend/reka-ui-usage.md`).
  -->
  <!--
    N2 follow-up (2026-09-20): meta row —— checkpoint 徽标 + 耗时
    chip 并排右对齐。两者原本都是 .msg flex column 的独立子元素
    (各 align-self: flex-end,不包一行会竖着叠);徽标在左、耗时
    在右(耗时是最右侧的既有锚点,位置不动)。
  -->
  <div
    v-if="showCheckpointBadge || showLatency"
    class="msg__meta-row"
  >
    <button
      v-if="showCheckpointBadge"
      type="button"
      class="msg__checkpoint"
      data-testid="msg-checkpoint-chip"
      :title="`本轮有 checkpoint(改动 ${checkpointFiles} 个文件),点击查看 diff`"
      @click="emit('turn-diff')"
    >
      <Icon
        name="history"
        :size="11"
        icon-class="msg__checkpoint-icon"
      />
      <span class="msg__checkpoint-label">checkpoint</span>
      <span class="msg__checkpoint-count">{{ checkpointFiles }}</span>
    </button>
    <TooltipProvider v-if="showLatency">
      <TooltipRoot :delay-duration="150">
        <TooltipTrigger as-child>
          <span
            class="msg__latency"
            data-testid="msg-latency-chip"
          >{{ latencyTotalLabel }}</span>
        </TooltipTrigger>
        <TooltipPortal>
          <TooltipContent
            class="msg__latency-tooltip"
            :side-offset="4"
          >
            <div
              v-for="row in latencyRows"
              :key="row.label"
              class="msg__latency-tooltip-row"
              :data-testid="`msg-latency-tooltip-row-${row.label}`"
            >
              <span>{{ row.label }}</span>
              <span>{{ row.value }}</span>
            </div>
            <TooltipArrow class="msg__latency-tooltip-arrow" :size="6" />
          </TooltipContent>
        </TooltipPortal>
      </TooltipRoot>
    </TooltipProvider>
  </div>
</template>

<style scoped>
.msg__error {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-top: 4px;
  padding: 0 14px;
  font-size: var(--text-sm);
  color: var(--color-tool-error-text);
}

.msg__error-icon {
  flex-shrink: 0;
}

.msg__error-text {
  /* 把 retry button 推到文本末尾靠右时,保留 text 的 ellipsis 行为 */
  white-space: pre-wrap;
  word-break: break-word;
}

/* A5 R2 (2026-07-17): retry button. Inline 在错误文本后,小尺寸 +
   8px 间距;复用工具行按钮风格(无新 design token)。disabled 态
   颜色退到 muted + cursor not-allowed。 */
/* A5 R2: retry 按钮由全局 .btn 家族承载(danger-soft·sm,hover 红
   tint 与原语义一致);仅保留 margin 几何与 user-select 行为。 */
.msg__error-retry {
  margin-left: 6px;
  user-select: none;
}

/* N1 (R1.2): 「测试连接」按钮 —— 与 retry 同几何(muted·sm 家族
   承载本体,区别于 retry 的 danger-soft:两个动作语义不同,配色
   也应区分)。 */
.msg__error-test {
  margin-left: 6px;
  user-select: none;
}

/* N1 (R1.2): 行内测试结果 —— ok 用 success 绿,fail 用与错误行同
   源的 error 红加深一档字重;mono 小字,与 latency chip 同密度。 */
.msg__test-result {
  margin-left: 6px;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  user-select: none;
}

.msg__test-result--ok {
  color: var(--color-status-success);
}

.msg__test-result--fail {
  color: var(--color-tool-error-text);
}

/* F5 (LLM Latency Tracking): per-message latency chip. Sits
   at the bottom-right of the assistant bubble. The chip
   itself is the TooltipTrigger; the tooltip content is the
   three-row breakdown (TTFB / 生成 / 端到端).

   Visual decisions:
   - 11px mono font to match the existing density (token
     usage chip in ChatInput uses the same).
   - 0.5px muted color so it doesn't fight the bubble for
     attention — the user sees it on glance but isn't
     pulled in.
   - Right-aligned via `align-self: flex-end` (the parent
     `.msg` is `display: flex; flex-direction: column`,
     so the chip is the rightmost element of the bubble
     column). */
/* N2 follow-up (2026-09-20): checkpoint 徽标 + 耗时 chip 的共享
   行 —— 接管原 latency chip 的右对齐与贴角 margin(2026-08-29
   ui-visual-polish r2 的 2px 贴角语义原样上移到行容器)。 */
.msg__meta-row {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  align-self: flex-end;
  margin-top: 2px;
}

/* checkpoint 徽标:accent-muted 底 + accent 字,mono 小字与耗时
   chip 同密度;底色差让它从右侧的 muted 耗时字旁「跳出来」
   (醒目 = 可发现的还原点/入口)。点击 = 开「本轮 diff」弹窗。 */
.msg__checkpoint {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 1px 8px;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  font-weight: var(--weight-semibold);
  color: var(--color-accent-text);
  background: var(--color-accent-muted);
  border: 0;
  border-radius: var(--radius-sm);
  cursor: pointer;
  user-select: none;
}

.msg__checkpoint:hover {
  background: color-mix(in srgb, var(--color-accent) 28%, transparent);
}

.msg__checkpoint-icon {
  display: inline-flex;
  color: inherit;
}

/* 文件数上标:同色降不透明度,作徽标的数值尾注(0 不会出现,
   filesChangedAt 闸掉净零轮)。 */
.msg__checkpoint-count {
  opacity: 0.72;
}

.msg__latency {
  display: inline-flex;
  align-items: center;
  padding: 0 6px;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  font-weight: var(--weight-semibold);
  color: var(--color-text-secondary);
  cursor: help;
  border-radius: var(--radius-sm);
  user-select: none;
}

.msg__latency:hover {
  color: var(--color-text-secondary);
}

/* Tooltip content (reka-ui `TooltipContent` portal to body
   — must use :deep() per `.trellis/spec/frontend/reka-ui-usage.md`
   gotcha). The popover floats above the chip (default
   side is "top"). 11px mono, single-column row layout. */
:deep(.msg__latency-tooltip) {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: 6px 10px;
  min-width: 140px;
  z-index: var(--z-over-modal);
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  animation: msg-latency-tooltip-enter var(--duration-base) var(--ease-out);
}

:deep(.msg__latency-tooltip-row) {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  padding: 1px 0;
}

:deep(.msg__latency-tooltip-row span:first-child) {
  color: var(--color-text-secondary);
}

:deep(.msg__latency-tooltip-arrow) {
  fill: var(--color-bg-surface);
  stroke: var(--color-bg-border);
}

@keyframes msg-latency-tooltip-enter {
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
