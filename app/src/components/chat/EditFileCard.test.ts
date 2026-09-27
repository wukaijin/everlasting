// Tests for `EditFileCard.vue` — edit_file 专属卡片。
//
// 2026-09-02 聚焦错误折叠契约(此前该卡无专属测试文件)。
// 2026-09-03 改为单节点 CSS 折叠:错误文本常驻同一节点,点 toggle
// 只切 `--open` 类(单行省略 ↔ 全文),不增删 DOM:
//   1. 错误结果 → 默认折叠(无 --open),文本节点已在(单行省略态)。
//   2. 点 toggle → 只切类,子元素数量不变;再点收起,节点仍在。
// 审批接线(store mock)仿 ShellCard.test.ts。
// 2026-09-26 增补(diffview-enhance):行内 word-diff 片段、CRLF 归一
// 口径(无 \r 断言)、截断点落在 del/add run 之间的钉死用例。

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

  /** 展开 diff 体(默认收起)。 */
  async function mountExpanded(call: ToolCallInfo, result?: ToolResultInfo) {
    const w = mountCard({ call, result });
    await w.get(".edit-card__toggle").trigger("click");
    return w;
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

  // ------------------------------------------------------------------
  // 09-26-diffview-enhance:行内 word-diff + CRLF 归一 + 截断边界
  // ------------------------------------------------------------------

  describe("intra-line word diff (diffview-enhance)", () => {
    it("修改行渲染 edit-diff-mark 片段,ctx 行无片段,拼接还原", async () => {
      const w = await mountExpanded(
        makeCall({
          input: {
            path: "a.ts",
            old_string: "const value = compute(old);",
            new_string: "const value = compute(new);",
          },
        }),
      );
      const rows = w.findAll(".edit-diff-line");
      // diffLines 全量替换 → 1 del + 1 add。
      expect(rows.length).toBe(2);
      expect(rows[0]!.classes()).toContain("edit-diff-line--del");
      expect(rows[0]!.findAll(".edit-diff-mark--del").length).toBe(1);
      expect(rows[1]!.classes()).toContain("edit-diff-line--add");
      expect(rows[1]!.findAll(".edit-diff-mark--add").length).toBe(1);
      // 拼接还原(读原始 textContent,VTU text() 会 trim)。
      expect((rows[0]!.find(".edit-diff-line__text").element as HTMLElement).textContent).toBe(
        "const value = compute(old);",
      );
      expect((rows[1]!.find(".edit-diff-line__text").element as HTMLElement).textContent).toBe(
        "const value = compute(new);",
      );
    });

    it("CRLF 输入:行拆分点归一,渲染行无 \\r,截断启发式同口径不误报", async () => {
      // 两侧各 191 行(190 ctx + 尾行),和 382 ≤ 400 帽:若计行口径被
      // CRLF 干扰(把 "ctx N\r" 数成两行/漏行)此处就会翻。
      const ctxLines = Array.from({ length: 190 }, (_, i) => `ctx ${i}`).join("\r\n");
      const w = await mountExpanded(
        makeCall({
          input: {
            path: "crlf.txt",
            old_string: `${ctxLines}\r\nold tail\r\n`,
            new_string: `${ctxLines}\r\nnew tail\r\n`,
          },
        }),
      );
      for (const t of w.findAll(".edit-diff-line__text")) {
        expect((t.element as HTMLElement).textContent).not.toContain("\r");
      }
      expect(w.find(".edit-card__truncated").exists()).toBe(false);
    });

    it("钉死:截断点恰落在 del-run 与 add-run 之间 → 被截的 del-run 整行染色,无 mark", async () => {
      // 构造 rows = 398 ctx + 2 del + 2 add(共 402),slice(0,400) 恰把
      // add-run 截走:del-run 失去配对 → 必须整行染色。若未来把配对
      // pass 挪到截断前,这两行会带出 mark 片段,本用例即翻。
      const ctx = Array.from({ length: 398 }, (_, i) => `keep ${i}`);
      const oldStr = `${[...ctx, "del A", "del B"].join("\n")}\n`;
      const newStr = `${[...ctx, "add A", "add B"].join("\n")}\n`;
      const w = await mountExpanded(
        makeCall({ input: { path: "big.txt", old_string: oldStr, new_string: newStr } }),
      );
      const rows = w.findAll(".edit-diff-line");
      expect(rows.length).toBe(400); // MAX_ROWS 帽
      expect(w.find(".edit-card__truncated").exists()).toBe(true);
      // 末两行是保留的 del-run:无任何 mark(配对发生在截断后)。
      const tail = rows.slice(-2);
      for (const row of tail) {
        expect(row.classes()).toContain("edit-diff-line--del");
        expect(row.findAll(".edit-diff-mark").length).toBe(0);
      }
      // 全卡无 mark(add-run 已被截走,ctx 行本就无)。
      expect(w.findAll(".edit-diff-mark").length).toBe(0);
    });
  });
});
