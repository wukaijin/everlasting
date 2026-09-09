// DiscussionSummaryCard tests — C2 证据链结构化收官卡
// (09-09-gc-c2-evidence-summary)。
//
// 覆盖三组:
// ① 结构化渲染:input 带 conclusions/open_questions → stance 徽章 +
//    锚点行 + 开放问题段;summary 叙事仍渲染(两段共存);
// ② 校验记号叠加:validatedDetail(行级 JSON,锚点带 check)按
//    path+line 匹配 → ✓/⚠ 记号;无匹配键 = 未校验(data-check=
//    "unchecked",不显示记号 span);坏 JSON 降级不炸;
// ③ 兜底:input 无结构化参数(旧剧本/朴素收官)→ 纯文本渲染,
//    DOM 无 structured 区(旧场零回归)。

import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import DiscussionSummaryCard from "./DiscussionSummaryCard.vue";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";
import { createPinia, setActivePinia } from "pinia";

function makeCall(input: Record<string, unknown>): ToolCallInfo {
  return { id: "toolu_1", name: "end_discussion", input };
}

function makeResult(text: string): ToolResultInfo {
  return {
    toolUseId: "toolu_1",
    content: text,
    isError: false,
  };
}

const STRUCTURED_INPUT = {
  summary: "叙事:三方达成一致",
  conclusions: [
    {
      claim: "grep 相对 glob 已修",
      anchors: [{ path: "app/src-tauri/src/tools/grep.rs", line: 88 }],
      stance: "verified",
    },
    { claim: "压缩阈值可能误触发", stance: "inferred" },
    { claim: "沙盒粒度存在分歧", stance: "disputed" },
  ],
  open_questions: ["macOS 沙盒行为未验"],
};

const VALIDATED_DETAIL = JSON.stringify({
  conclusions: [
    {
      claim: "grep 相对 glob 已修",
      anchors: [
        { path: "app/src-tauri/src/tools/grep.rs", line: 88, check: "ok" },
      ],
      stance: "verified",
    },
    {
      claim: "断证条目",
      anchors: [{ path: "app/src/foo.rs", line: 42, check: "not_found" }],
      stance: "verified",
    },
  ],
  open_questions: [],
});

describe("DiscussionSummaryCard — C2 structured rendering", () => {
  it("renders stance badges, anchors and open questions from tool_use input", () => {
    setActivePinia(createPinia());
    const w = mount(DiscussionSummaryCard, {
      props: {
        call: makeCall(STRUCTURED_INPUT),
        result: makeResult("叙事:三方达成一致"),
      },
    });
    const structured = w.find('[data-testid="discussion-summary-conclusions"]');
    expect(structured.exists()).toBe(true);
    const stances = structured.findAll(".discussion-summary__stance");
    expect(stances.map((s) => s.text())).toEqual(["实证", "推测", "争议"]);
    expect(stances.map((s) => attributes(s)).join(",")).toContain("verified");
    // 锚点行:verified 条目挂锚点,inferred 无锚点无行。
    const anchors = structured.findAll(".discussion-summary__anchor");
    expect(anchors).toHaveLength(1);
    expect(anchors[0].text()).toContain("app/src-tauri/src/tools/grep.rs:88");
    // 开放问题段。
    expect(structured.text()).toContain("开放问题");
    expect(structured.text()).toContain("macOS 沙盒行为未验");
    // summary 叙事仍渲染(两段共存)。
    expect(w.find(".msg__markdown").exists()).toBe(true);
    // live 期未收官:无校验记号,data-check=unchecked。
    expect(anchors[0].attributes("data-check")).toBe("unchecked");
    expect(anchors[0].find(".discussion-summary__check").exists()).toBe(false);
  });

  it("merges validation marks from validatedDetail by path+line", () => {
    setActivePinia(createPinia());
    const w = mount(DiscussionSummaryCard, {
      props: {
        call: makeCall(STRUCTURED_INPUT),
        result: makeResult("叙事"),
        validatedDetail: VALIDATED_DETAIL,
      },
    });
    const anchors = w.findAll(".discussion-summary__anchor");
    expect(anchors).toHaveLength(1);
    // 匹配键(grep.rs:88)→ ok 记号。
    expect(anchors[0].attributes("data-check")).toBe("ok");
    const check = anchors[0].find(".discussion-summary__check");
    expect(check.exists()).toBe(true);
    expect(check.classes()).toContain("discussion-summary__check--ok");
    expect(check.text()).toContain("✓");
    // validatedDetail 里的断证锚点(app/foo.rs:42)不在 input 里,
    // 不产生额外渲染行(渲染源 = input,校验源只叠记号)。
  });

  it("marks broken anchors ⚠ when check is not ok", () => {
    setActivePinia(createPinia());
    const input = {
      summary: "s",
      conclusions: [
        {
          claim: "断证结论",
          anchors: [{ path: "app/src/foo.rs", line: 42 }],
          stance: "verified",
        },
      ],
      open_questions: [],
    };
    const w = mount(DiscussionSummaryCard, {
      props: { call: makeCall(input), result: makeResult("s"), validatedDetail: VALIDATED_DETAIL },
    });
    const anchor = w.find(".discussion-summary__anchor");
    expect(anchor.attributes("data-check")).toBe("not_found");
    const check = anchor.find(".discussion-summary__check");
    expect(check.classes()).toContain("discussion-summary__check--bad");
    expect(check.text()).toContain("⚠ 文件不存在");
  });

  it("degrades to text rendering when input has no structured params", () => {
    setActivePinia(createPinia());
    const w = mount(DiscussionSummaryCard, {
      props: {
        call: makeCall({ summary: "朴素收官" }),
        result: makeResult("朴素收官"),
      },
    });
    expect(w.find('[data-testid="discussion-summary-conclusions"]').exists()).toBe(false);
    expect(w.find(".msg__markdown").exists()).toBe(true);
  });

  it("tolerates garbage validatedDetail JSON without crashing", () => {
    setActivePinia(createPinia());
    const w = mount(DiscussionSummaryCard, {
      props: {
        call: makeCall(STRUCTURED_INPUT),
        result: makeResult("叙事"),
        validatedDetail: "{not json",
      },
    });
    const anchors = w.findAll(".discussion-summary__anchor");
    expect(anchors[0].attributes("data-check")).toBe("unchecked");
  });

  it("drops malformed conclusion entries without failing the card", () => {
    setActivePinia(createPinia());
    const input = {
      summary: "s",
      conclusions: [
        null,
        { claim: "   " },
        { anchors: [{ path: "x.rs" }] }, // 无 claim
        { claim: "好条目", stance: "bogus" },
      ],
      open_questions: [42, "合法问题"],
    };
    const w = mount(DiscussionSummaryCard, {
      props: { call: makeCall(input), result: makeResult("s") },
    });
    const structured = w.find('[data-testid="discussion-summary-conclusions"]');
    expect(structured.exists()).toBe(true);
    const stances = structured.findAll(".discussion-summary__stance");
    expect(stances).toHaveLength(1);
    // stance 非法值降级 inferred。
    expect(attributes(stances[0])).toContain("inferred");
    // open_questions 过滤非字符串项。
    expect(structured.text()).toContain("合法问题");
    expect(structured.text()).not.toContain("42");
  });

  it("shows pending placeholder before tool_result arrives", () => {
    setActivePinia(createPinia());
    const w = mount(DiscussionSummaryCard, {
      props: { call: makeCall(STRUCTURED_INPUT) },
    });
    expect(w.find(".discussion-summary__pending").exists()).toBe(true);
    expect(w.find('[data-testid="discussion-summary-conclusions"]').exists()).toBe(false);
  });
});

function attributes(wrapper: { attributes: () => Record<string, string> }): string {
  return JSON.stringify(wrapper.attributes());
}
