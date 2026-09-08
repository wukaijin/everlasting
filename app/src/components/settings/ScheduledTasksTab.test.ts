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
import { defineComponent, h } from "vue";
import { setActivePinia, createPinia } from "pinia";
import { SelectRoot, RadioGroupRoot } from "reka-ui";

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
    target_mode: "fixed",
    model_id: null,
    last_run_session_id: null,
    name: "早报",
    prompt: "汇总昨日进展",
    schedule: { kind: "daily", at: "09:00" },
    enabled: true,
    created_by: "user",
    created_at: 1_000,
    last_fired_at: null,
    next_fire_at: 4_000_000_000,
    run_count: 0,
    max_runs: null,
    ends_at: null,
    group_chat_config: null,
    last_fire_outcome: null,
    ...overrides,
  };
}

/** M4a group_chat 行的存档展开配置(形状 = Rust GroupChatTaskConfig)。 */
function gcConfig(): NonNullable<ScheduledTask["group_chat_config"]> {
  return {
    moderator_model_id: "uuid-mini",
    participants: [
      { name: "架构", model_id: "uuid-glm", persona_md: "架构视角" },
      { name: "后端", model_id: "uuid-ds", persona_md: "后端视角" },
    ],
  };
}

/** M4a 模型目录 stub:preset JSON 里的名字(MiniMax-M3 / glm-5.3 /
 *  GLM-5.3-Flash / deepseek-v4-flash)可解析成 UUID。 */
const GC_MODELS = [
  {
    id: "uuid-mini",
    providerId: "prov-1",
    providerDisplayName: "MiniMax",
    displayName: "MiniMax-M3",
    modelName: "MiniMax-M3",
  },
  {
    id: "uuid-glm",
    providerId: "prov-2",
    providerDisplayName: "Zhipu",
    displayName: "GLM-5.3",
    modelName: "glm-5.3",
  },
  {
    id: "uuid-flash",
    providerId: "prov-2",
    providerDisplayName: "Zhipu",
    displayName: "GLM-5.3-Flash",
    modelName: "GLM-5.3-Flash",
  },
  {
    id: "uuid-ds",
    providerId: "prov-3",
    providerDisplayName: "DeepSeek",
    displayName: "DeepSeek V4",
    modelName: "deepseek-v4-flash",
  },
];

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
    // 模型下拉数据源(组件挂载即拉;空列表 = 下拉无可选项)。
    if (cmd === "list_models") return [];
    if (cmd === "get_default_model") return null;
    return null;
  });
}

/** M4a group_chat 用 stub:模型目录 = GC_MODELS(preset 名字可解析);
 *  sessions 里带一个 group_chat 场(最近场联查展示用)。 */
function stubBackendGc(tasks: ScheduledTask[]) {
  invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "list_scheduled_tasks") return tasks;
    if (cmd === "list_sessions") {
      return [
        {
          id: "s1",
          title: args?.projectId === "p1" ? "旧会话" : "beta 会话",
          session_type: "chat",
        },
        { id: "s-gc-run", title: "上一场审议", session_type: "group_chat" },
      ];
    }
    if (cmd === "list_models") return GC_MODELS;
    if (cmd === "get_default_model") return null;
    return null;
  });
}

/** AppDatePicker / AppTimeField 的测试替身:渲染成普通 text input,
 *  走同一字符串 v-model 契约($attrs 里的 data-testid / class 落到
 *  input)。表单接线 / 校验 / 提交 payload 的断言不依赖 reka 弹层;
 *  两个包装组件自身的转换逻辑有专门单测(AppDatePicker.test.ts /
 *  AppTimeField.test.ts)。 */
function stringFieldStub() {
  return defineComponent({
    inheritAttrs: false,
    props: { modelValue: { type: String, default: "" } },
    emits: ["update:modelValue"],
    setup(props, { emit, attrs }) {
      return () =>
        h("input", {
          ...attrs,
          type: "text",
          value: props.modelValue ?? "",
          onInput: (e: Event) =>
            emit("update:modelValue", (e.target as HTMLInputElement).value),
        });
    },
  });
}

async function mountTab() {
  const w = mount(ScheduledTasksTab, {
    global: {
      plugins: [createPinia()],
      stubs: {
        AppDatePicker: stringFieldStub(),
        AppTimeField: stringFieldStub(),
      },
    },
  });
  await flushPromises();
  return w;
}

function openForm(w: ReturnType<typeof mount>) {
  return w.get('[data-testid="sched-form"]');
}

/** 经 SelectRoot 的 update:modelValue 事件选值(等价原 native
 * select.setValue —— 测 v-model 接线,不测弹层交互)。SelectRoot 是
 * renderless provider,按 DOM 序索引:0=project,1=session(仅指定档),
 * 2=kind,3=weekday(仅 weekly 渲染);专用/每次新建档 session 隐藏,
 * model 顶到 1。 */
async function pickSelect(
  form: ReturnType<typeof openForm>,
  index: number,
  value: string,
) {
  form.findAllComponents(SelectRoot)[index].vm.$emit("update:modelValue", value);
  await flushPromises();
}

/** 目标档 radio(RadioGroupRoot 全表单唯一):emit update:modelValue,
 *  与 pickSelect 同款接线测法(jsdom 点 label 的转发不可靠)。 */
async function pickTargetMode(
  form: ReturnType<typeof openForm>,
  mode: "existing" | "dedicated" | "per_run" | "group_chat",
) {
  form.getComponent(RadioGroupRoot).vm.$emit("update:modelValue", mode);
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

  it("agent 来源徽标:created_by='agent' 渲染,user 不渲染(08-29-schedule-task-tool)", async () => {
    stubBackend([row(), row({ id: "task-2", name: "agent 排的", created_by: "agent" })]);
    const w = await mountTab();
    const userCard = w.get('[data-testid="sched-card-task-1"]');
    const agentCard = w.get('[data-testid="sched-card-task-2"]');
    expect(userCard.find(".sched-tab__card-origin").exists()).toBe(false);
    expect(agentCard.find(".sched-tab__card-origin").exists()).toBe(true);
    expect(agentCard.find(".sched-tab__card-origin").text()).toBe("agent");
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

    // 默认 daily:存在 time 输入;切到 interval:出现数量输入 + 单位下拉。
    // SelectRoot DOM 序:0=project → 1=session → 2=kind → 3=interval
    // 单位(仅 interval 渲染;weekly 时 3=weekday)。
    await pickSelect(form, 2, "interval");
    await form.find('[data-testid="sched-interval-count"]').setValue("45");

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
        // F2b:update 显式带结束条件(null = 清空,表单模型单一条件)。
        maxRuns: null,
        endsAt: null,
      }),
    );
  });
});

describe("ScheduledTasksTab F2b 调度扩展", () => {
  /** F2b 共用:打开表单、填基础字段(默认 daily)。 */
  async function openFilledForm() {
    stubBackend([]);
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    await form.find("input[type='text']").setValue("扩展档");
    await pickSelect(form, 0, "p1");
    await pickSelect(form, 1, "s1");
    await form.find("textarea").setValue("p");
    return { w, form };
  }

  function stubCreate() {
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "create_scheduled_task") return row({ id: "new-x" });
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
  }

  it("hourly 档:分钟输入,提交 {kind:hourly,minute}", async () => {
    const { w, form } = await openFilledForm();
    await pickSelect(form, 2, "hourly");
    await form.find('[data-testid="sched-hourly-minute"]').setValue("20");
    stubCreate();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call?.[1].schedule).toBe('{"kind":"hourly","minute":20}');
  });

  it("monthly 档:几号 + 时分,提交 {kind:monthly,day,at}", async () => {
    const { w, form } = await openFilledForm();
    await pickSelect(form, 2, "monthly");
    await form.find('[data-testid="sched-monthly-day"]').setValue("15");
    await form.find('[data-testid="sched-at-time"]').setValue("08:30");
    stubCreate();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call?.[1].schedule).toBe('{"kind":"monthly","day":15,"at":"08:30"}');
  });

  it("固定频率单位换算:2 小时 → every_min 120;编辑回填 1440 → 1 天", async () => {
    const { w, form } = await openFilledForm();
    await pickSelect(form, 2, "interval");
    await form.find('[data-testid="sched-interval-count"]').setValue("2");
    await pickSelect(form, 3, "hour");
    stubCreate();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call?.[1].schedule).toBe('{"kind":"interval","every_min":120}');

    // 编辑回填:every_min 1440 → 数量 1 + 单位 day。
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_scheduled_tasks")
        return [row({ schedule: { kind: "interval", every_min: 1440 } })];
      if (cmd === "list_sessions") return [];
      return null;
    });
    const w2 = await mountTab();
    await w2.get('[data-testid="sched-edit-task-1"]').trigger("click");
    await flushPromises();
    const form2 = openForm(w2);
    expect(
      (form2.find('[data-testid="sched-interval-count"]').element as HTMLInputElement).value,
    ).toBe("1");
    // 单位下拉的 model 值经 SelectRoot 组件树断言(第 4 个,kind 之后)。
    expect(form2.findAllComponents(SelectRoot)[3].props("modelValue")).toBe("day");
  });

  it("结束条件(固定时间):限定次数 → create 带 maxRuns", async () => {
    const { w, form } = await openFilledForm();
    await form.find('input[name="sched-end"][value="count"]').setValue(true);
    await form.find('[data-testid="sched-max-runs"]').setValue("5");
    stubCreate();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call?.[1].maxRuns).toBe(5);
    expect(call?.[1].endsAt).toBeUndefined();
  });

  it("结束条件(固定频率):结束日期 → create 带 endsAt(当日 23:59:59.999)", async () => {
    const { w, form } = await openFilledForm();
    await pickSelect(form, 2, "interval");
    const tomorrow = new Date(Date.now() + 86_400_000);
    const pad = (n: number) => n.toString().padStart(2, "0");
    const dateStr = `${tomorrow.getFullYear()}-${pad(tomorrow.getMonth() + 1)}-${pad(tomorrow.getDate())}`;
    await form.find('input[name="sched-end"][value="date"]').setValue(true);
    await form.find('[data-testid="sched-end-date"]').setValue(dateStr);
    stubCreate();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    const expected = new Date(
      tomorrow.getFullYear(),
      tomorrow.getMonth(),
      tomorrow.getDate(),
      23, 59, 59, 999,
    ).getTime();
    expect(call?.[1].endsAt).toBe(expected);
    expect(call?.[1].maxRuns).toBeUndefined();
  });

  it("校验:次数上限 0 / 过去日期 → 内联错误,不发起 create", async () => {
    const { w, form } = await openFilledForm();
    // 固定时间(daily)+ 次数 0。
    await form.find('input[name="sched-end"][value="count"]').setValue(true);
    await form.find('[data-testid="sched-max-runs"]').setValue("0");
    invokeMock.mockClear();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    expect(w.find(".sched-tab__error").text()).toContain("次数上限");
    expect(invokeMock).not.toHaveBeenCalledWith("create_scheduled_task", expect.anything());

    // 固定频率 + 昨天日期。
    await pickSelect(form, 2, "interval");
    const yesterday = new Date(Date.now() - 86_400_000);
    const pad = (n: number) => n.toString().padStart(2, "0");
    const dateStr = `${yesterday.getFullYear()}-${pad(yesterday.getMonth() + 1)}-${pad(yesterday.getDate())}`;
    await form.find('input[name="sched-end"][value="date"]').setValue(true);
    await form.find('[data-testid="sched-end-date"]').setValue(dateStr);
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    expect(w.find(".sched-tab__error").text()).toContain("结束日期");
    expect(invokeMock).not.toHaveBeenCalledWith("create_scheduled_task", expect.anything());
  });

  it("卡片:达限完成显示「已完成」+ 进度行;过期结束显示「已结束」", async () => {
    stubBackend([
      row({ enabled: false, run_count: 3, max_runs: 3 }),
      row({
        id: "task-2",
        enabled: false,
        run_count: 5,
        max_runs: null,
        ends_at: Date.now() - 1_000,
      }),
    ]);
    const w = await mountTab();
    const card1 = w.get('[data-testid="sched-card-task-1"]');
    expect(card1.text()).toContain("已完成");
    expect(card1.text()).toContain("已触发 3/3 次");
    const card2 = w.get('[data-testid="sched-card-task-2"]');
    expect(card2.text()).toContain("已结束");
    expect(card2.text()).toContain("已触发 5 次");
  });
});

describe("ScheduledTasksTab 单次档与模型指定(CH11-1)", () => {
  async function openFilledForm() {
    stubBackend([]);
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    await form.find("input[type='text']").setValue("单次任务");
    await pickSelect(form, 0, "p1");
    await pickSelect(form, 1, "s1");
    await form.find("textarea").setValue("p");
    return { w, form };
  }

  function stubCreate() {
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "create_scheduled_task") return row({ id: "new-x" });
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
  }

  /** 明天本地 20:30 的日期/时刻字符串 + 对应 epoch ms。 */
  function tomorrowEvening(): { date: string; time: string; ms: number } {
    const d = new Date(Date.now() + 86_400_000);
    d.setDate(d.getDate() + 1);
    d.setHours(20, 30, 0, 0);
    const pad = (n: number) => n.toString().padStart(2, "0");
    return {
      date: `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`,
      time: "20:30",
      ms: d.getTime(),
    };
  }

  it("单次档:日期 + 时刻控件,提交 {kind:once,at_ms} 且不带结束条件", async () => {
    const { w, form } = await openFilledForm();
    await pickSelect(form, 2, "once");
    // 单次档不渲染结束条件块(无意义:唯一触发点即终点)。
    expect(form.find('input[name="sched-end"]').exists()).toBe(false);
    const t = tomorrowEvening();
    await form.find('[data-testid="sched-once-date"]').setValue(t.date);
    await form.find('[data-testid="sched-once-time"]').setValue(t.time);
    stubCreate();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call?.[1].schedule).toBe(JSON.stringify({ kind: "once", at_ms: t.ms }));
    expect(call?.[1].maxRuns).toBeUndefined();
    expect(call?.[1].endsAt).toBeUndefined();
  });

  it("单次档校验:未选时间 / 过去时间 → 内联错误,不发起 create", async () => {
    const { w, form } = await openFilledForm();
    await pickSelect(form, 2, "once");
    invokeMock.mockClear();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    expect(w.find(".sched-tab__error").text()).toContain("请选择单次触发的时间");
    expect(invokeMock).not.toHaveBeenCalledWith("create_scheduled_task", expect.anything());

    const yesterday = new Date(Date.now() - 86_400_000);
    const pad = (n: number) => n.toString().padStart(2, "0");
    const pastDate = `${yesterday.getFullYear()}-${pad(yesterday.getMonth() + 1)}-${pad(yesterday.getDate())}`;
    await form.find('[data-testid="sched-once-date"]').setValue(pastDate);
    await form.find('[data-testid="sched-once-time"]').setValue("09:00");
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    expect(w.find(".sched-tab__error").text()).toContain("晚于当前时间");
    expect(invokeMock).not.toHaveBeenCalledWith("create_scheduled_task", expect.anything());
  });

  it("新建专用 session 档:radio 切换 → 模型下拉出现,选中的 modelId 进 create args", async () => {
    stubBackend([]);
    // 模型目录:onPickModel 只接受 catalog 中的 id,须返回真实条目。
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_scheduled_tasks") return [];
      if (cmd === "list_sessions") {
        return [
          { id: "s1", title: "旧会话", session_type: "chat" },
        ];
      }
      if (cmd === "list_models")
        return [
          {
            id: "model-7",
            providerId: "prov-1",
            providerDisplayName: "Acme",
            displayName: "GPT-X",
            modelName: "gpt-x",
          },
        ];
      if (cmd === "get_default_model") return null;
      return null;
    });
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    await form.find("input[type='text']").setValue("专用");
    await pickSelect(form, 0, "p1");
    await form.find("textarea").setValue("p");
    // 默认「指定 session」档:模型下拉不渲染;三张 radio 卡齐全(创建态)。
    expect(form.find('[data-testid="sched-model-select"]').exists()).toBe(false);
    expect(form.find('[data-testid="sched-target-existing"]').exists()).toBe(true);
    expect(form.find('[data-testid="sched-target-dedicated"]').exists()).toBe(true);
    expect(form.find('[data-testid="sched-target-per_run"]').exists()).toBe(true);
    await pickTargetMode(form, "dedicated");
    expect(form.find('[data-testid="sched-model-select"]').exists()).toBe(true);
    // 专用档 SelectRoot 序:0=project,1=model(session 下拉已隐藏)。
    await pickSelect(form, 1, "model-7");
    stubCreate();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call?.[1].modelId).toBe("model-7");
    expect(call?.[1].targetSessionId).toBeUndefined();
    expect(call?.[1].targetMode).toBeUndefined();
  });

  it("每次新建 session 档:create 带 targetMode=per_run 且不带 targetSessionId", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_scheduled_tasks") return [];
      if (cmd === "list_sessions") {
        return [{ id: "s1", title: "旧会话", session_type: "chat" }];
      }
      if (cmd === "list_models") return [];
      if (cmd === "get_default_model") return null;
      return null;
    });
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    await form.find("input[type='text']").setValue("每跑");
    await pickSelect(form, 0, "p1");
    await form.find("textarea").setValue("p");
    await pickTargetMode(form, "per_run");
    // session 下拉已隐藏:未选 session 也能提交(无需固定目标)。
    expect(form.find('[data-testid="sched-session-select"]').exists()).toBe(false);
    stubCreate();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call?.[1].targetMode).toBe("per_run");
    expect(call?.[1].targetSessionId).toBeUndefined();
  });

  it("per_run 卡片:meta 显示「每次新建 session」,能解析时带最近 run session", async () => {
    stubBackend([
      row({
        id: "per-1",
        target_mode: "per_run",
        target_session_id: null,
        last_run_session_id: null,
      }),
      row({
        id: "per-2",
        target_mode: "per_run",
        target_session_id: null,
        last_run_session_id: "s1",
      }),
    ]);
    const w = await mountTab();
    const card1 = w.get('[data-testid="sched-card-per-1"]');
    expect(card1.text()).toContain("每次新建 session");
    expect(card1.text()).not.toContain("最近:");
    const card2 = w.get('[data-testid="sched-card-per-2"]');
    expect(card2.text()).toContain("最近:旧会话");
  });

  it("per_run 编辑:回填该档 + 模型绑定;update 提交 targetSessionId=null 清固定绑定", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_scheduled_tasks")
        return [
          row({
            target_mode: "per_run",
            target_session_id: null,
            model_id: "model-7",
            last_run_session_id: null,
          }),
        ];
      if (cmd === "list_sessions") {
        return [{ id: "s1", title: "旧会话", session_type: "chat" }];
      }
      if (cmd === "list_models")
        return [
          {
            id: "model-7",
            providerId: "prov-1",
            providerDisplayName: "Acme",
            displayName: "GPT-X",
            modelName: "gpt-x",
          },
        ];
      if (cmd === "get_default_model") return null;
      return null;
    });
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-task-1"]').trigger("click");
    const form = openForm(w);
    // 编辑态:dedicated 卡不出现(fixed 行统一回显「指定 session」)。
    expect(form.find('[data-testid="sched-target-dedicated"]').exists()).toBe(false);
    expect(form.find('[data-testid="sched-session-select"]').exists()).toBe(false);
    // per_run 行的模型绑定回填经 SelectRoot model 断言(0=project,1=model)。
    expect(form.findAllComponents(SelectRoot)[1].props("modelValue")).toBe("model-7");
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task")
        return row({ target_mode: "per_run", target_session_id: null });
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "update_scheduled_task");
    expect(call?.[1].targetMode).toBe("per_run");
    expect(call?.[1].targetSessionId).toBeNull();
    expect(call?.[1].modelId).toBe("model-7");
  });

  it("fixed 行编辑切到 per_run:update 带 targetMode + targetSessionId null", async () => {
    stubBackend([row()]);
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-task-1"]').trigger("click");
    const form = openForm(w);
    await pickTargetMode(form, "per_run");
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task") return row({ target_mode: "per_run", target_session_id: null });
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "update_scheduled_task");
    expect(call?.[1].targetMode).toBe("per_run");
    expect(call?.[1].targetSessionId).toBeNull();
    expect(call?.[1].modelId).toBeNull();
  });

  it("卡片:once 任务已触发 → 状态「已完成」且「下次」列为 —;过期未触发 → 「已结束」", async () => {
    stubBackend([
      row({
        schedule: { kind: "once", at_ms: Date.now() - 60_000 },
        enabled: false,
        run_count: 1,
        next_fire_at: Date.now() + 86_400_000, // 后端 fallback 展示值,前端应忽略
      }),
      row({
        id: "task-2",
        schedule: { kind: "once", at_ms: Date.now() - 60_000 },
        enabled: false,
        run_count: 0,
      }),
    ]);
    const w = await mountTab();
    const card1 = w.get('[data-testid="sched-card-task-1"]');
    expect(card1.text()).toContain("已完成");
    expect(card1.text()).toContain("下次:—");
    const card2 = w.get('[data-testid="sched-card-task-2"]');
    expect(card2.text()).toContain("已结束");
  });

  it("编辑回填:once 任务预填日期/时刻,更新提交同 at_ms", async () => {
    const at = new Date(Date.now() + 2 * 86_400_000);
    at.setHours(7, 15, 0, 0);
    stubBackend([row({ schedule: { kind: "once", at_ms: at.getTime() } })]);
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-task-1"]').trigger("click");
    await flushPromises();
    const form = openForm(w);
    const pad = (n: number) => n.toString().padStart(2, "0");
    const expectedDate = `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`;
    expect(
      (form.find('[data-testid="sched-once-date"]').element as HTMLInputElement).value,
    ).toBe(expectedDate);
    expect(
      (form.find('[data-testid="sched-once-time"]').element as HTMLInputElement).value,
    ).toBe("07:15");
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task") return row({ schedule: { kind: "once", at_ms: at.getTime() } });
      if (cmd === "list_scheduled_tasks") return [row({ schedule: { kind: "once", at_ms: at.getTime() } })];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "update_scheduled_task");
    expect(call?.[1].schedule).toBe(JSON.stringify({ kind: "once", at_ms: at.getTime() }));
    // 单次档更新显式清空结束条件(不残留旧值)。
    expect(call?.[1].maxRuns).toBeNull();
    expect(call?.[1].endsAt).toBeNull();
  });
});

describe("ScheduledTasksTab M4a 定时审议(group_chat 档)", () => {
  function stubCreateGc() {
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "create_scheduled_task")
        return row({ id: "new-gc", target_mode: "group_chat", target_session_id: null });
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
  }

  /** group_chat 行存档任务(最近场 + outcome 已落账)。 */
  function gcRow(): ScheduledTask {
    return row({
      id: "gc-1",
      name: "每周评审",
      target_mode: "group_chat",
      target_session_id: null,
      group_chat_config: gcConfig(),
      last_run_session_id: "s-gc-run",
      last_fire_outcome: "started",
      schedule: { kind: "weekly", weekday: "fri", at: "18:00" },
    });
  }

  it("创建态出现第四档 radio;选中后 preset 默认 review + 主持人 + 预览与成本标注", async () => {
    stubBackendGc([]);
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    expect(form.find('[data-testid="sched-target-group_chat"]').exists()).toBe(true);
    // session 模型下拉(dedicated/per_run 专用)不渲染;gc 面板渲染。
    expect(form.find('[data-testid="sched-model-select"]').exists()).toBe(false);
    await pickTargetMode(form, "group_chat");
    // gc 档 SelectRoot 序:0=project,1=preset(默认 review),2=moderator,3=kind。
    expect(form.findAllComponents(SelectRoot)[1].props("modelValue")).toBe("review");
    expect(form.find('[data-testid="sched-gc-moderator"]').exists()).toBe(true);
    const roster = form.find('[data-testid="sched-gc-participants"]');
    expect(roster.exists()).toBe(true);
    expect(roster.text()).toContain("架构");
    expect(roster.text()).toContain("Zhipu · GLM-5.3");
    expect(roster.text()).toContain("3 参与 × ≤30 轮");
  });

  it("提交 = 提交时展开:preset 名字解析成 UUID + persona_md 逐字;不带 target/modelId", async () => {
    stubBackendGc([]);
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    await form.find("input[type='text']").setValue("每周审议");
    await pickSelect(form, 0, "p1");
    await form.find("textarea").setValue("复盘本周改动");
    await pickTargetMode(form, "group_chat");
    stubCreateGc();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call).toBeTruthy();
    expect(call?.[1].targetMode).toBe("group_chat");
    expect(call?.[1].targetSessionId).toBeUndefined();
    expect(call?.[1].modelId).toBeUndefined();
    const cfg = call?.[1].groupChatConfig;
    expect(cfg.moderator_model_id).toBe("uuid-mini");
    expect(cfg.participants.map((p: { name: string }) => p.name)).toEqual(["架构", "产品", "后端"]);
    expect(cfg.participants.map((p: { model_id: string }) => p.model_id)).toEqual([
      "uuid-glm",
      "uuid-flash",
      "uuid-ds",
    ]);
    // persona_md = 边界文本 + "\n\n" + 公共纪律(M1 composePresets 同构)。
    for (const p of cfg.participants) {
      expect(p.persona_md).toContain("发言纪律");
      expect(p.persona_md).toContain("\n\n");
    }
  });

  it("主持人改选:下拉选另一模型 → 提交的 moderator_model_id 跟随改选", async () => {
    stubBackendGc([]);
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    await form.find("input[type='text']").setValue("每周审议");
    await pickSelect(form, 0, "p1");
    await form.find("textarea").setValue("p");
    await pickTargetMode(form, "group_chat");
    await pickSelect(form, 2, "uuid-flash");
    stubCreateGc();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call?.[1].groupChatConfig.moderator_model_id).toBe("uuid-flash");
  });

  it("校验:preset 模型不在目录 → 内联错误,不发起 create(成本防线闸口)", async () => {
    stubBackend([]); // 模型目录为空 → moderator 解析失败
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    await form.find("input[type='text']").setValue("每周审议");
    await pickSelect(form, 0, "p1");
    await form.find("textarea").setValue("p");
    await pickTargetMode(form, "group_chat");
    invokeMock.mockClear();
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    expect(w.find(".sched-tab__error").text()).toContain("不在模型目录");
    expect(invokeMock).not.toHaveBeenCalledWith("create_scheduled_task", expect.anything());
  });

  it("编辑 group_chat 行:preset 占位「未选择(使用存档配置)」+ 存档预览;不重选提交 = 配置缺省不动", async () => {
    stubBackendGc([gcRow()]);
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-gc-1"]').trigger("click");
    const form = openForm(w);
    // 编辑态三卡(existing / per_run / group_chat),group_chat 回显。
    expect(form.find('[data-testid="sched-target-group_chat"]').exists()).toBe(true);
    expect(form.find('[data-testid="sched-target-dedicated"]').exists()).toBe(false);
    await pickTargetMode(form, "group_chat");
    // preset 下拉未选择(modelValue undefined)+ 快照语义提示。
    expect(form.findAllComponents(SelectRoot)[1].props("modelValue")).toBeUndefined();
    const hint = form.find('[data-testid="sched-gc-snapshot-hint"]');
    expect(hint.text()).toContain("未选择预设");
    expect(hint.text()).toContain("存档配置");
    // 存档展开结果只读预览(主持人反查显示名 + 参与阵容)。
    expect(form.text()).toContain("MiniMax · MiniMax-M3");
    const roster = form.find('[data-testid="sched-gc-participants"]');
    expect(roster.text()).toContain("架构");
    expect(roster.text()).toContain("Zhipu · GLM-5.3");
    expect(roster.text()).toContain("2 参与 × ≤30 轮");
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task") return gcRow();
      if (cmd === "list_scheduled_tasks") return [gcRow()];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "update_scheduled_task");
    expect(call?.[1].targetMode).toBe("group_chat");
    // 缺省不动:groupChatConfig 不进 args(wire 缺省 = 后端保留存档)。
    expect(call?.[1]).not.toHaveProperty("groupChatConfig");
  });

  it("编辑重选 preset:预览切换 + 提示变覆盖;提交带重新展开的 groupChatConfig", async () => {
    stubBackendGc([gcRow()]);
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-gc-1"]').trigger("click");
    const form = openForm(w);
    await pickTargetMode(form, "group_chat");
    // gc 档 SelectRoot 序:0=project,1=preset,2=moderator,3=kind。
    await pickSelect(form, 1, "arch");
    const hint = form.find('[data-testid="sched-gc-snapshot-hint"]');
    expect(hint.text()).toContain("覆盖存档配置");
    // 预览切到 preset 展开(arch = 架构 + 后端)。
    const roster = form.find('[data-testid="sched-gc-participants"]');
    expect(roster.text()).toContain("后端");
    expect(roster.text()).not.toContain("产品");
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task") return gcRow();
      if (cmd === "list_scheduled_tasks") return [gcRow()];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "update_scheduled_task");
    expect(call?.[1].groupChatConfig).toEqual({
      moderator_model_id: "uuid-mini",
      participants: [
        { name: "架构", model_id: "uuid-glm", persona_md: expect.stringContaining("发言纪律") },
        { name: "后端", model_id: "uuid-ds", persona_md: expect.stringContaining("发言纪律") },
      ],
    });
  });

  it("编辑 group_chat 行切 per_run:提交 targetMode=per_run 且不带配置(后端自动清)", async () => {
    stubBackendGc([gcRow()]);
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-gc-1"]').trigger("click");
    const form = openForm(w);
    await pickTargetMode(form, "per_run");
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task")
        return row({ id: "gc-1", target_mode: "per_run", target_session_id: null });
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "update_scheduled_task");
    expect(call?.[1].targetMode).toBe("per_run");
    expect(call?.[1]).not.toHaveProperty("groupChatConfig");
  });

  it("零回归锚:fixed 行编辑态不出现 group_chat 卡(仍两档)", async () => {
    stubBackendGc([row()]);
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-task-1"]').trigger("click");
    const form = openForm(w);
    expect(form.find('[data-testid="sched-target-existing"]').exists()).toBe(true);
    expect(form.find('[data-testid="sched-target-per_run"]').exists()).toBe(true);
    expect(form.find('[data-testid="sched-target-group_chat"]').exists()).toBe(false);
  });

  it("卡片:「审议」徽标 + 定时审议 meta(成本标注/最近场)+ outcome 状态行", async () => {
    stubBackendGc([gcRow()]);
    const w = await mountTab();
    const card = w.get('[data-testid="sched-card-gc-1"]');
    expect(card.find(".sched-tab__card-gc").text()).toBe("审议");
    expect(card.text()).toContain("定时审议");
    expect(card.text()).toContain("2 参与 × ≤30 轮");
    expect(card.text()).toContain("最近:上一场审议");
    expect(card.find('[data-testid="sched-outcome-gc-1"]').text()).toBe("已开跑");
  });

  it("卡片:outcome 为 null(从未触发)不渲染状态行", async () => {
    stubBackendGc([
      row({ id: "gc-2", target_mode: "group_chat", target_session_id: null, group_chat_config: gcConfig() }),
    ]);
    const w = await mountTab();
    const card = w.get('[data-testid="sched-card-gc-2"]');
    expect(card.find('[data-testid="sched-outcome-gc-2"]').exists()).toBe(false);
  });

  // -------------------------------------------------------------------
  // gce-m4c(09-08)Token 预算输入:两态落 config + 编辑态 dirty 独立
  // 重交(design §2.3)+ 非法值拦截。
  // -------------------------------------------------------------------
  function gcRowWithBudget(budget: number | null): ScheduledTask {
    return {
      ...gcRow(),
      group_chat_config: {
        ...gcConfig(),
        ...(budget !== null ? { token_budget: budget } : {}),
      },
    };
  }

  async function fillGcCreateForm() {
    stubBackendGc([]);
    const w = await mountTab();
    await w.get('[data-testid="sched-create-btn"]').trigger("click");
    const form = openForm(w);
    await form.find("input[type='text']").setValue("每周审议");
    await pickSelect(form, 0, "p1");
    await form.find("textarea").setValue("p");
    await pickTargetMode(form, "group_chat");
    return { w, form };
  }

  it("创建态预算两态:留空 → config 无 token_budget 键;填数 → 随 config 提交", async () => {
    // 留空(缺省 = 不限,键不写)。
    const first = await fillGcCreateForm();
    stubCreateGc();
    await first.w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    let call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call?.[1].groupChatConfig).toBeTruthy();
    expect(call?.[1].groupChatConfig).not.toHaveProperty("token_budget");

    // 填 500000 → config 带 token_budget。
    const second = await fillGcCreateForm();
    await second.form.find('[data-testid="sched-gc-budget"]').setValue("500000");
    stubCreateGc();
    await second.w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    call = invokeMock.mock.calls.find((c) => c[0] === "create_scheduled_task");
    expect(call?.[1].groupChatConfig.token_budget).toBe(500000);
  });

  it("编辑态只改预算未重选 preset:存档配置 + 新预算整体重交", async () => {
    stubBackendGc([gcRowWithBudget(1000)]);
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-gc-1"]').trigger("click");
    const form = openForm(w);
    await pickTargetMode(form, "group_chat");
    // 存档预算回填;preset 仍「未选择」。
    expect(
      (form.find('[data-testid="sched-gc-budget"]').element as HTMLInputElement).value,
    ).toBe("1000");
    expect(form.findAllComponents(SelectRoot)[1].props("modelValue")).toBeUndefined();
    await form.find('[data-testid="sched-gc-budget"]').setValue("9999");
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task") return gcRowWithBudget(9999);
      if (cmd === "list_scheduled_tasks") return [gcRowWithBudget(9999)];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "update_scheduled_task");
    // 预算 dirty ⇒ 即使未重选 preset 也整体重交(存档阵容原样 + 新预算)。
    expect(call?.[1].groupChatConfig).toEqual({
      moderator_model_id: "uuid-mini",
      participants: gcConfig().participants,
      token_budget: 9999,
    });
  });

  it("编辑态清空预算(存档有 1000):重交存档配置且无 token_budget 键(清除)", async () => {
    stubBackendGc([gcRowWithBudget(1000)]);
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-gc-1"]').trigger("click");
    const form = openForm(w);
    await pickTargetMode(form, "group_chat");
    await form.find('[data-testid="sched-gc-budget"]').setValue("");
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task") return gcRow();
      if (cmd === "list_scheduled_tasks") return [gcRow()];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "update_scheduled_task");
    expect(call?.[1].groupChatConfig).toBeTruthy();
    expect(call?.[1].groupChatConfig).not.toHaveProperty("token_budget");
    expect(call?.[1].groupChatConfig.participants).toEqual(gcConfig().participants);
  });

  it("编辑态预算未动且未重选 preset:维持缺省不发(既有语义零回归)", async () => {
    stubBackendGc([gcRowWithBudget(1000)]);
    const w = await mountTab();
    await w.get('[data-testid="sched-edit-gc-1"]').trigger("click");
    const form = openForm(w);
    await pickTargetMode(form, "group_chat");
    invokeMock.mockClear();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task") return gcRowWithBudget(1000);
      if (cmd === "list_scheduled_tasks") return [gcRowWithBudget(1000)];
      return null;
    });
    await w.get('[data-testid="sched-submit"]').trigger("click");
    await flushPromises();
    const call = invokeMock.mock.calls.find((c) => c[0] === "update_scheduled_task");
    expect(call?.[1]).not.toHaveProperty("groupChatConfig");
  });

  it("非法预算(0 / 1.5 / -5)→ 内联错误条,不发起 IPC", async () => {
    const { w, form } = await fillGcCreateForm();
    for (const bad of ["0", "1.5", "-5"]) {
      await form.find('[data-testid="sched-gc-budget"]').setValue(bad);
      invokeMock.mockClear();
      await w.get('[data-testid="sched-submit"]').trigger("click");
      await flushPromises();
      expect(w.find(".sched-tab__error").text()).toContain("正整数");
      expect(invokeMock).not.toHaveBeenCalledWith(
        "create_scheduled_task",
        expect.anything(),
      );
    }
  });
});
