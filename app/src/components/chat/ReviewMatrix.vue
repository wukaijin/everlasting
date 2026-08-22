<script setup lang="ts">
// C2 (review visualization view, 2026-07-26).
//
// The review matrix panel — embedded at the top of `<ChatPanel>`
// for review workflow sessions (design §1). Header (collapse
// toggle + summary) + tab switch (matrix / dimension compare).
//
// Pure display: no emits back to the parent. The only "action"
// is the user folding / unfolding the panel itself + the
// in-component interactions (cell expand, source_run_id jump,
// dimension switch).
//
// Refresh is driven externally: `<ChatPanel>` calls
// `useReviewStateStore.start(slug)` on mount and
// `streamController.handleToolCall` routes `write_file` events
// to `useReviewStateStore.handleReviewStateWritten`. This
// component just reads `reviewStateStore.state` reactively.
//
// Error states (PRD R3 解析降级):
//   - `error.kind === "missing"` → parent's `shouldShowReviewMatrix`
//     hides us entirely (we never render).
//   - `error.kind === "invalid"` → render the error card with a
//     SubagentDrawer link hint.
//   - `error.kind === "network"` → render the error card with a
//     retry button.

import { computed, ref } from "vue";
import { useReviewStateStore } from "../../stores/reviewState";
import Icon from "../Icon.vue";
import ReviewMatrixGrid from "./ReviewMatrixGrid.vue";
import ReviewDimensionCompare from "./ReviewDimensionCompare.vue";

const reviewStateStore = useReviewStateStore();

/** Collapse state. Default OPEN — review is high-information
 *  density and the user wants it in peripheral vision while
 *  they answer `askUserQuestion` in chat. */
const collapsed = ref(false);

/** Active tab: "matrix" (round × model grid) or "dim" (dimension
 *  compare). Defaults to the grid — it's the entry view. */
type Tab = "matrix" | "dim";
const activeTab = ref<Tab>("matrix");

const state = computed(() => reviewStateStore.state);
const error = computed(() => reviewStateStore.error);
const loading = computed(() => reviewStateStore.loading);

/** Header summary: current round + total findings. Concise so the
 *  collapsed state still tells the user where the review is. */
const summary = computed<string>(() => {
  const s = state.value;
  if (!s) return "";
  const totalFindings = s.rounds.reduce(
    (n, r) =>
      n +
      Object.values(r.models).reduce((m, mv) => m + mv.findings.length, 0),
    0,
  );
  return `第 ${s.current_round} 轮 · ${s.rounds.length} 轮 · ${totalFindings} 条 finding`;
});

async function retry(): Promise<void> {
  // `currentSlugForRouting` is the live slug; refresh re-reads.
  const slug = reviewStateStore.currentSlugForRouting;
  if (slug) {
    await reviewStateStore.refresh(slug);
  }
}
</script>

<template>
  <section v-if="error" class="review-matrix review-matrix--error">
    <div class="review-matrix__head">
      <Icon name="warn" :size="14" />
      <span class="review-matrix__title">Review 数据暂不可用</span>
    </div>
    <p class="review-matrix__error-body">
      <template v-if="error.kind === 'invalid'">
        review-state.json 解析失败:{{ error.detail ?? "未知错误" }}
      </template>
      <template v-else-if="error.kind === 'network'">
        加载 review 数据失败,请检查 daemon 连接。
      </template>
      <template v-else>review-state.json 不存在。</template>
    </p>
    <div class="review-matrix__error-actions">
      <button
        v-if="error.kind === 'network'"
        type="button"
        class="review-matrix__btn"
        :disabled="loading"
        @click="retry"
      >
        {{ loading ? "重试中..." : "重试" }}
      </button>
      <span class="review-matrix__hint">
        可在 SubagentDrawer 查看各 reviewer 的原始产出。
      </span>
    </div>
  </section>

  <section v-else-if="state" class="review-matrix">
    <header class="review-matrix__head">
      <button
        type="button"
        class="review-matrix__collapse"
        :aria-expanded="!collapsed"
        :aria-label="collapsed ? '展开 review 矩阵' : '折叠 review 矩阵'"
        @click="collapsed = !collapsed"
      >
        <Icon :name="collapsed ? 'chevron-right' : 'chevron-down'" :size="12" />
      </button>
      <span class="review-matrix__title">Review 矩阵</span>
      <span class="review-matrix__summary">{{ summary }}</span>
      <span
        v-if="loading"
        class="review-matrix__loading"
        aria-live="polite"
      >
        同步中...
      </span>
      <div class="review-matrix__tabs">
        <button
          type="button"
          class="review-matrix__tab"
          :class="{ 'review-matrix__tab--active': activeTab === 'matrix' }"
          @click="activeTab = 'matrix'"
        >
          轮次×模型
        </button>
        <button
          type="button"
          class="review-matrix__tab"
          :class="{ 'review-matrix__tab--active': activeTab === 'dim' }"
          @click="activeTab = 'dim'"
        >
          维度对比
        </button>
      </div>
    </header>

    <div v-if="!collapsed" class="review-matrix__body">
      <ReviewMatrixGrid v-show="activeTab === 'matrix'" :state="state" />
      <ReviewDimensionCompare
        v-if="activeTab === 'dim'"
        :state="state"
      />
      <p class="review-matrix__guide">
        ↑ 在 chat 中回答 askUserQuestion 指挥收敛
      </p>
    </div>
  </section>
</template>

<style scoped>
/* Token 迁移(08-22-review-token-migration):原 var(--bg-default, #fff) /
   var(--text-*, …) / var(--border-subtle, …) 均为未定义变量,浅色 fallback
   在暗色应用内渲染成白底面板。现全部消费正式 token;层级遵循 design-tokens
   约定:面板=surface、头/子面=elevated,文字按 primary/secondary/muted 三档。
   原 @media (prefers-color-scheme: dark) 补丁随迁移删除(app 为 dark-only)。 */
.review-matrix {
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  background: var(--color-bg-surface);
  margin-bottom: var(--space-2);
  overflow: hidden;
}

.review-matrix__head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  /* 6px/10px:介于 --space-1 与 --space-2 之间的半步,紧凑头行不取整档 */
  padding: 6px 10px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
}

.review-matrix__collapse {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  border: none;
  background: transparent;
  color: var(--color-text-secondary);
  cursor: pointer;
  /* 3px:图标钮内圆角,低于 --radius-sm 的装饰档 */
  border-radius: 3px;
}
.review-matrix__collapse:hover {
  background: var(--color-bg-hover);
}

.review-matrix__title {
  font-weight: var(--weight-semibold);
  font-size: var(--text-base);
  color: var(--color-text-primary);
}

.review-matrix__summary {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

.review-matrix__loading {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

.review-matrix__tabs {
  margin-left: auto;
  display: flex;
  gap: 2px;
}

.review-matrix__tab {
  padding: 2px var(--space-2);
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--color-text-secondary);
  font-size: var(--text-xs);
  cursor: pointer;
}
.review-matrix__tab:hover {
  background: var(--color-bg-hover);
}
/* 激活态用 --color-bg-selected(spec:"selected list item / active nav")*/
.review-matrix__tab--active {
  background: var(--color-bg-selected);
  color: var(--color-text-primary);
  border-color: var(--color-bg-border-strong);
}

.review-matrix__body {
  padding: 10px;
}

.review-matrix__guide {
  margin: var(--space-2) 0 0;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  text-align: center;
}

/* Error state */
.review-matrix--error .review-matrix__head {
  border-bottom: none;
}
.review-matrix__error-body {
  margin: 0;
  padding: 0 10px var(--space-2);
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}
.review-matrix__error-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 10px 10px;
}
.review-matrix__btn {
  padding: 3px 10px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg-surface);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  cursor: pointer;
}
.review-matrix__btn:hover:not(:disabled) {
  background: var(--color-bg-hover);
}
.review-matrix__btn:disabled {
  opacity: 0.6;
  cursor: progress;
}
.review-matrix__hint {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}
</style>
