// Tests for `stores/scheduledTasks.ts` — F2 定时任务管理面 store。
// transport 全量 mock(SearchTab.test.ts 同款);覆盖 list 缓存 /
// create / update / remove 的 args 形状(camelCase 顶层 key,后端
// snake 字段直拷)+ 失败策略(列表保留旧值)。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useScheduledTasksStore } from "./scheduledTasks";
import type { ScheduledTask } from "./scheduledTasks";

function row(overrides: Partial<ScheduledTask> = {}): ScheduledTask {
  return {
    id: "task-1",
    project_id: "p1",
    target_session_id: "s1",
    name: "早报",
    prompt: "汇总进展",
    schedule: { kind: "daily", at: "09:00" },
    enabled: true,
    created_by: "user",
    created_at: 1_000,
    last_fired_at: null,
    next_fire_at: 2_000,
    run_count: 0,
    max_runs: null,
    ends_at: null,
    ...overrides,
  };
}

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
});

describe("scheduledTasks store", () => {
  it("load → list_scheduled_tasks 全量缓存", async () => {
    invokeMock.mockResolvedValueOnce([row()]);
    const store = useScheduledTasksStore();
    await store.load();
    expect(invokeMock).toHaveBeenCalledWith("list_scheduled_tasks");
    expect(store.tasks).toHaveLength(1);
    expect(store.loaded).toBe(true);
    expect(store.error).toBeNull();
  });

  it("create 传 camelCase 顶层 key;targetSessionId 缺省时不进 args(后端新建专用 session)", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "create_scheduled_task") return row({ id: "task-2" });
      if (cmd === "list_scheduled_tasks") return [row({ id: "task-2" })];
      return null;
    });
    const store = useScheduledTasksStore();
    const created = await store.create({
      projectId: "p1",
      name: "巡检",
      prompt: "跑一遍测试",
      schedule: JSON.stringify({ kind: "interval", every_min: 30 }),
    });
    expect(invokeMock).toHaveBeenCalledWith("create_scheduled_task", {
      projectId: "p1",
      name: "巡检",
      prompt: "跑一遍测试",
      schedule: '{"kind":"interval","every_min":30}',
    });
    expect(created.id).toBe("task-2");
    // create 后重拉,徽章/软警示数据新鲜。
    expect(store.tasks.map((t) => t.id)).toEqual(["task-2"]);
  });

  it("create 带 targetSessionId 时 args 携带该字段", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "create_scheduled_task") return row();
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    const store = useScheduledTasksStore();
    await store.create({
      projectId: "p1",
      targetSessionId: "s9",
      name: "n",
      prompt: "p",
      schedule: "{}",
      enabled: false,
    });
    expect(invokeMock).toHaveBeenCalledWith("create_scheduled_task", {
      projectId: "p1",
      targetSessionId: "s9",
      name: "n",
      prompt: "p",
      schedule: "{}",
      enabled: false,
    });
  });

  it("update 只携带传入的 patch 字段(enabled false→true 语义在后端)", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task")
        return row({ enabled: false, name: "晚报" });
      if (cmd === "list_scheduled_tasks") return [row({ enabled: false })];
      return null;
    });
    const store = useScheduledTasksStore();
    await store.update("task-1", { enabled: false, name: "晚报" });
    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "update_scheduled_task",
    );
    expect(call?.[1]).toEqual({ id: "task-1", enabled: false, name: "晚报" });
    expect(call?.[1]).not.toHaveProperty("prompt");
    expect(call?.[1]).not.toHaveProperty("targetSessionId");
  });

  it("F2b:create 携带 maxRuns / endsAt;缺省不进 args", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "create_scheduled_task") return row({ max_runs: 5 });
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    const store = useScheduledTasksStore();
    await store.create({
      projectId: "p1",
      name: "限次",
      prompt: "p",
      schedule: '{"kind":"daily","at":"09:00"}',
      maxRuns: 5,
      endsAt: 4_102_444_800_000,
    });
    expect(invokeMock).toHaveBeenCalledWith("create_scheduled_task", {
      projectId: "p1",
      name: "限次",
      prompt: "p",
      schedule: '{"kind":"daily","at":"09:00"}',
      maxRuns: 5,
      endsAt: 4_102_444_800_000,
    });
  });

  it("F2b:update 的 maxRuns/endsAt 显式 null 透传(wire null = 清空为不限)", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_scheduled_task")
        return row({ max_runs: null, ends_at: null });
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    const store = useScheduledTasksStore();
    await store.update("task-1", { maxRuns: null, endsAt: 4_102_444_800_000 });
    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "update_scheduled_task",
    );
    expect(call?.[1]).toEqual({
      id: "task-1",
      maxRuns: null,
      endsAt: 4_102_444_800_000,
    });
  });

  it("remove 返回后端真删布尔并重拉", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "delete_scheduled_task") return true;
      if (cmd === "list_scheduled_tasks") return [];
      return null;
    });
    const store = useScheduledTasksStore();
    const deleted = await store.remove("task-1");
    expect(deleted).toBe(true);
    expect(store.tasks).toHaveLength(0);
  });

  it("load 失败写 error 且保留旧列表", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_scheduled_tasks") throw new Error("db locked");
      return null;
    });
    const store = useScheduledTasksStore();
    store.tasks = [row()];
    await store.load();
    expect(store.error).toContain("db locked");
    expect(store.tasks).toHaveLength(1);
  });

  it("enabledTasksForSession 只回 enabled 且按 target_session_id 过滤", () => {
    invokeMock.mockResolvedValue([]);
    const store = useScheduledTasksStore();
    store.tasks = [
      row({ id: "a", target_session_id: "s1", enabled: true }),
      row({ id: "b", target_session_id: "s1", enabled: false }),
      row({ id: "c", target_session_id: "s2", enabled: true }),
    ];
    expect(store.enabledTasksForSession("s1").map((t) => t.id)).toEqual(["a"]);
    expect(store.enabledTasksForSession(null)).toEqual([]);
  });
});
