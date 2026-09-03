<script setup lang="ts">
// DiskTab — Settings「存储」分类(F3 磁盘治理 PR3,2026-09-03,design §7)。
//
// 三段式:
//   1. 占用概览:get_disk_usage 渲染条目(label + 人类可读大小)+ 总量行
//      + 刷新按钮;进入 tab 自动拉一次。
//   2. 开关:照 GeneralTab 的 FlagRow 模式两行(diskGovernorEnabled /
//      outputsAgeCleanupEnabled),读值与写入都走 configStore(PR1 已接)。
//   3. 立即清理:pending 态按钮 → run_disk_cleanup → toast 展示逐项回收
//      摘要(共回收 X)→ resolve 成功后自动重新 get_disk_usage 刷新概览
//      (AC7「数字同步下降」的实现闭环)。
//
// kill-switch(diskGovernorEnabled)关闭时按钮**仍可用**(手动语义,
// AC9),附一行说明文字。按钮直调 IPC 而不经 configStore —— 概览数据
// 是本组件局部状态(仅此 tab 消费),不值得进全局 store。

import { onMounted, reactive, ref } from "vue";
import { transport } from "../../transport";
import { useConfigStore } from "../../stores/config";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";

const config = useConfigStore();
const projects = useProjectsStore();

// ----- 第 1 段:占用概览 -----

interface DiskUsageEntry {
  key: string;
  label: string;
  bytes: number;
}

interface DiskUsageReport {
  entries: DiskUsageEntry[];
  totalBytes: number;
}

/** 后端 DiskGovernorOutcome 的 camelCase wire 形态(PR1 结构直传)。 */
interface CleanupResult {
  items: number;
  reclaimedBytes: number;
}

interface DiskGovernorOutcome {
  workerWorktrees: CleanupResult;
  orphanSessionWorktrees: CleanupResult;
  outputs: CleanupResult;
  backups: CleanupResult;
}

const report = ref<DiskUsageReport | null>(null);
const usageLoading = ref(false);

/** 字节 → 人类可读(B/KB/MB/GB/TB;≥100 取整,否则 1 位小数)。 */
function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n;
  let i = -1;
  do {
    v /= 1024;
    i++;
  } while (v >= 1024 && i < units.length - 1);
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

async function refreshUsage(): Promise<void> {
  if (usageLoading.value) return;
  usageLoading.value = true;
  try {
    report.value = await transport.invoke<DiskUsageReport>("get_disk_usage");
  } catch (e) {
    projects.showToast(`获取磁盘占用失败:${extractErrorMessage(e)}`, "error");
  } finally {
    usageLoading.value = false;
  }
}

onMounted(refreshUsage);

// ----- 第 2 段:开关(GeneralTab FlagRow 同款) -----

const pending = reactive<Record<string, boolean>>({});

interface FlagRow {
  key: string;
  title: string;
  description: string;
  value: () => boolean;
  set: (on: boolean) => Promise<void>;
}

const rows: FlagRow[] = [
  {
    key: "diskGovernorEnabled",
    title: "自动磁盘回收",
    description:
      "每天自动回收:过期 worker worktree、孤儿 worktree 与输出、过期输出与旧备份。关闭后仅停止自动节拍,不影响下方手动清理。",
    value: () => config.diskGovernorEnabled,
    set: (on) => config.setDiskGovernorEnabled(on),
  },
  {
    key: "outputsAgeCleanupEnabled",
    title: "过期输出回收",
    description:
      "会话仍在时,其工具输出 spill 超过 30 天也自动回收(旧工具结果的「查看全文」将失效)。孤儿输出不受此开关影响,恒回收。",
    value: () => config.outputsAgeCleanupEnabled,
    set: (on) => config.setOutputsAgeCleanupEnabled(on),
  },
];

async function onToggle(row: FlagRow): Promise<void> {
  if (pending[row.key]) return;
  const target = !row.value();
  pending[row.key] = true;
  try {
    await row.set(target);
  } catch (e) {
    projects.showToast(`设置失败:${extractErrorMessage(e)}`, "error");
  } finally {
    delete pending[row.key];
  }
}

// ----- 第 3 段:立即清理 -----

/** toast 摘要的逐项文案(与后端 DiskGovernorOutcome 字段一一对应)。 */
const CLEANUP_ITEMS: Array<{ key: keyof DiskGovernorOutcome; label: string }> = [
  { key: "workerWorktrees", label: "过期 worker worktree" },
  { key: "orphanSessionWorktrees", label: "孤儿 session worktree" },
  { key: "outputs", label: "工具输出 spill" },
  { key: "backups", label: "旧数据库备份" },
];

const cleaning = ref(false);

async function runCleanup(): Promise<void> {
  if (cleaning.value) return;
  cleaning.value = true;
  try {
    const outcome = await transport.invoke<DiskGovernorOutcome>("run_disk_cleanup");
    const total = CLEANUP_ITEMS.reduce((sum, i) => sum + outcome[i.key].reclaimedBytes, 0);
    const reclaimed = CLEANUP_ITEMS.filter((i) => outcome[i.key].items > 0)
      .map((i) => `${i.label} ${outcome[i.key].items} 项`)
      .join("、");
    projects.showToast(
      reclaimed
        ? `清理完成,共回收 ${formatBytes(total)}:${reclaimed}`
        : "清理完成,当前没有可回收项",
      "info",
    );
    // AC7:resolve 成功后自动重新拉概览,数字同步下降。
    await refreshUsage();
  } catch (e) {
    projects.showToast(`清理失败:${extractErrorMessage(e)}`, "error");
  } finally {
    cleaning.value = false;
  }
}
</script>

<template>
  <div class="disk-tab">
    <!-- 第 1 段:占用概览 -->
    <section class="disk-tab__section">
      <div class="disk-tab__section-head">
        <span class="disk-tab__section-title">磁盘占用</span>
        <button
          type="button"
          class="disk-tab__refresh"
          :disabled="usageLoading"
          @click="refreshUsage"
        >
          {{ usageLoading ? "统计中…" : "刷新" }}
        </button>
      </div>
      <p v-if="usageLoading && !report" class="disk-tab__hint">正在统计各目录占用…</p>
      <template v-else-if="report">
        <ul class="disk-tab__usage-list">
          <li v-for="e in report.entries" :key="e.key" class="disk-tab__usage-row">
            <span class="disk-tab__usage-label">{{ e.label }}</span>
            <span class="disk-tab__usage-bytes">{{ formatBytes(e.bytes) }}</span>
          </li>
        </ul>
        <div class="disk-tab__usage-total">
          <span>合计</span>
          <span class="disk-tab__usage-total-bytes">{{ formatBytes(report.totalBytes) }}</span>
        </div>
      </template>
      <p v-else class="disk-tab__hint">暂无数据,点击「刷新」重新统计。</p>
    </section>

    <!-- 第 2 段:回收开关 -->
    <section class="disk-tab__section">
      <span class="disk-tab__section-title">回收策略</span>
      <ul class="disk-tab__list">
        <li v-for="row in rows" :key="row.key" class="disk-tab__row">
          <div class="disk-tab__text">
            <span class="disk-tab__title">{{ row.title }}</span>
            <span class="disk-tab__desc">{{ row.description }}</span>
          </div>
          <button
            type="button"
            role="switch"
            :aria-checked="row.value()"
            :aria-label="row.title"
            class="disk-tab__switch"
            :class="{ 'disk-tab__switch--on': row.value() }"
            :disabled="!!pending[row.key]"
            @click="onToggle(row)"
          >
            <span class="disk-tab__switch-knob" />
          </button>
        </li>
      </ul>
    </section>

    <!-- 第 3 段:立即清理 -->
    <section class="disk-tab__section">
      <span class="disk-tab__section-title">手动清理</span>
      <p class="disk-tab__desc">
        立即执行一轮完整回收(过期 worker worktree、孤儿 worktree 与输出、过期输出、旧备份)。
      </p>
      <p v-if="!config.diskGovernorEnabled" class="disk-tab__hint">
        自动回收已关闭;手动「立即清理」不受影响。
      </p>
      <button
        type="button"
        class="disk-tab__cleanup"
        :disabled="cleaning"
        @click="runCleanup"
      >
        {{ cleaning ? "清理中…" : "立即清理" }}
      </button>
    </section>
  </div>
</template>

<style scoped>
.disk-tab {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}

.disk-tab__section {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.disk-tab__section + .disk-tab__section {
  border-top: 1px solid var(--color-bg-border);
  padding-top: var(--space-4);
}

.disk-tab__section-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
}

.disk-tab__section-title {
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
}

.disk-tab__refresh {
  flex-shrink: 0;
}

/* 概览列表 */
.disk-tab__usage-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
}

.disk-tab__usage-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-2) 0;
  border-bottom: 1px solid var(--color-bg-border);
}

.disk-tab__usage-row:last-child {
  border-bottom: 0;
}

.disk-tab__usage-label {
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  min-width: 0;
}

.disk-tab__usage-bytes {
  flex-shrink: 0;
  font-size: var(--text-sm);
  font-variant-numeric: tabular-nums;
  color: var(--color-text-secondary);
}

.disk-tab__usage-total {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-2) 0 0;
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
}

.disk-tab__usage-total-bytes {
  font-variant-numeric: tabular-nums;
}

/* 开关(GeneralTab / ScheduledTasksTab 同款药丸) */
.disk-tab__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
}

.disk-tab__row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-3) 0;
  border-bottom: 1px solid var(--color-bg-border);
}

.disk-tab__row:last-child {
  border-bottom: 0;
}

.disk-tab__text {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  min-width: 0;
}

.disk-tab__title {
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
}

.disk-tab__desc {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
}

.disk-tab__hint {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

.disk-tab__switch {
  flex-shrink: 0;
  width: 36px;
  height: 20px;
  border-radius: 999px;
  border: 1px solid var(--color-bg-border-strong);
  background: var(--color-bg-app);
  padding: 0;
  position: relative;
  cursor: pointer;
  transition:
    background var(--duration-base) var(--ease-out),
    border-color var(--duration-base) var(--ease-out);
}

.disk-tab__switch--on {
  background: var(--color-accent);
  border-color: var(--color-accent);
}

.disk-tab__switch:disabled {
  opacity: 0.5;
  cursor: default;
}

.disk-tab__switch-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--color-text-primary);
  transition: transform var(--duration-base) var(--ease-out);
}

.disk-tab__switch--on .disk-tab__switch-knob {
  transform: translateX(16px);
  background: var(--color-text-on-accent);
}

.disk-tab__cleanup {
  align-self: flex-start;
}

/* 移动端:全局 `.settings-modal button { min-height: 44px }` 会把药丸撑成
   大方块(GeneralTab / ScheduledTasksTab 同款压回视觉尺寸)。 */
@media (max-width: 767px) {
  .disk-tab__switch {
    min-width: 0;
    min-height: 0;
  }
}
</style>
