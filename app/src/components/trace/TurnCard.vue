<script setup lang="ts">
// TurnCard — one card on the trace timeline. Renders a single
// `TurnTrace` row with:
//   1. seq + latency (from `messages` buffer — see note below)
//   2. token 5-field mini bar (pure CSS, color tokens from
//      `--color-tool-*` family — no chart lib)
//   3. compaction sub-card (with `audit-item--critical` for
//      `degradation === "still_over"`)
//   4. loop_hint sub-card (1-2 连击 soft hint)
//   5. breadcrumb sub-card (workflow task meta text)
//   6. audit events (tool calls etc.) — `<TraceEventItem>` rows
//
// Latency source: the trace store doesn't carry the per-turn
// latency (that's `messages.latency`, owned by
// `streamController`). The card reads
// `chatStore.currentSessionLatencyTurns[seq]` to look up the
// assistant row's latency for the matching seq. When absent
// (live path: latency not yet computed; pre-F5 rows: no
// latency), the card renders "—".

import { computed } from "vue";
import Icon from "../Icon.vue";
import { abbreviateTokens, tokenUsageLevel } from "../../utils/tokenUsage";
import { useChatStore } from "../../stores/chat";
import type { TurnTrace } from "../../types/turnTrace";
import TraceEventItem from "./TraceEventItem.vue";

const props = defineProps<{
  trace: TurnTrace;
}>();

const chatStore = useChatStore();

/** The total of the 5 token fields. Drives the mini-bar
 *  widths (each segment is a percentage of the total). When
 *  the total is 0, the bar renders an empty placeholder. */
const tokenTotal = computed<number>(() => {
  const t = props.trace.tokenUsage;
  if (!t) return 0;
  return (
    t.input_tokens +
    t.output_tokens +
    t.cache_creation_input_tokens +
    t.cache_read_input_tokens +
    t.context_input_tokens
  );
});

/** Per-segment pixel widths for the mini-bar. Computed once
 *  per render; bars below 2% are clamped to a 1px minimum so
 *  they remain visible (a 0.5% cache_read slice would
 *  otherwise round to 0). */
const tokenBarSegments = computed<
  Array<{ widthPct: number; colorVar: string; label: string; value: number }>
>(() => {
  const t = props.trace.tokenUsage;
  if (!t) return [];
  const total = tokenTotal.value;
  if (total === 0) return [];
  const segs = [
    {
      key: "input",
      value: t.input_tokens,
      colorVar: "var(--color-tool-read)",
      label: "Input",
    },
    {
      key: "cache_creation",
      value: t.cache_creation_input_tokens,
      colorVar: "var(--color-tool-thinking)",
      label: "Cache Create",
    },
    {
      key: "cache_read",
      value: t.cache_read_input_tokens,
      colorVar: "var(--color-accent)",
      label: "Cache Read",
    },
    {
      key: "output",
      value: t.output_tokens,
      colorVar: "var(--color-tool-write)",
      label: "Output",
    },
  ];
  return segs
    .filter((s) => s.value > 0)
    .map((s) => ({
      widthPct: Math.max(0.5, (s.value / total) * 100),
      colorVar: s.colorVar,
      label: `${s.label}: ${abbreviateTokens(s.value)}`,
      value: s.value,
    }));
});

/** Latency lookup: the trace store doesn't carry the
 *  per-turn latency, but `streamController` exposes it via
 *  the chat store's per-session turn latency array. The
 *  `currentSessionLatencyTurns` getter returns
 *  `LatencyInfo[] | null` — each entry is the latency of an
 *  assistant message, but the array is NOT keyed by seq
 *  (the chat store's projection strips seq). For the
 *  trace viewer's per-seq lookup we need to read the
 *  controller's raw messages and find the assistant
 *  message with `seq === trace.seq`. */
const latency = computed<{ totalMs: number | null } | null>(() => {
  // Pinia auto-unwraps computeds; `chatStore.messages` is the
  // current session's reactive ChatMessage[] (empty array
  // when no session is active — see chat.ts `messages`).
  const msgs = chatStore.messages;
  if (!msgs || msgs.length === 0) return null;
  const found = msgs.find(
    (m) => m.role === "assistant" && m.seq === props.trace.seq,
  );
  if (!found || !found.latency) return null;
  return { totalMs: found.latency.totalMs ?? null };
});

/** Compact human label for the latency cell ("3.2s" /
 *  "—" / null placeholder). */
const latencyLabel = computed<string>(() => {
  const l = latency.value;
  if (!l || l.totalMs === null) return "—";
  if (l.totalMs < 1000) return `${l.totalMs}ms`;
  const s = l.totalMs / 1000;
  return s < 10 ? `${s.toFixed(1)}s` : `${Math.round(s)}s`;
});

/** Context-window utilization — the trace card's
 *  "over-budget" red highlight. We don't have a reliable
 *  model-window constant in the store layer (each model
 *  has its own 200K / 1M etc.), so the heuristic is the
 *  sum of the 4 input-side fields vs. a conservative 200K
 *  reference. If the project later adds per-model window
 *  metadata, swap the constant for a derived lookup. The
 *  colors reuse the existing `tokenUsageLevel` ladder so
 *  the trace card matches the ChatInput hint chip's
 *  threshold logic. */
const contextUtilPct = computed<number | null>(() => {
  const t = props.trace.tokenUsage;
  if (!t) return null;
  // 200K is the conservative Anthropic default; the
  // per-model window can be smaller or larger but the
  // color-band is the same shape (ok / warn / alert). The
  // exact percentage is informational — the user reads
  // ">" or "<" the threshold, not the raw number.
  const CONTEXT_WINDOW_REF = 200_000;
  const input = t.context_input_tokens || t.input_tokens;
  if (input === 0) return null;
  return (input / CONTEXT_WINDOW_REF) * 100;
});

const contextLevel = computed<"ok" | "warn" | "alert" | "none">(() => {
  const pct = contextUtilPct.value;
  if (pct === null) return "none";
  return tokenUsageLevel(pct);
});

/** True when the compaction sub-card should render with the
 *  red "critical" border. The trigger is
 *  `degradation === "still_over"` (the C3 hard-kill branch:
 *  context was over-budget, compaction couldn't bring it
 *  under, the turn is about to abort). */
const compactionCritical = computed<boolean>(
  () => props.trace.compaction?.degradation === "still_over",
);

/** Audit event rows attached to this turn (the 回看 path
 *  groups them by `turnSeq`; the live path doesn't carry
 *  audit events). Empty array when not loaded. */
const auditEvents = computed(() => props.trace.auditEvents ?? []);

/** True when the card has anything other than `id` /
 *  `seq` / `createdAt` to show. Drives the empty-state
 *  placeholder ("本 turn 暂无可观测信号"). */
const hasAnyTrace = computed<boolean>(
  () =>
    !!props.trace.tokenUsage ||
    !!props.trace.compaction ||
    !!props.trace.loopHint ||
    !!props.trace.breadcrumb ||
    auditEvents.value.length > 0,
);

/** Ungrouped audit events live in the `UNGROUPED_SEQ`
 *  bucket; the TurnTimeline renders that bucket as a
 *  single footer card with `seq === MAX_SAFE_INTEGER` and
 *  no latency / token data. The label and chrome differ
 *  from a regular card — handled here via a prop-derived
 *  computed so the parent can mount the same component
 *  for both shapes. */
const isUngroupedBucket = computed<boolean>(
  () => props.trace.seq === Number.MAX_SAFE_INTEGER,
);

const ungroupedLabel = computed<string>(() =>
  isUngroupedBucket.value ? "未关联 turn" : `Turn ${props.trace.seq}`,
);
</script>

<template>
  <article
    class="turn-card"
    :class="{
      'turn-card--critical': compactionCritical,
      'turn-card--ungrouped': isUngroupedBucket,
    }"
  >
    <header class="turn-card__header">
      <span
        class="turn-card__seq"
        :class="{ 'turn-card__seq--ungrouped': isUngroupedBucket }"
      >
        {{ ungroupedLabel }}
      </span>
      <span class="turn-card__latency" :title="latencyLabel">
        <Icon name="clock" :size="12" />
        {{ latencyLabel }}
      </span>
      <span
        v-if="contextLevel !== 'none'"
        class="turn-card__ctx"
        :class="`turn-card__ctx--${contextLevel}`"
        :title="`context 占用 ${contextUtilPct?.toFixed(1)}%`"
      >
        <Icon name="info" :size="12" />
        {{ contextUtilPct?.toFixed(0) }}%
      </span>
    </header>

    <!-- Token 5-field mini bar -->
    <div
      v-if="trace.tokenUsage"
      class="turn-card__tokens"
      :title="`Input ${abbreviateTokens(trace.tokenUsage.input_tokens)} · Cache Create ${abbreviateTokens(trace.tokenUsage.cache_creation_input_tokens)} · Cache Read ${abbreviateTokens(trace.tokenUsage.cache_read_input_tokens)} · Output ${abbreviateTokens(trace.tokenUsage.output_tokens)}`"
    >
      <div class="turn-card__token-bar" aria-hidden="true">
        <div
          v-for="(seg, idx) in tokenBarSegments"
          :key="idx"
          class="turn-card__token-segment"
          :style="{
            width: seg.widthPct + '%',
            background: seg.colorVar,
          }"
          :title="seg.label"
        />
      </div>
      <div class="turn-card__token-legend">
        <span class="turn-card__token-cell">
          in {{ abbreviateTokens(trace.tokenUsage.input_tokens) }}
        </span>
        <span class="turn-card__token-cell">
          cc {{ abbreviateTokens(trace.tokenUsage.cache_creation_input_tokens) }}
        </span>
        <span class="turn-card__token-cell">
          cr {{ abbreviateTokens(trace.tokenUsage.cache_read_input_tokens) }}
        </span>
        <span class="turn-card__token-cell">
          out {{ abbreviateTokens(trace.tokenUsage.output_tokens) }}
        </span>
      </div>
    </div>

    <!-- Compaction sub-card (C3 压缩) -->
    <div
      v-if="trace.compaction"
      class="turn-card__sub"
      :class="{ 'turn-card__sub--critical': compactionCritical }"
    >
      <Icon
        :name="compactionCritical ? 'warn' : 'shrink'"
        :size="12"
        class="turn-card__sub-icon"
      />
      <span class="turn-card__sub-title">C3 压缩</span>
      <span class="turn-card__sub-body">
        {{ abbreviateTokens(trace.compaction.tokens_before) }} →
        {{ abbreviateTokens(trace.compaction.tokens_after) }}
        <span class="turn-card__sub-meta">
          ({{ trace.compaction.dropped_count }} 块 · {{ trace.compaction.degradation }})
        </span>
      </span>
    </div>

    <!-- Loop hint sub-card (C2 1-2 连击 soft hint) -->
    <div
      v-if="trace.loopHint"
      class="turn-card__sub"
      :class="{ 'turn-card__sub--warn': trace.loopHint.verdict_kind === 'hard' }"
    >
      <Icon name="repeat" :size="12" class="turn-card__sub-icon" />
      <span class="turn-card__sub-title">循环检测</span>
      <span class="turn-card__sub-body">
        第 {{ trace.loopHint.hit_count }} 次连击 ·
        {{ trace.loopHint.verdict_kind === "hard" ? "硬触发" : "软提示" }}
      </span>
    </div>

    <!-- Workflow breadcrumb sub-card -->
    <div v-if="trace.breadcrumb" class="turn-card__sub">
      <Icon name="list-tree" :size="12" class="turn-card__sub-icon" />
      <span class="turn-card__sub-title">Workflow</span>
      <span class="turn-card__sub-body">
        <span v-if="trace.breadcrumb.task_slug" class="turn-card__bc-slug">
          {{ trace.breadcrumb.task_slug }}
        </span>
        <span v-if="trace.breadcrumb.status" class="turn-card__bc-status">
          · {{ trace.breadcrumb.status }}
        </span>
        <span
          v-if="trace.breadcrumb.breadcrumb_text"
          class="turn-card__bc-text"
        >
          · {{ trace.breadcrumb.breadcrumb_text.slice(0, 80) }}{{
            trace.breadcrumb.breadcrumb_text.length > 80 ? "…" : ""
          }}
        </span>
      </span>
    </div>

    <!-- Audit events for this turn (回看 path) -->
    <div v-if="auditEvents.length > 0" class="turn-card__audits">
      <details class="turn-card__audits-details">
        <summary class="turn-card__audits-summary">
          <Icon name="terminal" :size="12" />
          {{ auditEvents.length }} 条审计事件
        </summary>
        <ul class="turn-card__audits-list">
          <li v-for="ev in auditEvents" :key="ev.id">
            <TraceEventItem :row="ev" />
          </li>
        </ul>
      </details>
    </div>

    <!-- Empty state (live path: turn just started, no signals yet) -->
    <div v-if="!hasAnyTrace && !isUngroupedBucket" class="turn-card__empty">
      本 turn 暂无可观测信号
    </div>
  </article>
</template>

<style scoped>
.turn-card {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 10px 12px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid transparent;
  border-radius: var(--radius-md);
  transition: background var(--duration-fast) var(--ease-out);
  min-width: 0;
}

.turn-card:hover {
  background: var(--color-bg-elevated);
}

.turn-card--critical {
  border-left-color: var(--color-tool-error);
}

.turn-card--ungrouped {
  border-left-color: var(--color-text-muted);
  background: var(--color-bg-app);
}

.turn-card__header {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.turn-card__seq {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-primary);
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  font-weight: var(--weight-medium);
}

.turn-card__seq--ungrouped {
  color: var(--color-text-muted);
  background: var(--color-bg-elevated);
}

.turn-card__latency {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}

.turn-card__ctx {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--color-bg-border);
}

.turn-card__ctx--ok {
  color: var(--color-status-success);
  border-color: color-mix(in srgb, var(--color-status-success) 35%, transparent);
}

.turn-card__ctx--warn {
  color: var(--color-tool-shell);
  border-color: color-mix(in srgb, var(--color-tool-shell) 35%, transparent);
}

.turn-card__ctx--alert {
  color: var(--color-tool-error);
  border-color: color-mix(in srgb, var(--color-tool-error) 35%, transparent);
}

.turn-card__tokens {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.turn-card__token-bar {
  display: flex;
  width: 100%;
  height: 6px;
  background: var(--color-bg-app);
  border-radius: var(--radius-sm);
  overflow: hidden;
  border: 1px solid var(--color-bg-border);
}

.turn-card__token-segment {
  height: 100%;
  min-width: 1px;
  transition: opacity var(--duration-fast) var(--ease-out);
}

.turn-card__token-segment:hover {
  opacity: 0.8;
}

.turn-card__token-legend {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  font-family: var(--font-mono);
  font-size: var(--text-2xs);
  color: var(--color-text-muted);
}

.turn-card__token-cell {
  white-space: nowrap;
}

.turn-card__sub {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  font-size: var(--text-xs);
  padding: 4px 6px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-text-muted);
  border-radius: var(--radius-sm);
}

.turn-card__sub--critical {
  border-left-color: var(--color-tool-error);
  background: color-mix(in srgb, var(--color-tool-error) 6%, transparent);
}

.turn-card__sub--warn {
  border-left-color: var(--color-tool-shell);
}

.turn-card__sub-icon {
  flex-shrink: 0;
  color: var(--color-text-muted);
}

.turn-card__sub--critical .turn-card__sub-icon {
  color: var(--color-tool-error);
}

.turn-card__sub--warn .turn-card__sub-icon {
  color: var(--color-tool-shell);
}

.turn-card__sub-title {
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
  flex-shrink: 0;
}

.turn-card__sub-body {
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
  word-break: break-all;
  min-width: 0;
}

.turn-card__sub-meta {
  color: var(--color-text-muted);
  font-size: var(--text-2xs);
}

.turn-card__bc-slug {
  color: var(--color-tool-thinking);
  font-weight: var(--weight-medium);
}

.turn-card__bc-status {
  color: var(--color-text-muted);
  font-size: var(--text-2xs);
}

.turn-card__bc-text {
  color: var(--color-text-secondary);
  font-size: var(--text-2xs);
}

.turn-card__audits {
  margin-top: 2px;
}

.turn-card__audits-details {
  font-size: var(--text-xs);
}

.turn-card__audits-summary {
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--color-text-muted);
  list-style: none;
  user-select: none;
  padding: 2px 4px;
  border-radius: var(--radius-sm);
  transition: background var(--duration-fast) var(--ease-out);
}

.turn-card__audits-summary:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-secondary);
}

.turn-card__audits-summary::-webkit-details-marker {
  display: none;
}

.turn-card__audits-list {
  list-style: none;
  padding: 0;
  margin: 6px 0 0 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.turn-card__empty {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  text-align: center;
  padding: 6px 0;
  font-style: italic;
}
</style>
