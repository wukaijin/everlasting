<script setup lang="ts">
// DiscussionSummaryCard — 群聊收尾专用卡片(08-07-group-chat-role-history-isolation
// follow-up;C2 证据链结构化渲染 09-09-gc-c2-evidence-summary)。
//
// `end_discussion` 的 `summary` 参数是整场讨论的最终结论 —— 对用户是全场最有
// 价值的产出,却藏在工具调用的 tool_result 信封里(只以 ToolCallCard 的 output
// 呈现,200px 截断 + 可滚动)。本卡片把该总结提取出来,渲染为可见的"讨论总结"
// 块:徽章 + 全文(markdown 渲染,不折叠、不截断)。
//
// C2(09-09):`end_discussion` 增结构化参数(conclusions + open_questions)后,
// 卡片双通道渲染 ——
// ① live 期:从 `call.input`(tool_use 参数,与消息同行)解析结构化区,
//   即时渲染 stance 徽章(✅实证/💭推测/⚖争议)+ 锚点行 + 开放问题;
// ② 收官后:`validatedDetail`(store 从 load_session 合并的
//   `sessions.discussion_detail` JSON,锚点带编排器后校验结果)按
//   path+line 匹配叠加核验记号(✓ 核过 / ⚠ 断证 / 无记号 = 未校验)。
// 无结构化参数(旧剧本 / 朴素收官 / 旧场 rehydrate)→ 纯文本渲染,零回归;
// 结构化在场时 summary 叙事全文仍渲染(两段共存,叙事在结构化区之后)。
//
// live 流式:tool_result 到达前(仅 tool_use 已 emit)显示占位态;result 到达后
// (或 rehydrate 路径)显示总结全文。与 ToolCallCard 共用同一 `result` 数据源
// (`getToolResult`),替换 end_discussion 的卡片位置,不新增 DOM 层级。

import { computed } from "vue";
import { extractToolResultDisplay } from "../../utils/messageFormat";
import { renderMarkdown } from "../../utils/markdown";
import { useCodeBlockCopy } from "../../composables/useCodeBlockCopy";
import type {
  DiscussionAnchor,
  DiscussionDetail,
  ToolCallInfo,
  ToolResultInfo,
} from "../../stores/chat.types";

const props = defineProps<{
  call: ToolCallInfo;
  result?: ToolResultInfo;
  /** 收官后 store 合并的行级 detail(`sessions.discussion_detail`
   *  JSON 文本,锚点带 check)。live 期 / 旧场 / 跨会话只读预览为
   *  null —— 锚点显示为未校验,不误叠他场结果。 */
  validatedDetail?: string | null;
}>();

// CH4-5: delegated copy handler for fenced-code chrome in the v-html body.
const { onMarkdownClick } = useCodeBlockCopy();

/** 解 `{result, cwd}` 信封后的总结全文;result 未到达时为空。 */
const summary = computed<string>(() => {
  if (!props.result) return "";
  return extractToolResultDisplay(props.result.content).trim();
});

/** 总结正文的 markdown HTML(与消息气泡同一 renderMarkdown + DOMPurify 管线)。 */
const html = computed<string>(() => renderMarkdown(summary.value));

/** live 流式中 tool_result 尚未到达的占位态。 */
const pending = computed<boolean>(() => !props.result);

const CHECK_VALUES = [
  "ok",
  "not_found",
  "line_out_of_range",
  "outside_root",
  "unvalidated",
] as const;

/** 未知 shape 一律降级 null(渲染走纯文本兜底),解析不抛错。
 *  check 字段只在行级 detail(JSON.parse 后)出现——live 期 input
 *  里没有,解析时保留(live 锚点自然无 check)。 */
function parseDetail(v: unknown): DiscussionDetail | null {
  if (!v || typeof v !== "object") return null;
  const obj = v as { conclusions?: unknown; open_questions?: unknown };
  const conclusions = Array.isArray(obj.conclusions)
    ? obj.conclusions.flatMap((c): DiscussionDetail["conclusions"] => {
        if (!c || typeof c !== "object") return [];
        const o = c as { claim?: unknown; anchors?: unknown; stance?: unknown };
        if (typeof o.claim !== "string" || !o.claim.trim()) return [];
        const anchors = Array.isArray(o.anchors)
          ? o.anchors.flatMap((a): DiscussionAnchor[] => {
              if (!a || typeof a !== "object") return [];
              const ao = a as { path?: unknown; line?: unknown; check?: unknown };
              if (typeof ao.path !== "string" || !ao.path) return [];
              const check =
                typeof ao.check === "string" &&
                (CHECK_VALUES as readonly string[]).includes(ao.check)
                  ? (ao.check as DiscussionAnchor["check"])
                  : undefined;
              return [
                {
                  path: ao.path,
                  line: typeof ao.line === "number" ? ao.line : null,
                  ...(check ? { check } : {}),
                },
              ];
            })
          : [];
        const stance =
          o.stance === "verified" || o.stance === "disputed" ? o.stance : "inferred";
        return [{ claim: o.claim, anchors, stance }];
      })
    : [];
  const openQuestions = Array.isArray(obj.open_questions)
    ? obj.open_questions.filter((q): q is string => typeof q === "string" && !!q.trim())
    : [];
  if (conclusions.length === 0 && openQuestions.length === 0) return null;
  return { conclusions, open_questions: openQuestions };
}

/** 结构化区(live 数据源 = tool_use 参数;与消息同行,跨会话预览也正确)。 */
const structured = computed<DiscussionDetail | null>(() => parseDetail(props.call.input));

/** 收官后的行级 detail(锚点 check 的唯一来源);坏 JSON → null 不炸。 */
const validated = computed<DiscussionDetail | null>(() => {
  if (!props.validatedDetail) return null;
  try {
    return parseDetail(JSON.parse(props.validatedDetail));
  } catch {
    return null;
  }
});

/** path+line → check 映射(从行级 detail 建;input 里的锚点按此键查记号)。 */
const checkByKey = computed<Map<string, NonNullable<DiscussionAnchor["check"]>>>(() => {
  const m = new Map<string, NonNullable<DiscussionAnchor["check"]>>();
  for (const c of validated.value?.conclusions ?? []) {
    for (const a of c.anchors) {
      if (a.check) m.set(`${a.path}:${a.line ?? ""}`, a.check);
    }
  }
  return m;
});

function anchorKey(a: { path: string; line?: number | null }): string {
  return `${a.path}:${a.line ?? ""}`;
}

const STANCE_LABEL: Record<DiscussionDetail["conclusions"][number]["stance"], string> = {
  verified: "实证",
  inferred: "推测",
  disputed: "争议",
};

const CHECK_LABEL: Record<NonNullable<DiscussionAnchor["check"]>, string> = {
  ok: "✓",
  not_found: "⚠ 文件不存在",
  line_out_of_range: "⚠ 行号越界",
  outside_root: "⚠ 项目根之外",
  unvalidated: "·",
};
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
      <!-- C2 结构化区:stance 徽章 + 锚点行(带核验记号)+ 开放问题。 -->
      <div
        v-if="structured"
        class="discussion-summary__structured"
        data-testid="discussion-summary-conclusions"
      >
        <div
          v-for="(c, i) in structured.conclusions"
          :key="i"
          class="discussion-summary__conclusion"
        >
          <span class="discussion-summary__stance" :data-stance="c.stance">
            {{ STANCE_LABEL[c.stance] }}
          </span>
          <span class="discussion-summary__claim">{{ c.claim }}</span>
          <div
            v-for="(a, j) in c.anchors"
            :key="j"
            class="discussion-summary__anchor"
            :data-check="checkByKey.get(anchorKey(a)) ?? 'unchecked'"
            :title="
              checkByKey.has(anchorKey(a))
                ? CHECK_LABEL[checkByKey.get(anchorKey(a))!]
                : '未校验'
            "
          >
            <code>{{ a.path }}{{ a.line != null ? `:${a.line}` : "" }}</code>
            <span
              v-if="checkByKey.has(anchorKey(a))"
              class="discussion-summary__check"
              :class="{
                'discussion-summary__check--ok':
                  checkByKey.get(anchorKey(a)) === 'ok',
                'discussion-summary__check--bad':
                  checkByKey.get(anchorKey(a)) !== 'ok',
              }"
            >
              {{ CHECK_LABEL[checkByKey.get(anchorKey(a))!] }}
            </span>
          </div>
        </div>
        <div
          v-if="structured.open_questions.length > 0"
          class="discussion-summary__openqs"
        >
          <div class="discussion-summary__openqs-title">开放问题</div>
          <div
            v-for="(q, i) in structured.open_questions"
            :key="i"
            class="discussion-summary__openq"
          >
            {{ q }}
          </div>
        </div>
      </div>
      <!-- summary 叙事全文:结构化在场时仍渲染(叙述串联是 moderator 的
           声音,结构化是机器可读结论,两者互补)。 -->
      <span
        v-if="summary"
        class="msg__markdown"
        @click="onMarkdownClick"
        v-html="html"
      />
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
  color: var(--color-accent-text);
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
  line-height: var(--leading-relaxed);
  white-space: normal;
}

/* ---- C2 结构化区 ---- */

.discussion-summary__structured {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  margin-bottom: var(--space-3);
}

.discussion-summary__conclusion {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 6px;
}

.discussion-summary__stance {
  flex: none;
  font-size: 11px;
  font-weight: 600;
  border-radius: var(--radius-sm);
  padding: 1px 7px;
}

.discussion-summary__stance[data-stance="verified"] {
  color: var(--color-success-text, var(--color-accent-text));
  background: var(--color-success-muted, var(--color-accent-muted));
}

.discussion-summary__stance[data-stance="inferred"] {
  color: var(--color-text-secondary);
  background: color-mix(in srgb, var(--color-text-primary) 8%, transparent);
}

.discussion-summary__stance[data-stance="disputed"] {
  color: var(--color-warning-text, var(--color-text-primary));
  background: var(--color-warning-muted, transparent);
}

.discussion-summary__claim {
  min-width: 0;
}

.discussion-summary__anchor {
  flex-basis: 100%;
  display: flex;
  align-items: center;
  gap: 6px;
  padding-left: var(--space-4);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}

.discussion-summary__anchor code {
  font-family: var(--font-mono);
  font-size: 0.95em;
}

.discussion-summary__check {
  white-space: nowrap;
}

.discussion-summary__check--ok {
  color: var(--color-success-text, var(--color-accent-text));
}

.discussion-summary__check--bad {
  color: var(--color-danger-text, var(--color-warning-text, var(--color-text-primary)));
  font-weight: 600;
}

.discussion-summary__openqs {
  border-top: 1px dashed var(--color-bg-border);
  padding-top: var(--space-2);
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.discussion-summary__openqs-title {
  font-size: var(--text-xs);
  font-weight: 600;
  color: var(--color-text-muted);
  margin-bottom: 2px;
}

.discussion-summary__openq {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

/* 08-14 ux-polish-r1 WP2 2.1(评审 B2):总结正文的 markdown 节奏。
   组件里 `<span class="msg__markdown">` 只是复用了 MessageItem 的类名,
   MessageItem 的 :deep 规则带 data-v 属性选择器,在本组件作用域外不生效
   (Tailwind preflight 已把 p/ul/li 的 margin 清零)—— 之前总结全文的段
   落/列表之间是零间距,是"密集感"的真实来源之一。这里镜像 MessageItem
   的 markdown 垂直节奏(p 12 / li 4 / 列表 4+12 / h* 16+4,尾元素清零),
   两处 markdown 观感保持一致;后续如再加 markdown 渲染面,考虑抽全局
   `.markdown-body` 类。 */
.discussion-summary__body :deep(p) {
  margin: 0 0 var(--space-3) 0;
}

.discussion-summary__body :deep(p:last-child) {
  margin-bottom: 0;
}

.discussion-summary__body :deep(h1),
.discussion-summary__body :deep(h2),
.discussion-summary__body :deep(h3),
.discussion-summary__body :deep(h4),
.discussion-summary__body :deep(h5),
.discussion-summary__body :deep(h6) {
  margin: var(--space-4) 0 var(--space-1) 0;
  font-weight: var(--weight-semibold);
  line-height: var(--leading-tight);
}

.discussion-summary__body :deep(h1:first-child),
.discussion-summary__body :deep(h2:first-child),
.discussion-summary__body :deep(h3:first-child) {
  margin-top: 0;
}

.discussion-summary__body :deep(ul),
.discussion-summary__body :deep(ol) {
  margin: var(--space-1) 0 var(--space-3) 0;
  padding-left: var(--space-6);
}

/* CH4-2:preflight 吃掉 list marker,镜像块五处同步补回。 */
.discussion-summary__body :deep(ul) {
  list-style: disc;
}

.discussion-summary__body :deep(ol) {
  list-style: decimal;
}

.discussion-summary__body :deep(ul:last-child),
.discussion-summary__body :deep(ol:last-child) {
  margin-bottom: 0;
}

.discussion-summary__body :deep(li) {
  margin: var(--space-1) 0;
}

.discussion-summary__body :deep(li:last-child) {
  margin-bottom: 0;
}

.discussion-summary__body :deep(strong) {
  font-weight: var(--weight-semibold);
}

/* 镜像 MessageItem .msg__markdown 的 chip/pre 节奏(2026-08-29
   ui-visual-polish r1:去描边只留底色,padding +1px 补高度)。 */
.discussion-summary__body :deep(code) {
  font-family: var(--font-mono);
  font-size: 0.9em;
  padding: 2px 5px;
  border-radius: 3px;
  background: color-mix(in srgb, var(--color-text-primary) 8%, transparent);
}

.discussion-summary__body :deep(pre) {
  margin: var(--space-2) 0;
  padding: 10px 12px;
  background: color-mix(in srgb, var(--color-text-primary) 6%, transparent);
  border-radius: var(--radius-md);
  overflow-x: auto;
  line-height: 1.45;
}

/* CH4-5:围栏代码块 chrome,五处镜像同步(grep md-code 找全)。 */
.discussion-summary__body :deep(.md-code) {
  margin: var(--space-2) 0;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  overflow: hidden;
}

.discussion-summary__body :deep(.md-code__head) {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 4px 10px;
  background: var(--color-bg-elevated);
  border-bottom: 1px solid var(--color-bg-border);
  font-size: 11px;
}

.discussion-summary__body :deep(.md-code__lang) {
  font-family: var(--font-mono);
  font-weight: 600;
  color: var(--color-text-secondary);
  text-transform: lowercase;
}

.discussion-summary__body :deep(.md-code__copy) {
  padding: 1px 8px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--color-text-secondary);
  font-size: 11px;
  cursor: pointer;
}

.discussion-summary__body :deep(.md-code__copy:hover) {
  color: var(--color-text-primary);
}

.discussion-summary__body :deep(.md-code pre) {
  margin: 0;
  border: none;
  border-radius: 0;
}

.discussion-summary__body :deep(pre code) {
  padding: 0;
  background: transparent;
  border: 0;
  font-size: 0.9em;
  white-space: pre;
}

.discussion-summary__body :deep(a) {
  color: var(--color-accent-text);
  text-decoration: underline;
  text-underline-offset: 2px;
}

.discussion-summary__body :deep(blockquote) {
  margin: var(--space-2) 0 var(--space-3) 0;
  padding: var(--space-1) var(--space-3);
  border-left: 3px solid var(--color-bg-border);
  color: var(--color-text-secondary);
  font-style: italic;
}
</style>
