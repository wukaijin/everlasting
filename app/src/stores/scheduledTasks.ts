// useScheduledTasksStore — Pinia store for the F2 scheduled tasks
// management surface (2026-08-28, task `08-28-f2-scheduled-tasks`).
//
// Settings「定时任务」tab 的 CRUD 包装 + session header 徽章的数据源:
//   1. `load()` — 拉全量任务列表(创建序);AppShell 启动拉一次(徽章),
//      tab 打开 / 每次变更后再拉(管理面)。
//   2. `create` / `update` / `remove` — 走后端校验(目标 session 存在且
//      classic、schedule 合法),成功后重拉列表,保持徽章/软警示的数据
//      新鲜。
//   3. `enabledTasksForSession(sessionId)` — 纯读缓存;session header 时钟
//      徽章与表单「同 session 已有 enabled 任务」软警示共用。
//
// Wire 形状:字段 snake_case 直拷 Rust `ScheduledTaskPayload`
// (BACKLOG §5.2 决策:struct 字段不加 camelCase rename)。`schedule`
// 是后端已解析的 preset 对象(损坏存量行降级 `null`,UI 按「未知档位」
// 渲染);`next_fire_at` 是纯展示值(触发判定在调度器每 tick 重算)。
//
// Failure policy(沿 audit store):IPC 失败写 `error` / 抛给调用方 toast,
// 列表保留旧值不崩。

import { defineStore } from "pinia";
import { ref } from "vue";
import { transport } from "../transport";

/** schedule preset 档位(prd D2;镜像 Rust `ScheduleSpec`,internally
 *  tagged `kind`,weekday 为 chrono 三字母小写 "mon".."sun")。后续新档位
 *  additive:这里加 union 分支 + UI 渲染分支,未知 kind 显示「未知档位」。 */
export type ScheduleSpec =
  | { kind: "daily"; at: string }
  | { kind: "interval"; every_min: number }
  | { kind: "weekly"; weekday: string; at: string };

/** `ScheduledTaskPayload` wire 形状(snake_case 直拷)。 */
export interface ScheduledTask {
  id: string;
  project_id: string;
  target_session_id: string;
  name: string;
  prompt: string;
  schedule: ScheduleSpec | null;
  enabled: boolean;
  /** epoch ms。 */
  created_at: number;
  /** epoch ms;null = 从未触发。 */
  last_fired_at: number | null;
  /** epoch ms;纯展示值。 */
  next_fire_at: number;
}

/** `create_scheduled_task` 入参(camelCase 顶层 key,transport 扳 snake)。 */
export interface CreateScheduledTaskInput {
  projectId: string;
  /** undefined / 空 = 新建专用 session(标题同任务名,后端定)。 */
  targetSessionId?: string;
  name: string;
  prompt: string;
  /** JSON 字符串(前端 stringify preset 对象;后端 parse_schedule 校验)。 */
  schedule: string;
  enabled?: boolean;
}

/** `update_scheduled_task` 的部分更新 patch(`undefined` 字段后端不动)。 */
export interface UpdateScheduledTaskInput {
  name?: string;
  prompt?: string;
  schedule?: string;
  targetSessionId?: string;
  enabled?: boolean;
}

export const useScheduledTasksStore = defineStore("scheduledTasks", () => {
  const tasks = ref<ScheduledTask[]>([]);
  const loading = ref(false);
  const loaded = ref(false);
  const error = ref<string | null>(null);

  /** 全量列表(创建序,后端排序)。失败保留旧列表 + 写 error。 */
  async function load(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      tasks.value = await transport.invoke<ScheduledTask[]>(
        "list_scheduled_tasks",
      );
      loaded.value = true;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  async function create(input: CreateScheduledTaskInput): Promise<ScheduledTask> {
    const row = await transport.invoke<ScheduledTask>("create_scheduled_task", {
      projectId: input.projectId,
      ...(input.targetSessionId ? { targetSessionId: input.targetSessionId } : {}),
      name: input.name,
      prompt: input.prompt,
      schedule: input.schedule,
      ...(input.enabled !== undefined ? { enabled: input.enabled } : {}),
    });
    await load();
    return row;
  }

  /** 部分更新(`undefined` 字段不动存量;enabled false→true 时后端置
   *  `last_fired_at = now`,重启用不补跑存量)。 */
  async function update(
    id: string,
    patch: UpdateScheduledTaskInput,
  ): Promise<ScheduledTask> {
    const row = await transport.invoke<ScheduledTask>("update_scheduled_task", {
      id,
      ...(patch.name !== undefined ? { name: patch.name } : {}),
      ...(patch.prompt !== undefined ? { prompt: patch.prompt } : {}),
      ...(patch.schedule !== undefined ? { schedule: patch.schedule } : {}),
      ...(patch.targetSessionId !== undefined
        ? { targetSessionId: patch.targetSessionId }
        : {}),
      ...(patch.enabled !== undefined ? { enabled: patch.enabled } : {}),
    });
    await load();
    return row;
  }

  /** 硬删。返回后端真删与否(`false` = 已被他端删,调用方按幂等成功处理)。 */
  async function remove(id: string): Promise<boolean> {
    const deleted = await transport.invoke<boolean>("delete_scheduled_task", {
      id,
    });
    await load();
    return deleted;
  }

  /** 某 session 名下的 enabled 任务(纯缓存读,不发起 IPC)。session
   *  header 徽章 + 表单软警示共用;`tasks` 未加载时返回空数组(徽章
   *  不渲染,可接受的降态——AppShell 启动即拉)。 */
  function enabledTasksForSession(sessionId: string | null | undefined): ScheduledTask[] {
    if (!sessionId) return [];
    return tasks.value.filter((t) => t.enabled && t.target_session_id === sessionId);
  }

  return {
    tasks,
    loading,
    loaded,
    error,
    load,
    create,
    update,
    remove,
    enabledTasksForSession,
  };
});
