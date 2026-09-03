// Tests for `EditFileCard.vue` — edit_file 专属卡片。
//
// 2026-09-02 聚焦错误折叠契约(此前该卡无专属测试文件)。
// 2026-09-03 改为单节点 CSS 折叠:错误文本常驻同一节点,点 toggle
// 只切 `--open` 类(单行省略 ↔ 全文),不增删 DOM:
//   1. 错误结果 → 默认折叠(无 --open),文本节点已在(单行省略态)。
//   2. 点 toggle → 只切类,子元素数量不变;再点收起,节点仍在。
// 审批接线(store mock)仿 ShellCard.test.ts;diff 行级渲染不在本
// 文件覆盖范围(纯 jsdiff 消费,无分支)。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: vi.fn(async () => () => {}),
  },
}));

import EditFileCard from "./EditFileCard.vue";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";

function makeCall(overrides: Partial<ToolCallInfo> = {}): ToolCallInfo {
  return {
    id: "tu-1",
    name: "edit_file",
    input: {
      path: "STRUCTURE.md",
      old_string: "line one\n",
      new_string: "line one\nline two\n",
    },
    ...overrides,
  };
}

function makeResult(overrides: Partial<ToolResultInfo> = {}): ToolResultInfo {
  return {
    toolUseId: "tu-1",
    content: "Successfully edited 'STRUCTURE.md'.",
    isError: false,
    ...overrides,
  };
}

describe("EditFileCard", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(true);
  });

  function mountCard(props: { call: ToolCallInfo; result?: ToolResultInfo }) {
    return mount(EditFileCard, {
      props,
      global: { stubs: { Icon: true } },
    });
  }

  describe("error collapse (2026-09-02, 09-03 单节点)", () => {
    const ERROR_TEXT =
      "old_string not found in 'STRUCTURE.md'. Read the file again.\nClosest match is (lines 889-891):\nline 890 details…";

    it("error result → 单节点 toggle,默认折叠(无 --open),全文已在 DOM", () => {
      const w = mountCard({
        call: makeCall(),
        result: makeResult({ content: ERROR_TEXT, isError: true }),
      });
      const banner = w.get(".edit-card__error");
      // toggle 在场,展开态为 false,无 --open 修饰类。
      const toggle = banner.get(".edit-card__error-toggle");
      expect(toggle.attributes("aria-expanded")).toBe("false");
      expect(toggle.classes()).not.toContain("edit-card__error-toggle--open");
      // 单节点:全文常驻同一节点,不再 v-if 额外 pre。
      const text = banner.get(".edit-card__error-text");
      expect(text.text()).toContain("old_string not found");
      expect(text.text()).toContain("Closest match");
      // 只有一个文本节点——展开不许多出一层 DOM。
      expect(banner.findAll(".edit-card__error-text").length).toBe(1);
    });

    it("clicking the toggle 只切 CSS 类,不增删 DOM 节点", async () => {
      const w = mountCard({
        call: makeCall(),
        result: makeResult({ content: ERROR_TEXT, isError: true }),
      });
      const banner = w.get(".edit-card__error");
      const childCountBefore = banner.element.childElementCount;
      const toggle = w.get(".edit-card__error-toggle");
      await toggle.trigger("click");
      expect(toggle.attributes("aria-expanded")).toBe("true");
      expect(toggle.classes()).toContain("edit-card__error-toggle--open");
      // 同一节点仍在,子元素数量不变——展开只是 CSS 变化。
      expect(w.find(".edit-card__error-text").exists()).toBe(true);
      expect(banner.element.childElementCount).toBe(childCountBefore);
      expect(banner.findAll(".edit-card__error-text").length).toBe(1);
      await toggle.trigger("click");
      expect(toggle.attributes("aria-expanded")).toBe("false");
      expect(toggle.classes()).not.toContain("edit-card__error-toggle--open");
      // 收起后文本节点仍在(只是 CSS 回到单行省略)。
      expect(w.find(".edit-card__error-text").exists()).toBe(true);
    });

    it("success result renders no error banner and no result text line", () => {
      // 2026-09-02:"Successfully edited …"结果文案行已移除——header
      // ✓ done + diff 视图已承载全部信号,该行是视觉噪音(guard 锁死)。
      const w = mountCard({
        call: makeCall(),
        result: makeResult(),
      });
      expect(w.find(".edit-card__error").exists()).toBe(false);
      expect(w.find(".edit-card__result").exists()).toBe(false);
      expect(w.text()).not.toContain("Successfully edited");
    });
  });
});
