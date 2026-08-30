<script setup lang="ts">
// PermissionActions — interactive permission-ask action block (4 按钮 +
// 拒绝理由 textarea 交互)。
//
// 2026-08-30 (task `08-30-shell-description` PR2): extracted from
// `PermissionAskBody.vue`'s interactive actions block so the new
// `<ShellCard>` 一体化审批 can reuse the exact same button row /
// feedback interaction without mounting the whole PermissionAskBody
// (which would re-render the "需要权限" head + box — ShellCard 需要
// 的是无独立容器的裸按钮列)。
//
// **行为保持搬运**(AC4 验收线):markup、class 名、交互逻辑、
// allowAlwaysLabel 分叉全部原样搬入 —— PermissionAskBody 既有测试
// 零改动全绿是本次抽取的硬约束,DOM class(`permission-ask-body__*`)
// 因此保留原名(测试 / `:deep()` 以之为锚,改名即行为漂移)。
//
// Props(per design §3.2):`{ ask, onRespond, hideAllowAlways? }`。
// 0 store —— 父组件持有 store 并传入 `onRespond`(FT-F-001 D3 同款
// 边界)。`onRespond` 为 undefined 时按钮不渲染(防御,与
// PermissionAskBody 原 v-if 守卫一致)。

import { computed, ref } from "vue";
import {
  type PermissionAsk,
  type PermissionDecision,
} from "../../stores/permissions";

const props = withDefaults(
  defineProps<{
    /** The pending ask. Only `ask.workerRunId` is read here — it
     *  forks the "allow_always" button label. */
    ask: PermissionAsk;
    /** Decision callback. When absent the whole action row does not
     *  render (matches the PermissionAskBody guard this block was
     *  extracted from). */
    onRespond?: (decision: PermissionDecision, reason?: string) => void;
    /** When `true`, the "始终允许" button is NOT rendered.
     *  PermissionAskBody forwards its own prop; ShellCard omits it. */
    hideAllowAlways?: boolean;
  }>(),
  {
    onRespond: undefined,
    hideAllowAlways: false,
  },
);

// 2026-06-26 (task `06-26-subagent-per-run-grant` Step 2): the
// "allow_always" button label forks by ask scope.
//   - Main-chat ask (`ask.workerRunId` absent) → `始终允许`. The
//     backend persists the grant to `session_tool_permissions`
//     (survives across requests in the same session).
//   - Worker ask (`ask.workerRunId` present) → `本次运行始终允许`.
//     The backend persists the grant to a per-run in-memory cache
//     (`RunGrantCache`) that dies with the worker run — the label
//     makes the run-scoped semantics explicit so the user doesn't
//     confuse it with the main-chat session-level persistence.
//
// The wire is still `"allow_always"`; the backend forks the
// persistence target by `is_worker` (parent → DB; worker → run
// cache).
const allowAlwaysLabel = computed<string>(() =>
  props.ask.workerRunId ? "本次运行始终允许" : "始终允许",
);

// Interactive-only local state (moved verbatim from PermissionAskBody).
const showFeedback = ref(false);
const feedback = ref("");

function respond(decision: PermissionDecision): void {
  if (!props.onRespond) return;
  props.onRespond(decision);
}

function submitFeedback(): void {
  if (!props.onRespond) return;
  props.onRespond("deny", feedback.value.trim() || undefined);
  showFeedback.value = false;
  feedback.value = "";
}

function cancelFeedback(): void {
  showFeedback.value = false;
  feedback.value = "";
}
</script>

<template>
  <!-- Fragment root(v-if/v-else 二选一,同时只渲染一个 div),挂在
       父 flex 容器里与抽取前 DOM 完全一致(不引入包装盒,flex gap
       节奏不变)。挂载与否由父级守卫(interactive && onRespond)。 -->
  <div v-if="showFeedback" class="permission-ask-body__feedback">
    <textarea
      v-model="feedback"
      class="permission-ask-body__textarea"
      rows="2"
      placeholder="告诉 agent 为什么拒绝 / 该怎么做（可选）"
    ></textarea>
    <div class="permission-ask-body__feedback-actions">
      <button
        type="button"
        class="permission-ask-body__btn permission-ask-body__btn--deny btn btn--muted btn--sm"
        @click="submitFeedback"
      >提交拒绝</button>
      <button
        type="button"
        class="permission-ask-body__btn btn btn--muted btn--sm"
        @click="cancelFeedback"
      >取消</button>
    </div>
  </div>
  <div v-else class="permission-ask-body__actions">
    <button
      type="button"
      class="permission-ask-body__btn permission-ask-body__btn--once btn btn--muted btn--sm"
      @click="respond('allow_once')"
    >仅一次</button>
    <button
      v-if="!hideAllowAlways"
      type="button"
      class="permission-ask-body__btn permission-ask-body__btn--always btn btn--primary btn--sm"
      @click="respond('allow_always')"
    >{{ allowAlwaysLabel }}</button>
    <button
      type="button"
      class="permission-ask-body__btn permission-ask-body__btn--deny btn btn--muted btn--sm"
      @click="respond('deny')"
    >拒绝</button>
    <button
      type="button"
      class="permission-ask-body__btn permission-ask-body__btn--deny btn btn--muted btn--sm"
      @click="showFeedback = true"
    >拒绝并说明</button>
  </div>
</template>

<style scoped>
/* 以下规则整体自 PermissionAskBody 搬入(action 块专属;class 名
 * 保持 `permission-ask-body__*` 以锚定既有测试与全局 .btn 家族协作)。 */
.permission-ask-body__actions,
.permission-ask-body__feedback-actions {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}

/* 按钮由全局 .btn 家族承载(紧凑档 sm:仅一次/取消/拒绝 = muted·sm,
   始终允许 = primary·sm);deny 是"红字描边"语义,家族无对应变体,
   本地覆写文字/边框色。 */
.permission-ask-body__btn--deny {
  color: var(--color-tool-error-text);
  border-color: var(--color-tool-error);
}

.permission-ask-body__textarea {
  width: 100%;
  font: inherit;
  font-family: var(--font-sans);
  font-size: var(--text-xs);
  padding: 4px 6px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg-surface);
  color: var(--color-text-primary);
  resize: vertical;
}
</style>
