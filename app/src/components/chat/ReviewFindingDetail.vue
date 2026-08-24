<script setup lang="ts">
// C2 (review visualization view, 2026-07-26).
//
// Single-finding detail card. Rendered inline when the user
// expands a cell in `<ReviewMatrixGrid>` or a row in
// `<ReviewDimensionCompare>`. Pure display + the one
// "action": the source_run_id jump (a button that fetches the
// reviewer's original `final_text` via `get_subagent_run` and
// shows it in `<MarkdownDetailModal>` — design §10.3, lightweight
// popover NOT the full `<SubagentDrawer>`).
//
// This is the层次 2 safety net (评审议题 2): the user compares
// "what the main LLM distilled" (this card's `issue` /
// `suggestion` / `triage`) against "what the reviewer actually
// said" (the modal's `final_text`).

import { ref } from "vue";
import { transport } from "../../transport";
import type { SubagentRunRow } from "../../stores/subagentRuns.types";
import type { ReviewFinding, Severity, TriageDecision } from "../../types/review-state";
import Icon from "../Icon.vue";
import MarkdownDetailModal from "../common/MarkdownDetailModal.vue";

const props = defineProps<{
  finding: ReviewFinding;
}>();

/** Severity → display label + CSS modifier. Matches the C3 R7
 *  enum (critical/high/medium/low/info) verbatim — drift here
 *  would silently mis-label findings. */
const SEVERITY_META: Record<Severity, { label: string; cls: string }> = {
  critical: { label: "致命", cls: "critical" },
  high: { label: "高", cls: "high" },
  medium: { label: "中", cls: "medium" },
  low: { label: "低", cls: "low" },
  info: { label: "提示", cls: "info" },
};

/** Triage decision → display label + CSS modifier. The synthesizing
 *  LLM records adopt/reject/defer per finding; this surfaces the
 *  decision + reason so the convergence is traceable (PRD R3). */
const TRIAGE_META: Record<TriageDecision, { label: string; cls: string }> = {
  adopt: { label: "采纳", cls: "adopt" },
  reject: { label: "驳回", cls: "reject" },
  defer: { label: "搁置", cls: "defer" },
};

const severityMeta = SEVERITY_META[props.finding.severity] ?? SEVERITY_META.info;
const triageMeta = props.finding.triage
  ? TRIAGE_META[props.finding.triage.decision] ?? null
  : null;

// --- source_run_id jump (层次 2 safety net) ----------------------

const sourceOpen = ref(false);
const sourceLoading = ref(false);
const sourceText = ref<string>("");
const sourceError = ref<string | null>(null);

/** Fetch the reviewer's `final_text` via `get_subagent_run` and
 *  open `<MarkdownDetailModal>`. Defensive on null (run row gone)
 *  + on transport failure (toast-free; the error surfaces inline
 *  on the button). */
async function openSource(): Promise<void> {
  if (sourceOpen.value || sourceLoading.value) return;
  sourceLoading.value = true;
  sourceError.value = null;
  try {
    const row = await transport.invoke<SubagentRunRow | null>(
      "get_subagent_run",
      { runId: props.finding.source_run_id },
    );
    if (!row) {
      sourceError.value = "原始 run 不存在（可能已清理）";
      return;
    }
    sourceText.value = row.finalText ?? "";
    sourceOpen.value = true;
  } catch (e) {
    sourceError.value = e instanceof Error ? e.message : String(e);
  } finally {
    sourceLoading.value = false;
  }
}
</script>

<template>
  <div class="review-finding-detail">
    <div class="review-finding-detail__head">
      <span
        class="review-finding-detail__severity"
        :class="`review-finding-detail__severity--${severityMeta.cls}`"
      >
        {{ severityMeta.label }}
      </span>
      <span class="review-finding-detail__dimension">{{ finding.dimension }}</span>
      <span
        v-if="triageMeta && finding.triage"
        class="review-finding-detail__triage"
        :class="`review-finding-detail__triage--${triageMeta.cls}`"
      >
        {{ triageMeta.label }}
      </span>
    </div>

    <p class="review-finding-detail__issue">{{ finding.issue }}</p>

    <p v-if="finding.suggestion" class="review-finding-detail__suggestion">
      <span class="review-finding-detail__label">建议</span>
      {{ finding.suggestion }}
    </p>

    <p v-if="finding.location" class="review-finding-detail__location">
      <span class="review-finding-detail__label">位置</span>
      {{ finding.location }}
    </p>

    <p v-if="finding.triage && finding.triage.reason" class="review-finding-detail__triage-reason">
      <span class="review-finding-detail__label">triage 理由</span>
      {{ finding.triage.reason }}
    </p>

    <div class="review-finding-detail__actions">
      <button
        type="button"
        class="review-finding-detail__source-btn btn btn--muted btn--sm"
        :disabled="sourceLoading"
        @click="openSource"
      >
        <Icon :name="sourceLoading ? 'thinking' : 'document'" :size="12" />
        <span>{{ sourceLoading ? "加载中..." : "查看 reviewer 原话" }}</span>
      </button>
      <span v-if="sourceError" class="review-finding-detail__source-error">
        {{ sourceError }}
      </span>
    </div>

    <MarkdownDetailModal
      v-model:open="sourceOpen"
      title="Reviewer 原始产出"
      source="reply"
      :markdown="sourceText"
    />
  </div>
</template>

<style scoped>
/* Token 迁移(08-22-review-token-migration):同 ReviewMatrix.vue 头注。
   severity 徽章由"裸 Tailwind 600 档实底 + 白字"改为与 verdict/triage 徽章
   一致的淡彩底 + 400 档文字 token(填充 500 / 文字 400 原则);critical 与
   high 同用 tool-error 色系,以 tint 浓度区分层级。容器背景改 transparent:
   父面已提供 elevated(grid 展开行)或 surface(维度对比列),避免子面与父
   面同底,边框负责 delineation。 */
.review-finding-detail {
  padding: var(--space-2) 10px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  background: transparent;
  font-size: var(--text-sm);
  /* 08-14 ux-polish-r1 WP2:finding 详情是多行 prose 阅读面 → --leading-relaxed */
  line-height: var(--leading-relaxed);
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.review-finding-detail__head {
  display: flex;
  align-items: center;
  gap: 6px;
}

.review-finding-detail__severity {
  padding: 1px 6px;
  border-radius: 3px;
  font-size: var(--text-xs);
  font-weight: var(--weight-semibold);
}
.review-finding-detail__severity--critical {
  background: color-mix(in srgb, var(--color-tool-error) 26%, transparent);
  color: var(--color-tool-error-text);
}
.review-finding-detail__severity--high {
  background: color-mix(in srgb, var(--color-tool-error) 14%, transparent);
  color: var(--color-tool-error-text);
}
.review-finding-detail__severity--medium {
  background: color-mix(in srgb, var(--color-status-warn) 14%, transparent);
  color: var(--color-status-warn);
}
.review-finding-detail__severity--low {
  background: color-mix(in srgb, var(--color-accent) 14%, transparent);
  color: var(--color-accent-text);
}
.review-finding-detail__severity--info {
  background: color-mix(in srgb, var(--color-text-muted) 14%, transparent);
  color: var(--color-text-secondary);
}

.review-finding-detail__dimension {
  color: var(--color-text-secondary);
  font-size: var(--text-xs);
}

.review-finding-detail__triage {
  margin-left: auto;
  padding: 1px 6px;
  border-radius: 3px;
  font-size: var(--text-xs);
  font-weight: var(--weight-semibold);
}
.review-finding-detail__triage--adopt {
  background: color-mix(in srgb, var(--color-status-success) 15%, transparent);
  color: var(--color-status-success);
}
.review-finding-detail__triage--reject {
  background: color-mix(in srgb, var(--color-tool-error) 15%, transparent);
  color: var(--color-tool-error-text);
}
.review-finding-detail__triage--defer {
  background: color-mix(in srgb, var(--color-text-muted) 15%, transparent);
  color: var(--color-text-secondary);
}

.review-finding-detail__issue {
  margin: 0;
  color: var(--color-text-primary);
}

.review-finding-detail__suggestion,
.review-finding-detail__location,
.review-finding-detail__triage-reason {
  margin: 0;
  color: var(--color-text-secondary);
}

.review-finding-detail__label {
  font-weight: var(--weight-semibold);
  margin-right: var(--space-1);
  color: var(--color-text-muted);
}

.review-finding-detail__actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-top: 2px;
}

/* 源码查看钮由全局 .btn 家族承载(muted sm;hover wash 收敛为家族
 * accent-muted 方向,与 Review 系其它按钮一致)。 */
/* 加载中 progress 光标;opacity 落家族 disabled 0.5。 */
.review-finding-detail__source-btn:disabled {
  cursor: progress;
}

.review-finding-detail__source-error {
  color: var(--color-tool-error-text);
  font-size: var(--text-xs);
}
</style>
