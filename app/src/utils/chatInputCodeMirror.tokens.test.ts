// 08-26-f5-verify-followups P2 — 输入框 `@token` chip 化的 decoration
// 契约:tokenHighlightPlugin(chatInputTokens.ts,经 chatInputCodeMirror
// 的 extension 列表挂载)在 doc 变化后于 `.cm-content` 内产出带
// `cm-token-file` class 的 mark span,chip 样式(ChatInput.vue)挂在该
// class 上 —— 本文件钉住「DOM 里出现 chip class + 覆盖的文本正确」,
// 覆盖:
//   - 相对路径 token(`@foo.md`)与 `@/绝对路径` 形态(P2 扩展了
//     FILE_RE 首字符类,此前 `@/etc/hosts` 落不进 chip);
//   - 正在输入中的未完成 token(`@fo`)同色,不区分状态;
//   - 边界规则:邮箱 `name@host.com`、行中裸 `@` 不着色;
//   - 文档变更(panel 选词插入 / 清空)后全量重算正确。
//
// 复用 chatInputCodeMirror.files.test.ts 的 Host 挂载模式 + jsdom Range
// 补丁(该文件对本仓库 jsdom 缺 layout API 的说明同样适用)。

import { describe, it, expect } from "vitest";
import { defineComponent, h, ref } from "vue";
import { mount, type VueWrapper } from "@vue/test-utils";
import {
  useChatInputCodeMirror,
  type ChatInputCodeMirrorApi,
} from "./chatInputCodeMirror";

/** mount 是同步的,setup 运行后立即填充 —— 每个 test 挂载前置 null,
 *  防止跨用例串号。 */
let mountedApi: ChatInputCodeMirrorApi | null = null;

const Host = defineComponent({
  setup() {
    const host = ref<HTMLDivElement | null>(null);
    const api = useChatInputCodeMirror({
      host,
      sending: ref(false),
      placeholder: ref(undefined),
      onSubmit: () => {},
    });
    mountedApi = api;
    return () => h("div", { ref: host });
  },
});

function mountHost(): { wrapper: VueWrapper; api: ChatInputCodeMirrorApi } {
  mountedApi = null;
  const wrapper = mount(Host);
  expect(wrapper.element.querySelector(".cm-editor")).toBeTruthy();
  const api = mountedApi;
  expect(api).toBeTruthy();
  return { wrapper, api: api! };
}

// jsdom 的 Range 没有 layout API。本文件会 dispatch CM doc 变更
// (replaceDoc),触发 CM 的 rAF measure → clientRectsFor(textNode) →
// Range.getClientRects,在 jsdom 下抛 "not a function"。返回空列表即可
// 让 CM 优雅回退(rects.length != 1 → 默认字号路径);被测的 decoration
// 是纯 doc → DecorationSet 投影,不依赖任何 layout 结果。只在本文件内
// 打补丁,不动全局 setup。
{
  const rangeProto = Range.prototype as unknown as {
    getClientRects?: () => DOMRectList;
    getBoundingClientRect?: () => DOMRect;
  };
  if (!rangeProto.getClientRects) {
    rangeProto.getClientRects = () => [] as unknown as DOMRectList;
    rangeProto.getBoundingClientRect = () =>
      ({ width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, x: 0, y: 0 }) as DOMRect;
  }
}

/** 所有 chip mark 的文本列表(断言用)。限定在 `.cm-content` 内 ——
 *  钉住 mark 挂在编辑器正文档位而不是 panel 等外部 DOM。 */
function chipTexts(wrapper: VueWrapper): string[] {
  return Array.from(
    wrapper.element.querySelectorAll(".cm-content .cm-token-file"),
  ).map((el) => (el as HTMLElement).textContent ?? "");
}

describe("input chip decoration: cm-token-file marks (08-26-f5 P2)", () => {
  it("marks a relative-path @token after insertion (panel-select shape)", () => {
    const { wrapper, api } = mountHost();
    api.replaceDoc("看看 @foo.md 的内容");
    expect(chipTexts(wrapper)).toEqual(["@foo.md"]);
    wrapper.unmount();
  });

  it("marks CJK filenames (Unicode FILE_RE — JS \\w never matches CJK)", () => {
    const { wrapper, api } = mountHost();
    api.replaceDoc("看看 @台风智能体文档.docx 是什么");
    expect(chipTexts(wrapper)).toEqual(["@台风智能体文档.docx"]);
    wrapper.unmount();
  });

  it("marks the @/absolute-path form (system-root insert shape)", () => {
    const { wrapper, api } = mountHost();
    api.replaceDoc("参考 @/etc/hosts 配置", 15);
    expect(chipTexts(wrapper)).toEqual(["@/etc/hosts"]);
    wrapper.unmount();
  });

  it("colors an in-progress token the same way (no state distinction)", () => {
    const { wrapper, api } = mountHost();
    api.replaceDoc("@fo", 3);
    expect(chipTexts(wrapper)).toEqual(["@fo"]);
    wrapper.unmount();
  });

  it("marks multiple tokens in one doc", () => {
    const { wrapper, api } = mountHost();
    api.replaceDoc("@a.md 和 @b.pdf 对比", 15);
    expect(chipTexts(wrapper)).toEqual(["@a.md", "@b.pdf"]);
    wrapper.unmount();
  });

  it("does not mark emails or mid-word @ (boundary rule)", () => {
    const { wrapper, api } = mountHost();
    api.replaceDoc("发到 name@host.com 说明 a@b 的情况", 22);
    expect(chipTexts(wrapper)).toEqual([]);
    wrapper.unmount();
  });

  it("recomputes on doc change: insert adds the chip, clear removes it", () => {
    const { wrapper, api } = mountHost();
    api.replaceDoc("前言 @report.pdf 后记", 14);
    expect(chipTexts(wrapper)).toEqual(["@report.pdf"]);

    // 面板选词插入 = replaceDoc 整文替换;清空(发送后)同样走 doc 变更,
    // decoration 必须跟随重算,不留陈旧 mark。
    api.replaceDoc("没有引用了", 5);
    expect(chipTexts(wrapper)).toEqual([]);
    wrapper.unmount();
  });

  it("plain text without @ produces no marks", () => {
    const { wrapper, api } = mountHost();
    api.replaceDoc("普通一句话,没有任何引用", 12);
    expect(chipTexts(wrapper)).toEqual([]);
    wrapper.unmount();
  });
});
