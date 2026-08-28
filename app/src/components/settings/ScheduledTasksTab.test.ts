// Tests for `ScheduledTasksTab.vue` — Settings「定时任务」tab(F2 WP2)。
//
// 契约(design §7 + implement.md 前端测试清单):
//   1. 挂载拉列表;卡片渲染名称 / schedule 人话 / 上次·下次触发;
//      停用行灰显。
//   2. 表单档位切换:daily↔interval↔weekly 的参数控件随 kind 切换,
//      提交的 schedule JSON 与档位一致。
//   3. 校验:空名 / 空 prompt / 未选 session → 表单内联错误,不发起 IPC。
//   4. 同 session 已有 enabled 任务 → 软警示(不硬拒,仍可提交)。
//   5. 列表启停交互:switch 点击 → update(enabled 取反)。
//   6. 删除走 ConfirmDialog 确认后才调 delete。
//
// transport / projects store mock(SearchTab.test.ts 同款);config
// store 用真 pinia(默认 scheduledTasksEnabled=true 不渲染 killwarn)。

import { describe, it, expect, beforeEach, beforeAll, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";
import { SelectRoot } from "reka-ui";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

const showToastMock = vi.fn();
vi.mock("../../stores/projects", () => ({
  useProjectsStore: () => ({
    showToast: showToastMock,
    projects: [
      { id: "p1", name: "alpha", path: "/tmp/alpha" },
      { id: "p2", name: "beta", path: "/tmp/beta" },
    ],
    currentProjectId: "p1",
    projectById: (id: string) =>
      ({ id, name: id === "p1" ? "alpha" : "beta" }) as never,
  }),
}));

import ScheduledTasksTab from "./ScheduledTasksTab.vue";
import type { ScheduledTask } from "../../stores/scheduledTasks";

function row(overrides: Partial<ScheduledTask> = {}): ScheduledTask {
  return {
    id: "task-1",
    project_id: "p1",
    target_session_id: "s1",
    name: "早报",
    prompt: "汇总昨日进展",
    schedule: { kind: "daily", at: "09:00" },
    enabled: true,
    created_at: 1_000,
    last_fired_at: null,
    next_fire_at: 4_000_000_000,
    ...overrides,
  };
}

/** list + per-project sessions 的缺省 stub。 */
function stubBackend(tasks: ScheduledTask[]) {
  invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "list_scheduled_tasks") return tasks;
    if (cmd === "list_sessions") {
      return [
        {
          id: "s1",
          title: args?.projectId === "p1" ? "旧会话" : "beta 会话",
          session_type: "chat",
        },
        { id: "s-group", title: "群聊", session_type: "group_chat" },
      ];
    }
    return null;
  });
}

async function mountTab() {
  const w = mount(ScheduledTasksTab, { global: { plugins: [createPinia()] } });
  await flushPromises();
  return w;
}

function openForm(w: ReturnType<typeof mount>) {
  return w.get('[data-testid="sched-form"]');
}

/** 经 SelectRoot 的 update:modelValue 事件选值(等价原 native
 * select.setValue —— 测 v-model 接线,不测弹层交互)。SelectRoot 是
 * renderless provider,按 DOM 序索引:0=project,1=session,2=kind,
 * 3=weekday(仅 weekly 渲染)。 */
async function pickSelect(
  form: ReturnType<typeof openForm>,
  index: number,
  value: string,
) {
  form.findAllComponents(SelectRoot)[index].vm.$emit("update:modelValue", value);
  await flushPromises();
}

beforeAll(() => {
  // jsdom 未实现 Pointer Capture API,reka SelectTrigger 的 pointerdown
  // handler 调 hasPointerCapture 会抛错;且 jsdom 合成的 pointerdown 事件
  // 没有 button 属性(=== 0 判定不过,打不开)。这里 stub 掉 capture API,
  // 打开路径改走 keydown(OPEN_KEYS 含 Enter,jsdom 键盘事件完整)。
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
});

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
  showToastMock.mockClear();
});

describe("ScheduledTasksTab 列表", () => {
  it("挂载拉列表,卡片渲染名称 / 人话档位 / 启停 switch", async () => {
    stubBackend([row()]);
    const w = await mountTab();
    expect(invokeMock).toHaveBeenCalledWith("list_scheduled_tasks");
    const card = w.get('[data-testid="sched-card-task-1"]');
    expect(card.text()).toContain("早报");
    expect(card.text()).toContain("每天 09:00");
    expect(card.text()).toContain("启用中");
    expect(w.find('[data-testid="sched-toggle-task-1"]').exists()).toBe(true);
  });

  it("停用行灰显且状态标「已停用」", async () => {
    stubBackend([row({ enabled: false })]);
    const w = await mountTab();
    const card = w.get('[data-testid="sched-card-task-1"]');
    expect(card.classes()).toContain("sched-tab__card--disabled");
    expect(card.text()).toContain("已停用");
  });

  it("switch 点击 → update enabled 取反", async () => {
    stubBackend([row()]);
    const w = await mountTab();
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task") return row({ enabled: false });
      if (cmd === "list_scheduled_tasks") return [row({ enabled: false })];
      return null;
    });
    await w.get('[data-testid="sched-toggle-task-1"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("update_scheduled_task", {
      id: "task-1",
      enabled: false,
    });
  });

  it("删除需 ConfirmDialog 确认后才调 delete", async () => {
    stubBackend([row()]);
    const w = await mountTab();
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "delete_scheduled_task") return true;
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    await w.get('[data-testid="sched-delete-task-1"]').trigger("click");
    await flushPromises();
    expect(invokeMock).not.toHaveBeenCalledWith("delete_scheduled_task", expect.anything());
    // ConfirmDialog 渲染后点确认(精确匹配 btn--danger,避免命中卡片
    // 的 sched-tab__card-btn--danger 后缀)。
    const confirmBtn = w
      .findAll("button")
      .find(
        (b) =>
          b.text() === "删除" &&
          b.classes().some((c) => c === "btn--danger"),
      );
    expect(confirmBtn).toBeTruthy();
    await confirmBtn!.trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("delete_scheduled_task", { id: "task-1" });
  });
});

describe("ScheduledTasksTab 表单", () => {
  it("档位切换:daily → interval 参数控件变化,提交 JSON 与档位一致", async () => {
    stubBackend([]);
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);

    // 默认 daily:存在 time 输入;切到 interval:出现 number 输入。
    // SelectRoot DOM 序:0=project → 1=session → 2=kind → 参数控件 →
    // prompt textarea(weekday=3 仅 weekly 渲染)。
    await pickSelect(form, 2, "interval");
    await form.find('[data-testid="sched-every-min"]').setValue("45");

    await form.find("input[type='text']").setValue("巡检");
    await pickSelect(form, 0, "p1");
    await pickSelect(form, 1, "s1");
    await form.find("textarea").setValue("跑一遍测试");

    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "create_scheduled_task") return row({ id: "new-1" });
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();

    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "create_scheduled_task",
    );
    expect(call).toBeTruthy();
    expect(call?.[1].schedule).toBe('{"kind":"interval","every_min":45}');
  });

  it("校验:未选 session(未勾专用)→ 内联错误,不发起 create", async () => {
    stubBackend([]);
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    await form.find("input[type='text']").setValue("巡检");
    await form.find("textarea").setValue("p");
    invokeMock.mockClear();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    expect(w.find(".sched-tab__error").text()).toContain("session");
    expect(invokeMock).not.toHaveBeenCalledWith(
      "create_scheduled_task",
      expect.anything(),
    );
  });

  it("同 session 已有 enabled 任务 → 软警示渲染但可提交(不硬拒)", async () => {
    stubBackend([row()]); // s1 已有 enabled 任务「早报」
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    await form.find("input[type='text']").setValue("第二单");
    await pickSelect(form, 1, "s1");
    await form.find("textarea").setValue("p");
    await flushPromises();
    const warn = w.find('[data-testid="sched-soft-warning"]');
    expect(warn.exists()).toBe(true);
    expect(warn.text()).toContain("早报");
    // 提交仍被放行(软警示语义)。
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "create_scheduled_task") return row({ id: "new-2" });
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith(
      "create_scheduled_task",
      expect.objectContaining({ targetSessionId: "s1", name: "第二单" }),
    );
  });

  it("session 下拉只含 classic(群聊被过滤)", async () => {
    stubBackend([]);
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    await flushPromises();
    const form = openForm(w);
    // 真实 UI 路径:键盘打开 session 下拉(Enter ∈ reka OPEN_KEYS),
    // SelectContent teleport 到 document.body,断言弹层 option 只有
    // classic session(群聊被过滤)。
    await form.find('[data-testid="sched-session-select"]').trigger("keydown", { key: "Enter" });
    await flushPromises();
    const items = Array.from(document.querySelectorAll('[role="option"]')).map(
      (el) => el.textContent?.trim() ?? "",
    );
    expect(items).toEqual(["旧会话"]);
    // 卸载清掉 teleport 到 body 的弹层,避免污染后续用例的 DOM 查询。
    w.unmount();
  });

  it("编辑回填:openEdit 预填表单并以 update 提交", async () => {
    stubBackend([
      row({ schedule: { kind: "weekly", weekday: "fri", at: "18:30" } }),
    ]);
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-task-1"]').trigger("click");
    const form = openForm(w);
    expect((form.find("input[type='text']").element as HTMLInputElement).value).toBe("早报");
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task") return row();
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith(
      "update_scheduled_task",
      expect.objectContaining({
        id: "task-1",
        schedule: '{"kind":"weekly","weekday":"fri","at":"18:30"}',
      }),
    );
  });
});
