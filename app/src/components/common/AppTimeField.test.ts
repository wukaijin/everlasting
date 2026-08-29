// Tests for `AppTimeField.vue` — 分段时间输入 wrapper(reka TimeField)。
//
// 契约(PRD):
//   1. 字符串 v-model:"HH:mm" ⇄ TimeValue 转换收敛在组件内。
//   2. 空值 = placeholder 灰字态(分段带 data-placeholder)。
//   3. 分段键入数字 → 所有段齐后 emit "HH:mm"(pad 两位)。
//   4. $attrs(data-testid / aria-label)转发到根元素。
//
// 纯组件,无 transport / store 依赖,直接 mount。

import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";
import AppTimeField from "./AppTimeField.vue";

const SEG = '[data-reka-time-field-segment]';
const hourSeg = '[data-reka-time-field-segment="hour"]';
const minuteSeg = '[data-reka-time-field-segment="minute"]';

describe("AppTimeField", () => {
  it("空值:渲染 hour/literal/minute 三段,占位破折号灰字态", () => {
    const w = mount(AppTimeField, { props: { modelValue: "" } });
    const parts = w.findAll(SEG).map((s) =>
      s.attributes("data-reka-time-field-segment"),
    );
    // zh-CN + 24h:恰好三段,无 dayPeriod。
    expect(parts).toEqual(["hour", "literal", "minute"]);
    // reka 空段显示占位破折号(不是占位时间数字)。
    expect(w.find(hourSeg).text()).toBe("––");
    expect(w.find(hourSeg).attributes("data-placeholder")).toBeDefined();
    expect(w.find(".atf").classes()).toContain("atf--empty");
  });

  it("modelValue 回填:08:30 → 分段显示 08/30,退出 placeholder 态", () => {
    const w = mount(AppTimeField, { props: { modelValue: "08:30" } });
    expect(w.find(hourSeg).text()).toBe("08");
    expect(w.find(minuteSeg).text()).toBe("30");
    expect(w.find(hourSeg).attributes("data-placeholder")).toBeUndefined();
    expect(w.find(".atf").classes()).not.toContain("atf--empty");
  });

  it("分段键入:hour 9 → minute 3,0 → emit 09:30(补零)", async () => {
    const w = mount(AppTimeField, { props: { modelValue: "" } });
    await w.find(hourSeg).trigger("keydown", { key: "9" });
    await w.find(minuteSeg).trigger("keydown", { key: "3" });
    await w.find(minuteSeg).trigger("keydown", { key: "0" });
    const emissions = w.emitted("update:modelValue");
    expect(emissions?.length).toBeGreaterThan(0);
    // 项目 tsconfig lib < es2022,无 Array.prototype.at。
    expect(emissions?.[emissions.length - 1]?.[0]).toBe("09:30");
  });

  it("非法 modelValue(越界/格式错)按未选处理,不抛错", () => {
    for (const bad of ["99:99", "ab:cd", "7点半"]) {
      const w = mount(AppTimeField, { props: { modelValue: bad } });
      expect(w.find(hourSeg).attributes("data-placeholder")).toBeDefined();
      w.unmount();
    }
  });

  it("$attrs 转发:data-testid / aria-label 落到根元素", () => {
    const w = mount(AppTimeField, {
      props: { modelValue: "" },
      attrs: { "data-testid": "sched-at-time", "aria-label": "触发时刻" },
    });
    expect(w.find(".atf").attributes("data-testid")).toBe("sched-at-time");
    expect(w.find(".atf").attributes("aria-label")).toBe("触发时刻");
  });
});
