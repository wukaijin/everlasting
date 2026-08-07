<script setup lang="ts">
// DiscussionSummaryCard — 群聊收尾专用卡片(08-07-group-chat-role-history-isolation
// follow-up)。
//
// `end_discussion` 的 `summary` 参数是整场讨论的最终结论 —— 对用户是全场最有
// 价值的产出,却藏在工具调用的 tool_result 信封里(只以 ToolCallCard 的 output
// 呈现,200px 截断 + 可滚动)。本卡片把该总结提取出来,渲染为可见的"讨论总结"
// 块:徽章 + 全文(markdown 渲染,不折叠、不截断)。
//
// live 流式:tool_result 到达前(仅 tool_use 已 emit)显示占位态;result 到达后
// (或 rehydrate 路径)显示总结全文。与 ToolCallCard 共用同一 `result` 数据源
// (`getToolResult`),替换 end_discussion 的卡片位置,不新增 DOM 层级。

import { computed } from "vue";
import { extractToolResultDisplay } from "../../utils/messageFormat";
import { renderMarkdown } from "../../utils/markdown";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";

const props = defineProps<{
  call: ToolCallInfo;
  result?: ToolResultInfo;
}>();

/** 解 `{result, cwd}` 信封后的总结全文;result 未到达时为空。 */
const summary = computed<string>(() => {
  if (!props.result) return "";
  return extractToolResultDisplay(props.result.content).trim();
});

/** 总结正文的 markdown HTML(与消息气泡同一 renderMarkdown + DOMPurify 管线)。 */
const html = computed<string>(() => renderMarkdown(summary.value));

/** live 流式中 tool_result 尚未到达的占位态。 */
const pending = computed<boolean>(() => !props.result);
</script>

<template>
  <div class="discussion-summary" :data-testid="`discussion-summary-${call.id}`">
    <div class="discussion-summary__header">
      <span class="discussion-summary__badge">讨论总结</span>
      <span v-if="pending" class="discussion-summary__pending">
        主持人正在结束讨论…
      </span>
    </div>
    <div v-if="!pending" class="discussion-summary__body">
      <span class="msg__markdown" v-html="html" />
    </div>
  </div>
</template>

<style scoped>
.discussion-summary {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-accent);
  border-radius: var(--radius-md);
  padding: 10px 14px;
  max-width: 100%;
}

.discussion-summary__header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
}

.discussion-summary__badge {
  font-size: var(--text-xs);
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--color-accent);
  background: var(--color-accent-muted);
  border-radius: var(--radius-sm);
  padding: 2px 8px;
}

.discussion-summary__pending {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

.discussion-summary__body {
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  line-height: 1.6;
  white-space: normal;
}
</style>
