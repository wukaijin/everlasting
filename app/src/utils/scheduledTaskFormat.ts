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

/** schedule 档位 → 人话一行(列表卡 + 编辑回显共用)。
 *  `daily 09:00` → 「每天 09:00」;`interval 30` → 「每 30 分钟」;
 *  `weekly mon 09:00` → 「每周一 09:00」。 */
export function describeSchedule(spec: ScheduleSpec | null): string {
  if (!spec || typeof spec !== "object") return "未知档位";
  switch (spec.kind) {
    case "daily":
      return `每天 ${spec.at}`;
    case "interval":
      return `每 ${spec.every_min} 分钟`;
    case "weekly":
      return `每${weekdayLabel(spec.weekday)} ${spec.at}`;
    default:
      return "未知档位";
  }
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
