// Tests for `RevertConfirmModal.vue` — N2 PR3(2026-09-20, task
// `09-20-n2-checkpoint-revert`)确认弹窗的评审重排清单契约:
//
//   1. foreign 警告区**仅非空渲染**(null / 空数组都不出现);
//   2. 确认按钮文案带还原文件数(「还原 N 个文件」);
//   3. 归属 badge 三态齐全,Unknown 中性(muted class 区分);
//   4. gitignore 双重不可见**常驻脚注**(任何 preview 态都在);
//   5. StalePreview 错误 → 内联提示 + 「重新预览」按钮(emit
//      repreview);SessionBusy → 明确文案、无恢复按钮;
//   6. confirm / cancel 意图 emit;无逐文件勾选控件(还原语义是
//      整树,不是文件挑选)。
//
// 组件是纯呈现(数据在父),单测直接以 props 驱动各态。

import { describe, it, expect, beforeEach } from "vitest";
import { mount, DOMWrapper, type VueWrapper } from "@vue/test-utils";

import RevertConfirmModal from "./RevertConfirmModal.vue";
import type { RevertPreview } from "../../stores/turnCheckpoints";

function preview(
  overrides: Partial<RevertPreview> = {},
): RevertPreview {
  return {
    files: [
      { path: "a.txt", action: "checkout", attribution: "tool_written" },
      { path: "sub/b.txt", action: "delete", attribution: "shell_write" },
      { path: "c.txt", action: "checkout", attribution: "unknown" },
    ],
    foreign_delta: null,
    target_seq: 2,
    target_created_at: 1_700_000_000_000,
    preview_token: "aa:bb",
    ...overrides,
  };
}

function mountModal(
  props: Partial<{
    open: boolean;
    preview: RevertPreview | null;
    loading: boolean;
    executing: boolean;
    error: string | null;
    errorKind: string | null;
  }> = {},
) {
  return mount(RevertConfirmModal, {
    props: {
      open: true,
      preview: preview(),
      loading: false,
      executing: false,
      error: null,
      errorKind: null,
      ...props,
    },
    global: { stubs: { Icon: true, Transition: true } },
    attachTo: document.body,
  });
}

// 2026-09-20 层级修复:弹窗 Teleport 到 body,wrapper.find 只扫组件
// 锚点看不到 portal 内容 —— 查询统一走 document.body(与 attachTo
// 挂载点同根,beforeEach 清 body 时一并清掉)。
const q = (_w: VueWrapper, sel: string) =>
  new DOMWrapper(document.body.querySelector(sel));
const exists = (_w: VueWrapper, sel: string) =>
  document.body.querySelector(sel) !== null;
const all = (_w: VueWrapper, sel: string) =>
  Array.from(document.body.querySelectorAll(sel)).map(
    (el) => new DOMWrapper(el),
  );

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("RevertConfirmModal — 还原集清单与归属 badge", () => {
  it("逐文件渲染 action + path + 归属 badge(tool/shell/unknown)", () => {
    const w = mountModal();
    const rows = all(w, "[data-testid='revert-file-row']");
    expect(rows).toHaveLength(3);

    expect(rows[0]!.text()).toContain("还原");
    expect(rows[0]!.text()).toContain("a.txt");
    expect(
      rows[0]!.find("[data-testid='revert-badge-tool_written']").exists(),
    ).toBe(true);

    expect(rows[1]!.text()).toContain("删除");
    expect(
      rows[1]!.find("[data-testid='revert-badge-shell_write']").exists(),
    ).toBe(true);

    // Unknown badge 在场且走中性 class(muted),与 tool/shell 区分。
    const unknownBadge = rows[2]!.find(
      "[data-testid='revert-badge-unknown']",
    );
    expect(unknownBadge.exists()).toBe(true);
    expect(unknownBadge.classes()).toContain("revert-file__badge--unknown");
    w.unmount();
  });

  it("无逐文件勾选控件(整树还原语义)", () => {
    const w = mountModal();
    expect(all(w, "input[type='checkbox']")).toHaveLength(0);
    w.unmount();
  });
});

describe("RevertConfirmModal — 评审重排清单", () => {
  it("foreign_delta 非空:警告区渲染,含「非本会话快照内变更」措辞与路径", () => {
    const w = mountModal({
      preview: preview({
        foreign_delta: [
          { path: "hand.txt", status: "modified", added: 1, removed: 1, diff_text: "" },
        ],
      }),
    });
    const warn = q(w, "[data-testid='revert-foreign-warning']");
    expect(warn.exists()).toBe(true);
    expect(warn.text()).toContain("非本会话快照内变更");
    expect(warn.text()).toContain("hand.txt");
    w.unmount();
  });

  it("foreign_delta 为 null / 空数组:警告区完全不渲染(仅非空渲染)", () => {
    for (const fd of [null, []]) {
      const w = mountModal({ preview: preview({ foreign_delta: fd }) });
      expect(exists(w, "[data-testid='revert-foreign-warning']")).toBe(false);
      w.unmount();
    }
  });

  it("确认按钮文案带还原文件数;确认/取消发意图", async () => {
    const w = mountModal();
    const btn = q(w, "[data-testid='revert-confirm-btn']");
    expect(btn.text()).toBe("还原 3 个文件");

    await btn.trigger("click");
    expect(w.emitted("confirm")).toHaveLength(1);

    await q(w, "[data-testid='revert-cancel-btn']").trigger("click");
    expect(w.emitted("cancel")).toHaveLength(1);
    w.unmount();
  });

  it("gitignore 双重不可见脚注常驻(有/无 foreign 都在)", () => {
    const w1 = mountModal();
    expect(q(w1, "[data-testid='revert-gitignore-note']").exists()).toBe(true);
    expect(q(w1, "[data-testid='revert-gitignore-note']").text()).toContain(
      ".gitignore",
    );
    w1.unmount();

    const w2 = mountModal({
      preview: preview({
        foreign_delta: [
          { path: "x", status: "modified", added: 1, removed: 0, diff_text: "" },
        ],
      }),
    });
    expect(q(w2, "[data-testid='revert-gitignore-note']").exists()).toBe(true);
    w2.unmount();
  });
});

describe("RevertConfirmModal — 错误内联", () => {
  it("StalePreview:内联提示 + 「重新预览」按钮,点击 emit repreview", async () => {
    const w = mountModal({
      preview: null,
      error: "预览后文件已再次变化,还原集已过期;请重新预览",
      errorKind: "StalePreview",
    });
    const err = q(w, "[data-testid='revert-error']");
    expect(err.exists()).toBe(true);

    const btn = q(w, "[data-testid='revert-repreview-btn']");
    expect(btn.exists()).toBe(true);
    await btn.trigger("click");
    expect(w.emitted("repreview")).toHaveLength(1);
    w.unmount();
  });

  it("SessionBusy:明确文案在场,无重新预览按钮", () => {
    const w = mountModal({
      preview: null,
      error: "会话正在运行中,请先停止当前轮次再回退",
      errorKind: "SessionBusy",
    });
    expect(q(w, "[data-testid='revert-error']").text()).toContain(
      "会话正在运行中",
    );
    expect(exists(w, "[data-testid='revert-repreview-btn']")).toBe(false);
    w.unmount();
  });

  it("错误态下确认按钮禁用(preview 不在场)", () => {
    const w = mountModal({
      preview: null,
      error: "boom",
      errorKind: null,
    });
    expect(
      (q(w, "[data-testid='revert-confirm-btn']").element as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    w.unmount();
  });
});
