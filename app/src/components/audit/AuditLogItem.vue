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
  transition: background 0.1s;
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
  font-size: 11px;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.audit-item__kind {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 999px;
  border: 1px solid;
  font-family: var(--font-sans);
  font-weight: 500;
  white-space: nowrap;
  flex-shrink: 0;
}

.audit-item__tool {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--color-text-primary);
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  padding: 1px 6px;
  border-radius: 4px;
  white-space: nowrap;
}

.audit-item__input {
  font-family: var(--font-mono);
  font-size: 12px;
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
  font-size: 11px;
  color: var(--color-text-secondary);
}

.audit-item__exit {
  font-family: var(--font-mono);
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 4px;
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-app);
  color: var(--color-text-secondary);
}

.audit-item__exit--ok {
  color: var(--color-tool-write);
  border-color: color-mix(in srgb, var(--color-tool-write) 35%, transparent);
}

.audit-item__exit--fail {
  color: var(--color-tool-error);
  border-color: color-mix(in srgb, var(--color-tool-error) 35%, transparent);
}

.audit-item__reason {
  font-size: 12px;
  color: var(--color-tool-error);
  line-height: 1.4;
  word-break: break-word;
}

.audit-item__mode {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--color-tool-thinking);
}

.audit-item__raw {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--color-text-muted);
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  padding: 4px 6px;
  border-radius: 4px;
  word-break: break-all;
  line-height: 1.4;
}
</style>
