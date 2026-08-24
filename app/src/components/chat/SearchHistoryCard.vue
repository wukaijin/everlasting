<script setup lang="ts">
// SearchHistoryCard — `search_history` 专属卡片(D2②+,
// 08-17-search-history-card)。
//
// 该 tool 的 tool_result 是给 LLM 的紧凑文本(一行一 hit,无
// session_id),在通用 ToolCallCard 里被截断且展开也难读。本卡片
// **替换** ToolCallCard 渲染(照 end_discussion →
// DiscussionSummaryCard 先例):从 tool_use.input 自取
// {query, scope, limit} 重查 ① 的 `search_messages` IPC 拿结构化
// hits —— live 与 replay 同一条路径(input 随消息持久化),不经
// streamController、后端零改动。CTA 经 useSearchModal 预填打开
// ① 的 SearchModal(完整列表 + 预览 + 跳 session)。
//
// 状态机(design §2):
//   pending   — 有 call 无 result(流式窗口):加载态
//   error     — result.is_error:渲染后端错误文案
//   degrade   — 重查失败:降级渲染 result.content 原文(保底不白屏)
//   empty     — 重查 0 命中:query 回显
//   hits      — 前 3 条 + 「查看全部」CTA
//
// 不读 Pinia 的部分仅限渲染所需的两项(currentProjectId 作 scope
// 映射、currentSessionId 作「本会话」标记)—— 与 askCard 的
// resolver 读 store 同级,不入 store(state-management:搜索态是
// 组件局部状态)。

import { computed, onMounted, ref } from "vue";
import { transport } from "../../transport";
import { useSearchModal } from "../../composables/useSearchModal";
import { extractToolResultDisplay } from "../../utils/messageFormat";
import { hitTimeLabel, splitSnippetAt } from "../../utils/searchHits";
import { useProjectsStore } from "../../stores/projects";
import { useChatStore } from "../../stores/chat";
import type { MessageSearchHit } from "../../stores/chat.types";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";

const CARD_PREVIEW_ROWS = 3;

const props = defineProps<{
  call: ToolCallInfo;
  result?: ToolResultInfo | null;
}>();

const projectsStore = useProjectsStore();
const chatStore = useChatStore();
const { open: openSearchModal } = useSearchModal();

// --- input 解析(防御式:input 来自 LLM,shape 由后端 schema 约束) ---

const query = computed(() => String(props.call.input?.query ?? "").trim());
const projectId = computed<string | null>(() =>
  props.call.input?.scope === "current_project"
    ? (projectsStore.currentProjectId ?? null)
    : null,
);
const limit = computed(() => {
  const n = Number(props.call.input?.limit);
  return Number.isFinite(n) && n >= 1 ? Math.floor(n) : 20;
});

// --- 重查状态 ---

const hits = ref<MessageSearchHit[] | null>(null);
const requiringFailed = ref(false);

/** 后端 tool_result 文案(错误态 / 降级态共用)。 */
const resultText = computed(() =>
  props.result ? extractToolResultDisplay(props.result.content).trim() : "",
);

const state = computed<"pending" | "error" | "degrade" | "empty" | "hits" | "loading">(() => {
  if (!props.result) return "pending";
  if (props.result.isError) return "error";
  if (requiringFailed.value) return "degrade";
  if (hits.value === null) return "loading";
  return hits.value.length > 0 ? "hits" : "empty";
});

const previewRows = computed(() => (hits.value ?? []).slice(0, CARD_PREVIEW_ROWS));
const totalHits = computed(() => hits.value?.length ?? 0);

function openFullResults(): void {
  if (!query.value) return;
  openSearchModal({ query: query.value, projectId: projectId.value });
}

// --- 渲染 helper(复刻 ① 的 timeLabel / splitSnippet 视觉语言) ---

// hit-rendering helpers shared with ① SearchModal (utils/searchHits).
function timeLabel(iso: string): string {
  return hitTimeLabel(iso);
}

function splitSnippet(snippet: string): [string, string | null, string] {
  return splitSnippetAt(snippet, query.value);
}

const isCurrentSession = (h: MessageSearchHit): boolean =>
  h.session_id === chatStore.currentSessionId;

// --- 重查(live 与 replay 同路:onMounted 一次) ---

onMounted(async () => {
  if (!props.result || props.result.isError || !query.value) return;
  try {
    hits.value = await transport.invoke<MessageSearchHit[]>("search_messages", {
      query: query.value,
      projectId: projectId.value,
      limit: limit.value,
    });
  } catch {
    requiringFailed.value = true;
  }
});
</script>

<template>
  <div class="shcard" :data-testid="`search-history-card-${call.id}`">
    <div class="shcard__header">
      <span class="shcard__badge">历史检索</span>
      <span class="shcard__query" :title="query">“{{ query }}”</span>
    </div>

    <!-- pending:tool_result 未到(流式窗口) -->
    <div v-if="state === 'pending'" class="shcard__state">正在检索历史…</div>

    <!-- error:后端拒绝(空 query / DB 错) -->
    <div v-else-if="state === 'error'" class="shcard__state shcard__state--error">
      {{ resultText || "检索失败" }}
    </div>

    <!-- degrade:重查失败 → 保底渲染工具原始输出 -->
    <div v-else-if="state === 'degrade'" class="shcard__state">
      <p class="shcard__degrade-note">结果列表加载失败,以下为工具原始输出:</p>
      <pre class="shcard__raw">{{ resultText }}</pre>
    </div>

    <!-- loading:result 已到,重查在途 -->
    <div v-else-if="state === 'loading'" class="shcard__state">正在加载结果…</div>

    <!-- empty:重查 0 命中 -->
    <div v-else-if="state === 'empty'" class="shcard__state">
      没有找到与 “{{ query }}” 匹配的会话或消息
    </div>

    <!-- hits:前 3 条 + CTA -->
    <template v-else>
      <div class="shcard__rows">
        <div v-for="h in previewRows" :key="`${h.kind}-${h.session_id}-${h.seq ?? 't'}`" class="shcard__row">
          <div class="shcard__row-main">
            <span class="shcard__row-title">
              <span v-if="h.kind === 'title'" class="shcard__kind">[标题]</span>{{ h.session_title }}
            </span>
            <span class="shcard__row-meta">
              {{ h.project_name ?? h.project_id }} · {{ timeLabel(h.updated_at) }}<template v-if="h.kind === 'content'">
                · #{{ h.seq }} {{ h.role }}</template
              ><template v-if="isCurrentSession(h)"> · 本会话</template>
            </span>
          </div>
          <span v-if="h.snippet" class="shcard__snippet">
            <span>{{ splitSnippet(h.snippet)[0] }}</span><mark v-if="splitSnippet(h.snippet)[1]">{{ splitSnippet(h.snippet)[1] }}</mark><span>{{ splitSnippet(h.snippet)[2] }}</span>
          </span>
        </div>
      </div>
      <button type="button" class="shcard__cta btn btn--muted" @click="openFullResults">
        共 {{ totalHits }} 条命中 · 点击查看全部
      </button>
    </template>
  </div>
</template>

<style scoped>
.shcard {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-accent);
  border-radius: var(--radius-md);
  padding: 10px 14px;
  max-width: 100%;
}

.shcard__header {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-2);
}

.shcard__badge {
  flex-shrink: 0;
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  color: var(--color-accent);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.shcard__query {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.shcard__state {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  padding: var(--space-1) 0;
}

.shcard__state--error {
  color: var(--color-error, var(--color-text-secondary));
  white-space: pre-wrap;
  word-break: break-all;
}

.shcard__degrade-note {
  margin: 0 0 var(--space-1);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

.shcard__raw {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 160px;
  overflow-y: auto;
}

.shcard__rows {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.shcard__row {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  padding-left: var(--space-2);
  border-left: 2px solid var(--color-bg-border);
}

.shcard__row-main {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.shcard__row-title {
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.shcard__kind {
  color: var(--color-text-muted);
  font-weight: var(--weight-normal);
  margin-right: 4px;
}

.shcard__row-meta {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

.shcard__snippet {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-relaxed);
  display: -webkit-box;
  -webkit-line-clamp: 1;
  -webkit-box-orient: vertical;
  overflow: hidden;
  word-break: break-all;
}

.shcard__snippet mark {
  background: transparent;
  color: var(--color-accent);
  font-weight: var(--weight-medium);
}

/* 底部 CTA 由全局 .btn 家族承载(muted,hover 转 accent);此处仅
   保留几何(margin/width/min-height)。原裸 transition 0.15s ease
   删,落家族 fast。 */
.shcard__cta {
  margin-top: var(--space-2);
  width: 100%;
  min-height: 36px;
}

/* 移动端 hit-area(responsive-mobile §6:可点元素 ≥44px)。 */
@media (max-width: 768px) {
  .shcard__cta {
    min-height: 44px;
  }
}
</style>
