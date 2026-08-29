<script setup lang="ts">
// AppTimeField — 分段式时间输入(HH:mm),包装 reka-ui 2.9.9
// TimeFieldRoot/TimeFieldInput。对外暴露字符串 v-model("HH:mm" | ""),
// TimeValue 转换收敛在组件内部(消费方 ScheduledTasksTab 的 form.at /
// form.onceTime 保持纯字符串模型)。
//
// - 24 小时制:zh-CN locale 缺省 24h,但 zh 时间格式会多出一个前置
//   空白 literal 分段(dayPeriod 槽位残留,见 visibleSegments),故再传
//   hourCycle=24 关掉 hour12。reka 2.9.9 的 HourCycle 类型就是数字
//   12 | 24(normalizeHourCycle 只认数字,字符串会被静默忽略)。
// - 无值分段显示占位破折号(reka 内建),data-placeholder 灰字。
// - inheritAttrs:false + $attrs 显式转发到根元素:父级 data-testid /
//   aria-label 才能落到 DOM(reka-ui-usage.md「wrapper 转发 data-*」)。

import { computed } from "vue";
import { TimeFieldRoot, TimeFieldInput } from "reka-ui";
import { Time } from "@internationalized/date";

const props = defineProps<{
  /** "HH:mm"(24h)或 ""(未选)。 */
  modelValue: string;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", v: string): void;
}>();

defineOptions({ inheritAttrs: false });

const pad = (n: number): string => n.toString().padStart(2, "0");

/** "HH:mm" → Time(reka modelValue 类型);非法/空 → undefined。 */
function parseToTime(v: string): Time | undefined {
  const m = /^(\d{1,2}):(\d{2})$/.exec(v);
  if (!m) return undefined;
  const h = Number(m[1]);
  const min = Number(m[2]);
  if (h < 0 || h > 23 || min < 0 || min > 59) return undefined;
  return new Time(h, min);
}

const timeValue = computed(() => parseToTime(props.modelValue));

/** 空白 literal(zh-CN 时间格式 dayPeriod 槽位残留)不渲染。泛型
 *  保住 reka slot 的 segment 元素类型(part: SegmentPart)不降级。 */
function visibleSegments<T extends { part: string; value: string }>(
  segs: readonly T[],
): T[] {
  return segs.filter((s) => !(s.part === "literal" && s.value.trim() === ""));
}

/** reka emit 的 TimeValue(Time | CalendarDateTime | ZonedDateTime,reka
 *  内部类型,包根未导出)结构上都有 hour/minute,按结构收参即可。 */
function onTimeChange(v: { hour: number; minute: number } | undefined): void {
  if (!v) {
    emit("update:modelValue", "");
    return;
  }
  emit("update:modelValue", `${pad(v.hour)}:${pad(v.minute)}`);
}
</script>

<template>
  <TimeFieldRoot
    v-bind="$attrs"
    v-slot="{ segments }"
    class="atf"
    :class="{ 'atf--empty': !modelValue }"
    :model-value="timeValue"
    locale="zh-CN"
    :hour-cycle="24"
    granularity="minute"
    :disabled="disabled"
    @update:model-value="onTimeChange"
  >
    <TimeFieldInput
      v-for="segment in visibleSegments(segments)"
      :key="segment.part"
      :part="segment.part"
    >
      {{ segment.value }}
    </TimeFieldInput>
  </TimeFieldRoot>
</template>

<style scoped>
.atf {
  display: inline-flex;
  align-items: center;
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  font-variant-numeric: tabular-nums;
  width: 100%;
  box-sizing: border-box;
  cursor: text;
  transition: border-color var(--duration-base) var(--ease-out);
}

.atf:hover {
  border-color: var(--color-accent-muted);
}

.atf:focus-within {
  outline: none;
  border-color: var(--color-accent);
}

/* 未选值:分段灰字(placeholder 态,分段各带 data-placeholder)。 */
.atf :deep([data-reka-time-field-segment][data-placeholder]) {
  color: var(--color-text-muted);
}

/* 可编辑分段(hour/minute):聚焦高亮条。 */
.atf :deep([data-reka-time-field-segment]:not([data-reka-time-field-segment="literal"])) {
  padding: 0 2px;
  border-radius: var(--radius-sm);
  text-align: center;
  caret-color: transparent;
  transition: background var(--duration-fast) var(--ease-out);
}

.atf :deep([data-reka-time-field-segment]:not([data-reka-time-field-segment="literal"]):focus) {
  outline: none;
  background: color-mix(in srgb, var(--color-accent) 18%, transparent);
}

/* 字面量 ":" 弱化。 */
.atf :deep([data-reka-time-field-segment="literal"]) {
  color: var(--color-text-secondary);
  padding: 0 1px;
}

.atf[data-disabled] {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
