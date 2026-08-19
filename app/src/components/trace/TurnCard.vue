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
 *  "over-budget" red highlight. unified-context-budget WP1
 *  (2026-08-19): the denominator is now the per-model
 *  `context_window` snapshot on the trace row (request-time,
 *  written by the backend). Pre-column rows fall back to the
 *  old conservative 200K reference. The colors reuse the
 *  existing `tokenUsageLevel` ladder so the trace card matches
 *  the ChatInput hint chip's threshold logic. */
const contextUtilPct = computed<number | null>(() => {
  const t = props.trace.tokenUsage;
  if (!t) return null;
  const input = t.context_input_tokens || t.input_tokens;
  if (input === 0) return null;
  const window = props.trace.contextWindow ?? 200_000;
  return (input / window) * 100;
});

/** Per-model window label for the ctx chip tooltip ("200K" /
 *  "1M"; 旧行回退 200K 不单列,数值一致)。 */
const contextWindowLabel = computed<string>(() => {
  const window = props.trace.contextWindow ?? 200_000;
  return abbreviateTokens(window);
});

const contextLevel = computed<"ok" | "warn" | "alert" | "none">(() => {
  const pct = contextUtilPct.value;
  if (pct === null) return "none";
  return tokenUsageLevel(pct);
});

/** C7 (2026-08-14): tools[] estimated tokens as a share of the
 *  turn's context_input. Per design §R1 the formula is
 *  `tools_token / context_input_tokens` — NOT
 *  `tools_token / (context_input + tools_token)`, which would
 *  double-count (context_input already contains tools). `null`
 *  when tools_token was never written (pre-column / worker rows)
 *  or context_input is 0/absent. */
const toolsPct = computed<number | null>(() => {
  const tools = props.trace.toolsToken;
  const t = props.trace.tokenUsage;
  if (tools == null || !t) return null;
  const ctx = t.context_input_tokens || 0;
  if (ctx <= 0) return null;
  return (tools / ctx) * 100;
});

/** memory-block-governance WP1 (2026-08-15): memory instruction
 *  blocks' estimated tokens as a share of the turn's
 *  context_input. Same no-double-count formula as `toolsPct`
 *  (context_input already contains the memory blocks). `null`
 *  when memoryToken was never written (pre-column / worker rows)
 *  or context_input is 0/absent. */
const memoryPct = computed<number | null>(() => {
  const mem = props.trace.memoryToken;
  const t = props.trace.tokenUsage;
  if (mem == null || !t) return null;
  const ctx = t.context_input_tokens || 0;
  if (ctx <= 0) return null;
  return (mem / ctx) * 100;
});

/** B1 (2026-08-16) image-multimodal R6: image-block tokens (all
 *  Image blocks in the request — current-turn pastes + rebuilt
 *  history) as a share of the turn's context_input. Same
 *  no-double-count formula as `toolsPct`. `null` when
 *  imagesToken was never written (pre-column rows) or
 *  context_input is 0/absent. */
const imagesPct = computed<number | null>(() => {
  const img = props.trace.imagesToken;
  const t = props.trace.tokenUsage;
  if (img == null || !t) return null;
  const ctx = t.context_input_tokens || 0;
  if (ctx <= 0) return null;
  return (img / ctx) * 100;
});

/** unified-context-budget WP1 (2026-08-19): @文件切片(全部 user
 *  message 的注入正文 est 之和)占 context_input 的比例。同
 *  `toolsPct` 的 no-double-count 公式。`null` when atFilesToken
 *  was never written(pre-column / worker / 零注入 rows)or
 *  context_input is 0/absent。 */
const atFilesPct = computed<number | null>(() => {
  const at = props.trace.atFilesToken;
  const t = props.trace.tokenUsage;
  if (at == null || !t) return null;
  const ctx = t.context_input_tokens || 0;
  if (ctx <= 0) return null;
  return (at / ctx) * 100;
});

/** unified-context-budget WP1 (2026-08-19): system 切片(system
 *  prompt 本体 + skill listing 归因)占 context_input 的比例。 */
const systemPct = computed<number | null>(() => {
  const sys = props.trace.systemToken;
  const t = props.trace.tokenUsage;
  if (sys == null || !t) return null;
  const ctx = t.context_input_tokens || 0;
  if (ctx <= 0) return null;
  return (sys / ctx) * 100;
});

/** unified-context-budget WP1 (2026-08-19): 预算构成条 —— 五个
 *  归因切片 + 历史残差在**实发总量**(context_input,provider 计量)
 *  内的占比。残差 = ctx − Σ切片,钳 0(本地 cl100k 切片估算与
 *  provider 计量有系统性偏差,残差吸收之,prd D9/AC4)。全部切片
 *  都缺列(旧行)时不渲染;条满宽 = 100% 的 context_input,窗口
 *  占用看 header 的 ctx chip。 */
const budgetBarSegments = computed<
  Array<{ widthPct: number; colorVar: string; label: string; value: number }>
>(() => {
  const t = props.trace.tokenUsage;
  if (!t) return [];
  const ctx = t.context_input_tokens || 0;
  if (ctx <= 0) return [];
  const slices = [
    {
      key: "tools",
      value: props.trace.toolsToken ?? 0,
      colorVar: "var(--color-tool-thinking)",
      label: "tools[]",
    },
    {
      key: "mem",
      value: props.trace.memoryToken ?? 0,
      colorVar: "var(--color-accent)",
      label: "memory",
    },
    {
      key: "img",
      value: props.trace.imagesToken ?? 0,
      colorVar: "var(--color-tool-read)",
      label: "images",
    },
    {
      key: "at",
      value: props.trace.atFilesToken ?? 0,
      colorVar: "var(--color-tool-write)",
      label: "@文件",
    },
    {
      key: "sys",
      value: props.trace.systemToken ?? 0,
      colorVar: "var(--color-tool-shell)",
      label: "system",
    },
  ];
  if (slices.every((s) => s.value <= 0)) return [];
  const used = slices.reduce((acc, s) => acc + s.value, 0);
  const residual = Math.max(0, ctx - used);
  return [
    ...slices,
    {
      key: "residual",
      value: residual,
      colorVar: "var(--color-bg-border)",
      label: "历史+杂项",
    },
  ]
    .filter((s) => s.value > 0)
    .map((s) => ({
      widthPct: Math.max(0.5, (s.value / ctx) * 100),
      colorVar: s.colorVar,
      label: `${s.label}: ${abbreviateTokens(s.value)}`,
      value: s.value,
    }));
});

/** Tooltip string for the whole token block. Joins the 5-field
 *  breakdown with the tools[] estimate (C7) when present. */
const tokensTitle = computed<string>(() => {
  const t = props.trace.tokenUsage;
  const parts: string[] = [];
  if (t) {
    parts.push(`Input ${abbreviateTokens(t.input_tokens)}`);
    parts.push(
      `Cache Create ${abbreviateTokens(t.cache_creation_input_tokens)}`,
    );
    parts.push(`Cache Read ${abbreviateTokens(t.cache_read_input_tokens)}`);
    parts.push(`Output ${abbreviateTokens(t.output_tokens)}`);
  }
  if (props.trace.toolsToken != null) {
    const tok = abbreviateTokens(props.trace.toolsToken);
    parts.push(
      toolsPct.value != null
        ? `Tools[] ≈${tok} (${toolsPct.value.toFixed(0)}% of context)`
        : `Tools[] ≈${tok}`,
    );
  }
  if (props.trace.memoryToken != null) {
    const tok = abbreviateTokens(props.trace.memoryToken);
    parts.push(
      memoryPct.value != null
        ? `Memory ≈${tok} (${memoryPct.value.toFixed(0)}% of context)`
        : `Memory ≈${tok}`,
    );
  }
  if (props.trace.imagesToken != null) {
    const tok = abbreviateTokens(props.trace.imagesToken);
    parts.push(
      imagesPct.value != null
        ? `Images ≈${tok} (${imagesPct.value.toFixed(0)}% of context)`
        : `Images ≈${tok}`,
    );
  }
  if (props.trace.atFilesToken != null) {
    const tok = abbreviateTokens(props.trace.atFilesToken);
    parts.push(
      atFilesPct.value != null
        ? `@文件 ≈${tok} (${atFilesPct.value.toFixed(0)}% of context)`
        : `@文件 ≈${tok}`,
    );
  }
  if (props.trace.systemToken != null) {
    const tok = abbreviateTokens(props.trace.systemToken);
    parts.push(
      systemPct.value != null
        ? `System ≈${tok} (${systemPct.value.toFixed(0)}% of context)`
        : `System ≈${tok}`,
    );
  }
  return parts.join(" · ");
});

/** True when the compaction sub-card should render with the
 *  red "critical" border. The trigger is
 *  `degradation === "still_over"` (the C3 hard-kill branch:
 *  context was over-budget, compaction couldn't bring it
 *  under, the turn is about to abort). */
const compactionCritical = computed<boolean>(
  () => props.trace.compaction?.degradation === "still_over",
);

/** C3 摘要式压缩 (2026-08-18):method 徽标 —— summary = LLM 摘要
 *  路径(主路径),mechanical = 机械丢组(fallback / gate 关),none =
 *  旧回看行缺字段(traceStore 已 `?? "none"` 归一)。 */
const compactionMethodLabel = computed<string>(() => {
  switch (props.trace.compaction?.method) {
    case "summary":
      return "摘要";
    case "mechanical":
      return "机械";
    default:
      return "—";
  }
});

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
        :title="`context 占用 ${contextUtilPct?.toFixed(1)}%(窗口 ${contextWindowLabel})`"
      >
        <Icon name="info" :size="12" />
        {{ contextUtilPct?.toFixed(0) }}%
      </span>
    </header>

    <!-- Token 5-field mini bar (+ C7 tools[] estimate cell) -->
    <div
      v-if="trace.tokenUsage"
      class="turn-card__tokens"
      :title="tokensTitle"
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
        <!-- C7: separately-measured tools[] token estimate (NOT a
             bar segment — it's a slice already inside context_input,
             shown only as a count + share-of-context tooltip). -->
        <span
          v-if="trace.toolsToken != null"
          class="turn-card__token-cell turn-card__token-cell--tools"
          :title="
            toolsPct != null
              ? `tools[] 估算 ≈${abbreviateTokens(trace.toolsToken)}(约 context 的 ${toolsPct.toFixed(0)}%)`
              : `tools[] 估算 ≈${abbreviateTokens(trace.toolsToken)}`
          "
        >
          tools {{ abbreviateTokens(trace.toolsToken) }}
        </span>
        <!-- WP1 (2026-08-15): memory instruction blocks estimate —
             same slice-of-context treatment as the tools[] cell. -->
        <span
          v-if="trace.memoryToken != null"
          class="turn-card__token-cell turn-card__token-cell--memory"
          :title="
            memoryPct != null
              ? `memory 指令块估算 ≈${abbreviateTokens(trace.memoryToken)}(约 context 的 ${memoryPct.toFixed(0)}%)`
              : `memory 指令块估算 ≈${abbreviateTokens(trace.memoryToken)}`
          "
        >
          mem {{ abbreviateTokens(trace.memoryToken) }}
        </span>
        <!-- B1 (2026-08-16) R6: image-block estimate — same
             slice-of-context treatment as the tools[]/memory cells.
             Gated on > 0 (design: 无图轮 images_token=0,零图不渲染
             noise cell), unlike tools/mem which also surface their
             0s when the column exists. -->
        <span
          v-if="trace.imagesToken != null && trace.imagesToken > 0"
          class="turn-card__token-cell turn-card__token-cell--images"
          :title="
            imagesPct != null
              ? `图片块估算 ≈${abbreviateTokens(trace.imagesToken)}(约 context 的 ${imagesPct.toFixed(0)}%)`
              : `图片块估算 ≈${abbreviateTokens(trace.imagesToken)}`
          "
        >
          img {{ abbreviateTokens(trace.imagesToken) }}
        </span>
        <!-- unified-context-budget WP1 (2026-08-19): @文件切片 —
             同 img 的 >0 gate(零注入不渲染 noise cell)。 -->
        <span
          v-if="trace.atFilesToken != null && trace.atFilesToken > 0"
          class="turn-card__token-cell turn-card__token-cell--atfiles"
          :title="
            atFilesPct != null
              ? `@文件注入估算 ≈${abbreviateTokens(trace.atFilesToken)}(约 context 的 ${atFilesPct.toFixed(0)}%)`
              : `@文件注入估算 ≈${abbreviateTokens(trace.atFilesToken)}`
          "
        >
          @ {{ abbreviateTokens(trace.atFilesToken) }}
        </span>
        <!-- WP1: system 切片 — 同 tools/mem 的列存在即展示。 -->
        <span
          v-if="trace.systemToken != null"
          class="turn-card__token-cell turn-card__token-cell--system"
          :title="
            systemPct != null
              ? `system(prompt+skills)估算 ≈${abbreviateTokens(trace.systemToken)}(约 context 的 ${systemPct.toFixed(0)}%)`
              : `system(prompt+skills)估算 ≈${abbreviateTokens(trace.systemToken)}`
          "
        >
          sys {{ abbreviateTokens(trace.systemToken) }}
        </span>
      </div>
      <!-- unified-context-budget WP1 (2026-08-19): 预算构成条 —
           五归因切片 + 历史残差在实发总量(context_input)内的构成。
           全切片缺列(旧行)不渲染;各段 title 见 budgetBarSegments。 -->
      <div
        v-if="budgetBarSegments.length > 0"
        class="turn-card__budget-bar"
        aria-hidden="true"
      >
        <div
          v-for="(seg, idx) in budgetBarSegments"
          :key="idx"
          class="turn-card__budget-segment"
          :style="{
            width: seg.widthPct + '%',
            background: seg.colorVar,
          }"
          :title="seg.label"
        />
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
          ({{ trace.compaction.dropped_count }} 块 · {{ trace.compaction.degradation }} ·
          <span
            :class="trace.compaction.method === 'summary' ? 'turn-card__badge--summary' : ''"
          >{{ compactionMethodLabel }}</span>)
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

    <!-- unified-context-budget WP2 (2026-08-19): 预算裁剪徽标 ——
         关卡⑤硬卡静默裁剪了本请求(live 路径;持久化记录在
         context_budget_trim 审计行)。 -->
    <div
      v-if="trace.budgetTrim"
      class="turn-card__sub turn-card__sub--budget"
      :title="`裁剪后 ≈${abbreviateTokens(trace.budgetTrim.post_total)} / 窗口 ${abbreviateTokens(trace.budgetTrim.window)}`"
    >
      <Icon name="shrink" :size="12" class="turn-card__sub-icon" />
      <span class="turn-card__sub-title">预算裁剪</span>
      <span class="turn-card__sub-body">
        −{{ abbreviateTokens(trace.budgetTrim.freed_tokens) }}(旧 @文件/图/memory)
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
  font-size: var(--text-sm);
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
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

.turn-card__ctx {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-family: var(--font-mono);
  font-size: var(--text-sm);
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
  color: var(--color-tool-error-text);
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
  height: 8px;
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
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

.turn-card__token-cell {
  white-space: nowrap;
}

/* C7: tools[] is a separately-measured slice of context (not one of
   the 4 bar segments), so it gets a faint border to read as its own
   dimension rather than a 5th segment of the bar. */
.turn-card__token-cell--tools {
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 0 4px;
  color: var(--color-tool-thinking);
}

/* WP1 (2026-08-15): memory instruction-block estimate — same
 * pill treatment, accent-tinted to distinguish from tools. */
.turn-card__token-cell--memory {
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 0 4px;
  color: var(--color-accent-text);
}

/* B1 (2026-08-16) R6: image-block estimate — same pill treatment,
 * read-tinted (the input-bar segment color) to distinguish from
 * both tools (thinking-tinted) and memory (accent-tinted). */
.turn-card__token-cell--images {
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 0 4px;
  color: var(--color-tool-read);
}

/* unified-context-budget WP1 (2026-08-19): @文件切片 — write-tinted
 * (与预算构成条同色),sys 切片 — shell-tinted。 */
.turn-card__token-cell--atfiles {
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 0 4px;
  color: var(--color-tool-write);
}

.turn-card__token-cell--system {
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 0 4px;
  color: var(--color-tool-shell);
}

/* WP1: 预算构成条 — 与 token 5-field bar 同构的细条(6px),满宽 =
 * 100% context_input;残差段用 border 色保持低调(它是"其余历史",
 * 不是被治理对象)。 */
.turn-card__budget-bar {
  display: flex;
  width: 100%;
  height: 6px;
  background: var(--color-bg-app);
  border-radius: var(--radius-sm);
  overflow: hidden;
  border: 1px solid var(--color-bg-border);
}

.turn-card__budget-segment {
  height: 100%;
  min-width: 1px;
  transition: opacity var(--duration-fast) var(--ease-out);
}

.turn-card__budget-segment:hover {
  opacity: 0.8;
}

.turn-card__sub {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  font-size: var(--text-sm);
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

/* unified-context-budget WP2: 预算裁剪徽标 —— shell 色系(治理动作,
 * 非错误);与 compaction sub-card 并列时一眼可辨。 */
.turn-card__sub--budget {
  border-left-color: var(--color-accent);
}

.turn-card__sub-icon {
  flex-shrink: 0;
  color: var(--color-text-muted);
}

.turn-card__sub--critical .turn-card__sub-icon {
  color: var(--color-tool-error-text);
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
  font-size: var(--text-xs);
}

/* C3 摘要式压缩 (2026-08-18):method=summary 徽标强调 —— 用户扫
   TurnCard 一眼能分辨"这轮压缩是 LLM 摘要还是机械丢组"。 */
.turn-card__badge--summary {
  color: var(--color-accent);
  font-weight: 600;
}

.turn-card__bc-slug {
  color: var(--color-tool-thinking);
  font-weight: var(--weight-medium);
}

.turn-card__bc-status {
  color: var(--color-text-muted);
  font-size: var(--text-xs);
}

.turn-card__bc-text {
  color: var(--color-text-secondary);
  font-size: var(--text-xs);
}

.turn-card__audits {
  margin-top: 2px;
}

.turn-card__audits-details {
  font-size: var(--text-sm);
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
  font-size: var(--text-sm);
  color: var(--color-text-muted);
  text-align: center;
  padding: 6px 0;
  font-style: italic;
}
</style>
