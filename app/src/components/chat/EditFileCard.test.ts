// Tests for `EditFileCard.vue` — edit_file 专属卡片。
//
// 2026-09-02 聚焦错误折叠契约(此前该卡无专属测试文件):
//   1. 错误结果 → banner 折叠(默认只有一行摘要,全文 pre 不在)。
//   2. 摘要行 = 错误首行(扫描性:不展开也知道挂在哪个错上)。
//   3. 点 toggle → 全文展开;再点收起。
//   4. 成功结果 → 无 banner,且"Successfully edited"文案行不渲染
//      (2026-09-02 移除,header ✓ done + diff 已承载信号)。
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

  describe("error collapse (2026-09-02)", () => {
    const ERROR_TEXT =
      "old_string not found in 'STRUCTURE.md'. Read the file again.\nClosest match is (lines 889-891):\nline 890 details…";

    it("error result → banner collapsed by default, full text hidden", () => {
      const w = mountCard({
        call: makeCall(),
        result: makeResult({ content: ERROR_TEXT, isError: true }),
      });
      const banner = w.get(".edit-card__error");
      // toggle 在场,展开态为 false。
      const toggle = banner.get(".edit-card__error-toggle");
      expect(toggle.attributes("aria-expanded")).toBe("false");
      // 全文 pre 未渲染——错误全文不再常显。
      expect(banner.find(".edit-card__error-text").exists()).toBe(false);
    });

    it("collapsed summary shows the first error line only", () => {
      const w = mountCard({
        call: makeCall(),
        result: makeResult({ content: ERROR_TEXT, isError: true }),
      });
      const preview = w.get(".edit-card__error-preview");
      expect(preview.text()).toBe(
        "old_string not found in 'STRUCTURE.md'. Read the file again.",
      );
      // 首行以外的内容(第二行起)不进摘要。
      expect(preview.text()).not.toContain("Closest match");
    });

    it("clicking the toggle expands the full error text, clicking again collapses", async () => {
      const w = mountCard({
        call: makeCall(),
        result: makeResult({ content: ERROR_TEXT, isError: true }),
      });
      const toggle = w.get(".edit-card__error-toggle");
      await toggle.trigger("click");
      expect(toggle.attributes("aria-expanded")).toBe("true");
      const text = w.get(".edit-card__error-text");
      expect(text.text()).toContain("old_string not found");
      expect(text.text()).toContain("Closest match");
      await toggle.trigger("click");
      expect(w.find(".edit-card__error-text").exists()).toBe(false);
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
