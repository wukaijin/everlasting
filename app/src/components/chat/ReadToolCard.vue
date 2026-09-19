<script setup lang="ts">
// ReadToolCard — `glob` / `list_dir` / `read_file` 的紧凑卡
// (2026-09-19,task `09-19-tool-card-compact-read`)。
//
// 背景:这三个只读检视工具此前都吃通用 `ToolCallCard`,形态是
// 「header 行 + `▸ input` 行 + `▸ output · N chars` 行」—— 实测每张
// 85px(e2e 种子六形态)。一个回合连着七八次查找就吃掉半屏,而其中
// 占高度的两行 summary 里几乎没有用户当下要的信息。
//
// 本卡片**替换**通用渲染(同 SearchHistoryCard / EditFileCard /
// ShellCard 先例,MessageItem 的 v-else-if 链),形态:
//
//   ▸ ⌕ glob · src/**/*.vue                6 matches   ✓ 0.2s
//
// 一行 ≈24px,信息密度反而更高:
//   - chip(:readToolChip)把「读的是哪儿」提到 headline —— glob 现在
//     显示 pattern(通用卡的 chip 只认 input.path,此前 glob 完全看不出
//     搜了什么);
//   - meta(:readToolMeta)把规模提到同一行(命中数 / 条目数 / 行范围);
//   - 输出与输入按需展开(点击整行),收起态**不进 DOM** —— 长输出既不
//     占高也不参与渲染。
//
// 出口径不缩水(PRD R3):
//   - 审批:命中 `pendingAsk` 时审批区**无条件渲染**,与收起/展开无关
//     (三个工具都是权限层 Tier 4 的 path 工具,项目外读取靠它拦);
//   - 读图:ToolResultImages 常驻(缩略图就是结果本身);
//   - 错误:红左条 + ✗ error,展开看原文。
//
// header 复用共享 `ToolCallHeader`(RULE-FrontSubagent-001 禁止复制),
// 只经 `#title-meta` 槽塞 meta;chevron 放在 header 外的行首 ——
// 它是"行可展开"的语义,不属于 header。
//
// 审批接线逐行照抄 ToolCallCard(224-257)与 ShellCard,0 新 store 逻辑。

import { computed, ref, watch } from "vue";
import { useChatStore } from "../../stores/chat";
import {
  usePermissionsStore,
  type PermissionDecision,
} from "../../stores/permissions";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";
import { toolAccentVar, toolIcon } from "../../utils/messageFormat";
import { readToolChip, readToolMeta } from "../../utils/toolSummary";
import { abbreviateDuration } from "../../utils/duration";
import Icon from "../Icon.vue";
import ToolCallHeader from "./ToolCallHeader.vue";
import ToolInputBody from "./ToolInputBody.vue";
import ToolOutputBody from "./ToolOutputBody.vue";
import ToolResultImages from "./ToolResultImages.vue";
import PermissionAskBody from "./PermissionAskBody.vue";

const props = defineProps<{
  call: ToolCallInfo;
  result?: ToolResultInfo;
  /** 08-21-b1-image-followups R6 同款:读图缩略图的附件 URL 需要会话 id。
   *  缺省(跨会话只读预览)则只渲染文本结果。 */
  sessionId?: string;
}>();

/** 展开态。组件局部状态 —— 收起是默认,展开是用户的临时意图,不进
 *  store(Pinia 里没有任何"哪张卡开着"的位置,也不该有)。 */
const open = ref(false);

const isError = computed(() => !!props.result?.isError);
const hasResult = computed(() => !!props.result);

const accent = computed(() => {
  if (isError.value) return "var(--color-tool-error)";
  return toolAccentVar(props.call.name);
});

/** headline 左侧:这次读的是哪儿(pattern / path / cwd 占位)。 */
const chip = computed<string | null>(() =>
  readToolChip(props.call.name, props.call.input),
);

/** headline 右侧:这次拿了多少(命中数 / 条目数 / 行范围)。流式中与
 *  报错态为 null(没有"规模"可言),槽位自然留空。 */
const meta = computed<string | null>(() =>
  readToolMeta(props.call.name, props.result ?? null),
);

const statusText = computed<string>(() => {
  if (isError.value) return "error";
  if (hasResult.value) return "done";
  return "running…";
});

/** 单 tool 耗时(与 ToolCallCard 同款):running → "…";有结果但没有
 *  duration_ms(工具集早期落库的行)→ 空串,槽位留白。 */
const durationLabel = computed<string>(() => {
  if (!hasResult.value) return "…";
  const d = props.result?.durationMs;
  if (typeof d !== "number") return "";
  return abbreviateDuration(d);
});

const statusIconName = computed<string>(() => {
  if (isError.value) return "x";
  if (hasResult.value) return "check";
  return "ellipsis";
});

function toggle(): void {
  open.value = !open.value;
}

// ------------------------------------------------------------------
// inline approval(接线照抄 ToolCallCard:224-257)
// ------------------------------------------------------------------

const chatStore = useChatStore();
const permStore = usePermissionsStore();

const pendingAsk = computed(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return undefined;
  const ask = permStore.getPending(sid);
  return ask && ask.toolUseId === props.call.id ? ask : undefined;
});

/** 只在还没有结果时展示审批区:ask → 用户决策 → 工具执行 → 结果到达,
 *  结果一到审批窗口就关了。 */
const isPendingApproval = computed(
  () => !hasResult.value && !!pendingAsk.value,
);

async function respondApproval(
  decision: PermissionDecision,
  reason?: string,
): Promise<void> {
  if (!pendingAsk.value) return;
  await permStore.respond(pendingAsk.value.rid, decision, reason);
}

/** 结果到达即清 pending —— 否则 store 的 120s 定时器会在审批早已解决后
 *  弹「已超时」toast(与 ToolCallCard 同款护栏)。 */
watch(hasResult, (now, was) => {
  if (now && !was) {
    const sid = chatStore.currentSessionId;
    if (sid && permStore.hasPending(sid)) {
      permStore.clearPending(sid);
    }
  }
});
</script>

<template>
  <div
    class="rocard"
    :class="{
      'rocard--error': isError,
      'rocard--running': !hasResult && !isError,
      'rocard--open': open,
    }"
    :style="{ borderLeftColor: accent }"
  >
    <!-- 整行是展开开关:role=button + tabindex + Enter/Space(照
         dispatch_subagent 整卡可点的既有先例)。刻意不用原生
         <button> —— 里面装着 ToolCallHeader 的 div,button 不允许。 -->
    <div
      class="rocard__row"
      role="button"
      tabindex="0"
      :aria-expanded="open"
      @click="toggle"
      @keydown.enter.prevent="toggle"
      @keydown.space.prevent="toggle"
    >
      <span class="rocard__chevron" aria-hidden="true">
        <Icon :name="open ? 'chevron-down' : 'chevron-right'" :size="12" />
      </span>
      <ToolCallHeader
        class="rocard__header"
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
        <template #title-meta>
          <span v-if="meta" class="rocard__meta">{{ meta }}</span>
        </template>
      </ToolCallHeader>
    </div>

    <!-- 展开区:输出(裸 pre,自己不带折叠壳)+ 输入 details。收起态
         v-if 之外 —— 长输出既不占高也不进 DOM。 -->
    <div v-if="open" class="rocard__body">
      <ToolOutputBody
        v-if="result"
        :content="result.content"
        :is-error="result.isError"
        :collapsible="false"
      />
      <ToolInputBody
        v-if="call.input && Object.keys(call.input).length > 0"
        :name="call.name"
        :input="call.input"
      />
    </div>

    <!-- 审批区不受展开态约束:项目外读取的放行不能被一次误收起的点击
         藏掉。 -->
    <div v-if="isPendingApproval && pendingAsk" class="rocard__approval">
      <PermissionAskBody
        mode="interactive"
        :ask="pendingAsk"
        :on-respond="respondApproval"
        :repo-root="chatStore.currentCwd"
      />
    </div>

    <ToolResultImages
      v-if="result?.images?.length && sessionId"
      :images="result.images"
      :session-id="sessionId"
    />
  </div>
</template>

<style scoped>
/* 容器 chrome 照 ToolCallCard(背景 / 边框 / 3px 左条 / mono 字体 +
   --error / --running 变体),纵向 padding 从 8px 收到 3px —— 1 行内容的
   卡片总高 ≈24px(design §3 的高度预算)。全 design token,0 hex。 */
.rocard {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-text-muted);
  border-radius: var(--radius-md);
  padding: 2px 8px;
  font-size: var(--text-sm);
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  max-width: 100%;
}

.rocard--error {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}

.rocard--running {
  border-left-color: var(--color-tool-shell);
}

/* 行 hover/负 margin:高亮铺满卡片内宽(抵消容器的横向 padding),
   让"整行可点"在视觉上说清楚。line-height 收到 --leading-tight:
   继承来的 1.55(18.6px @12px)是卡片高度的主要来源,单行内容不需要
   那么松 —— 收到 1.3 后整卡 ≈26px。 */
.rocard__row {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
  margin: 0 -6px;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  line-height: var(--leading-tight);
}

.rocard__row:hover {
  background: var(--color-bg-elevated);
}

/* 展开态:行保持 hover 底色 —— 展开区与它的行在视觉上连成一块,
   滚过一屏长输出后还能认出这段输出属于哪张卡。 */
.rocard--open > .rocard__row {
  background: var(--color-bg-elevated);
}

.rocard__row:focus-visible {
  outline: 2px solid var(--color-accent);
  outline-offset: -1px;
}

.rocard__chevron {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  color: var(--color-text-muted);
}

/* header 吃掉行内剩余宽度;min-width:0 让 chip 的 ellipsis 生效。 */
.rocard__header {
  flex: 1;
  min-width: 0;
}

/* header 的 title 默认 align-items: baseline —— baseline 对齐会把 14px
   图标的底边压到文字基线上,连带撑出 ~4px 下行空间(实测 title 19.6px,
   而最高子元素只有 15.6px)。紧凑卡按中心对齐,单行内视觉等价(图标
   14px 与文字 15.6px 中心差 <1px),整卡回到 ~26px。用 :deep 覆盖而非
   改共享组件:baseline 是通用卡 + 抽屉 + 权限卡共用的既有节奏,本卡
   不该把它一起改掉(ShellCard 的 :deep(.tool-call-header__status) 先例)。 */
.rocard__header :deep(.tool-call-header__title) {
  align-items: center;
}

/* meta:短标记,不压缩、不换行(chip 先被 ellipsis)。 */
.rocard__meta {
  flex-shrink: 0;
  white-space: nowrap;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

.rocard__body {
  margin-top: 4px;
  padding-top: 4px;
  border-top: 1px solid var(--color-bg-border);
}

.rocard__approval {
  margin-top: 6px;
}

/* 移动端:纵向再收 1px(紧凑 chip 不适用 44px 触摸目标规则,
   responsive-mobile §6 DEC-6 —— 44px 留给主操作)。 */
@media (max-width: 767px) {
  .rocard {
    padding: 2px 8px;
  }
}
</style>
