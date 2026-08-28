// Scheduled-task display helpers — pure formatting for the F2
// 「定时任务」 surfaces (Settings tab + session header badge title).
//
// Extracted so the 人话 rules are unit-testable without a Vue renderer
// (same rationale as ./time.ts / ./duration.ts). Everything here is a
// pure function over the wire shapes — no IPC, no DOM.
//
// Wire contract (mirror of Rust, BACKLOG §5.2 snake_case):
//   - `ScheduleSpec` = stores/scheduledTasks.ts re-export; the preset
//     union mirrors `scheduler::compute::ScheduleSpec` (internally
//     tagged `kind`; weekday is chrono's lowercase 3-letter form).
//   - Unknown / corrupt `kind` (legacy row) → 「未知档位」 — never throw.

import type { ScheduleSpec } from "../stores/scheduledTasks";

/** 周几下拉的选项(wire 值 = chrono Weekday serde 形)。 */
export const WEEKDAY_OPTIONS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "mon", label: "周一" },
  { value: "tue", label: "周二" },
  { value: "wed", label: "周三" },
  { value: "thu", label: "周四" },
  { value: "fri", label: "周五" },
  { value: "sat", label: "周六" },
  { value: "sun", label: "周日" },
];

/** 周几 wire 值 → 中文;未知值原样返回(防御)。 */
export function weekdayLabel(weekday: string): string {
  return WEEKDAY_OPTIONS.find((w) => w.value === weekday)?.label ?? weekday;
}

/** 固定频率档的单位选项(F2b)。存库恒 `every_min` 分钟 —— 单位是纯 UI
 *  换算(时 60 / 天 1440 / 周 10080,整除无精度损失,后端零感知)。 */
export const INTERVAL_UNITS: ReadonlyArray<{
  value: string;
  label: string;
  minutes: number;
}> = [
  { value: "minute", label: "分钟", minutes: 1 },
  { value: "hour", label: "小时", minutes: 60 },
  { value: "day", label: "天", minutes: 1440 },
  { value: "week", label: "周", minutes: 10080 },
];

/** `every_min` → `(数量, 单位)`:取能整除的最大单位,否则原值分钟
 *  (1440 → 1 天;90 → 90 分钟;10080 → 1 周)。表单编辑回填与
 *  `describeSchedule` 人话共用。 */
export function splitEveryMin(
  everyMin: number,
): { n: number; unit: string; label: string } {
  for (let i = INTERVAL_UNITS.length - 1; i > 0; i--) {
    const u = INTERVAL_UNITS[i]!;
    if (everyMin > 0 && everyMin % u.minutes === 0) {
      return { n: everyMin / u.minutes, unit: u.value, label: u.label };
    }
  }
  return { n: everyMin, unit: "minute", label: "分钟" };
}

/** schedule 档位 → 人话一行(列表卡 + 编辑回显共用)。
 *  `daily 09:00` → 「每天 09:00」;`interval 30` → 「每 30 分钟」;
 *  `weekly mon 09:00` → 「每周一 09:00」;F2b:`hourly 30` →
 *  「每小时 30 分」、`weekdays` → 「每工作日 09:00」、`monthly 15`
 *  → 「每月 15 号 09:00」。 */
export function describeSchedule(spec: ScheduleSpec | null): string {
  if (!spec || typeof spec !== "object") return "未知档位";
  switch (spec.kind) {
    case "daily":
      return `每天 ${spec.at}`;
    case "interval": {
      const { n, label } = splitEveryMin(spec.every_min);
      return n === 1 ? `每${label}` : `每 ${n} ${label}`;
    }
    case "weekly":
      return `每${weekdayLabel(spec.weekday)} ${spec.at}`;
    case "hourly":
      return `每小时 ${spec.minute.toString().padStart(2, "0")} 分`;
    case "weekdays":
      return `每工作日 ${spec.at}`;
    case "monthly":
      return `每月 ${spec.day} 号 ${spec.at}`;
    default:
      return "未知档位";
  }
}

/** 已触发次数片段(F2b 列表卡):`max_runs` 设置 → 「N/M 次」,否则
 *  「N 次」。 */
export function describeRunCount(
  runCount: number,
  maxRuns: number | null | undefined,
): string {
  return typeof maxRuns === "number"
    ? `${runCount}/${maxRuns} 次`
    : `${runCount} 次`;
}

/** 结束日期片段(F2b 列表卡;仅 ends_at 设置时非空):「至 2026-12-31」。
 *  ends_at 含当日(当日到期点照常触发,prd D9),展示日期粒度即可。 */
export function describeEndDate(endsAt: number | null | undefined): string {
  if (typeof endsAt !== "number" || !Number.isFinite(endsAt)) return "";
  const d = new Date(endsAt);
  if (Number.isNaN(d.getTime())) return "";
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `至 ${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** 次数上限完成态(F2b D8):自动停用且计数达限 = 「已完成」,区别于
 *  用户手动停用(灰显「已停用」)。 */
export function completedByRunLimit(task: {
  enabled: boolean;
  run_count: number;
  max_runs: number | null;
}): boolean {
  return (
    !task.enabled &&
    typeof task.max_runs === "number" &&
    task.run_count >= task.max_runs
  );
}

/** 结束日期完成态(F2b D8):自动停用且 ends_at 已过 = 「已结束」。 */
export function completedByEndDate(task: {
  enabled: boolean;
  ends_at: number | null;
}): boolean {
  return (
    !task.enabled && typeof task.ends_at === "number" && Date.now() > task.ends_at
  );
}

/** epoch ms → 本地 `MM-DD HH:mm`(列表的上次/下次触发列;触发点几乎
 *  总是跨「天」粒度,日期比时刻更有信息量)。非法/缺失 → "—"。 */
export function formatFireTime(ms: number | null | undefined): string {
  if (typeof ms !== "number" || !Number.isFinite(ms)) return "—";
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "—";
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** prompt 多行摘要:压平空白 + 截断(列表卡的单行摘要,不展开全文)。 */
export function summarizePrompt(prompt: string, max = 60): string {
  const flat = prompt.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max)}…` : flat;
}
