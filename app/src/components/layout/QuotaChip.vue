<script setup lang="ts">
// QuotaChip — AppHeader 常驻的 5h 滚动窗口配额入口(08-20-turn-usage-
// event-quota-view WP3)。手写 popover 模式(`popover-pattern.md`,与
// ModeSelect/ModelSelect 同族;顶栏几何 → 向下开)。
//
// 两态:
// - chip 常驻:`5h ▸ 1.2M`(abbreviateTokens);设了额度加迷你进度条
//   + 占比着色(tokenUsageLevel 同档语义:≥75% 红 / ≥50% 黄)。
// - 弹层:per-provider 段(总量 + 主/worker 拆分 + 额度条)/ 小时分布
//   CSS 柱 / top sessions(点击 openSessionInProject 跳转)/ 设置行
//   (窗口小时 + 额度,保存调 set_quota_settings)。
//
// 刷新:打开弹层时 refresh()(store 另有 mount + finalize 两个触发点)。
// 移动端 <430px:chip 收缩为纯图标(S6a 窄屏降级档)。

import { computed, onMounted, onUnmounted, ref } from "vue";

import { useChatStore } from "../../stores/chat";
import { useQuotaStore } from "../../stores/quota";
import { abbreviateTokens } from "../../utils/tokenUsage";
import Icon from "../Icon.vue";

const quotaStore = useQuotaStore();
const chatStore = useChatStore();

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

const chipLabel = computed(() => abbreviateTokens(quotaStore.chipTotal));
const pct = computed(() =>
  quotaStore.chipPct != null ? Math.round(quotaStore.chipPct * 100) : null,
);
/** 占比着色档(对齐 ChatInput hint 的 49/50/74/75 语义,复用 CSS 变量)。 */
const levelClass = computed(() => {
  if (pct.value == null) return "";
  if (pct.value >= 75) return "quota-chip--danger";
  if (pct.value >= 50) return "quota-chip--warn";
  return "";
});

/** 小时分布柱:归一化高度(最大桶 = 100%)。 */
function barHeight(value: number, max: number): number {
  if (max <= 0) return 0;
  return Math.max(4, Math.round((value / max) * 100));
}
const maxHourlyInput = computed(() => {
  let max = 0;
  for (const p of quotaStore.report?.providers ?? []) {
    for (const h of p.hourly) max = Math.max(max, h.inputTokens);
  }
  return max;
});

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
  <div ref="root" class="quota-chip-root">
    <button
      class="quota-chip"
      :class="levelClass"
      type="button"
      title="5h 滚动窗口用量"
      aria-label="打开配额窗口详情"
      @click="toggle"
    >
      <Icon name="bolt" :size="14" />
      <span class="quota-chip__window">{{ quotaStore.report?.windowHours ?? 5 }}h</span>
      <span class="quota-chip__total">{{ chipLabel }}</span>
      <span
        v-if="pct != null"
        class="quota-chip__bar"
        :aria-label="`已用 ${pct}%`"
      ><span class="quota-chip__bar-fill" :style="{ width: `${pct}%` }" /></span>
    </button>

    <div v-if="open" class="quota-pop" role="dialog" aria-label="窗口用量详情">
      <div v-if="quotaStore.error" class="quota-pop__error">
        拉取失败:{{ quotaStore.error }}
      </div>
      <div v-if="quotaStore.loading && !quotaStore.report" class="quota-pop__muted">
        加载中…
      </div>

      <template v-for="p in quotaStore.report?.providers ?? []" :key="p.providerId">
        <div class="quota-pop__provider">
          <div class="quota-pop__provider-head">
            <span class="quota-pop__provider-name">{{
              p.displayName ?? p.providerId
            }}</span>
            <span class="quota-pop__provider-total">
              {{ abbreviateTokens(p.totals.inputTokens) }} in ·
              {{ abbreviateTokens(p.totals.outputTokens) }} out
            </span>
          </div>
          <div class="quota-pop__split">
            主 {{ abbreviateTokens(p.mainTotals.inputTokens) }} · worker
            {{ abbreviateTokens(p.workerTotals.inputTokens) }}
          </div>
          <div v-if="p.hourly.length" class="quota-pop__bars" aria-hidden="true">
            <div
              v-for="h in p.hourly"
              :key="h.hour"
              class="quota-pop__bar-col"
              :title="`${h.hour.slice(11, 16)} · in ${abbreviateTokens(h.inputTokens)}`"
              :style="{ height: `${barHeight(h.inputTokens, maxHourlyInput)}%` }"
            />
          </div>
        </div>
      </template>
      <div
        v-if="quotaStore.report && quotaStore.report.providers.length === 0"
        class="quota-pop__muted"
      >
        窗口内暂无用量
      </div>

      <div v-if="quotaStore.report?.topSessions.length" class="quota-pop__sessions">
        <div class="quota-pop__section-title">窗口内 session</div>
        <button
          v-for="s in quotaStore.report.topSessions"
          :key="s.sessionId"
          class="quota-pop__session-row"
          type="button"
          :disabled="!s.projectId"
          :title="s.sessionId"
          @click="jumpToSession(s.sessionId, s.projectId)"
        >
          <span class="quota-pop__session-title">{{
            s.title || s.sessionId.slice(0, 8)
          }}</span>
          <span class="quota-pop__session-nums">
            {{ abbreviateTokens(s.windowMainInput + s.windowWorkerInput) }} /
            累计 {{ abbreviateTokens(s.lifetimeInput) }}
          </span>
        </button>
      </div>

      <div class="quota-pop__settings">
        <div class="quota-pop__section-title">设置</div>
        <label class="quota-pop__field">
          窗口(h)
          <input v-model="hoursInput" class="quota-pop__input" type="number" min="1" max="168" />
        </label>
        <label class="quota-pop__field">
          额度(token)
          <input v-model="limitInput" class="quota-pop__input" type="number" min="0" placeholder="未设置" />
        </label>
        <button class="quota-pop__save" type="button" :disabled="saving" @click="saveSettings">
          {{ saving ? "保存中…" : "保存" }}
        </button>
        <div v-if="settingsError" class="quota-pop__error">{{ settingsError }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 顶栏 chip 家族(与 PendingBadge 同排)。 */
.quota-chip-root {
  position: relative;
  display: inline-flex;
  flex-shrink: 0;
}
.quota-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0 8px;
  border-radius: var(--radius-full, 999px);
  border: 1px solid var(--color-bg-border);
  background: transparent;
  color: var(--color-text-secondary);
  font-size: 12px;
  font-family: inherit;
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out),
    color var(--duration-fast) var(--ease-out);
}
.quota-chip:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}
.quota-chip--warn {
  color: var(--color-status-warning, #b45309);
}
.quota-chip--danger {
  color: var(--color-status-danger, #dc2626);
}
.quota-chip__bar {
  position: relative;
  width: 36px;
  height: 4px;
  border-radius: 2px;
  background: var(--color-bg-border);
  overflow: hidden;
}
.quota-chip__bar-fill {
  display: block;
  height: 100%;
  background: currentcolor;
  border-radius: 2px;
}

/* 弹层:向下开(顶栏几何;ModeSelect/ModelSelect 是底部向上,此处相反)。 */
.quota-pop {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  width: 280px;
  max-height: 70vh;
  overflow-y: auto;
  padding: 10px;
  border-radius: var(--radius-md, 8px);
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  box-shadow: var(--shadow-md, 0 8px 24px rgba(0, 0, 0, 0.18));
  z-index: 60;
  display: flex;
  flex-direction: column;
  gap: 10px;
  font-size: 12px;
  color: var(--color-text-primary);
}
.quota-pop__provider {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.quota-pop__provider-head {
  display: flex;
  justify-content: space-between;
  gap: 8px;
}
.quota-pop__provider-name {
  font-weight: 600;
}
.quota-pop__provider-total,
.quota-pop__split,
.quota-pop__session-nums,
.quota-pop__muted {
  color: var(--color-text-secondary);
}
.quota-pop__split {
  font-size: 11px;
}
.quota-pop__bars {
  display: flex;
  align-items: flex-end;
  gap: 2px;
  height: 36px;
  margin-top: 4px;
}
.quota-pop__bar-col {
  flex: 1;
  min-width: 3px;
  background: var(--color-accent, #3b82f6);
  border-radius: 1px;
  opacity: 0.75;
}
.quota-pop__section-title {
  font-size: 11px;
  color: var(--color-text-secondary);
  margin-bottom: 4px;
}
.quota-pop__session-row {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  width: 100%;
  padding: 4px 6px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: inherit;
  font-family: inherit;
  font-size: 12px;
  text-align: left;
  cursor: pointer;
}
.quota-pop__session-row:hover:not(:disabled) {
  background: var(--color-bg-elevated);
}
.quota-pop__session-row:disabled {
  cursor: default;
  opacity: 0.6;
}
.quota-pop__session-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.quota-pop__settings {
  border-top: 1px solid var(--color-bg-border);
  padding-top: 8px;
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: center;
}
.quota-pop__field {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--color-text-secondary);
}
.quota-pop__input {
  width: 72px;
  padding: 2px 6px;
  border-radius: 6px;
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  color: inherit;
  font-family: inherit;
  font-size: 12px;
}
.quota-pop__save {
  padding: 3px 10px;
  border-radius: 6px;
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  color: inherit;
  font-family: inherit;
  font-size: 12px;
  cursor: pointer;
}
.quota-pop__save:disabled {
  opacity: 0.6;
  cursor: default;
}
.quota-pop__error {
  color: var(--color-status-danger, #dc2626);
}

/* S6a 窄屏降级:<430px chip 收缩为纯图标 + 窗口数字。 */
@media (max-width: 429px) {
  .quota-chip__total,
  .quota-chip__bar {
    display: none;
  }
}
</style>
