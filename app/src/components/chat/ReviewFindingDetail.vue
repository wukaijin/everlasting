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
        class="review-finding-detail__source-btn"
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
.review-finding-detail {
  padding: 8px 10px;
  border: 1px solid var(--border-subtle, rgba(0, 0, 0, 0.08));
  border-radius: 6px;
  background: var(--bg-elevated, rgba(0, 0, 0, 0.02));
  font-size: 12px;
  line-height: 1.5;
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
  font-size: 11px;
  font-weight: 600;
  color: #fff;
}
.review-finding-detail__severity--critical { background: #b91c1c; }
.review-finding-detail__severity--high { background: #dc2626; }
.review-finding-detail__severity--medium { background: #d97706; }
.review-finding-detail__severity--low { background: #2563eb; }
.review-finding-detail__severity--info { background: #6b7280; }

.review-finding-detail__dimension {
  color: var(--text-secondary, #555);
  font-size: 11px;
}

.review-finding-detail__triage {
  margin-left: auto;
  padding: 1px 6px;
  border-radius: 3px;
  font-size: 11px;
  font-weight: 600;
}
.review-finding-detail__triage--adopt {
  background: rgba(34, 197, 94, 0.15);
  color: #15803d;
}
.review-finding-detail__triage--reject {
  background: rgba(220, 38, 38, 0.15);
  color: #b91c1c;
}
.review-finding-detail__triage--defer {
  background: rgba(107, 114, 128, 0.15);
  color: #4b5563;
}

.review-finding-detail__issue {
  margin: 0;
  color: var(--text-primary, #1f2937);
}

.review-finding-detail__suggestion,
.review-finding-detail__location,
.review-finding-detail__triage-reason {
  margin: 0;
  color: var(--text-secondary, #555);
}

.review-finding-detail__label {
  font-weight: 600;
  margin-right: 4px;
  color: var(--text-tertiary, #6b7280);
}

.review-finding-detail__actions {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 2px;
}

.review-finding-detail__source-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  border: 1px solid var(--border-subtle, rgba(0, 0, 0, 0.12));
  border-radius: 4px;
  background: var(--bg-default, #fff);
  color: var(--text-secondary, #555);
  font-size: 11px;
  cursor: pointer;
}
.review-finding-detail__source-btn:hover:not(:disabled) {
  background: var(--bg-hover, rgba(0, 0, 0, 0.04));
}
.review-finding-detail__source-btn:disabled {
  opacity: 0.6;
  cursor: progress;
}

.review-finding-detail__source-error {
  color: #b91c1c;
  font-size: 11px;
}

@media (prefers-color-scheme: dark) {
  .review-finding-detail {
    border-color: var(--border-subtle, rgba(255, 255, 255, 0.1));
    background: var(--bg-elevated, rgba(255, 255, 255, 0.03));
  }
  .review-finding-detail__source-btn {
    background: var(--bg-default, #1f2937);
    color: var(--text-secondary, #d1d5db);
  }
}
</style>
