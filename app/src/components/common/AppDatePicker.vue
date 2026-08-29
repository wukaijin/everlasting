<script setup lang="ts">
// AppDatePicker — 日期选择(触发按钮 + 日历弹层),包装 reka-ui 2.9.9
// DatePickerRoot 家族。对外暴露字符串 v-model("yyyy-MM-dd" | ""),
// DateValue 转换收敛在组件内部(消费方 ScheduledTasksTab 的
// form.onceDate / form.endDate 保持纯字符串模型)。
//
// - 弹层组合:DatePickerContent → DatePickerCalendar(内部即
//   CalendarRoot,2.9.9 slot 提供数组形 grid:Grid<DateValue>[])。
//   注意:**没有 DatePickerPortal 可用** —— 2.9.9 包根不导出该组件
//   (dist 里有文件但 import 得到 undefined,会渲染成 invalid vnode),
//   DatePickerContent 自带 portal。
// - locale 钉 "zh-CN":星期表头 / 标题(「2026年8月」)与中文 UI 一致,
//   也让 jsdom 测试确定化。
// - preventDeselect:再点已选日期不清空(清空走表单语义,不是误触点)。
// - minValue 挡今天之前的日期(once 过期 / 结束日期早于今天的表单校验
//   语义不变,这里只是前置拦截)。
// - inheritAttrs:false + $attrs 显式转发到触发按钮:父级 data-testid /
//   aria-label 落到 DOM(reka-ui-usage.md「wrapper 转发 data-*」)。
// - 弹层 portal 到 body:样式规则包 :deep()(reka-ui-usage.md gotcha);
//   z-index 取 --z-over-modal(settings modal 之上,与 Select 弹层一致)。
//   DatePickerContent 保持 position: static —— popper wrapper 已带
//   fixed 定位,内容再 fixed 会让 floating-ui 按 0×0 计算对齐(spec
//   2026-08-29 警告)。

import { computed } from "vue";
import {
  DatePickerRoot,
  DatePickerTrigger,
  DatePickerContent,
  DatePickerCalendar,
  DatePickerHeader,
  DatePickerHeading,
  DatePickerPrev,
  DatePickerNext,
  DatePickerGrid,
  DatePickerGridHead,
  DatePickerGridBody,
  DatePickerGridRow,
  DatePickerHeadCell,
  DatePickerCell,
  DatePickerCellTrigger,
} from "reka-ui";
import { CalendarDate } from "@internationalized/date";
import type { DateValue } from "@internationalized/date";
import Icon from "../Icon.vue";

const props = defineProps<{
  /** "yyyy-MM-dd" 或 ""(未选)。 */
  modelValue: string;
  /** 未选时触发按钮的占位文案。 */
  placeholder?: string;
  /** 可选的最早日期("yyyy-MM-dd");早于它的格子禁选。 */
  minValue?: string;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", v: string): void;
}>();

defineOptions({ inheritAttrs: false });

const pad = (n: number): string => n.toString().padStart(2, "0");

/** "yyyy-MM-dd" → CalendarDate(reka modelValue 类型);非法 → undefined。 */
function parseToDate(v: string): CalendarDate | undefined {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(v);
  if (!m) return undefined;
  const d = new CalendarDate(Number(m[1]), Number(m[2]), Number(m[3]));
  return d;
}

const dateValue = computed(() => parseToDate(props.modelValue));

const minValueDate = computed(() =>
  props.minValue ? parseToDate(props.minValue) : undefined,
);

/** 日历初始定位:已选日期优先,否则今天。 */
const fallbackDate = computed<CalendarDate>(() => {
  const now = new Date();
  return new CalendarDate(now.getFullYear(), now.getMonth() + 1, now.getDate());
});

function onPick(v: DateValue | undefined): void {
  if (!v) {
    emit("update:modelValue", "");
    return;
  }
  emit("update:modelValue", `${v.year}-${pad(v.month)}-${pad(v.day)}`);
}
</script>

<template>
  <DatePickerRoot
    :model-value="dateValue"
    :default-placeholder="dateValue ?? fallbackDate"
    :min-value="minValueDate"
    locale="zh-CN"
    :prevent-deselect="true"
    :close-on-select="true"
    @update:model-value="onPick"
  >
    <DatePickerTrigger v-bind="$attrs" class="adp__trigger">
      <span
        class="adp__trigger-text"
        :class="{ 'adp__trigger-text--empty': !modelValue }"
      >{{ modelValue || placeholder || "选择日期" }}</span>
      <Icon name="calendar" :size="12" icon-class="adp__trigger-icon" />
    </DatePickerTrigger>
    <DatePickerContent class="adp__pop" align="start" :side-offset="4">
      <DatePickerCalendar v-slot="{ weekDays, grid }">
        <DatePickerHeader class="adp__header">
          <DatePickerPrev class="adp__nav" aria-label="上个月">
            <Icon name="chevron-left" :size="12" />
          </DatePickerPrev>
          <DatePickerHeading class="adp__heading" />
          <DatePickerNext class="adp__nav" aria-label="下个月">
            <Icon name="chevron-right" :size="12" />
          </DatePickerNext>
        </DatePickerHeader>
        <DatePickerGrid
          v-for="g in grid"
          :key="g.value.toString()"
          class="adp__grid"
        >
          <DatePickerGridHead>
            <DatePickerGridRow class="adp__row">
              <DatePickerHeadCell
                v-for="day in weekDays"
                :key="day"
                class="adp__head-cell"
              >
                {{ day }}
              </DatePickerHeadCell>
            </DatePickerGridRow>
          </DatePickerGridHead>
          <DatePickerGridBody>
            <DatePickerGridRow
              v-for="weekDates in g.rows"
              :key="weekDates[0]!.toString()"
              class="adp__row"
            >
              <DatePickerCell
                v-for="date in weekDates"
                :key="date.toString()"
                :date="date"
                class="adp__cell"
              >
                <DatePickerCellTrigger
                  :day="date"
                  :month="g.value"
                  class="adp__day"
                />
              </DatePickerCell>
            </DatePickerGridRow>
          </DatePickerGridBody>
        </DatePickerGrid>
      </DatePickerCalendar>
    </DatePickerContent>
  </DatePickerRoot>
</template>

<style scoped>
/* 触发按钮:与表单 input / Select trigger 同款 token(镜像
   .sched-tab__trigger)。触发按钮在组件自身模板内(非 teleport),
   scoped 正常作用。 */
.adp__trigger {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  font-family: inherit;
  width: 100%;
  box-sizing: border-box;
  cursor: pointer;
  text-align: left;
  transition: border-color var(--duration-base) var(--ease-out);
}

.adp__trigger:hover {
  border-color: var(--color-accent-muted);
}

.adp__trigger[data-state="open"] {
  border-color: var(--color-accent);
}

.adp__trigger-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

.adp__trigger-text--empty {
  color: var(--color-text-muted);
}

.adp__trigger-icon {
  color: var(--color-text-muted);
  flex-shrink: 0;
}

</style>

<style>
/* ── 日历弹层(portal 到 body,非 scoped,见上方说明)────────────── */
.adp__pop {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: 8px;
  z-index: var(--z-over-modal) !important;
  animation: adp-pop-in var(--duration-base) var(--ease-out);
}

@keyframes adp-pop-in {
  from {
    opacity: 0;
    transform: translateY(-4px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

.adp__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 4px;
}

.adp__heading {
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.adp__nav {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--color-text-secondary);
  cursor: pointer;
}

.adp__nav:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.adp__grid {
  border-collapse: collapse;
}

.adp__row {
  display: flex;
}

.adp__head-cell {
  width: 30px;
  height: 24px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: var(--text-2xs);
  color: var(--color-text-muted);
  font-weight: var(--weight-medium);
  letter-spacing: 0.02em;
}

.adp__cell {
  width: 30px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  position: relative;
}

.adp__day {
  width: 24px;
  height: 24px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 0;
  padding: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--color-text-primary);
  font-size: var(--text-xs);
  font-family: inherit;
  font-variant-numeric: tabular-nums;
  cursor: pointer;
}

.adp__day:hover {
  background: var(--color-bg-elevated);
}

.adp__day[data-highlighted] {
  outline: none;
  background: var(--color-bg-elevated);
}

/* 已选日期:accent 实底。 */
.adp__day[data-selected] {
  background: var(--color-accent);
  color: var(--color-text-on-accent);
  font-weight: var(--weight-semibold);
}

/* 今天:未选中时用描边提示(选中时 accent 实底已足够)。 */
.adp__day[data-today]:not([data-selected]) {
  box-shadow: inset 0 0 0 1px var(--color-accent-muted);
  color: var(--color-accent-text);
}

/* 非当月格子 / 禁用(minValue 之前)弱化。 */
.adp__day[data-outside-view] {
  color: var(--color-text-muted);
  opacity: 0.55;
}

.adp__day[data-disabled] {
  color: var(--color-text-muted);
  opacity: 0.35;
  cursor: not-allowed;
  pointer-events: none;
}

/* reka 的 fixed popper 包裹层默认 z auto,会被 settings modal(z 2001)
   盖住(2026-08-29 截图实证弹层不可见);内容元素是 static,z-index
   无效,只能提包裹层。:has() 从内容反选。 */
body > div:has(> .adp__pop) {
  z-index: var(--z-over-modal);
}
</style>
