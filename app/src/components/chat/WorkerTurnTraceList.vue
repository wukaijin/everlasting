<script setup lang="ts">
// WorkerTurnTraceList — SubagentDrawer 的 per-run「Token 明细」折叠区
// (08-20-worker-turn-trace-persist PR3)。
//
// 数据源:store.loadRunTurnTraces(runId) → 后端
// `list_worker_turn_traces` IPC → turn_trace 表中 run_id = 该 run 的
// worker per-turn 行(usage / tools_token / system_token /
// context_window;memory/images/@文件 列按 worker 语义恒 NULL)。
//
// 拉取时机:默认收起;首次展开触发一次拉取。run 仍处 running 态时
// 每次 expand 都 force 重拉(worker 行随 Done 事件逐轮累积);终态
// run 拉一次即粘性缓存(store.runTracesByRunId,不随 drawer 关闭清)。
//
// ⚠️ 行内 seq 是 worker loop 自己的游标(非父 messages 全局 seq),
// 仅作 run 内排序/展示,勿跨 run 比较(prd Risks)。
//
// 空态:后端对未知 run / 迁移前数据返回空 Vec(非错误)→ 显示
// 「无 per-turn 记录」。失败:store.runTracesError 一行降级文案,
// 不 toast(trace 是次级检查面)。

import { computed, ref } from "vue";
import { useSubagentRunsStore } from "../../stores/subagentRuns";
import type { TurnTrace } from "../../types/turnTrace";
import Icon from "../Icon.vue";

const props = defineProps<{
  /** The worker run id (`subagent_runs.id`). */
  runId: string;
}>();

const store = useSubagentRunsStore();

const expanded = ref(false);

const traces = computed<TurnTrace[] | undefined>(() =>
  store.runTracesByRunId.get(props.runId),
);
const loading = computed(() => store.runTracesLoading.get(props.runId) === true);
const error = computed(() => store.runTracesError.get(props.runId) ?? null);
/** running 态下 worker 行仍在累积 —— expand 时 force 重拉。 */
const isRunning = computed(
  () => store.getRunCache.get(props.runId)?.status === "running",
);

function toggle(): void {
  expanded.value = !expanded.value;
  if (expanded.value) {
    void store.loadRunTurnTraces(props.runId, { force: isRunning.value });
  }
}

/** 前端预算行分母回退(与 TurnCard 的 200_000 回退一致 —— worker
 * 行按契约都带 context_window,回退只服务异常/旧行)。 */
const WINDOW_FALLBACK = 200_000;

function ctxPct(t: TurnTrace): string {
  const ctx = t.tokenUsage?.context_input_tokens;
  if (!ctx || ctx <= 0) return "—";
  const window = t.contextWindow ?? WINDOW_FALLBACK;
  return `${((ctx / window) * 100).toFixed(0)}%`;
}

function cacheRate(t: TurnTrace): string {
  const ctx = t.tokenUsage?.context_input_tokens;
  const read = t.tokenUsage?.cache_read_input_tokens;
  if (!ctx || ctx <= 0 || read === undefined) return "—";
  return `${((read / ctx) * 100).toFixed(0)}%`;
}

/** 1234 → "1234";12500 → "12.5k";50000 → "50k"(整千不带小数)。
 *  数字列紧凑展示(与 TracePanel TurnCard 的体量感对齐,组件内自足,
 *  不引 utils)。 */
function fmt(n: number | undefined): string {
  if (n === undefined) return "—";
  if (n >= 100_000) return `${Math.round(n / 1000)}k`;
  if (n >= 10_000) {
    const k = n / 1000;
    return `${Number.isInteger(k) ? k : k.toFixed(1)}k`;
  }
  return String(n);
}
</script>

<template>
  <div class="worker-turn-trace">
    <button
      type="button"
      class="worker-turn-trace__header"
      :aria-expanded="expanded"
      @click="toggle"
    >
      <Icon :name="expanded ? 'chevron-down' : 'chevron-right'" :size="12" />
      <span>Token 明细</span>
      <span
        v-if="loading"
        class="app-spinner app-spinner--xs worker-turn-trace__spinner"
        aria-label="加载中"
      />
      <span
        v-else-if="traces !== undefined"
        class="worker-turn-trace__count"
      >{{ traces.length }} 轮</span>
    </button>

    <div v-if="expanded" class="worker-turn-trace__body">
      <p v-if="error !== null" class="worker-turn-trace__error" role="alert">
        拉取失败: {{ error }}
      </p>
      <p
        v-else-if="traces !== undefined && traces.length === 0"
        class="worker-turn-trace__empty"
      >
        无 per-turn 记录(旧 run 或迁移前数据)
      </p>
      <table v-else-if="traces !== undefined" class="worker-turn-trace__table">
        <thead>
          <tr>
            <th>轮</th>
            <th>in</th>
            <th>out</th>
            <th>cache读</th>
            <th>命中率</th>
            <th>tools</th>
            <th>ctx占比</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="t in traces" :key="t.id">
            <td>#{{ t.seq }}</td>
            <td>{{ fmt(t.tokenUsage?.input_tokens) }}</td>
            <td>{{ fmt(t.tokenUsage?.output_tokens) }}</td>
            <td>{{ fmt(t.tokenUsage?.cache_read_input_tokens) }}</td>
            <td>{{ cacheRate(t) }}</td>
            <td>{{ fmt(t.toolsToken) }}</td>
            <td>{{ ctxPct(t) }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>

<style scoped>
.worker-turn-trace {
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
}

.worker-turn-trace__header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 12px;
  border: none;
  background: transparent;
  color: var(--color-text-primary);
  font: inherit;
  font-family: var(--font-sans);
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  cursor: pointer;
}

.worker-turn-trace__header:hover {
  background: var(--color-bg-surface);
}

.worker-turn-trace__count {
  color: var(--color-text-muted);
  font-weight: var(--weight-regular);
}

/* 形态由全局 .app-spinner--xs 原语提供(style.css);此处类名留作测试/检索钩子 */

.worker-turn-trace__body {
  padding: 0 12px 10px;
}

.worker-turn-trace__error {
  margin: 0;
  padding: 6px 0;
  color: var(--color-tool-error);
  font-size: var(--text-sm);
}

.worker-turn-trace__empty {
  margin: 0;
  padding: 6px 0;
  color: var(--color-text-muted);
  font-size: var(--text-sm);
}

.worker-turn-trace__table {
  width: 100%;
  border-collapse: collapse;
  font-family: var(--font-mono, monospace);
  font-size: var(--text-sm);
}

.worker-turn-trace__table th {
  text-align: right;
  color: var(--color-text-muted);
  font-weight: var(--weight-regular);
  padding: 2px 6px;
  border-bottom: 1px solid var(--color-bg-border);
}

.worker-turn-trace__table td {
  text-align: right;
  padding: 3px 6px;
  color: var(--color-text-primary);
  border-bottom: 1px solid var(--color-bg-border);
  font-variant-numeric: tabular-nums;
}

/* 首列(轮号)左对齐,与表头对齐。 */
.worker-turn-trace__table th:first-child,
.worker-turn-trace__table td:first-child {
  text-align: left;
}

.worker-turn-trace__table tbody tr:last-child td {
  border-bottom: none;
}
</style>
