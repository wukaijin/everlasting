<script setup lang="ts">
// EditFileCard — `edit_file` 专属卡片 (git diff 风格的常驻红绿视图)。
//
// 背景: `ToolCallCard` 对 `edit_file` 只有折叠的 input + 可点的
// "diff" 弹窗(需点按钮 + 拉会话级 worktree diff),用户想要像 git
// diff 那样常驻、红绿、扫一眼就看懂的变更视图。本卡片**替换**
// `MessageItem` 中的通用 `ToolCallCard` 渲染(同 `SearchHistoryCard` /
// `DiscussionSummaryCard` 先例),直接从 `tool_use.input.{old_string,
// new_string}` 算行级 diff,无需走 `chatStore.fetchDiff`。
//
// 形态:
//   - Header: ToolCallHeader(图标/文件名/状态/耗时) + 右侧 +N/-N 计数
//   - Body: 红(删除) / 绿(新增) / 灰(上下文) 行,默认收起,点开关展开
//   - 错误/进行中/待审批与 ToolCallCard 同语义,避免流程断层
//   - 输入缺失时降级为 ToolInputBody 兜底(防御 LLM 畸形 input)

import { computed, ref } from "vue";
import { diffLines } from "diff";
import { useChatStore } from "../../stores/chat";
import {
  usePermissionsStore,
  type PermissionDecision,
} from "../../stores/permissions";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";
import {
  extractToolResultDisplay,
  toolAccentVar,
  toolIcon,
} from "../../utils/messageFormat";
import { abbreviateDuration } from "../../utils/duration";
import ToolCallHeader from "./ToolCallHeader.vue";
import ToolInputBody from "./ToolInputBody.vue";
import PermissionAskBody from "./PermissionAskBody.vue";
import Icon from "../Icon.vue";

const props = defineProps<{
  call: ToolCallInfo;
  result?: ToolResultInfo;
}>();

const isError = computed(() => !!props.result?.isError);
const hasResult = computed(() => !!props.result);

const filePath = computed<string | null>(() => {
  const p = props.call.input?.path;
  return typeof p === "string" && p.length > 0 ? p : null;
});

const replaceAll = computed<boolean>(() => props.call.input?.replace_all === true);

const oldStr = computed<string | null>(() => {
  const v = props.call.input?.old_string;
  return typeof v === "string" ? v : null;
});
const newStr = computed<string | null>(() => {
  const v = props.call.input?.new_string;
  return typeof v === "string" ? v : null;
});

const hasInputStrings = computed(
  () => typeof oldStr.value === "string" && typeof newStr.value === "string",
);

const accent = computed(() => {
  if (isError.value) return "var(--color-tool-error)";
  return toolAccentVar(props.call.name);
});

const statusText = computed(() => {
  if (isError.value) return "error";
  if (hasResult.value) return "done";
  return "running…";
});

const statusIconName = computed(() => {
  if (isError.value) return "x";
  if (hasResult.value) return "check";
  return "ellipsis";
});

const durationLabel = computed(() => {
  if (!hasResult.value) return "…";
  const d = props.result?.durationMs;
  if (typeof d !== "number") return "";
  return abbreviateDuration(d);
});

const displayContent = computed<string | null>(() => {
  if (!props.result) return null;
  return extractToolResultDisplay(props.result.content);
});

// ------------------------------------------------------------------
// 行级 diff(基于 jsdiff diffLines,0 store)
// ------------------------------------------------------------------

type DiffRow = { kind: "add" | "del" | "ctx"; text: string };

const MAX_ROWS = 400;

const diffRows = computed<DiffRow[] | null>(() => {
  if (!hasInputStrings.value) return null;
  const a = oldStr.value as string;
  const b = newStr.value as string;
  if (a === b) return [{ kind: "ctx", text: a }];
  try {
    const parts = diffLines(a, b);
    const rows: DiffRow[] = [];
    for (const part of parts) {
      const raw = part.value;
      // split, dropping artifact trailing "" when value ends with \n
      let lines = raw.split("\n");
      if (raw.endsWith("\n") && lines[lines.length - 1] === "") lines.pop();
      // diffLines can emit a single "" for empty input — render as one empty line
      if (lines.length === 1 && lines[0] === "" && raw === "") {
        // empty file side: keep one empty row so prefix still visible
      }
      for (const line of lines) {
        if (part.added) rows.push({ kind: "add", text: line });
        else if (part.removed) rows.push({ kind: "del", text: line });
        else rows.push({ kind: "ctx", text: line });
      }
      if (rows.length >= MAX_ROWS) break;
    }
    return rows.slice(0, MAX_ROWS);
  } catch {
    return null;
  }
});

const truncated = computed<boolean>(() => {
  if (!diffRows.value) return false;
  // Heuristic: if either string has >MAX_ROWS lines, we truncated
  const aLines = (oldStr.value ?? "").split("\n").length;
  const bLines = (newStr.value ?? "").split("\n").length;
  return aLines + bLines > MAX_ROWS || diffRows.value.length >= MAX_ROWS;
});

const addedCount = computed(() => diffRows.value?.filter((r) => r.kind === "add").length ?? 0);
const removedCount = computed(() => diffRows.value?.filter((r) => r.kind === "del").length ?? 0);

const diffExpanded = ref(false);

// ------------------------------------------------------------------
// 错误详情折叠(2026-09-02):错误全文不再常显,默认只留一行摘要
// (首行 + ellipsis),点 toggle 展开。完整文本仍从 displayContent
// 取,展开后原样渲染(pre-wrap)。
// ------------------------------------------------------------------
const errorExpanded = ref(false);

const errorPreview = computed<string>(() => {
  const c = displayContent.value ?? "";
  const first = c.split("\n", 1)[0] ?? "";
  return first.length > 200 ? first.slice(0, 200) + "…" : first;
});

// ------------------------------------------------------------------
// inline approval(复用 ToolCallCard 语义,避免审批链路断层)
// ------------------------------------------------------------------

const chatStore = useChatStore();
const permStore = usePermissionsStore();

const pendingAsk = computed(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return undefined;
  const ask = permStore.getPending(sid);
  return ask && ask.toolUseId === props.call.id ? ask : undefined;
});

const isPendingApproval = computed(() => !hasResult.value && !!pendingAsk.value);

async function respondApproval(decision: PermissionDecision, reason?: string) {
  if (!pendingAsk.value) return;
  await permStore.respond(pendingAsk.value.rid, decision, reason);
}
</script>

<template>
  <div
    class="edit-card"
    :class="{ 'edit-card--error': isError, 'edit-card--running': !hasResult && !isError }"
    :style="{ borderLeftColor: accent }"
    :data-testid="`edit-file-card-${call.id}`"
  >
    <ToolCallHeader
      :icon-name="toolIcon(call.name)"
      :name="call.name"
      :chip="filePath"
      :status-text="statusText"
      :status-icon-name="statusIconName"
      :duration-label="durationLabel"
      :is-error="isError"
      :is-running="!hasResult && !isError"
      :is-success="hasResult && !isError"
    >
      <template #status-extra>
        <span v-if="hasInputStrings" class="edit-card__counts" aria-hidden="true">
          <span v-if="addedCount > 0" class="edit-card__add">+{{ addedCount }}</span>
          <span v-if="removedCount > 0" class="edit-card__del">−{{ removedCount }}</span>
        </span>
        <span v-if="replaceAll" class="edit-card__pill" title="replace_all: true">replace_all</span>
        <button
          v-if="hasInputStrings && diffRows"
          type="button"
          class="edit-card__toggle btn btn--ghost btn--icon"
          :aria-expanded="diffExpanded"
          :aria-label="diffExpanded ? '收起 diff' : '展开 diff'"
          :title="diffExpanded ? '收起 diff' : '展开 diff'"
          @click.stop="diffExpanded = !diffExpanded"
        >
          <span
            class="edit-card__toggle-chevron"
            :class="{ 'edit-card__toggle-chevron--open': diffExpanded }"
          >
            <Icon name="chevron-right" :size="12" />
          </span>
        </button>
      </template>
    </ToolCallHeader>

    <!-- inline approval -->
    <div v-if="isPendingApproval && pendingAsk" class="edit-card__approval">
      <PermissionAskBody
        mode="interactive"
        :ask="pendingAsk"
        :on-respond="respondApproval"
        :repo-root="chatStore.currentCwd"
      />
    </div>

    <!-- error banner(默认折叠:一行红色摘要 + toggle,展开看全文;
         2026-09-02 前错误全文常显,长错误会把卡片撑成一堵红墙) -->
    <div v-if="isError && displayContent" class="edit-card__error">
      <button
        type="button"
        class="edit-card__error-toggle"
        :aria-expanded="errorExpanded"
        :title="errorExpanded ? '收起错误详情' : '展开错误详情'"
        @click.stop="errorExpanded = !errorExpanded"
      >
        <span
          class="edit-card__toggle-chevron"
          :class="{ 'edit-card__toggle-chevron--open': errorExpanded }"
        >
          <Icon name="chevron-right" :size="12" />
        </span>
        <Icon name="warn" :size="12" icon-class="edit-card__error-icon" />
        <span class="edit-card__error-preview">{{ errorPreview }}</span>
      </button>
      <pre v-if="errorExpanded" class="edit-card__error-text">{{ displayContent }}</pre>
    </div>

    <!-- git-style diff (默认收起,开关展开) -->
    <div v-if="hasInputStrings && diffRows && diffExpanded" class="edit-card__diff" :class="{ 'edit-card__diff--error': isError }">
      <div
        v-for="(row, i) in diffRows"
        :key="i"
        :class="['edit-diff-line', `edit-diff-line--${row.kind}`]"
      >
        <span class="edit-diff-line__prefix" aria-hidden="true">
          <template v-if="row.kind === 'add'">+</template>
          <template v-else-if="row.kind === 'del'">−</template>
          <template v-else>&nbsp;</template>
        </span>
        <span class="edit-diff-line__text">{{ row.text }}</span>
      </div>
      <div v-if="truncated" class="edit-card__truncated">
        … 变更过长,已截断至前 {{ MAX_ROWS }} 行。展开 input 可查看完整 old/new 文本。
      </div>
    </div>

    <!-- 输入缺失 / 解析失败 → 兜底为通用 input 视图 -->
    <ToolInputBody
      v-if="!hasInputStrings || !diffRows"
      :name="call.name"
      :input="call.input as Record<string, unknown>"
    />
    <!-- 成功态不渲染结果文案(2026-09-02 移除"Successfully edited"行,
         header ✓ done + diff 视图已承载全部信号;错误态见上方折叠 banner)。 -->
  </div>
</template>

<style scoped>
.edit-card {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-text-muted);
  border-radius: var(--radius-md);
  padding: 8px 12px;
  font-size: var(--text-sm);
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  max-width: 100%;
}

.edit-card--error {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}

.edit-card--running {
  border-left-color: var(--color-tool-shell);
}

.edit-card__counts {
  margin-left: 8px;
  display: inline-flex;
  gap: 4px;
  font-size: var(--text-xs);
  font-weight: var(--weight-semibold);
}

.edit-card__add {
  color: var(--color-tool-write);
}

.edit-card__del {
  color: var(--color-tool-error-text);
}

.edit-card__pill {
  margin-left: 6px;
  font-size: var(--text-2xs);
  letter-spacing: 0.04em;
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  color: var(--color-text-muted);
}

/* 展开/收起开关:icon-only ghost(透明底,hover 才有 wash)。chevron 固定
 * 用右向图标,展开时旋转 90°(→朝下),transform 过渡即切换动画。 */
.edit-card__toggle {
  margin-left: 8px;
}

.edit-card__toggle-chevron {
  display: inline-flex;
  transition: transform var(--duration-fast) var(--ease-out);
}

.edit-card__toggle-chevron--open {
  transform: rotate(90deg);
}

.edit-card__approval {
  margin-top: 6px;
}

.edit-card__error {
  margin-top: 6px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-tool-error) 30%, transparent);
  color: var(--color-tool-error-text);
  font-size: var(--text-xs);
  line-height: var(--leading-normal);
}

/* 折叠 toggle 行:chevron + warn + 单行摘要(ellipsis)。button reset
   后继承容器的红系文字。 */
.edit-card__error-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 0;
  background: transparent;
  border: none;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
  min-width: 0;
}

.edit-card__error-preview {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.edit-card__error-icon {
  flex-shrink: 0;
}

/* 展开后的错误全文:pre-wrap + 纵向滚动(超长错误不撑破卡片)。 */
.edit-card__error-text {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 200px;
  overflow-y: auto;
  font-family: inherit;
  min-width: 0;
}

.edit-card__diff {
  margin-top: 6px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg-app);
  overflow: hidden;
  max-height: 480px;
  overflow-y: auto;
}

.edit-card__diff--error {
  opacity: 0.85;
}

.edit-diff-line {
  display: grid;
  grid-template-columns: 16px 1fr;
  align-items: baseline;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  line-height: 1.5;
  white-space: pre;
  overflow-x: auto;
}

.edit-diff-line--add {
  background: rgba(16, 185, 129, 0.12);
}

.edit-diff-line--del {
  background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
}

.edit-diff-line--ctx {
  color: var(--color-text-secondary);
}

.edit-diff-line__prefix {
  text-align: center;
  color: var(--color-text-muted);
  user-select: none;
  border-right: 1px solid var(--color-bg-border);
}

.edit-diff-line--add .edit-diff-line__prefix {
  color: var(--color-tool-write);
}

.edit-diff-line--del .edit-diff-line__prefix {
  color: var(--color-tool-error-text);
}

.edit-diff-line__text {
  padding: 0 8px;
}

.edit-card__truncated {
  padding: 6px 8px;
  text-align: center;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  border-top: 1px dashed var(--color-bg-border);
  font-family: var(--font-sans);
}

@media (max-width: 767px) {
  .edit-card {
    padding: 6px 10px;
  }
}
</style>
