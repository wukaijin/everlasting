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
 *  tagged `kind`,weekday 为 chrono 三字母小写 "mon".."sun")。F2b 扩展
 *  hourly / weekdays / monthly 三档;CH11-1 补 once 单次档;后续新档位
 *  additive:这里加 union 分支 + UI 渲染分支,未知 kind 显示「未知档位」。 */
export type ScheduleSpec =
  | { kind: "daily"; at: string }
  | { kind: "interval"; every_min: number }
  | { kind: "weekly"; weekday: string; at: string }
  /** 每小时第 minute 分钟(F2b)。 */
  | { kind: "hourly"; minute: number }
  /** 每工作日(周一至五)的 at(F2b)。 */
  | { kind: "weekdays"; at: string }
  /** 每月 day 号的 at(F2b;短月无该日跳过该月)。 */
  | { kind: "monthly"; day: number; at: string }
  /** 单次:at_ms(epoch ms 本地时刻)触发恰好一次(CH11-1)。 */
  | { kind: "once"; at_ms: number };

/** 目标模式(wire `target_mode`,08-31-sched-per-run-session):
 *  `fixed` = 注入固定目标 session;`per_run` = 每次触发自动新建 session;
 *  `group_chat` = 定时审议(09-07-gce-m4a,fire 自动建群发题)。 */
export type TargetMode = "fixed" | "per_run" | "group_chat";

/** M4a 定时审议:存档的展开群聊配置(`group_chat_config` 列 JSON 的
 *  形状,直拷 Rust `db::scheduled_tasks::GroupChatTaskConfig`)。
 *  **创建时展开的完整配置** —— preset 展开发生在创建 UI,模型引用是
 *  models 表 UUID;daemon 与 DB 全程无 preset 名(design §3)。 */
export interface GroupChatTaskParticipant {
  name: string;
  model_id: string;
  /** 可选内联 persona(展开自 preset;镜像 Rust `Option<String>` +
   *  `serde(default)`:wire 上缺失 / null / 字符串三态合法)。 */
  persona_md?: string | null;
}

export interface GroupChatTaskConfig {
  moderator_model_id: string;
  participants: GroupChatTaskParticipant[];
}

/** M4a:最近一次 fire 的结局快照(wire `last_fire_outcome`,镜像 Rust
 *  `db::scheduled_tasks::fire_outcomes` 五值;null = 从未触发)。 */
export type LastFireOutcome =
  | "started"
  | "resumed"
  | "skipped_busy"
  | "error"
  | "recovered";

/** `ScheduledTaskPayload` wire 形状(snake_case 直拷)。 */
export interface ScheduledTask {
  id: string;
  project_id: string;
  /** fixed 档 = 目标 session;per_run 档 = null。 */
  target_session_id: string | null;
  /** 目标模式(见 {@link TargetMode})。 */
  target_mode: TargetMode;
  /** per_run 档每次新建 session 的模型绑定;null = 全局默认。 */
  model_id: string | null;
  /** per_run 档最近一次 fire 新建的 session;null = 从未触发。 */
  last_run_session_id: string | null;
  name: string;
  prompt: string;
  schedule: ScheduleSpec | null;
  enabled: boolean;
  /** 作者:'user'(设置 UI/IPC)或 'agent'(LLM schedule_task tool,
   *  08-29-schedule-task-tool)。列表卡据此渲染来源徽标。 */
  created_by: string;
  /** epoch ms。 */
  created_at: number;
  /** epoch ms;null = 从未触发。 */
  last_fired_at: number | null;
  /** epoch ms;纯展示值。 */
  next_fire_at: number;
  /** 已 fire 次数(F2b;dedup 跳过不计数)。 */
  run_count: number;
  /** 次数上限;null = 不限(F2b)。 */
  max_runs: number | null;
  /** 结束日期 epoch ms;null = 不限(F2b;含当日,当日到期点照常触发)。 */
  ends_at: number | null;
  /** M4a 定时审议档:存档的展开群聊配置(编辑回显用);损坏存量行降级
   *  null(与 `schedule` 同款),其余档恒 null。 */
  group_chat_config: GroupChatTaskConfig | null;
  /** M4a:最近一次 fire 的结局(null = 从未触发);任务卡状态行直读。 */
  last_fire_outcome: LastFireOutcome | null;
}

/** `create_scheduled_task` 入参(camelCase 顶层 key,transport 扳 snake)。 */
export interface CreateScheduledTaskInput {
  projectId: string;
  /** undefined / 空 = 新建专用 session(标题同任务名,后端定);
   *  per_run 档不传(后端拒绝两者同时出现)。 */
  targetSessionId?: string;
  /** "per_run" = 每次执行新建 session;"group_chat" = 定时审议自动建群
   *  (M4a,须随同带 groupChatConfig;缺省 = fixed)。 */
  targetMode?: TargetMode;
  name: string;
  prompt: string;
  /** JSON 字符串(前端 stringify preset 对象;后端 parse_schedule 校验)。 */
  schedule: string;
  enabled?: boolean;
  /** 次数上限(F2b;undefined = 不限)。 */
  maxRuns?: number;
  /** 结束日期 epoch ms(F2b;undefined = 不限)。 */
  endsAt?: number;
  /** 模型绑定:fixed 档仅「新建专用 session」分支生效(写 session 行);
   *  per_run 档存任务行,每次新建 session 时应用。undefined = 全局默认。
   *  group_chat 档不收(moderator 在 groupChatConfig 内,后端 400 互斥)。 */
  modelId?: string;
  /** M4a:targetMode="group_chat" 必带(创建 UI 展开 preset 的结果,
   *  模型已解析为 UUID);其余档不传(后端 400 互斥)。 */
  groupChatConfig?: GroupChatTaskConfig;
}

/** `update_scheduled_task` 的部分更新 patch(`undefined` 字段后端不动)。
 *  `maxRuns` / `endsAt` / `targetSessionId` / `modelId` 传 `null` = 显式
 *  清空(wire 显式 `null`,区别于缺省不动 —— 后端 double option;
 *  targetSessionId 清空即切 per_run 的绑定侧,须随同传 targetMode)。
 *  `groupChatConfig` 同为双层 Option:缺省 = 不动(编辑态未重选 preset
 *  时保留存档配置);对象 = 校验后写入;显式 `null` = 清空(仅切离
 *  group_chat 档合法 —— 本档任务切离时后端本就自动清空,前端无需传)。 */
export interface UpdateScheduledTaskInput {
  name?: string;
  prompt?: string;
  schedule?: string;
  targetSessionId?: string | null;
  targetMode?: TargetMode;
  modelId?: string | null;
  enabled?: boolean;
  maxRuns?: number | null;
  endsAt?: number | null;
  groupChatConfig?: GroupChatTaskConfig | null;
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
      ...(input.targetMode ? { targetMode: input.targetMode } : {}),
      name: input.name,
      prompt: input.prompt,
      schedule: input.schedule,
      ...(input.enabled !== undefined ? { enabled: input.enabled } : {}),
      ...(input.maxRuns !== undefined ? { maxRuns: input.maxRuns } : {}),
      ...(input.endsAt !== undefined ? { endsAt: input.endsAt } : {}),
      ...(input.modelId ? { modelId: input.modelId } : {}),
      ...(input.groupChatConfig ? { groupChatConfig: input.groupChatConfig } : {}),
    });
    await load();
    return row;
  }

  /** 部分更新(`undefined` 字段不动存量;enabled false→true 时后端置
   *  `last_fired_at = now` + `run_count = 0`,重启用不补跑、计数重置)。
   *  `maxRuns` / `endsAt` / `targetSessionId` / `modelId` 传 `null` 显式
   *  清空。 */
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
      ...(patch.targetMode !== undefined ? { targetMode: patch.targetMode } : {}),
      ...(patch.modelId !== undefined ? { modelId: patch.modelId } : {}),
      ...(patch.enabled !== undefined ? { enabled: patch.enabled } : {}),
      ...(patch.maxRuns !== undefined ? { maxRuns: patch.maxRuns } : {}),
      ...(patch.endsAt !== undefined ? { endsAt: patch.endsAt } : {}),
      // 双层 Option:显式 null 透传(清空);缺省( undefined)不进 args。
      ...(patch.groupChatConfig !== undefined
        ? { groupChatConfig: patch.groupChatConfig }
        : {}),
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
