<script setup lang="ts">
// ShellCard — `shell` / `run_background_shell` 专属卡片(2026-08-30,
// task `08-30-shell-description` PR3)。
//
// 背景: 通用 ToolCallCard 对 shell 只有折叠 header + 折叠 input
// <details>,命令文本默认不可见;审批时 PermissionAskBody 又不展示
// 命令原文(盲签)。本卡片**替换** MessageItem 中的通用渲染
// (同 EditFileCard / SearchHistoryCard 先例),形态(per design §3.1
// ASCII 基线):
//   - Header: ToolCallHeader(chip = toolHeaderChip:description →
//     命令首行兜底)+ run_background_shell 的 background pill +
//     待审批态 "等待审批"
//   - 命令块(常驻,所有状态可见):`$` 前缀 + command 原文,pre-wrap,
//     超长 max-height 滚动;`working_directory` 有则块内次行
//   - 一体化审批(pendingAsk 且 !hasResult):风险条 +
//     <PermissionActions>(PR2 抽取的共享按钮列);不渲染独立
//     "需要权限"容器,命令全文只出现一次
//   - 输出:done → ToolOutputBody 折叠;error → 红框 pre 常显
//     (截断 500 字符,不再叠折叠条)
//   - 降级:command 缺失/非 string → ToolInputBody 兜底(EditFileCard
//     同款防御;description 非字符串由 toolHeaderChip 按缺失处理)
//
// store 接线(pendingAsk 匹配 + respond)照抄 EditFileCard:148-163,
// 0 新 store 逻辑。workerRunId 在主面板恒无 → "始终允许"文案由
// PermissionActions 按 ask.workerRunId 分叉,天然正确。

import { computed } from "vue";
import { useChatStore } from "../../stores/chat";
import {
  RISK_LABEL_CN,
  RISK_META,
  usePermissionsStore,
  type PermissionDecision,
} from "../../stores/permissions";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";
import {
  extractToolResultDisplay,
  toolAccentVar,
  toolHeaderChip,
  toolIcon,
  truncateOutput,
} from "../../utils/messageFormat";
import { abbreviateDuration } from "../../utils/duration";
import ToolCallHeader from "./ToolCallHeader.vue";
import ToolInputBody from "./ToolInputBody.vue";
import ToolOutputBody from "./ToolOutputBody.vue";
import PermissionActions from "./PermissionActions.vue";

const props = defineProps<{
  call: ToolCallInfo;
  result?: ToolResultInfo;
}>();

const isError = computed(() => !!props.result?.isError);
const hasResult = computed(() => !!props.result);

/** `run_background_shell` 的 background pill(对齐 EditFileCard 的
 *  replace_all pill 样式);同步 `shell` 不渲染。 */
const isBackground = computed(() => props.call.name === "run_background_shell");

/** Header chip:description → 命令首行 → null(chip 隐藏)。
 *  纯函数三级兜底,与 DrawerToolCallCard 共用(design D1)。 */
const chip = computed<string | null>(() =>
  toolHeaderChip(props.call.name, props.call.input),
);

/** 命令原文(命令块常驻的数据源)。缺失/非 string → null,整卡降级
 *  ToolInputBody 兜底(PRD R3)。 */
const command = computed<string | null>(() => {
  const c = props.call.input?.command;
  return typeof c === "string" && c.length > 0 ? c : null;
});

/** cwd 次行(string 非空才渲染;缺失不渲染 —— PRD R5 兼容)。 */
const workingDirectory = computed<string | null>(() => {
  const w = props.call.input?.working_directory;
  return typeof w === "string" && w.length > 0 ? w : null;
});

const accent = computed(() => {
  if (isError.value) return "var(--color-tool-error)";
  return toolAccentVar(props.call.name);
});

const statusText = computed(() => {
  if (isError.value) return "error";
  if (hasResult.value) return "done";
  if (pendingAsk.value) return "等待审批";
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

/** done 态的折叠 output 走 ToolOutputBody(content 原样传入,组件内
 *  自行解 envelope);error 态走下方红框 pre,不经此分支。null =
 *  无 result 或 error(模板以 !== null 门控)。 */
const doneContent = computed<string | null>(() => {
  if (!props.result || isError.value) return null;
  return props.result.content;
});

const errorDisplay = computed<string | null>(() => {
  if (!props.result || !isError.value) return null;
  return truncateOutput(extractToolResultDisplay(props.result.content), 500);
});

// ------------------------------------------------------------------
// inline approval(接线照抄 EditFileCard:148-163,0 新 store 逻辑)
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

/** 风险条(design D3):复用 RISK_LABEL_CN / RISK_META,不复制映射。 */
const riskMeta = computed(() =>
  pendingAsk.value ? RISK_META[pendingAsk.value.risk] : null,
);
</script>

<template>
  <div
    class="shell-card"
    :class="{
      'shell-card--error': isError,
      'shell-card--running': !hasResult && !isError,
      'shell-card--waiting': isPendingApproval,
    }"
    :style="{ borderLeftColor: accent }"
    :data-testid="`shell-card-${call.id}`"
  >
    <ToolCallHeader
      :icon-name="toolIcon(call.name)"
      :name="call.name"
      :chip="chip"
      :status-text="statusText"
      :status-icon-name="statusIconName"
      :duration-label="durationLabel"
      :is-error="isError"
      :is-running="!hasResult && !isError"
      :is-success="hasResult && !isError"
    >
      <template #status-extra>
        <span v-if="isBackground" class="shell-card__pill">background</span>
      </template>
    </ToolCallHeader>

    <!-- 命令块(常驻,design D2):$ 前缀只在首行;多行命令 pre-wrap
         原样;cwd 次行 ellipsis + title。待审批态它就是审批的命令
         呈现本体,全文不重复第二次。 -->
    <div v-if="command" class="shell-card__command">
      <pre class="shell-card__cmd"><span
        class="shell-card__prompt"
        aria-hidden="true"
      >$ </span>{{ command }}</pre>
      <div
        v-if="workingDirectory"
        class="shell-card__cwd"
        :title="workingDirectory"
      >↳ {{ workingDirectory }}</div>
    </div>

    <!-- 一体化审批(design D3):风险条 + 共享按钮列。不用
         PermissionAskBody(避免"需要权限"独立盒子 + 命令重复);
         不传 hideAllowAlways → 主对话恒显"始终允许"。 -->
    <div v-if="isPendingApproval && pendingAsk && riskMeta" class="shell-card__approval">
      <div class="shell-card__risk">
        <span
          class="shell-card__risk-dot"
          :style="{ background: riskMeta.iconColor }"
          aria-hidden="true"
        ></span>
        <span class="shell-card__risk-label">
          风险: {{ RISK_LABEL_CN[pendingAsk.risk] }}
        </span>
        <span v-if="pendingAsk.reason" class="shell-card__risk-reason">
          · {{ pendingAsk.reason }}
        </span>
      </div>
      <PermissionActions :ask="pendingAsk" :on-respond="respondApproval" />
    </div>

    <!-- error 态:红框 pre 常显(截断 500,样式对齐
         tool-output-body__pre--error),不再叠折叠条(design D4)。 -->
    <pre
      v-if="errorDisplay"
      class="shell-card__error-out"
    >{{ errorDisplay }}</pre>

    <!-- done 态:折叠 output(直接复用现组件)。 -->
    <ToolOutputBody
      v-if="doneContent !== null"
      :content="doneContent"
      :is-error="false"
    />

    <!-- 输入缺失 / command 畸形 → 兜底为通用 input 视图(EditFileCard
         同款防御)。 -->
    <ToolInputBody v-if="!command" :name="call.name" :input="call.input" />
  </div>
</template>

<style scoped>
/* 卡片容器 chrome 照 EditFileCard(背景/边框/3px left bar/mono 字体 +
 * --error/--running 容器变体)。全 design token,0 hex。 */
.shell-card {
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

.shell-card--error {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}

.shell-card--running {
  border-left-color: var(--color-tool-shell);
}

/* 待审批态:status 文本 "等待审批" 用 amber(design D3 "amber,沿用
   running pulse")。ToolCallHeader 的 status 色由容器变体经 :deep
   注入(同 DrawerPermissionAskCard 的 :deep 注入先例),pulse 动画
   仍由 isRunning prop 驱动。 */
.shell-card--waiting :deep(.tool-call-header__status) {
  color: var(--color-tool-shell);
}

/* background pill:对齐 EditFileCard 的 replace_all pill。 */
.shell-card__pill {
  margin-left: 6px;
  font-size: var(--text-2xs);
  letter-spacing: 0.04em;
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  color: var(--color-text-muted);
}

/* 命令块:$ muted、command primary;pre-wrap + break-word,超长
   max-height 200px + overflow-y auto(对齐 tool-input-body__pre
   既有尺度,design D2)。 */
.shell-card__command {
  margin-top: 6px;
  padding: 6px 8px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  min-width: 0;
}

.shell-card__cmd {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 200px;
  overflow-y: auto;
  font-size: var(--text-xs);
  line-height: 1.4;
  color: var(--color-text-primary);
  font-family: var(--font-mono);
}

.shell-card__prompt {
  color: var(--color-text-muted);
  user-select: none;
}

/* cwd 次行:muted + ellipsis + title tooltip(块内,design D2)。 */
.shell-card__cwd {
  margin-top: 2px;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

/* 一体化审批:风险条(dot + 风险 label + reason)+ 共享按钮列。 */
.shell-card__approval {
  margin-top: 6px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.shell-card__risk {
  display: flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
  flex-wrap: wrap;
  font-family: var(--font-sans);
  font-size: var(--text-xs);
}

.shell-card__risk-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
  align-self: center;
}

.shell-card__risk-label {
  color: var(--color-text-secondary);
  font-weight: var(--weight-semibold);
  flex-shrink: 0;
}

/* reason:红系文字(design D3 "红系 muted 文本"),可换行。 */
.shell-card__risk-reason {
  color: var(--color-tool-error-text);
  min-width: 0;
  word-break: break-word;
}

/* error 红框 pre 常显:样式对齐 tool-output-body__pre / --error
   (400 档 error-text 文字 + 500 档 error 边框,08-15 对比度规则)。 */
.shell-card__error-out {
  margin: 6px 0 0;
  padding: 6px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-tool-error);
  border-radius: var(--radius-sm);
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 200px;
  overflow-y: auto;
  font-size: var(--text-xs);
  line-height: 1.4;
  color: var(--color-tool-error-text);
  font-family: var(--font-mono);
}

@media (max-width: 767px) {
  .shell-card {
    padding: 6px 10px;
  }
}
</style>
