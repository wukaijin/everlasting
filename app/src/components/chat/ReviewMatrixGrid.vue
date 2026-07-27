<script setup lang="ts">
// C2 (review visualization view, 2026-07-26).
//
// Round × model matrix — the main view. Rows = rounds, columns =
// the UNION of every `model_display` seen across all rounds (so
// a model that joined in round 2 still has a column for round 1,
// shown as "未参与"). Cells show verdict chip + findings count;
// clicking a cell expands its findings list inline.
//
// Failed models (`status` ∈ {error, incomplete, cancelled}) are
// greyed with a tooltip — PRD R3 adoption of 议题 4. The cell is
// still clickable (so the user can see any partial findings the
// synthesizer managed to extract), but the visual cue says "this
// review didn't finish cleanly".
//
// `models` map key = stable `model_id` (NOT display name — C3 R7).
// We key cells on `model_id` but display `model_display`; a model
// that gets re-labelled between rounds still collapses to one
// column (matching the synthesizer's intent).

import { computed, ref } from "vue";
import type {
  ModelVerdict,
  ReviewFinding,
  ReviewRound,
  ReviewState,
  RunStatus,
  Verdict,
} from "../../types/review-state";
import ReviewFindingDetail from "./ReviewFindingDetail.vue";

const props = defineProps<{
  state: ReviewState;
}>();

/** Verdict → display label + CSS modifier. Matches the C3 R7
 *  enum exactly (pass/pass_with_minor/revise/reject). */
const VERDICT_META: Record<Verdict, { label: string; cls: string }> = {
  pass: { label: "通过", cls: "pass" },
  pass_with_minor: { label: "小修", cls: "minor" },
  revise: { label: "修订", cls: "revise" },
  reject: { label: "拒绝", cls: "reject" },
};

/** Statuses that mean "this model's review didn't finish cleanly".
 *  The synthesizer may still extract partial findings from a
 *  cancelled run's transcript, so we don't hide the cell — just
 *  grey it + add a tooltip. */
const FAILED_STATUSES: ReadonlySet<RunStatus> = new Set<RunStatus>([
  "error",
  "incomplete",
  "cancelled",
]);

/** Column model: one per `model_id` seen in any round. Preserves
 *  first-seen order (round 1, then any new additions in round 2,
 *  etc.). `model_display` is from the latest round that has the
 *  model (so a re-label sticks). */
interface ColumnModel {
  modelId: string;
  modelDisplay: string;
}

/** Stable column list across all rounds. */
const columns = computed<ColumnModel[]>(() => {
  const seen = new Map<string, ColumnModel>();
  for (const r of props.state.rounds) {
    for (const [modelId, mv] of Object.entries(r.models)) {
      const existing = seen.get(modelId);
      if (!existing) {
        seen.set(modelId, { modelId, modelDisplay: mv.model_display });
      } else {
        // Re-label if the latest round uses a different display.
        existing.modelDisplay = mv.model_display;
      }
    }
  }
  return Array.from(seen.values());
});

/** Per-round, per-model verdict lookup. `null` = model didn't
 *  participate in this round (cell shows "未参与"). */
function verdictFor(round: ReviewRound, modelId: string): ModelVerdict | null {
  return round.models[modelId] ?? null;
}

/** Total findings across the whole state — the header summary. */
const totalFindings = computed<number>(() => {
  let n = 0;
  for (const r of props.state.rounds) {
    for (const mv of Object.values(r.models)) {
      n += mv.findings.length;
    }
  }
  return n;
});

/** Expand state: keyed by `${round}|${modelId}`. Clicking a cell
 *  toggles the inline findings list. Only one cell expanded at a
 *  time per row keeps the layout readable (multiple rows can have
 *  their own expanded cell). Actually we allow multiple — a Set
 *  keyed by the composite id. */
const expandedKeys = ref<Set<string>>(new Set());

function cellKey(round: number, modelId: string): string {
  return `${round}|${modelId}`;
}

function isExpanded(round: number, modelId: string): boolean {
  return expandedKeys.value.has(cellKey(round, modelId));
}

function toggleCell(round: number, modelId: string, mv: ModelVerdict | null): void {
  if (!mv) return; // "未参与" cells aren't clickable
  const key = cellKey(round, modelId);
  if (expandedKeys.value.has(key)) {
    expandedKeys.value.delete(key);
  } else {
    expandedKeys.value.add(key);
  }
  // Force reactivity on the Set mutation.
  expandedKeys.value = new Set(expandedKeys.value);
}

function verdictMeta(v: Verdict): { label: string; cls: string } {
  return VERDICT_META[v] ?? VERDICT_META.revise;
}

function isFailed(mv: ModelVerdict | null): boolean {
  return !!mv && FAILED_STATUSES.has(mv.status);
}

function findingsForCell(mv: ModelVerdict | null): ReviewFinding[] {
  return mv ? mv.findings : [];
}
</script>

<template>
  <div class="review-matrix-grid">
    <div class="review-matrix-grid__summary">
      共 {{ state.rounds.length }} 轮 · {{ totalFindings }} 条 finding
    </div>

    <div
      class="review-matrix-grid__table"
      role="grid"
      :style="{ '--rmg-cols': columns.length }"
    >
      <!-- Header row: corner + one column per model -->
      <div class="review-matrix-grid__row review-matrix-grid__row--head" role="row">
        <div class="review-matrix-grid__cell review-matrix-grid__cell--corner" role="columnheader">
          轮次
        </div>
        <div
          v-for="col in columns"
          :key="col.modelId"
          class="review-matrix-grid__cell review-matrix-grid__cell--head"
          role="columnheader"
          :title="col.modelId"
        >
          {{ col.modelDisplay }}
        </div>
      </div>

      <!-- One row per round -->
      <template v-for="r in state.rounds" :key="r.round">
        <div class="review-matrix-grid__row" role="row">
          <div
            class="review-matrix-grid__cell review-matrix-grid__cell--row-head"
            role="rowheader"
          >
            第 {{ r.round }} 轮
            <span
              v-if="r.convergence_note"
              class="review-matrix-grid__convergence"
              :title="r.convergence_note"
            >
              ⚑
            </span>
          </div>
          <div
            v-for="col in columns"
            :key="col.modelId"
            class="review-matrix-grid__cell review-matrix-grid__cell--body"
            :class="{
              'review-matrix-grid__cell--failed': isFailed(verdictFor(r, col.modelId)),
              'review-matrix-grid__cell--empty': !verdictFor(r, col.modelId),
              'review-matrix-grid__cell--expanded': isExpanded(r.round, col.modelId),
            }"
            role="gridcell"
            :title="isFailed(verdictFor(r, col.modelId)) ? '此模型本轮未完成' : undefined"
            :tabindex="verdictFor(r, col.modelId) ? 0 : -1"
            @click="toggleCell(r.round, col.modelId, verdictFor(r, col.modelId))"
            @keydown.enter.prevent="toggleCell(r.round, col.modelId, verdictFor(r, col.modelId))"
            @keydown.space.prevent="toggleCell(r.round, col.modelId, verdictFor(r, col.modelId))"
          >
            <template v-if="verdictFor(r, col.modelId)">
              <span
                class="review-matrix-grid__verdict"
                :class="`review-matrix-grid__verdict--${verdictMeta(verdictFor(r, col.modelId)!.verdict).cls}`"
              >
                {{ verdictMeta(verdictFor(r, col.modelId)!.verdict).label }}
              </span>
              <span class="review-matrix-grid__count">
                {{ verdictFor(r, col.modelId)!.findings.length }} 条
              </span>
            </template>
            <span v-else class="review-matrix-grid__absent">未参与</span>
          </div>
        </div>

        <!-- Expanded findings row: renders below the round row,
             spanning all columns. Shows the findings for every
             expanded cell in this round (typically just one). -->
        <div
          v-if="columns.some((c) => isExpanded(r.round, c.modelId))"
          class="review-matrix-grid__expanded"
          role="row"
        >
          <div class="review-matrix-grid__expanded-inner">
            <template v-for="col in columns" :key="col.modelId">
              <div
                v-if="isExpanded(r.round, col.modelId)"
                class="review-matrix-grid__expanded-col"
              >
                <div class="review-matrix-grid__expanded-head">
                  第 {{ r.round }} 轮 · {{ col.modelDisplay }}
                </div>
                <div
                  v-if="findingsForCell(verdictFor(r, col.modelId)).length === 0"
                  class="review-matrix-grid__expanded-empty"
                >
                  此模型本轮无 finding。
                </div>
                <ReviewFindingDetail
                  v-for="f in findingsForCell(verdictFor(r, col.modelId))"
                  :key="f.finding_id"
                  :finding="f"
                />
              </div>
            </template>
          </div>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.review-matrix-grid {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.review-matrix-grid__summary {
  font-size: 12px;
  color: var(--text-secondary, #555);
}

.review-matrix-grid__table {
  display: flex;
  flex-direction: column;
  border: 1px solid var(--border-subtle, rgba(0, 0, 0, 0.08));
  border-radius: 6px;
  overflow: hidden;
}

.review-matrix-grid__row {
  display: grid;
  /* CSS var --rmg-cols is set inline on the table; the fallback
     `3` keeps the layout sane if the var is missing. */
  grid-template-columns: 100px repeat(var(--rmg-cols, 3), minmax(120px, 1fr));
  border-bottom: 1px solid var(--border-subtle, rgba(0, 0, 0, 0.06));
}
.review-matrix-grid__row:last-child {
  border-bottom: none;
}

.review-matrix-grid__row--head {
  background: var(--bg-elevated, rgba(0, 0, 0, 0.02));
}

.review-matrix-grid__cell {
  padding: 8px 10px;
  font-size: 12px;
  border-right: 1px solid var(--border-subtle, rgba(0, 0, 0, 0.06));
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.review-matrix-grid__cell:last-child {
  border-right: none;
}

.review-matrix-grid__cell--corner,
.review-matrix-grid__cell--head,
.review-matrix-grid__cell--row-head {
  font-weight: 600;
  color: var(--text-primary, #1f2937);
}

.review-matrix-grid__cell--body {
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 6px;
}
.review-matrix-grid__cell--body:hover {
  background: var(--bg-hover, rgba(0, 0, 0, 0.03));
}
.review-matrix-grid__cell--empty,
.review-matrix-grid__cell--body[tabindex="-1"] {
  cursor: default;
}
.review-matrix-grid__cell--empty:hover {
  background: transparent;
}

.review-matrix-grid__cell--failed {
  opacity: 0.5;
  background: repeating-linear-gradient(
    45deg,
    transparent,
    transparent 4px,
    rgba(0, 0, 0, 0.03) 4px,
    rgba(0, 0, 0, 0.03) 8px
  );
}

.review-matrix-grid__cell--expanded {
  background: var(--bg-hover, rgba(0, 0, 0, 0.04));
}

.review-matrix-grid__convergence {
  color: #d97706;
  font-size: 10px;
  margin-left: 2px;
}

.review-matrix-grid__verdict {
  padding: 1px 6px;
  border-radius: 3px;
  font-size: 11px;
  font-weight: 600;
}
.review-matrix-grid__verdict--pass { background: rgba(34, 197, 94, 0.15); color: #15803d; }
.review-matrix-grid__verdict--minor { background: rgba(59, 130, 246, 0.15); color: #1d4ed8; }
.review-matrix-grid__verdict--revise { background: rgba(217, 119, 6, 0.15); color: #b45309; }
.review-matrix-grid__verdict--reject { background: rgba(220, 38, 38, 0.15); color: #b91c1c; }

.review-matrix-grid__count {
  color: var(--text-secondary, #6b7280);
  font-size: 11px;
}

.review-matrix-grid__absent {
  color: var(--text-tertiary, #9ca3af);
  font-style: italic;
}

.review-matrix-grid__expanded {
  border-bottom: 1px solid var(--border-subtle, rgba(0, 0, 0, 0.06));
  background: var(--bg-elevated, rgba(0, 0, 0, 0.02));
  padding: 8px 10px;
}

.review-matrix-grid__expanded-inner {
  display: grid;
  grid-auto-flow: column;
  grid-auto-columns: minmax(220px, 1fr);
  gap: 10px;
}

.review-matrix-grid__expanded-col {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.review-matrix-grid__expanded-head {
  font-weight: 600;
  font-size: 12px;
  color: var(--text-primary, #1f2937);
}

.review-matrix-grid__expanded-empty {
  font-size: 11px;
  color: var(--text-tertiary, #9ca3af);
  padding: 8px 4px;
}

@media (prefers-color-scheme: dark) {
  .review-matrix-grid__row--head,
  .review-matrix-grid__expanded {
    background: var(--bg-elevated, rgba(255, 255, 255, 0.03));
  }
  .review-matrix-grid__cell--failed {
    background: repeating-linear-gradient(
      45deg,
      transparent,
      transparent 4px,
      rgba(255, 255, 255, 0.04) 4px,
      rgba(255, 255, 255, 0.04) 8px
    );
  }
}
</style>
