// 08-20-turn-usage-event-quota-view WP3 — useQuotaStore。
//
// 5h 滚动窗口配额视图的数据层。2026-08-21 起唯一消费组件是
// ChatInput hint 行的 `<ChatInputTokenUsage>` 大号用量弹层(原
// AppHeader QuotaChip 已并入,顶栏不再常驻入口)。
//
//   后端 `usage_window` IPC(db::usage 聚合 turn_trace)
//     → refresh() 写 report
//     → computed: chipTotal / chipPct(弹层窗口总量 + 额度占比条)
//     → 弹层直接读 report(per-provider 拆分 / hourly / top sessions)
//
// 刷新时机(design 取舍:不做定时轮询、不做客户端增量推算 —— 滑动窗口
// 客户端推算必漂;5h 尺度下"跑完这轮后刷新"语义刚好):
//   1. 弹层组件挂载时(ChatInputTokenUsage mount);
//   2. 弹层打开时(toggle → open);
//   3. 每次 request 完成后(streamEvents done 分支 fire-and-forget)。
//
// `setSettings` 写 config 两键(quota_window_hours / quota_limit_tokens,
// 后端 `set_quota_settings`)后立即 refresh —— 弹层内保存即时反映。

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { transport } from "../transport";
import { extractErrorMessage } from "../utils/useErrorBus";

/** 后端 `UsageWindowReport` 的 wire 形状(camelCase,Rust
 * `#[serde(rename_all = "camelCase")]`)。 */
export interface UsageTotalsWire {
  inputTokens: number;
  outputTokens: number;
  cacheReadInputTokens: number;
  cacheCreationInputTokens: number;
}

export interface HourlyBucketWire {
  hour: string;
  inputTokens: number;
  outputTokens: number;
}

export interface ProviderUsageWire {
  providerId: string;
  displayName: string | null;
  totals: UsageTotalsWire;
  mainTotals: UsageTotalsWire;
  workerTotals: UsageTotalsWire;
  hourly: HourlyBucketWire[];
}

export interface SessionUsageWire {
  sessionId: string;
  title: string | null;
  projectId: string | null;
  windowMainInput: number;
  windowWorkerInput: number;
  lifetimeInput: number;
  lifetimeOutput: number;
}

export interface UsageWindowReportWire {
  windowHours: number;
  limitTokens: number | null;
  providers: ProviderUsageWire[];
  topSessions: SessionUsageWire[];
}

export const useQuotaStore = defineStore("quota", () => {
  const report = ref<UsageWindowReportWire | null>(null);
  const loading = ref(false);
  const error = ref<string | null>(null);

  /** chip 常驻总量 = 全 provider (input + output) 之和。 */
  const chipTotal = computed(() => {
    if (!report.value) return 0;
    return report.value.providers.reduce(
      (acc, p) => acc + p.totals.inputTokens + p.totals.outputTokens,
      0,
    );
  });

  /** 设了额度时的占比(0..1);未设 = null(chip 不画进度条)。 */
  const chipPct = computed(() => {
    const limit = report.value?.limitTokens;
    if (!limit || limit <= 0) return null;
    return Math.min(1, chipTotal.value / limit);
  });

  /** 拉取(弹层打开 / mount / finalize 后)。fire-and-forget 友好:
   * 失败写 error 不抛(chip 退化显示上次值)。 */
  async function refresh(): Promise<void> {
    loading.value = true;
    try {
      report.value = await transport.invoke<UsageWindowReportWire>(
        "usage_window",
        { providerId: null },
      );
      error.value = null;
    } catch (e) {
      error.value = extractErrorMessage(e);
    } finally {
      loading.value = false;
    }
  }

  /** 弹层设置行保存。`limitTokens = null` = 清除额度(只显示消耗)。 */
  async function setSettings(
    windowHours: number | null,
    limitTokens: number | null,
  ): Promise<void> {
    // 扁平参数(httpTransport 顶层 camel→snake 作 POST body;嵌套结构
    // 会在 HTTP 模式 miss 字段 —— 质检修正,house 模式同 set_remote_config)。
    await transport.invoke("set_quota_settings", { windowHours, limitTokens });
    await refresh();
  }

  return { report, loading, error, chipTotal, chipPct, refresh, setSettings };
});
