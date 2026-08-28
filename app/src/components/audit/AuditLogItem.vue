<script setup lang="ts">
// AuditLogItem — a single row in the AuditLogModal list. Renders
// one `AuditEventRow` with:
//
//   1. A leading icon whose family + color reflects the kind
//      (🔴 denied/critical, 🟢 allowed/granted/executed-success,
//      🟡 mode, ⏱ timeout).
//   2. The time (`HH:MM:SS`) + the kind chip.
//   3. Tool name + a 1-line `tool_input` preview (when applicable).
//   4. Extra fields per kind:
//      - denied/denied_yolo/ask/timeout: the `reason` string.
//      - tool_executed: `duration_ms` formatted + `exit_code`
//        color-coded (0 = green, -1 = "killed", non-zero = red,
//        null = hidden).
//      - mode_changed/yolo_entered/yolo_exited: `prev_mode → new_mode`.
//   5. A 3px red left border when `payload.critical === true`
//      (matches the PermissionModal's critical variant, per
//      design-tokens.md "Border Tokens" exception).
//
// The renderer dispatches on the parsed payload's kind family.
// Malformed / null payloads degrade to a "raw payload" fallback
// row — the renderer NEVER throws.
//
// Visual family is derived from `iconFamilyForKind(kind)`. The
// color tokens reuse the existing `--color-tool-*` family per
// design-tokens.md "Don't add a new `--color-*` token for a
// one-off use" — denied uses `--color-tool-error` (red), allowed
// uses `--color-tool-write` (emerald), mode uses `--color-tool-
// thinking` (violet), timeout uses `--color-tool-shell` (amber),
// cancelled uses `--color-text-muted` (gray). Executed-success
// reuses `--color-tool-write`; executed-failure reuses
// `--color-tool-error`.

import { computed } from "vue";
import Icon from "../Icon.vue";
import {
  formatDuration,
  formatTimeOfDay,
  iconFamilyForKind,
  labelForKind,
  parseAuditPayload,
  summarizeToolInput,
  type AuditEventRow,
} from "../../utils/audit";

const props = defineProps<{
  row: AuditEventRow;
}>();

const parsed = computed(() =>
  parseAuditPayload(props.row.kind, props.row.payloadJson),
);

const family = computed(() => iconFamilyForKind(props.row.kind));

/** Per-family icon name + color token. The color goes on the
 *  leading icon and the kind chip border. */
const meta = computed<{ iconName: string; colorVar: string }>(() => {
  const f = family.value;
  switch (f) {
    case "denied":
    case "denied-yolo":
      return { iconName: "shield-x", colorVar: "var(--color-tool-error)" };
    case "allowed":
      return { iconName: "check-mini", colorVar: "var(--color-tool-write)" };
    case "granted":
      return { iconName: "shield-check", colorVar: "var(--color-tool-write)" };
    case "ask":
      return { iconName: "shield-check", colorVar: "var(--color-tool-shell)" };
    case "timeout":
      return { iconName: "clock", colorVar: "var(--color-tool-shell)" };
    case "cancelled":
      return { iconName: "x", colorVar: "var(--color-text-muted)" };
    case "executed":
      return { iconName: "check-mini", colorVar: "var(--color-tool-write)" };
    case "mode":
      return { iconName: "circle-dot", colorVar: "var(--color-tool-thinking)" };
    // D3 PR1 (2026-06-17): user-initiated message edit.
    // Renders a pencil icon in the accent color (the same
    // accent the inline edit-mode row in MessageItem uses)
    // so the user has a single visual signal for "edit"
    // across the app.
    case "message-edit":
      return { iconName: "pencil", colorVar: "var(--color-accent)" };
    // D3 PR3 (2026-06-17): user-initiated message resend.
    // Renders a refresh icon in the accent color — same
    // icon as the MessageActionsMenu's Resend button, so
    // the user sees a single visual signal for "resend".
    case "message-resend":
      return { iconName: "refresh", colorVar: "var(--color-accent)" };
    // C2+ (2026-07-05, task `07-05-c2-loop-active-intervention`):
    // harness-driven 循环检测主动干预 — 3 consecutive loop-
    // detection hits triggered a QuestionStore round-trip with
    // the user (asked / terminated / continued action). Renders
    // an alert-triangle icon in the amber shell color (matches
    // the existing "timeout" / "ask" family tone — the loop
    // intervention is a soft warning, not a critical deny).
    case "loop-intervention":
      return {
        iconName: "warn",
        colorVar: "var(--color-tool-shell)",
      };
    // 08-18-max-turns-softcap (2026-08-19): the turn-budget softcap
    // ask (main loop hit 200 turns → 继续/压缩后续跑/停止 card).
    // Renders a clock icon in the amber shell color — the softcap
    // is a timing/budget confirmation, matching the timeout
    // family's tone.
    case "turn-limit-softcap":
      return {
        iconName: "clock",
        colorVar: "var(--color-tool-shell)",
      };
    // B9+ D4 (2026-07-13, task `07-13-b9plus-generative-ui-followup`):
    // user-triggered diff apply success. Renders a file-check
    // icon in `--color-tool-write` (emerald) — same color as
    // executed-success because the user just successfully wrote
    // files. Distinct icon so the audit row reads "applied a
    // diff" rather than "executed a tool".
    case "ui-diff-applied":
      return {
        iconName: "file-check",
        colorVar: "var(--color-tool-write)",
      };
    // unified-context-budget WP2 (2026-08-19, task
    // `08-19-unified-context-budget`): the 关卡⑤ hard gate's silent
    // trim. Renders a shrink icon in the accent color — 治理动作非
    // 错误,与 TracePanel 的预算裁剪徽标同色系。
    case "context-budget-trim":
      return {
        iconName: "shrink",
        colorVar: "var(--color-accent)",
      };
    // F2 定时任务 (2026-08-28, task `08-28-f2-scheduled-tasks`):
    // scheduler fire lifecycle. Renders a clock icon in the accent
    // color — 与 session header 的「定时」徽章同图标同色系
    // (调度触发是系统治理动作,非错误;error 动作不翻红,详情在
    // summary 行的 action/reason)。
    case "scheduled-task":
      return {
        iconName: "clock",
        colorVar: "var(--color-accent)",
      };
    default:
      return { iconName: "info", colorVar: "var(--color-text-muted)" };
  }
});

/** The `tool_executed` payload's exit_code, type-narrowed for
 *  the renderer. Falls back to `null` when the parsed payload
 *  isn't `tool_executed` (defensive — other kinds shouldn't
 *  have an exit_code field, but malformed data could). */
const executedExitCode = computed<number | null>(() => {
  if (parsed.value.kind !== "tool_executed") return null;
  const ec = parsed.value.payload.exit_code;
  if (typeof ec === "number" && Number.isFinite(ec)) return ec;
  return null;
});

/** The `tool_executed` duration formatted ("3.2s" / "1m 23s"). */
const executedDuration = computed<string>(() => {
  if (parsed.value.kind !== "tool_executed") return "";
  return formatDuration(parsed.value.payload.duration_ms);
});

/** Whether this row is a `tool_executed` whose exit_code indicates
 *  a failure (non-zero AND not null). Used to override the icon
 *  family to a failure variant — the meta() switch above uses
 *  `--color-tool-write` (success) for executed, but a failed
 *  execution should read red. */
const executedFailed = computed<boolean>(() => {
  if (parsed.value.kind !== "tool_executed") return false;
  const ec = executedExitCode.value;
  return ec !== null && ec !== 0;
});

/** Override the leading icon's color for failed `tool_executed`
 *  rows. */
const effectiveColor = computed<string>(() =>
  executedFailed.value ? "var(--color-tool-error)" : meta.value.colorVar,
);

/** Short label for the exit code chip.
 *  - `0`    → "exit 0"
 *  - `-1`   → "killed"
 *  - `N!=0` → "exit N"
 *  - `null` → "" (hidden)
 */
const exitCodeLabel = computed<string>(() => {
  const ec = executedExitCode.value;
  if (ec === null) return "";
  if (ec === -1) return "killed";
  if (ec === 0) return "exit 0";
  return `exit ${ec}`;
});

const kindLabel = computed<string>(() => labelForKind(props.row.kind));
const timeLabel = computed<string>(() => formatTimeOfDay(props.row.ts));

/** Tool name from any payload kind that carries one. */
const toolName = computed<string>(() => {
  const p = parsed.value;
  if (p.kind === "tool" || p.kind === "tool_executed") {
    return p.payload.tool_name ?? "";
  }
  return "";
});

/** One-line `tool_input` summary. */
const inputSummary = computed<string>(() => {
  const p = parsed.value;
  if (p.kind === "tool" || p.kind === "tool_executed") {
    return summarizeToolInput(p.payload.tool_name, p.payload.tool_input);
  }
  return "";
});

/** Deny reason (tool_denied / tool_denied_yolo / ask). */
const reasonText = computed<string>(() => {
  if (parsed.value.kind === "tool") {
    return parsed.value.payload.reason ?? "";
  }
  return "";
});

/** Mode transition label ("edit → yolo"). */
const modeTransition = computed<string>(() => {
  if (parsed.value.kind !== "mode") return "";
  const prev = parsed.value.payload.prev_mode ?? "?";
  const next = parsed.value.payload.new_mode ?? "?";
  return `${prev} → ${next}`;
});

/** C2+ (2026-07-05): loop-intervention summary line. Format:
 *  `循环检测干预 · 硬/软触发 · 第 N 次命中 · 询问/已终止/已继续`.
 *  Renders empty for non-loop_intervention kinds. */
const loopInterventionSummary = computed<string>(() => {
  if (parsed.value.kind !== "loop_intervention") return "";
  const p = parsed.value.payload;
  const kindLabel = p.verdict_kind === "hard" ? "硬触发" : "软触发";
  const hit = p.hit_count ?? 0;
  const actionLabel = (() => {
    switch (p.action) {
      case "asked":
        return "询问";
      case "terminated":
        return "已终止";
      case "continued":
        return "已继续";
      default:
        return p.action ?? "?";
    }
  })();
  return `循环检测干预 · ${kindLabel} · 第 ${hit} 次命中 · ${actionLabel}`;
});

/** MAX_TURNS softcap (08-18-max-turns-softcap): turn-limit softcap
 *  summary line. Format:
 *  `轮数软卡 · 第 N 轮 · 预算 B · 动作`. Renders empty for
 *  non-turn_limit_softcap kinds. */
const turnLimitSoftcapSummary = computed<string>(() => {
  if (parsed.value.kind !== "turn_limit_softcap") return "";
  const p = parsed.value.payload;
  const turn = p.turn ?? 0;
  const budget = p.budget ?? 0;
  const actionLabel = (() => {
    switch (p.action) {
      case "asked":
        return "询问";
      case "continued":
        return "已继续";
      case "compacted_continued":
        return "压缩后续跑";
      case "stopped":
        return "已停止";
      case "timeout_stopped":
        return "超时停止";
      case "cancelled":
        return "已取消";
      default:
        return p.action ?? "?";
    }
  })();
  return `轮数软卡 · 第 ${turn} 轮 · 预算 ${budget} · ${actionLabel}`;
});

/** B9+ D4 (2026-07-13): user-triggered diff apply summary. Format:
 *  `应用 diff · N 个文件 (+A / -B) [+M more]`. Lists up to 32 file
 *  paths inline (mirrors the backend `record_ui_diff_applied_audit`
 *  32-row cap); beyond that the row shows `+N more`. Empty for
 *  non-ui_diff_applied kinds. */
const uiDiffAppliedSummary = computed<string>(() => {
  if (parsed.value.kind !== "ui_diff_applied") return "";
  const p = parsed.value.payload;
  const files = p.files ?? [];
  const total = p.total_files ?? files.length;
  if (files.length === 0) {
    return total > 0 ? `应用 diff · ${total} 个文件` : "应用 diff";
  }
  const totalAdded = files.reduce((acc, f) => acc + (f.added ?? 0), 0);
  const totalRemoved = files.reduce((acc, f) => acc + (f.removed ?? 0), 0);
  const shown = files.slice(0, 3).map((f) => f.path ?? "?").join(", ");
  const more = total > files.length ? ` +${total - files.length} more` : "";
  return `应用 diff · ${total} 个文件 (+${totalAdded} / -${totalRemoved}) · ${shown}${more}`;
});

/** F2 定时任务 (2026-08-28): scheduler fire lifecycle summary. Format:
 *  `「任务名」 · 触发/补跑/去重跳过/队列关闭跳过/丢失/失败/已完成[ · reason]`.
 *  Renders empty for non-scheduled_task_fired kinds. */
const scheduledTaskSummary = computed<string>(() => {
  if (parsed.value.kind !== "scheduled_task_fired") return "";
  const p = parsed.value.payload;
  const name = p.task_name ?? p.task_id ?? "?";
  const actionLabel = (() => {
    switch (p.action) {
      case "fired":
        return "触发";
      case "catchup":
        return "补跑";
      case "skipped_dedup":
        return "去重跳过";
      case "skipped_queue_disabled":
        return "队列关闭跳过";
      case "lost":
        return "丢失";
      case "error":
        return "失败";
      case "completed":
        return "已完成";
      default:
        return p.action ?? "?";
    }
  })();
  // F2b completed 的 reason(max_runs / end_date)翻译成人话;其余动作
  // 的 reason 原样(如 queue_full)。
  const reasonLabel =
    p.action === "completed"
      ? p.reason === "max_runs"
        ? "达次数上限"
        : p.reason === "end_date"
          ? "已达结束日期"
          : p.reason
      : p.reason;
  const reason = reasonLabel ? ` · ${reasonLabel}` : "";
  return `「${name}」 · ${actionLabel}${reason}`;
});

/** Whether the payload was malformed / unknown — render a raw
 *  blob fallback row. */
const isRawPayload = computed<boolean>(() => parsed.value.kind === "raw");

/** Rendered raw payload string (truncated). */
const rawPayloadText = computed<string>(() => {
  if (parsed.value.kind !== "raw") return "";
  try {
    const s = JSON.stringify(parsed.value.raw);
    return s && s.length > 200 ? `${s.slice(0, 197)}...` : s ?? "";
  } catch {
    return String(parsed.value.raw);
  }
});

/** `true` when the row carries `payload.critical === true`.
 *  Drives the 3px red left border. */
const isCritical = computed<boolean>(() => {
  if (parsed.value.kind !== "tool") return false;
  return parsed.value.payload.critical === true;
});
</script>

<template>
  <li
    class="audit-item"
    :class="{ 'audit-item--critical': isCritical }"
  >
    <span
      class="audit-item__icon"
      :style="{ color: effectiveColor }"
      aria-hidden="true"
    >
      <Icon :name="meta.iconName" :size="14" />
    </span>

    <div class="audit-item__body">
      <div class="audit-item__head">
        <time class="audit-item__time">{{ timeLabel }}</time>
        <span
          class="audit-item__kind"
          :style="{
            color: effectiveColor,
            borderColor: `color-mix(in srgb, ${effectiveColor} 35%, transparent)`,
          }"
        >
          {{ kindLabel }}
        </span>
        <template v-if="toolName">
          <span class="audit-item__tool">{{ toolName }}</span>
        </template>
      </div>

      <div v-if="inputSummary" class="audit-item__input">
        {{ inputSummary }}
      </div>

      <div v-if="executedDuration || exitCodeLabel" class="audit-item__exec">
        <span v-if="executedDuration" class="audit-item__duration">
          {{ executedDuration }}
        </span>
        <span
          v-if="exitCodeLabel"
          class="audit-item__exit"
          :class="{
            'audit-item__exit--fail': executedFailed,
            'audit-item__exit--ok': executedExitCode === 0,
          }"
        >
          {{ exitCodeLabel }}
        </span>
      </div>

      <div v-if="reasonText" class="audit-item__reason">
        {{ reasonText }}
      </div>

      <div v-if="modeTransition" class="audit-item__mode">
        {{ modeTransition }}
      </div>

      <div v-if="loopInterventionSummary" class="audit-item__loop">
        {{ loopInterventionSummary }}
      </div>

      <div v-if="turnLimitSoftcapSummary" class="audit-item__loop">
        {{ turnLimitSoftcapSummary }}
      </div>

      <div v-if="uiDiffAppliedSummary" class="audit-item__ui-diff">
        {{ uiDiffAppliedSummary }}
      </div>

      <div v-if="scheduledTaskSummary" class="audit-item__scheduled">
        {{ scheduledTaskSummary }}
      </div>

      <div v-if="isRawPayload && rawPayloadText" class="audit-item__raw">
        {{ rawPayloadText }}
      </div>
    </div>
  </li>
</template>

<style scoped>
.audit-item {
  display: grid;
  grid-template-columns: 20px 1fr;
  gap: 8px;
  padding: 8px 10px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  border-left: 3px solid transparent;
  transition: background var(--duration-fast) var(--ease-out);
}

.audit-item:hover {
  background: var(--color-bg-elevated);
}

/* Critical variant: 3px red left border (matches PermissionModal
   --critical, design-tokens.md "Border Tokens" exception). */
.audit-item--critical {
  border-left-color: var(--color-tool-error);
}

.audit-item__icon {
  display: inline-flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 2px;
}

.audit-item__body {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.audit-item__head {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.audit-item__time {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.audit-item__kind {
  font-size: var(--text-xs);
  padding: 1px 6px;
  border-radius: 999px;
  border: 1px solid;
  font-family: var(--font-sans);
  font-weight: var(--weight-medium);
  white-space: nowrap;
  flex-shrink: 0;
}

.audit-item__tool {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-primary);
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  white-space: nowrap;
}

.audit-item__input {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: 1.4;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

.audit-item__exec {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  align-items: center;
}

.audit-item__duration {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}

.audit-item__exit {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-app);
  color: var(--color-text-secondary);
}

.audit-item__exit--ok {
  color: var(--color-tool-write);
  border-color: color-mix(in srgb, var(--color-tool-write) 35%, transparent);
}

.audit-item__exit--fail {
  color: var(--color-tool-error-text);
  border-color: color-mix(in srgb, var(--color-tool-error) 35%, transparent);
}

.audit-item__reason {
  font-size: var(--text-sm);
  color: var(--color-tool-error-text);
  line-height: 1.4;
  word-break: break-word;
}

.audit-item__mode {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  color: var(--color-tool-thinking);
}

.audit-item__loop {
  font-family: var(--font-sans);
  font-size: var(--text-sm);
  color: var(--color-tool-shell);
  line-height: 1.4;
}

/* B9+ D4 (2026-07-13): user-triggered diff apply row.
   Uses the same emerald as `--color-tool-write` because the user
   just successfully wrote files; the line lists affected paths +
   +/- counts so the user can see at a glance what got applied. */
.audit-item__ui-diff {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-tool-write);
  line-height: 1.4;
  word-break: break-all;
}

/* F2 定时任务 (2026-08-28): scheduler fire lifecycle line.
   Accent-tinted mono — 与 leading clock icon 同色系(治理动作)。 */
.audit-item__scheduled {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-accent);
  line-height: 1.4;
  word-break: break-word;
}

.audit-item__raw {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  padding: 4px 6px;
  border-radius: var(--radius-sm);
  word-break: break-all;
  line-height: 1.4;
}
</style>
