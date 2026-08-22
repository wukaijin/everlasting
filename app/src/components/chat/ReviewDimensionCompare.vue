<script setup lang="ts">
// C2 (review visualization view, 2026-07-26).
//
// Dimension comparison view — the core value of the matrix
// (PRD R3). The user picks a dimension (e.g. "清晰度"); the
// component shows each model's findings for that dimension
// horizontally, so diverging severity / triage across models
// is visible at a glance.
//
// "Each model" = models present in the LATEST round (we don't
// merge across rounds here — the comparison is "as of the most
// recent re-review", which is what the user acts on). Earlier
// rounds are reachable by switching the round selector.

import { computed, ref } from "vue";
import type { ReviewRound, ReviewState } from "../../types/review-state";
import ReviewFindingDetail from "./ReviewFindingDetail.vue";

const props = defineProps<{
  state: ReviewState;
}>();

/** Union of dimensions across ALL rounds, deduped + sorted. The
 *  dropdown needs the full history so the user can inspect a
 *  dimension that was only raised in an earlier round. */
const allDimensions = computed<string[]>(() => {
  const set = new Set<string>();
  for (const r of props.state.rounds) {
    for (const d of r.dimensions) set.add(d);
    // Also collect dimensions declared on individual findings
    // (defensive: the round-level `dimensions` array is the
    // canonical source, but a finding may carry a dimension
    // that wasn't pre-declared).
    for (const mv of Object.values(r.models)) {
      for (const f of mv.findings) set.add(f.dimension);
    }
  }
  return Array.from(set).sort((a, b) => a.localeCompare(b));
});

const selectedDimension = ref<string | null>(null);

/** Initialize / re-initialize the selection when the dimension
 *  set changes (e.g. a new round added a dimension). Picks the
 *  first dimension so the view isn't empty on first render. */
function ensureSelection(): void {
  if (allDimensions.value.length === 0) {
    selectedDimension.value = null;
    return;
  }
  if (!selectedDimension.value || !allDimensions.value.includes(selectedDimension.value)) {
    selectedDimension.value = allDimensions.value[0];
  }
}
// Run once on setup + whenever the dimension list shifts.
// `computed` + a watcher would also work; this is simpler since
// the list is already reactive via `props.state`.
ensureSelection();

/** Round selector — defaults to the latest round (last entry),
 *  the one the user is most likely acting on. */
const selectedRound = ref<number | null>(null);
function ensureRound(): void {
  if (props.state.rounds.length === 0) {
    selectedRound.value = null;
    return;
  }
  const latest = props.state.rounds[props.state.rounds.length - 1].round;
  if (selectedRound.value === null) selectedRound.value = latest;
}
ensureRound();

const currentRound = computed<ReviewRound | null>(() => {
  if (selectedRound.value === null) return null;
  return (
    props.state.rounds.find((r) => r.round === selectedRound.value) ?? null
  );
});

/** Per-model findings for the selected dimension in the selected
 *  round. Preserves the `models` map order (which is insertion
 *  order in JS). Models with NO finding for this dimension are
 *  still listed (with an empty array) so the user can see "this
 *  model didn't raise this dimension" — that's signal too. */
interface ModelColumn {
  modelId: string;
  modelDisplay: string;
  findings: import("../../types/review-state").ReviewFinding[];
}

const columns = computed<ModelColumn[]>(() => {
  const round = currentRound.value;
  if (!round || !selectedDimension.value) return [];
  const out: ModelColumn[] = [];
  for (const [modelId, mv] of Object.entries(round.models)) {
    out.push({
      modelId,
      modelDisplay: mv.model_display,
      findings: mv.findings.filter((f) => f.dimension === selectedDimension.value),
    });
  }
  return out;
});
</script>

<template>
  <div class="review-dim-compare">
    <div class="review-dim-compare__controls">
      <label class="review-dim-compare__control">
        <span class="review-dim-compare__control-label">维度</span>
        <select v-model="selectedDimension" class="review-dim-compare__select">
          <option v-if="allDimensions.length === 0" :value="null" disabled>
            （本轮无维度）
          </option>
          <option v-for="d in allDimensions" :key="d" :value="d">{{ d }}</option>
        </select>
      </label>
      <label class="review-dim-compare__control">
        <span class="review-dim-compare__control-label">轮次</span>
        <select v-model="selectedRound" class="review-dim-compare__select">
          <option v-for="r in state.rounds" :key="r.round" :value="r.round">
            第 {{ r.round }} 轮
          </option>
        </select>
      </label>
    </div>

    <div v-if="columns.length === 0" class="review-dim-compare__empty">
      当前轮次无模型数据。
    </div>

    <div v-else class="review-dim-compare__grid">
      <div
        v-for="col in columns"
        :key="col.modelId"
        class="review-dim-compare__column"
      >
        <div class="review-dim-compare__column-head">{{ col.modelDisplay }}</div>
        <div v-if="col.findings.length === 0" class="review-dim-compare__column-empty">
          未提及
        </div>
        <ReviewFindingDetail
          v-for="f in col.findings"
          :key="f.finding_id"
          :finding="f"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
/* Token 迁移(08-22-review-token-migration):同 ReviewMatrix.vue 头注。 */
.review-dim-compare {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.review-dim-compare__controls {
  display: flex;
  gap: var(--space-3);
  flex-wrap: wrap;
}

.review-dim-compare__control {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

.review-dim-compare__control-label {
  font-weight: var(--weight-semibold);
}

.review-dim-compare__select {
  padding: 3px 6px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
}

.review-dim-compare__empty {
  font-size: var(--text-sm);
  color: var(--color-text-muted);
  padding: var(--space-3);
  text-align: center;
}

.review-dim-compare__grid {
  display: grid;
  grid-auto-flow: column;
  grid-auto-columns: minmax(220px, 1fr);
  gap: 10px;
  overflow-x: auto;
}

.review-dim-compare__column {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.review-dim-compare__column-head {
  font-weight: var(--weight-semibold);
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  padding-bottom: 4px;
  border-bottom: 1px solid var(--color-bg-border);
}

.review-dim-compare__column-empty {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  padding: var(--space-2) 4px;
}
</style>
