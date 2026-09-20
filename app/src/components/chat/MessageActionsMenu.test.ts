// Tests for `MessageActionsMenu.vue` — N2 PR2(2026-09-20, task
// `09-20-n2-checkpoint-revert`)「本轮 diff」入口的渲染门。
//
// 覆盖(implement PR2「入口条件渲染」面):
//   - turnDiffAvailable=true:菜单出现「本轮 diff」项(轮末 assistant
//     卡形态),点击发 turnDiff 意图;
//   - turnDiffAvailable=false(默认):不渲染该项 —— user 卡 / 无行 /
//     基线 / 破链 / 非 git session 在父层(MessageItem)都落到 false,
//     这里钉住 prop 驱动的可见性契约;
//   - 既有三项(编辑/重发/复制)不受新增项影响。
//
// reka DropdownMenuContent teleports to <body>:真实 trigger 点击打开,
// portal DOM 在 document.body 查(HiddenProjectsMenu.test.ts 先例),
// beforeEach 清泄漏。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";

vi.mock("../../transport", () => ({
  transport: {
    invoke: async () => null,
    listen: async () => () => {},
  },
}));

import MessageActionsMenu from "./MessageActionsMenu.vue";

function mountMenu(turnDiffAvailable: boolean) {
  return mount(MessageActionsMenu, {
    props: {
      messageSeq: 3,
      sessionId: "s1",
      content: "回答正文",
      role: "assistant" as const,
      isEditing: false,
      isStreaming: false,
      turnDiffAvailable,
    },
    attachTo: document.body,
    global: { stubs: { Icon: true } },
  });
}

async function openMenu(w: ReturnType<typeof mountMenu>) {
  const trigger = w.find("[data-testid='msg-actions-trigger']");
  await trigger.trigger("click");
  await flushPromises();
}

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
});

describe("MessageActionsMenu — 「本轮 diff」入口渲染门", () => {
  it("turnDiffAvailable=true:菜单渲染「本轮 diff」,点击发 turnDiff", async () => {
    const w = mountMenu(true);
    await openMenu(w);

    const item = document.body.querySelector<HTMLElement>(
      "[data-testid='msg-actions-turn-diff']",
    );
    expect(item).not.toBeNull();
    expect(item?.textContent).toContain("本轮 diff");

    // reka 的 select 走键盘/指针选中;DOM click 也能触发 select 处理链
    // (item 上绑定了 @select → onTurnDiff → emit)。
    item?.click();
    await flushPromises();
    await w.vm.$nextTick();
    expect(w.emitted("turnDiff")).toHaveLength(1);
    w.unmount();
  });

  it("turnDiffAvailable=false:不渲染该项(默认关,user 卡/无行/基线/破链全走这里)", async () => {
    const w = mountMenu(false);
    await openMenu(w);
    expect(
      document.body.querySelector("[data-testid='msg-actions-turn-diff']"),
    ).toBeNull();
    w.unmount();
  });

  it("默认 props(不传 turnDiffAvailable):同 false,零行为回归", async () => {
    const w = mount(MessageActionsMenu, {
      props: {
        messageSeq: 1,
        sessionId: "s1",
        content: "x",
        role: "user" as const,
        isEditing: false,
        isStreaming: false,
        // turnDiffAvailable 故意缺省(withDefaults 落 false)—— 钉住
        // 默认关闭契约。
      },
      attachTo: document.body,
      global: { stubs: { Icon: true } },
    });
    await openMenu(w);
    expect(
      document.body.querySelector("[data-testid='msg-actions-turn-diff']"),
    ).toBeNull();

    // 既有三项仍在(新增项不挤占旧菜单)。
    for (const id of [
      "msg-actions-edit",
      "msg-actions-resend",
      "msg-actions-copy",
    ]) {
      expect(
        document.body.querySelector(`[data-testid='${id}']`),
      ).not.toBeNull();
    }
    w.unmount();
  });
});

// ---------------------------------------------------------------------------
// N2 PR3(2026-09-20,同任务)— 「回到此轮后」入口渲染门
// ---------------------------------------------------------------------------

describe("MessageActionsMenu — 「回到此轮后」入口渲染门", () => {
  function mountMenu2(opts: {
    role?: "user" | "assistant";
    revertAvailable?: boolean;
  }) {
    return mount(MessageActionsMenu, {
      props: {
        messageSeq: 3,
        sessionId: "s1",
        content: "回答正文",
        role: opts.role ?? ("assistant" as const),
        isEditing: false,
        isStreaming: false,
        turnDiffAvailable: true,
        revertAvailable: opts.revertAvailable,
      },
      attachTo: document.body,
      global: { stubs: { Icon: true } },
    });
  }

  it("revertAvailable=true:菜单渲染「回到此轮后」,与「本轮 diff」同区,点击发 revert", async () => {
    const w = mountMenu2({ revertAvailable: true });
    await openMenu(w);

    const item = document.body.querySelector<HTMLElement>(
      "[data-testid='msg-actions-revert']",
    );
    expect(item).not.toBeNull();
    expect(item?.textContent).toContain("回到此轮后");
    // 与「本轮 diff」同入口区(diff 项也在场)。
    expect(
      document.body.querySelector("[data-testid='msg-actions-turn-diff']"),
    ).not.toBeNull();

    item?.click();
    await flushPromises();
    await w.vm.$nextTick();
    expect(w.emitted("revert")).toHaveLength(1);
    w.unmount();
  });

  it("revertAvailable 缺省/false:不渲染该项(默认关)", async () => {
    const w = mountMenu2({});
    await openMenu(w);
    expect(
      document.body.querySelector("[data-testid='msg-actions-revert']"),
    ).toBeNull();
    w.unmount();
  });
});
