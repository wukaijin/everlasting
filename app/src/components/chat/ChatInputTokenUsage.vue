<script setup lang="ts">
// ChatInputTokenUsage — hint 行中位的 token 用量 chip + 大号用量明细弹层。
//
// 演进:2026-08-21 前是 hover Tooltip(四计数)+ AppHeader QuotaChip 弹层
// 两个入口;现合并为单入口——chip 点击打开向上弹层,内容分区:
//   1. 上下文占用 — 占比进度条 + % · 已用/容量 · 剩余;有切片数据时进度
//      条升级为**构成堆叠条**(消息/tools/system+技能/memory/@文件/图片
//      分段,段宽 ∝ 占比),下方常显图例明细(色点 + token + %,不靠悬停
//      —— 与 TracePanel TurnCard 的 hover 版形成对照,数据同源)
//   2. 上轮明细 — input / cache_read / cache_creation / output + 命中率
//      (SessionTokenUsage 是 last-turn 快照,2026-06-26 snapshot fix)
//   3. Nh 滚动窗口 — 总量(设额度加占比条)+ 平均缓存命中率 +
//      per-provider(主/worker 拆分 + 各自命中率 + 小时分布柱)+
//      top sessions(点击 openSessionInProject 跳转)
//   4. 设置 — 窗口小时 / 额度(set_quota_settings)
//
// Popover 是手写模式(`popover-pattern.md`,与 ChatInputLatencyPopover
// 同族):open + root ref + onDocumentClick 外点关闭 + Esc;几何 = 底部
// 向上开(`bottom: calc(100% + 4px)`),宽 420px 居中于 chip。
//
// 数据分面:session 快照走 props(父 HintRow 保持 0 store import);
// 构成切片走 useTraceStore(当前 session 主 loop 最近一轮带 usage 的
// TurnTrace,live turn_usage 事件已实时;弹层打开时若 store 未 scoped
// 到当前 session 则补一次 loadHistory);窗口聚合走 useQuotaStore
// (refresh = 本组件 mount / 弹层开 / streamEvents done 后三触发点)。
// 缓存命中率复用 `cacheRatePercent`(08-10 锁定语义:cache_read /
// context_input,分母 0 → null 渲染 "—")。窗口聚合的归一化分母 =
// Σinput + Σcc + Σcr(provider 同族不混:Anthropic 贡献三项和、OpenAI
// 只贡献 input,跨 provider 求和仍是正确口径)。
//
// 构成口径(budget.rs 不变量,08-19):五归因切片(tools / system+
// 技能 / memory / @文件 / 图片)之和 ≤ 实发总量,残差 = 总量 − Σ切片
// ≥ 0 即「消息历史」段(吸收本地 cl100k 估算与 provider 计量的系统
// 性偏差)。切片全缺列(旧行)时退回单色进度条,不渲染图例。

import { computed, onMounted, onUnmounted, ref } from "vue";

import { useChatStore } from "../../stores/chat";
import { useQuotaStore } from "../../stores/quota";
import { useTraceStore } from "../../stores/traceStore";
import {
  abbreviateTokens,
  cacheRatePercent,
  type TokenUsageLevel,
} from "../../utils/tokenUsage";
import type { SessionTokenUsage } from "../../stores/chat.types";
import type { TurnTrace } from "../../types/turnTrace";

const props = defineProps<{
  /** 上轮 token 快照(last-turn,非累计)。`null` = 老 session 未统计 →
   *  chip 渲染 "—",弹层上下文/明细段渲染空态。 */
  tokenUsage: SessionTokenUsage | null;
  /** 当前模型上下文容量(models catalog contextWindow,父级兜底 200K)。 */
  contextWindow: number;
  /** 父级从 context_input/contextWindow 算好的色档(49/50/74/75 语义)。 */
  usageLevel: TokenUsageLevel | null;
}>();

const quotaStore = useQuotaStore();
const chatStore = useChatStore();
const traceStore = useTraceStore();

// === Popover 状态(hand-rolled,popover-pattern.md)===

const open = ref(false);
const root = ref<HTMLElement | null>(null);

// 设置行(弹层内):编辑态从当前 report 派生,保存后后端 refresh 回填。
const hoursInput = ref<string>("");
const limitInput = ref<string>("");
const saving = ref(false);
const settingsError = ref<string | null>(null);

function syncSettingsInputs(): void {
  hoursInput.value = String(quotaStore.report?.windowHours ?? 5);
  limitInput.value =
    quotaStore.report?.limitTokens != null
      ? String(quotaStore.report.limitTokens)
      : "";
  settingsError.value = null;
}

function toggle(): void {
  open.value = !open.value;
  if (open.value) {
    syncSettingsInputs();
    void quotaStore.refresh();
    // 构成切片来源:store 未 scoped 到当前 session(冷启动恢复 / 直接
    // 打开弹层)时补一次回看;live 路径 turn_usage 事件已在维护 Map。
    const sid = chatStore.currentSessionId;
    if (sid && traceStore.currentSessionId !== sid) {
      void traceStore.loadHistory(sid);
    }
  }
}
function close(): void {
  open.value = false;
}

function onDocumentClick(e: MouseEvent): void {
  if (!open.value) return;
  const target = e.target as Node | null;
  if (root.value && target && !root.value.contains(target)) {
    close();
  }
}
function onKeydown(e: KeyboardEvent): void {
  if (open.value && e.key === "Escape") close();
}

if (typeof document !== "undefined") {
  document.addEventListener("click", onDocumentClick);
  document.addEventListener("keydown", onKeydown);
  onUnmounted(() => {
    document.removeEventListener("click", onDocumentClick);
    document.removeEventListener("keydown", onKeydown);
  });
}

onMounted(() => {
  void quotaStore.refresh();
});

// === 上下文占用(进度条主数据)===

const contextPct = computed<number | null>(() => {
  if (!props.tokenUsage || props.contextWindow <= 0) return null;
  return Math.min(
    100,
    Math.round((props.tokenUsage.context_input_tokens / props.contextWindow) * 100),
  );
});

const contextRemaining = computed<number | null>(() => {
  if (!props.tokenUsage) return null;
  return Math.max(0, props.contextWindow - props.tokenUsage.context_input_tokens);
});

/** 上轮缓存命中率(cache_read / context_input,unclamped)。 */
const lastTurnRate = computed<number | null>(() =>
  props.tokenUsage
    ? cacheRatePercent(
        props.tokenUsage.cache_read_input_tokens,
        props.tokenUsage.context_input_tokens,
      )
    : null,
);

// === 上下文构成(堆叠条 + 图例,数据 = traceStore 最近一轮)===

/** 当前 session 主 loop 最近一轮带 usage 的 trace(构成切片来源)。 */
const latestTrace = computed<TurnTrace | null>(() => {
  let best: TurnTrace | null = null;
  for (const t of traceStore.currentSessionTraces.values()) {
    if (!t.tokenUsage) continue;
    if (!best || t.seq > best.seq) best = t;
  }
  return best;
});

interface CompositionSlice {
  key: string;
  label: string;
  /** CSS color(设计 token / color-mix 派生,不硬编码 hex)。 */
  color: string;
  value: number;
}

/** 上下文构成:五归因切片 + 消息残差。null = 切片全缺(旧行)或无
 * trace → 进度条退回单色、不渲染图例。分类配色与 TracePanel TurnCard
 * 的构成条锚点一致(tools 紫 / system 琥珀 / memory 蓝 / @文件 绿 /
 * 图片 青),消息残差用中性 slate(占比通常最大,中性底 + 彩色归因段
 * 的层级更清晰)。 */
const contextComposition = computed<{
  slices: CompositionSlice[];
  ctx: number;
} | null>(() => {
  const t = latestTrace.value;
  const ctx = t?.tokenUsage?.context_input_tokens ?? 0;
  if (!t || ctx <= 0) return null;
  const attributed: CompositionSlice[] = [
    {
      key: "tools",
      label: "tools[]",
      color: "var(--color-tool-thinking)",
      value: t.toolsToken ?? 0,
    },
    {
      key: "system",
      label: "system+技能",
      color: "var(--color-tool-shell)",
      value: t.systemToken ?? 0,
    },
    {
      key: "memory",
      label: "memory",
      color: "var(--color-accent)",
      value: t.memoryToken ?? 0,
    },
    {
      key: "at-files",
      label: "@文件",
      color: "var(--color-tool-write)",
      value: t.atFilesToken ?? 0,
    },
    {
      key: "images",
      label: "图片",
      color: "var(--color-tool-read)",
      value: t.imagesToken ?? 0,
    },
  ];
  if (attributed.every((s) => s.value <= 0)) return null;
  const used = attributed.reduce((acc, s) => acc + s.value, 0);
  const messages = Math.max(0, ctx - used);
  const slices = [
    {
      key: "messages",
      label: "消息",
      color: "color-mix(in srgb, var(--color-text-primary) 45%, transparent)",
      value: messages,
    },
    ...attributed,
  ].filter((s) => s.value > 0);
  return { slices, ctx };
});

/** 堆叠条总宽(trace 实发口径;与头部快照数字瞬时差异 <1 轮,可忽略)。 */
const stackWidthPct = computed(() => {
  const ctx = contextComposition.value?.ctx ?? 0;
  if (ctx <= 0 || props.contextWindow <= 0) return null;
  return Math.min(100, (ctx / props.contextWindow) * 100);
});

/** 图例百分比(占实发总量的份额,与段宽同分母)。 */
function slicePct(slice: CompositionSlice, ctx: number): number {
  return Math.round((slice.value / ctx) * 100);
}

// === 窗口聚合派生 ===

const providers = computed(() => quotaStore.report?.providers ?? []);

/** 窗口平均缓存命中率:跨 provider 求和后一次算(见文件头口径注释)。 */
const windowRate = computed<number | null>(() => {
  if (providers.value.length === 0) return null;
  let input = 0;
  let cacheRead = 0;
  let cacheCreation = 0;
  for (const p of providers.value) {
    input += p.totals.inputTokens;
    cacheRead += p.totals.cacheReadInputTokens;
    cacheCreation += p.totals.cacheCreationInputTokens;
  }
  return cacheRatePercent(cacheRead, input + cacheRead + cacheCreation);
});

/** 设了额度时的窗口占比(0..1);未设 = null(不画占比条,AC5)。 */
const windowPct = computed(() =>
  quotaStore.chipPct != null ? Math.round(quotaStore.chipPct * 100) : null,
);

/** 占比条着色档(≥75 红 / ≥50 黄,同 chip 色档语义)。 */
function barLevelClass(pct: number | null): string {
  if (pct == null) return "";
  if (pct >= 75) return "chat-input__token-bar-fill--alert";
  if (pct >= 50) return "chat-input__token-bar-fill--warn";
  return "chat-input__token-bar-fill--ok";
}

/** 小时分布柱:归一化高度(最大桶 = 100%)。 */
function hourlyBarHeight(value: number, max: number): number {
  if (max <= 0) return 0;
  return Math.max(4, Math.round((value / max) * 100));
}
const maxHourlyInput = computed(() => {
  let max = 0;
  for (const p of providers.value) {
    for (const h of p.hourly) max = Math.max(max, h.inputTokens);
  }
  return max;
});

function providerRate(
  input: number,
  cacheRead: number,
  cacheCreation: number,
): number | null {
  return cacheRatePercent(cacheRead, input + cacheRead + cacheCreation);
}

async function saveSettings(): Promise<void> {
  saving.value = true;
  settingsError.value = null;
  try {
    // `type="number"` 的 v-model 会把输入转成 number(或空串),先统一
    // 成字符串再 trim。
    const hoursRaw = String(hoursInput.value ?? "").trim();
    const limitRaw = String(limitInput.value ?? "").trim();
    const hours = hoursRaw === "" ? null : Number(hoursRaw);
    const limit = limitRaw === "" ? null : Number(limitRaw);
    if (hours != null && (!Number.isFinite(hours) || hours < 1 || hours > 168)) {
      settingsError.value = "窗口需 1-168 小时";
      return;
    }
    if (limit != null && (!Number.isFinite(limit) || limit <= 0)) {
      settingsError.value = "额度需正数(token)";
      return;
    }
    await quotaStore.setSettings(hours, limit);
    syncSettingsInputs();
  } catch (e) {
    settingsError.value = String(e);
  } finally {
    saving.value = false;
  }
}

async function jumpToSession(sessionId: string, projectId: string | null): Promise<void> {
  if (!projectId) return;
  close();
  await chatStore.openSessionInProject(projectId, sessionId);
}
</script>

<template>
  <div ref="root" class="chat-input__token">
    <button
      class="chat-input__token-usage"
      :class="{
        [`chat-input__token-usage--${usageLevel}`]: usageLevel,
        'chat-input__token-usage--open': open,
      }"
      type="button"
      :aria-haspopup="'dialog'"
      :aria-expanded="open"
      title="用量明细:上下文 / 上轮 token / 滚动窗口"
      @click="toggle"
    >
      <template v-if="tokenUsage">
        {{ abbreviateTokens(tokenUsage.context_input_tokens) }}
        ·
        {{ contextPct }}% / {{ abbreviateTokens(contextWindow) }}
      </template>
      <template v-else>—</template>
    </button>

    <Transition name="chat-input-token-popover">
      <div
        v-if="open"
        class="chat-input__token-popover"
        role="dialog"
        aria-label="用量明细"
      >
        <!-- 1. 上下文占用 -->
        <section class="chat-input__token-section">
          <div class="chat-input__token-section-title">上下文占用</div>
          <template v-if="tokenUsage">
            <div class="chat-input__token-ctx-head">
              <span
                class="chat-input__token-ctx-pct"
                :class="usageLevel ? `chat-input__token-ctx-pct--${usageLevel}` : ''"
              >{{ contextPct }}%</span>
              <span class="chat-input__token-ctx-nums">
                {{ abbreviateTokens(tokenUsage.context_input_tokens) }} /
                {{ abbreviateTokens(contextWindow) }}
              </span>
            </div>
            <div class="chat-input__token-bar" aria-hidden="true">
              <!-- 有切片数据 → 构成堆叠条(段宽 ∝ 占比,段间 2px 缝);
                   无切片(旧行/无 trace)→ 单色档位条退化。 -->
              <div
                v-if="contextComposition && stackWidthPct != null"
                class="chat-input__token-bar-stack"
                :style="{ width: `${stackWidthPct}%` }"
              >
                <span
                  v-for="s in contextComposition.slices"
                  :key="s.key"
                  class="chat-input__token-bar-seg"
                  :style="{ flexGrow: s.value, background: s.color }"
                />
              </div>
              <span
                v-else
                class="chat-input__token-bar-fill"
                :class="barLevelClass(contextPct)"
                :style="{ width: `${contextPct}%` }"
              />
            </div>
            <!-- 构成图例:常显明细(色点 + token + %),不依赖悬停。 -->
            <div
              v-if="contextComposition"
              class="chat-input__token-legend"
              role="list"
              aria-label="上下文构成"
            >
              <div
                v-for="s in contextComposition.slices"
                :key="s.key"
                class="chat-input__token-legend-row"
                role="listitem"
              >
                <span
                  class="chat-input__token-legend-dot"
                  :style="{ background: s.color }"
                />
                <span class="chat-input__token-legend-label">{{ s.label }}</span>
                <span class="chat-input__token-legend-value">{{
                  abbreviateTokens(s.value)
                }}</span>
                <span class="chat-input__token-legend-pct"
                  >{{ slicePct(s, contextComposition.ctx) }}%</span
                >
              </div>
              <div class="chat-input__token-legend-note">
                构成按最近一轮请求归因(cl100k 估算);system 含技能清单
              </div>
            </div>
            <div
              v-if="contextRemaining != null"
              class="chat-input__token-ctx-sub"
            >
              剩余 {{ abbreviateTokens(contextRemaining) }}({{ 100 - (contextPct ?? 0) }}%)
            </div>
          </template>
          <div v-else class="chat-input__token-muted">升级前未统计</div>
        </section>

        <!-- 2. 上轮明细 -->
        <section v-if="tokenUsage" class="chat-input__token-section">
          <div class="chat-input__token-section-title">上轮明细</div>
          <div class="chat-input__token-rows">
            <div class="chat-input__token-row">
              <span>input</span>
              <span>{{ abbreviateTokens(tokenUsage.input_tokens) }}</span>
            </div>
            <div class="chat-input__token-row">
              <span>cache_read</span>
              <span>{{ abbreviateTokens(tokenUsage.cache_read_input_tokens) }}</span>
            </div>
            <div class="chat-input__token-row">
              <span>cache_creation</span>
              <span>{{ abbreviateTokens(tokenUsage.cache_creation_input_tokens) }}</span>
            </div>
            <div class="chat-input__token-row">
              <span>output</span>
              <span>{{ abbreviateTokens(tokenUsage.output_tokens) }}</span>
            </div>
            <div class="chat-input__token-row">
              <span>缓存命中率</span>
              <span>{{ lastTurnRate != null ? `${lastTurnRate}%` : "—" }}</span>
            </div>
          </div>
        </section>

        <!-- 3. 滚动窗口聚合 -->
        <section class="chat-input__token-section">
          <div class="chat-input__token-section-title">
            {{ quotaStore.report?.windowHours ?? 5 }}h 滚动窗口
          </div>
          <div v-if="quotaStore.error" class="chat-input__token-error">
            拉取失败:{{ quotaStore.error }}
          </div>
          <div
            v-if="quotaStore.loading && !quotaStore.report"
            class="chat-input__token-muted"
          >
            加载中…
          </div>

          <template v-if="providers.length">
            <div class="chat-input__token-rows">
              <div class="chat-input__token-row">
                <span>窗口总量</span>
                <span class="chat-input__token-strong">
                  {{ abbreviateTokens(quotaStore.chipTotal) }}
                </span>
              </div>
              <div class="chat-input__token-row">
                <span>平均缓存命中率</span>
                <span>{{ windowRate != null ? `${windowRate}%` : "—" }}</span>
              </div>
            </div>
            <div v-if="windowPct != null" class="chat-input__token-bar" aria-hidden="true">
              <span
                class="chat-input__token-bar-fill"
                :class="barLevelClass(windowPct)"
                :style="{ width: `${windowPct}%` }"
              />
            </div>

            <div
              v-for="p in providers"
              :key="p.providerId"
              class="chat-input__token-provider"
            >
              <div class="chat-input__token-provider-head">
                <span class="chat-input__token-provider-name">{{
                  p.displayName ?? p.providerId
                }}</span>
                <span class="chat-input__token-provider-total">
                  {{ abbreviateTokens(p.totals.inputTokens) }} in ·
                  {{ abbreviateTokens(p.totals.outputTokens) }} out
                </span>
              </div>
              <div class="chat-input__token-provider-split">
                主 {{ abbreviateTokens(p.mainTotals.inputTokens) }} · worker
                {{ abbreviateTokens(p.workerTotals.inputTokens) }}
                <template v-if="providerRate(p.totals.inputTokens, p.totals.cacheReadInputTokens, p.totals.cacheCreationInputTokens) != null">
                  · 命中
                  {{ providerRate(p.totals.inputTokens, p.totals.cacheReadInputTokens, p.totals.cacheCreationInputTokens) }}%
                </template>
              </div>
              <div v-if="p.hourly.length" class="chat-input__token-bars" aria-hidden="true">
                <div
                  v-for="h in p.hourly"
                  :key="h.hour"
                  class="chat-input__token-bar-col"
                  :title="`${h.hour.slice(11, 16)} · in ${abbreviateTokens(h.inputTokens)}`"
                  :style="{ height: `${hourlyBarHeight(h.inputTokens, maxHourlyInput)}%` }"
                />
              </div>
            </div>
          </template>
          <div
            v-else-if="quotaStore.report && !quotaStore.loading"
            class="chat-input__token-muted"
          >
            窗口内暂无用量
          </div>

          <div v-if="quotaStore.report?.topSessions.length" class="chat-input__token-sessions">
            <div class="chat-input__token-section-subtitle">窗口内 session</div>
            <button
              v-for="s in quotaStore.report.topSessions"
              :key="s.sessionId"
              class="chat-input__token-session-row"
              type="button"
              :disabled="!s.projectId"
              :title="s.sessionId"
              @click="jumpToSession(s.sessionId, s.projectId)"
            >
              <span class="chat-input__token-session-title">{{
                s.title || s.sessionId.slice(0, 8)
              }}</span>
              <span class="chat-input__token-session-nums">
                {{ abbreviateTokens(s.windowMainInput + s.windowWorkerInput) }} /
                累计 {{ abbreviateTokens(s.lifetimeInput) }}
              </span>
            </button>
          </div>
        </section>

        <!-- 4. 设置 -->
        <section class="chat-input__token-settings">
          <label class="chat-input__token-field">
            窗口(h)
            <input v-model="hoursInput" class="chat-input__token-input" type="number" min="1" max="168" />
          </label>
          <label class="chat-input__token-field">
            额度(token)
            <input v-model="limitInput" class="chat-input__token-input" type="number" min="0" placeholder="未设置" />
          </label>
          <button
            class="chat-input__token-save"
            type="button"
            :disabled="saving"
            @click="saveSettings"
          >
            {{ saving ? "保存中…" : "保存" }}
          </button>
          <div v-if="settingsError" class="chat-input__token-error">{{ settingsError }}</div>
        </section>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.chat-input__token {
  position: relative;
  display: inline-flex;
  flex-shrink: 0;
}

/* chip:沿用原 hint 行 token chip 的观感(色档类同名,S6a 隐藏规则在
   HintRow 里 :deep 命中本类);从 hover-help span 改为可点击 button。 */
.chat-input__token-usage {
  display: inline-flex;
  align-items: center;
  padding: 0 6px;
  font: inherit;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  white-space: nowrap;
  cursor: pointer;
  border: none;
  background: transparent;
  border-radius: var(--radius-sm);
  color: var(--color-text-muted);
  transition: color var(--duration-base) var(--ease-out),
    background var(--duration-base) var(--ease-out);
  user-select: none;
}
.chat-input__token-usage:hover,
.chat-input__token-usage--open {
  color: var(--color-text-primary);
  background: var(--color-bg-hover);
}
.chat-input__token-usage--ok {
  color: var(--color-status-success);
}
.chat-input__token-usage--warn {
  color: var(--color-status-warn);
}
.chat-input__token-usage--alert {
  color: var(--color-tool-error-text);
}

/* 大号弹层:向上开(hint 行在面板底部),宽 420px 居中于 chip;
   max-height 限高内滚。 */
.chat-input__token-popover {
  position: absolute;
  bottom: calc(100% + 4px);
  top: auto;
  left: 50%;
  transform: translateX(-50%);
  width: 420px;
  max-width: calc(100vw - 32px);
  max-height: min(520px, 68vh);
  overflow-y: auto;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md);
  z-index: 200;
  padding: var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  font-size: var(--text-sm);
  color: var(--color-text-primary);
}

.chat-input__token-section {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}
.chat-input__token-section + .chat-input__token-section {
  border-top: 1px solid var(--color-bg-border);
  padding-top: var(--space-3);
}
.chat-input__token-section-title {
  font-size: var(--text-2xs);
  font-weight: var(--weight-semibold);
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--color-text-muted);
}
.chat-input__token-section-subtitle {
  font-size: var(--text-2xs);
  color: var(--color-text-muted);
  margin-bottom: var(--space-1);
}

/* 上下文占用:大号百分比 + 进度条。 */
.chat-input__token-ctx-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-2);
}
.chat-input__token-ctx-pct {
  font-family: var(--font-mono);
  font-size: var(--text-lg);
  font-weight: var(--weight-semibold);
}
.chat-input__token-ctx-pct--ok {
  color: var(--color-status-success);
}
.chat-input__token-ctx-pct--warn {
  color: var(--color-status-warn);
}
.chat-input__token-ctx-pct--alert {
  color: var(--color-tool-error-text);
}
.chat-input__token-ctx-nums,
.chat-input__token-ctx-sub {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}

.chat-input__token-bar {
  height: 10px;
  border-radius: var(--radius-pill);
  background: var(--color-bg-border);
  overflow: hidden;
}
.chat-input__token-bar-fill {
  display: block;
  height: 100%;
  border-radius: var(--radius-pill);
  background: var(--color-accent);
  transition: width var(--duration-base) var(--ease-out);
}
.chat-input__token-bar-fill--ok {
  background: var(--color-status-success);
}
.chat-input__token-bar-fill--warn {
  background: var(--color-status-warn);
}
.chat-input__token-bar-fill--alert {
  background: var(--color-tool-error-text);
}

/* 构成堆叠条:外层 .chat-input__token-bar 是满宽 = 容量轨道,内层
   stack 宽 = 实发占比,内部按切片值 flex 分配,段间 2px 缝(缝色 =
   轨道色,父级 overflow hidden 收圆角)。 */
.chat-input__token-bar-stack {
  display: flex;
  gap: 2px;
  height: 100%;
}
.chat-input__token-bar-seg {
  flex-basis: 0;
  min-width: 2px;
}

/* 构成图例:色点 + 标签 + token + %,四列 grid 常显。 */
.chat-input__token-legend {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin-top: var(--space-1);
}
.chat-input__token-legend-row {
  display: grid;
  grid-template-columns: 8px 1fr auto 3em;
  gap: var(--space-2);
  align-items: center;
  font-size: var(--text-xs);
}
.chat-input__token-legend-dot {
  width: 8px;
  height: 8px;
  border-radius: 2px;
}
.chat-input__token-legend-label {
  color: var(--color-text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.chat-input__token-legend-value,
.chat-input__token-legend-pct {
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  text-align: right;
}
.chat-input__token-legend-pct {
  color: var(--color-text-secondary);
}
.chat-input__token-legend-note {
  font-size: var(--text-2xs);
  color: var(--color-text-muted);
  margin-top: 2px;
}

/* 明细 / 聚合行列表(label 左 value 右,mono)。 */
.chat-input__token-rows {
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
}
.chat-input__token-row {
  display: flex;
  justify-content: space-between;
  gap: var(--space-4);
}
.chat-input__token-row > span:first-child {
  color: var(--color-text-secondary);
  font-family: var(--font-sans);
}
.chat-input__token-strong {
  font-weight: var(--weight-semibold);
}

.chat-input__token-provider {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding-top: var(--space-1);
}
.chat-input__token-provider-head {
  display: flex;
  justify-content: space-between;
  gap: var(--space-2);
}
.chat-input__token-provider-name {
  font-weight: var(--weight-semibold);
}
.chat-input__token-provider-total,
.chat-input__token-provider-split {
  color: var(--color-text-secondary);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
}

.chat-input__token-bars {
  display: flex;
  align-items: flex-end;
  gap: 2px;
  height: 40px;
  margin-top: var(--space-1);
}
.chat-input__token-bar-col {
  flex: 1;
  min-width: 3px;
  background: var(--color-accent);
  border-radius: 1px;
  opacity: 0.75;
}

.chat-input__token-sessions {
  padding-top: var(--space-1);
}
.chat-input__token-session-row {
  display: flex;
  justify-content: space-between;
  gap: var(--space-2);
  width: 100%;
  padding: var(--space-1) var(--space-2);
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: var(--text-xs);
  text-align: left;
  cursor: pointer;
}
.chat-input__token-session-row:hover:not(:disabled) {
  background: var(--color-bg-hover);
}
.chat-input__token-session-row:disabled {
  cursor: default;
  opacity: 0.6;
}
.chat-input__token-session-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.chat-input__token-session-nums {
  color: var(--color-text-secondary);
  font-family: var(--font-mono);
  white-space: nowrap;
}

.chat-input__token-settings {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  align-items: center;
  border-top: 1px solid var(--color-bg-border);
  padding-top: var(--space-2);
}
.chat-input__token-field {
  display: inline-flex;
  align-items: center;
  gap: var(--space-1);
  color: var(--color-text-secondary);
  font-size: var(--text-xs);
}
.chat-input__token-input {
  width: 84px;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  color: inherit;
  font: inherit;
  font-size: var(--text-xs);
}
.chat-input__token-save {
  padding: var(--space-1) var(--space-3);
  border-radius: var(--radius-sm);
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  color: inherit;
  font: inherit;
  font-size: var(--text-xs);
  cursor: pointer;
}
.chat-input__token-save:disabled {
  opacity: 0.6;
  cursor: default;
}

.chat-input__token-muted {
  color: var(--color-text-muted);
}
.chat-input__token-error {
  color: var(--color-tool-error-text);
}

/* 开合动画:向上开,自 translateY(4px) 滑入;基态含居中 translateX。 */
.chat-input-token-popover-enter-active,
.chat-input-token-popover-leave-active {
  transition: opacity var(--duration-base) var(--ease-out),
    transform var(--duration-base) var(--ease-out);
  transform-origin: bottom center;
}
.chat-input-token-popover-enter-from,
.chat-input-token-popover-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(4px);
}
.chat-input-token-popover-leave-active {
  transition-duration: var(--duration-fast);
  transition-timing-function: ease-in;
}
</style>
