// Tests for `AppDatePicker.vue` — 日历弹层日期选择 wrapper(reka
// DatePicker)。
//
// 契约(PRD):
//   1. 字符串 v-model:"yyyy-MM-dd" ⇄ DateValue 转换收敛在组件内。
//   2. 空值 = 触发按钮 placeholder 文案 + 弹层默认定位今天。
//   3. 真实路径:点触发按钮开弹层(portal 到 body),点今天格子 emit
//      今天(SearchModal/ScheduledTasksTab 的 teleport 断言同款)。
//   4. $attrs 转发到触发按钮。
//
// 纯组件;portal 内容查询走 document.querySelector,卸载清 body。

import { describe, it, expect, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { DatePickerRoot } from "reka-ui";
import { CalendarDate } from "@internationalized/date";
import AppDatePicker from "./AppDatePicker.vue";

function todayStr(): string {
  const d = new Date();
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

beforeEach(() => {
  // 前序用例异常退出时的 body 残留清理(teleport 内容不随 wrapper 卸载)。
  document.body.innerHTML = "";
});

describe("AppDatePicker", () => {
  it("空值:触发按钮显示 placeholder,弹层不渲染", () => {
    const w = mount(AppDatePicker, {
      props: { modelValue: "", placeholder: "选择结束日期" },
      attachTo: document.body,
    });
    expect(w.find(".adp__trigger-text").text()).toBe("选择结束日期");
    expect(
      w.find(".adp__trigger-text").classes(),
    ).toContain("adp__trigger-text--empty");
    expect(document.querySelector(".adp__pop")).toBeNull();
    w.unmount();
  });

  it("modelValue 回填:触发按钮显示 yyyy-MM-dd", () => {
    const w = mount(AppDatePicker, {
      props: { modelValue: "2026-08-30" },
    });
    expect(w.find(".adp__trigger-text").text()).toBe("2026-08-30");
    expect(
      w.find(".adp__trigger-text").classes(),
    ).not.toContain("adp__trigger-text--empty");
  });

  it("Root emit 转换:DateValue → 字符串;undefined → 空串", async () => {
    const w = mount(AppDatePicker, { props: { modelValue: "" } });
    await w
      .findComponent(DatePickerRoot)
      .vm.$emit("update:modelValue", new CalendarDate(2026, 8, 30));
    expect(w.emitted("update:modelValue")?.[0]?.[0]).toBe("2026-08-30");
    await w
      .findComponent(DatePickerRoot)
      .vm.$emit("update:modelValue", undefined);
    expect(w.emitted("update:modelValue")?.[1]?.[0]).toBe("");
  });

  it("真实路径:点触发开弹层,点今天格子 emit 今天", async () => {
    const w = mount(AppDatePicker, {
      props: { modelValue: "" },
      attachTo: document.body,
    });
    await w.find(".adp__trigger").trigger("click");
    await flushPromises();
    const pop = document.querySelector(".adp__pop");
    expect(pop).not.toBeNull();
    // 日历定位在当前月,今天格子带 data-today;点它选择今天。
    const todayCell = pop!.querySelector<HTMLButtonElement>(
      ".adp__day[data-today]",
    );
    expect(todayCell).not.toBeNull();
    todayCell!.click();
    await flushPromises();
    const emissions = w.emitted("update:modelValue");
    expect(emissions?.length).toBeGreaterThan(0);
    expect(emissions?.[emissions.length - 1]?.[0]).toBe(todayStr());
    w.unmount();
  });

  it("$attrs 转发:data-testid / aria-label 落到触发按钮", () => {
    const w = mount(AppDatePicker, {
      props: { modelValue: "" },
      attrs: { "data-testid": "sched-end-date", "aria-label": "结束日期" },
    });
    expect(w.find(".adp__trigger").attributes("data-testid")).toBe(
      "sched-end-date",
    );
    expect(w.find(".adp__trigger").attributes("aria-label")).toBe("结束日期");
  });
});
