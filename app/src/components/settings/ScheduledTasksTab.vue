<script setup lang="ts">
// ScheduledTasksTab — Settings 第 8 个 tab「定时任务」(F2, task
// `08-28-f2-scheduled-tasks` design §7)。
//
// 一面两区:
//   1. 任务卡片列表 —— 名称 / 目标 session·project / schedule 人话 /
//      prompt 摘要 / 上次·下次触发 / 触发进度(已触发 N/M 次 · 至日期)/
//      启停 switch / 删除(ConfirmDialog 确认)。停用行灰显;F2b 自动
//      完成的任务显示「已完成/已结束」(区别于手动停用)。
//   2. 新建/编辑表单 —— project 下拉 → session 下拉(按 project 过滤,
//      仅 classic;勾选「新建专用 session」则不选,并可额外指定该
//      session 的模型[空 = 全局默认,写入 per-session 覆盖列])→
//      档位(单次 datetime-local[CH11-1]+ F2b 6 档:每小时 分钟 /
//      每天 时分 / 每工作日 时分 / 每周 周几+时分 / 每月 几号+时分 /
//      固定频率 数量+单位[分钟/小时/天/周,提交换算 every_min])→
//      结束条件(F2b:固定时间 = 永不/次数 N;固定频率 = 永不/结束日期
//      [含当日,提交转当日 23:59:59.999 本地 ms];单次档无结束条件)→
//      prompt textarea。
//      同 session 已有 enabled 任务 → **软警示**(不硬拒;调度器对同
//      session 同 tick 串行化,设计 §9)。
//
// 面板顶部注明「调度仅在 daemon 进程运行时生效」(GUI Full/tauri 逃生
// 模式可建任务但不会触发);`scheduled_tasks_enabled=false`(kill
// switch)时追加一行状态提示(仍可编辑任务,与后端语义一致)。
//
// session 列表数据:`list_sessions` 按 project 现取 + 组件内缓存
// (chat store 只持当前 project 的 sessions,管理面要跨 project)。
// 移动端沿 S6b:输入控件全宽 box-sizing、卡片纵向堆叠、触控目标由
// style.css 全局 44px 规则覆盖(`.settings-modal button`;reka
// SelectTrigger 渲染为 button,同 ModelForm/ProvidersTab 一并受益)。
//
// 下拉控件走 reka-ui SelectRoot(ProvidersTab/ModelForm/SubagentsTab
// 同款,2026-08-28 统一):原生 <select> 的 options 弹层是 OS 原生样式
// (浅色、无暗色主题),与其它 tab 的 reka 弹层不一致。空串值被 reka
// 2.9.9 禁止,「未选」态用 undefined model + SelectValue placeholder
// 表达(SubagentsTab sentinel 注释同源);project 切换时联动清空
// targetSessionId(原 @change 语义挪进 handler)。

import { computed, onMounted, reactive, ref } from "vue";
import {
  Label,
  CheckboxRoot,
  CheckboxIndicator,
  SelectRoot,
  SelectTrigger,
  SelectValue,
  SelectIcon,
  SelectPortal,
  SelectContent,
  SelectViewport,
  SelectItem,
  SelectItemText,
} from "reka-ui";
import ConfirmDialog from "../common/ConfirmDialog.vue";
import Icon from "../Icon.vue";
import { useScheduledTasksStore } from "../../stores/scheduledTasks";
import type { ScheduledTask, ScheduleSpec } from "../../stores/scheduledTasks";
import type { SessionSummary } from "../../stores/chat.types";
import { useProjectsStore } from "../../stores/projects";
import { useConfigStore } from "../../stores/config";
import { useModelsStore } from "../../stores/models";
import { transport } from "../../transport";
import { extractErrorMessage } from "../../utils/useErrorBus";
import {
  describeSchedule,
  formatFireTime,
  summarizePrompt,
  WEEKDAY_OPTIONS,
  INTERVAL_UNITS,
  splitEveryMin,
  describeRunCount,
  describeEndDate,
  completedByRunLimit,
  completedByEndDate,
  completedByOnce,
  displayNextFireAt,
} from "../../utils/scheduledTaskFormat";

const store = useScheduledTasksStore();
const projects = useProjectsStore();
const config = useConfigStore();
const models = useModelsStore();

// --- 列表区 ---------------------------------------------------------------

/** 目标 session 的标题(list 显示用)。sessionsByProject 懒加载缓存。 */
const sessionsByProject = reactive(new Map<string, SessionSummary[]>());
const sessionsLoading = ref(false);

async function ensureSessionsFor(projectIds: string[]): Promise<void> {
  const missing = projectIds.filter((id) => !sessionsByProject.has(id));
  if (missing.length === 0) return;
  sessionsLoading.value = true;
  try {
    const results = await Promise.all(
      missing.map(async (pid) => {
        try {
          const rows = await transport.invoke<SessionSummary[]>("list_sessions", {
            projectId: pid,
          });
          return [pid, rows] as const;
        } catch {
          return [pid, [] as SessionSummary[]] as const;
        }
      }),
    );
    for (const [pid, rows] of results) sessionsByProject.set(pid, rows);
  } finally {
    sessionsLoading.value = false;
  }
}

function sessionTitleOf(task: ScheduledTask): string {
  const rows = sessionsByProject.get(task.project_id);
  const hit = rows?.find((s) => s.id === task.target_session_id);
  return hit?.title ?? `session ${task.target_session_id.slice(0, 8)}`;
}

function projectNameOf(task: ScheduledTask): string {
  return projects.projectById(task.project_id)?.name ?? task.project_id.slice(0, 8);
}

// --- 启停 / 删除 ----------------------------------------------------------

const togglingIds = reactive(new Set<string>());

async function toggleEnabled(task: ScheduledTask): Promise<void> {
  if (togglingIds.has(task.id)) return;
  togglingIds.add(task.id);
  try {
    await store.update(task.id, { enabled: !task.enabled });
    projects.showToast(task.enabled ? "任务已停用" : "任务已启用", "info");
  } catch (e) {
    const msg = extractErrorMessage(e);
    projects.showToast(`切换启停失败：${msg}`, "error");
  } finally {
    togglingIds.delete(task.id);
  }
}

const deleteTarget = ref<ScheduledTask | null>(null);
const deleting = ref(false);

async function confirmDelete(): Promise<void> {
  const task = deleteTarget.value;
  if (!task) return;
  deleting.value = true;
  try {
    await store.remove(task.id);
    projects.showToast("任务已删除", "info");
    deleteTarget.value = null;
  } catch (e) {
    projects.showToast(`删除失败：${extractErrorMessage(e)}`, "error");
  } finally {
    deleting.value = false;
  }
}

// --- 表单区(新建 / 编辑共用) --------------------------------------------

/** F2b:档位扩到 6 个 —— 固定时间(hourly/daily/weekdays/weekly/monthly)
 *  + 固定频率(interval,单位换算后仍存 every_min);CH11-1 补单次
 *  (once,指定时刻一次,fire 后自动完成,无结束条件)。 */
type FormKind =
  | "once"
  | "hourly"
  | "daily"
  | "weekdays"
  | "weekly"
  | "monthly"
  | "interval";

/** 档位下拉选项(单次在最前,固定时间次之、固定频率殿后)。 */
const KIND_OPTIONS: ReadonlyArray<{ value: FormKind; label: string }> = [
  { value: "once", label: "单次" },
  { value: "hourly", label: "每小时" },
  { value: "daily", label: "每天" },
  { value: "weekdays", label: "每工作日" },
  { value: "weekly", label: "每周" },
  { value: "monthly", label: "每月" },
  { value: "interval", label: "固定频率" },
];

/** 固定时间类档位(F2b 类型 A):结束条件 = 永不 / 次数;固定频率
 *  (类型 B)= 永不 / 结束日期(prd D10:UI 按类型限定,后端两列通用)。 */
const FIXED_TIME_KINDS: ReadonlySet<string> = new Set([
  "hourly",
  "daily",
  "weekdays",
  "weekly",
  "monthly",
]);

/** reka `update:model-value` 载荷归一化(SubagentsTab onModelChange
 *  同款:单选场景收掉防御性的数组分支)。 */
function normalizeSelectValue(v: unknown): string {
  if (Array.isArray(v)) return typeof v[0] === "string" ? v[0] : "";
  return typeof v === "string" ? v : "";
}

function onPickProject(v: unknown): void {
  form.projectId = normalizeSelectValue(v);
  form.targetSessionId = "";
}

function onPickSession(v: unknown): void {
  form.targetSessionId = normalizeSelectValue(v);
}

function onPickKind(v: unknown): void {
  const k = normalizeSelectValue(v);
  if (KIND_OPTIONS.some((o) => o.value === k)) {
    form.kind = k as FormKind;
    // 切换类型时收窄结束条件选项:单次档无结束条件、固定频率没有
    // 「次数」、固定时间没有「日期」,残留的 endMode 立即回落到
    // 「永不」避免提交非法组合。
    if (k === "once") form.endMode = "never";
    if (form.endMode === "count" && !FIXED_TIME_KINDS.has(k)) form.endMode = "never";
    if (form.endMode === "date" && FIXED_TIME_KINDS.has(k)) form.endMode = "never";
  }
}

function onPickIntervalUnit(v: unknown): void {
  const u = normalizeSelectValue(v);
  if (INTERVAL_UNITS.some((x) => x.value === u)) form.intervalUnit = u;
}

/** 模型下拉(新建专用 session 时可选):平铺列表带 provider 前缀
 *  (SubagentsTab 同款 —— reka 分组原语在 Tauri webview 会丢内容)。
 *  `?? []`:load 失败 / 未加载时 models 可能仍为 null(防御,下拉空)。 */
const flatModelOptions = computed(() =>
  (models.models ?? [])
    .slice()
    .sort(
      (a, b) =>
        a.providerDisplayName.localeCompare(b.providerDisplayName) ||
        a.displayName.localeCompare(b.displayName),
    ),
);

function onPickModel(v: unknown): void {
  const m = normalizeSelectValue(v);
  if (m === "" || flatModelOptions.value.some((o) => o.id === m)) form.modelId = m;
}

/** 本地 `yyyy-MM-ddTHH:mm`(input[type=datetime-local] 值)→ epoch ms。
 *  datetime-local 无时区后缀,`new Date(v)` 按本地时区解释。非法 → null。 */
function localDatetimeToMs(v: string): number | null {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(v)) return null;
  const t = new Date(v).getTime();
  return Number.isFinite(t) ? t : null;
}

/** epoch ms → 本地 `yyyy-MM-ddTHH:mm`(编辑回填 datetime-local)。 */
function msToLocalDatetimeStr(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

const formOpen = ref(false);
const editingId = ref<string | null>(null);
const saving = ref(false);
const formError = ref<string | null>(null);

const form = reactive({
  name: "",
  projectId: "",
  newDedicated: false,
  targetSessionId: "",
  kind: "daily" as FormKind,
  at: "09:00",
  /** once:datetime-local 值(提交转本地 epoch ms)。 */
  onceAt: "",
  /** hourly:每小时第几分钟(0-59)。 */
  hourlyMinute: 30,
  /** monthly:每月几号(1-31)。 */
  monthlyDay: 1,
  weekday: "mon",
  /** interval:数量 + 单位(提交时换算 every_min = 数量 × 单位分钟数)。 */
  intervalCount: 30,
  intervalUnit: "minute",
  /** F2b 结束条件:never / count(固定时间)/ date(固定频率)。 */
  endMode: "never" as "never" | "count" | "date",
  maxRuns: 5,
  /** yyyy-MM-dd(input[type=date] 值;提交转当日 23:59:59.999 本地 ms)。 */
  endDate: "",
  /** 新建专用 session 绑定的模型(空 = 沿用全局默认;仅创建态有意义)。 */
  modelId: "",
  prompt: "",
});

/** 固定时间类档位(决定结束条件选项与档位字段渲染)。 */
const isFixedTime = computed(() => FIXED_TIME_KINDS.has(form.kind));

/** 所选 project 下的 classic session 选项(群聊不是合法目标,AC7)。 */
const sessionOptions = computed<SessionSummary[]>(() => {
  const rows = sessionsByProject.get(form.projectId) ?? [];
  return rows.filter((s) => s.session_type === "chat");
});

/** 软警示:所选 session 已有 enabled 任务(编辑时排除自身)。不硬拒 ——
 *  调度器对同 session 每 tick 至多 fire 一个(design §9)。 */
const softWarning = computed<string | null>(() => {
  if (form.newDedicated || !form.targetSessionId) return null;
  const others = store.tasks.filter(
    (t) =>
      t.enabled &&
      t.target_session_id === form.targetSessionId &&
      t.id !== editingId.value,
  );
  if (others.length === 0) return null;
  const names = others.map((t) => `「${t.name}」`).join("、");
  return `该 session 已有定时任务 ${names},同时触发将合并为一轮处理`;
});

/** 本地 `yyyy-MM-dd` → 当日 23:59:59.999 的 epoch ms(D9:结束日含当日,
 *  当天到期点照常触发,次日起不再)。非法 → null。 */
function endOfDayMs(dateStr: string): number | null {
  const parts = dateStr.split("-").map(Number);
  if (parts.length !== 3 || parts.some((n) => !Number.isFinite(n))) return null;
  const [y, m, d] = parts as [number, number, number];
  const dt = new Date(y, m - 1, d, 23, 59, 59, 999);
  if (Number.isNaN(dt.getTime())) return null;
  return dt.getTime();
}

/** epoch ms → 本地 `yyyy-MM-dd`(编辑回填 `input[type=date]`)。 */
function msToDateStr(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function resetForm(): void {
  form.name = "";
  form.projectId = projects.currentProjectId ?? "";
  form.newDedicated = false;
  form.targetSessionId = "";
  form.kind = "daily";
  form.at = "09:00";
  form.onceAt = "";
  form.hourlyMinute = 30;
  form.monthlyDay = 1;
  form.weekday = "mon";
  form.intervalCount = 30;
  form.intervalUnit = "minute";
  form.endMode = "never";
  form.maxRuns = 5;
  form.endDate = "";
  form.modelId = "";
  form.prompt = "";
}

function openCreate(): void {
  editingId.value = null;
  resetForm();
  formOpen.value = true;
  void ensureSessionsFor(form.projectId ? [form.projectId] : []);
}

function openEdit(task: ScheduledTask): void {
  editingId.value = task.id;
  form.name = task.name;
  form.projectId = task.project_id;
  form.newDedicated = false;
  form.targetSessionId = task.target_session_id;
  const spec = task.schedule;
  if (spec?.kind === "once") {
    form.kind = "once";
    form.onceAt = msToLocalDatetimeStr(spec.at_ms);
  } else if (spec?.kind === "hourly") {
    form.kind = "hourly";
    form.hourlyMinute = spec.minute;
  } else if (spec?.kind === "daily") {
    form.kind = "daily";
    form.at = spec.at;
  } else if (spec?.kind === "weekdays") {
    form.kind = "weekdays";
    form.at = spec.at;
  } else if (spec?.kind === "weekly") {
    form.kind = "weekly";
    form.weekday = spec.weekday;
    form.at = spec.at;
  } else if (spec?.kind === "monthly") {
    form.kind = "monthly";
    form.monthlyDay = spec.day;
    form.at = spec.at;
  } else if (spec?.kind === "interval") {
    form.kind = "interval";
    const { n, unit } = splitEveryMin(spec.every_min);
    form.intervalCount = n;
    form.intervalUnit = unit;
  } else {
    form.kind = "daily";
    form.at = "09:00";
  }
  // 结束条件回填:按当前档位类型取可表达的(UI 限定单一条件;另一列
  // 提交时显式清空,保持 wire 与表单模型一致)。单次档无结束条件。
  if (form.kind === "once") {
    form.endMode = "never";
  } else if (form.kind === "interval") {
    if (task.ends_at !== null) {
      form.endMode = "date";
      form.endDate = msToDateStr(task.ends_at);
    } else {
      form.endMode = "never";
      form.endDate = "";
    }
  } else if (task.max_runs !== null) {
    form.endMode = "count";
    form.maxRuns = task.max_runs;
  } else {
    form.endMode = "never";
  }
  form.prompt = task.prompt;
  formOpen.value = true;
  formError.value = null;
  void ensureSessionsFor([task.project_id]);
}

function cancelForm(): void {
  formOpen.value = false;
  editingId.value = null;
  formError.value = null;
}

/** 表单态 → schedule preset 对象。字段非法(负数/越界)返回 null
 *  (submitForm 已前置校验,这里是防御兜底)。 */
function buildScheduleSpec(): ScheduleSpec | null {
  switch (form.kind) {
    case "once": {
      const t = localDatetimeToMs(form.onceAt);
      return t === null ? null : { kind: "once", at_ms: t };
    }
    case "hourly": {
      const m = Math.floor(Number(form.hourlyMinute));
      if (!Number.isFinite(m) || m < 0 || m > 59) return null;
      return { kind: "hourly", minute: m };
    }
    case "daily":
      return { kind: "daily", at: form.at };
    case "weekdays":
      return { kind: "weekdays", at: form.at };
    case "weekly":
      return { kind: "weekly", weekday: form.weekday, at: form.at };
    case "monthly": {
      const d = Math.floor(Number(form.monthlyDay));
      if (!Number.isFinite(d) || d < 1 || d > 31) return null;
      return { kind: "monthly", day: d, at: form.at };
    }
    case "interval": {
      const n = Math.floor(Number(form.intervalCount));
      const unit = INTERVAL_UNITS.find((u) => u.value === form.intervalUnit);
      if (!unit || !Number.isFinite(n) || n < 1 || n * unit.minutes < 1) return null;
      return { kind: "interval", every_min: n * unit.minutes };
    }
  }
}

async function submitForm(): Promise<void> {
  formError.value = null;
  const name = form.name.trim();
  const prompt = form.prompt.trim();
  if (!name) {
    formError.value = "任务名称不能为空";
    return;
  }
  if (!prompt) {
    formError.value = "任务提示词(prompt)不能为空";
    return;
  }
  if (!form.projectId) {
    formError.value = "请选择 project";
    return;
  }
  if (!form.newDedicated && !form.targetSessionId) {
    formError.value = "请选择目标 session,或勾选「新建专用 session」";
    return;
  }
  // 档位字段(按档位细分错误信息)。
  if (form.kind === "once") {
    const t = localDatetimeToMs(form.onceAt);
    if (t === null) {
      formError.value = "请选择单次触发的时间";
      return;
    }
    // 与后端 create/update 校验同语义(过期时刻一出生即完成,无意义)。
    if (t <= Date.now()) {
      formError.value = "单次任务的触发时间必须晚于当前时间";
      return;
    }
  }
  if (form.kind === "hourly") {
    const m = Math.floor(Number(form.hourlyMinute));
    if (!Number.isFinite(m) || m < 0 || m > 59) {
      formError.value = "分钟必须在 0-59 之间";
      return;
    }
  }
  if (form.kind === "monthly") {
    const d = Math.floor(Number(form.monthlyDay));
    if (!Number.isFinite(d) || d < 1 || d > 31) {
      formError.value = "每月几号必须在 1-31 之间";
      return;
    }
  }
  if (form.kind === "interval") {
    const n = Math.floor(Number(form.intervalCount));
    if (!Number.isFinite(n) || n < 1) {
      formError.value = "频率数量必须为正整数";
      return;
    }
  }
  // F2b 结束条件(单次档无结束条件:两个条件都清空提交)。
  let maxRuns: number | null = null;
  let endsAt: number | null = null;
  if (form.kind === "once") {
    // 无条件清空(下方 endMode 分支不进)。
  } else if (form.endMode === "count") {
    const m = Math.floor(Number(form.maxRuns));
    if (!Number.isFinite(m) || m < 1) {
      formError.value = "次数上限必须是不小于 1 的整数";
      return;
    }
    maxRuns = m;
  } else if (form.endMode === "date") {
    const t = endOfDayMs(form.endDate);
    if (t === null) {
      formError.value = "请选择结束日期";
      return;
    }
    // 今天合法(当日 23:59:59.999 仍未来临,当天到期点照常触发);
    // 早于今天 → 已结束的任务无意义。
    if (t <= Date.now()) {
      formError.value = "结束日期不能早于今天";
      return;
    }
    endsAt = t;
  }
  const spec = buildScheduleSpec();
  if (!spec) {
    formError.value = "触发计划字段不合法";
    return;
  }
  saving.value = true;
  try {
    const schedule = JSON.stringify(spec);
    if (editingId.value) {
      // update 显式带 maxRuns/endsAt(null = 清空):切换结束方式后旧值
      // 不残留(表单模型每档位只有一个条件)。
      await store.update(editingId.value, {
        name,
        prompt,
        schedule,
        ...(form.newDedicated ? {} : { targetSessionId: form.targetSessionId }),
        maxRuns,
        endsAt,
      });
      projects.showToast("任务已更新", "info");
    } else {
      await store.create({
        projectId: form.projectId,
        ...(form.newDedicated ? {} : { targetSessionId: form.targetSessionId }),
        name,
        prompt,
        schedule,
        ...(maxRuns !== null ? { maxRuns } : {}),
        ...(endsAt !== null ? { endsAt } : {}),
        // 指定模型仅「新建专用 session」分支生效(空 = 全局默认)。
        ...(form.newDedicated && form.modelId ? { modelId: form.modelId } : {}),
      });
      projects.showToast("任务已创建", "info");
    }
    cancelForm();
  } catch (e) {
    const msg = extractErrorMessage(e);
    formError.value = msg;
    projects.showToast(`保存定时任务失败：${msg}`, "error");
  } finally {
    saving.value = false;
  }
}

// --- 列表卡(F2b 状态徽章 + 触发进度) -------------------------------------

/** 状态徽章文案:自动完成的「已完成/已结束」区别于手动停用的「已停用」
 *  (F2b D8:完成任务保留在列表,可重新启用,计数清零)。单次档
 *  (CH11-1):已消费唯一触发点 = 已完成,过期未触发 = 已结束。 */
function stateLabel(task: ScheduledTask): string {
  if (task.enabled) return "启用中";
  if (completedByRunLimit(task)) return "已完成";
  if (completedByOnce(task)) return task.run_count >= 1 ? "已完成" : "已结束";
  if (completedByEndDate(task)) return "已结束";
  return "已停用";
}

/** 卡片的触发进度行:已触发 N/M 次 · 至 YYYY-MM-DD(任一结束条件设置时
 *  才渲染;纯「永不结束」任务的上次触发时间已有展示,不重复)。 */
function cardEndSummary(task: ScheduledTask): string {
  const parts = [`已触发 ${describeRunCount(task.run_count, task.max_runs)}`];
  if (task.ends_at !== null) parts.push(describeEndDate(task.ends_at));
  return parts.join(" · ");
}

function hasEndCondition(task: ScheduledTask): boolean {
  return task.max_runs !== null || task.ends_at !== null;
}

onMounted(async () => {
  await store.load();
  // 列表展示需要目标 session 的标题(按 project 现取,缓存)。
  const pids = [...new Set(store.tasks.map((t) => t.project_id))];
  if (pids.length > 0) void ensureSessionsFor(pids);
  // 模型下拉的数据源(新建专用 session 时选模型;失败静默,下拉显示空)。
  if (!models.loaded) void models.load().catch(() => {});
});
</script>

<template>
  <div class="sched-tab">
    <p class="sched-tab__intro">
      定时任务按计划向目标 session 自动注入一轮 agent 运行,结果落在该
      session 的消息流里。<strong>调度仅在 daemon 进程运行时生效</strong>
      (GUI 逃生模式可建任务但不会触发)。
      <span v-if="!config.scheduledTasksEnabled" class="sched-tab__killwarn">
        当前全局调度开关已关闭,任务不会被触发。
      </span>
    </p>

    <!-- 新建/编辑表单(打开时列表头的新建按钮隐藏,表单自带标题) -->
    <section v-if="formOpen" class="sched-tab__form" data-testid="sched-form">
      <h3 class="sched-tab__section-title">
        {{ editingId ? "编辑任务" : "新建任务" }}
      </h3>

      <Label class="sched-tab__field">
        <span class="sched-tab__label">任务名称</span>
        <input
          v-model="form.name"
          type="text"
          class="sched-tab__input"
          placeholder="如:每日早报"
        />
      </Label>

      <Label class="sched-tab__field">
        <span class="sched-tab__label">Project</span>
        <SelectRoot
          :model-value="form.projectId || undefined"
          @update:model-value="onPickProject"
        >
          <SelectTrigger
            class="sched-tab__trigger"
            data-testid="sched-project-select"
            aria-label="Project"
          >
            <SelectValue placeholder="选择 project" />
            <SelectIcon class="sched-tab__trigger-icon">
              <Icon name="chevron-down" :size="12" />
            </SelectIcon>
          </SelectTrigger>
          <SelectPortal>
            <SelectContent
              class="sched-tab__dropdown"
              position="popper"
              :side-offset="4"
            >
              <SelectViewport class="sched-tab__dropdown-viewport">
                <SelectItem
                  v-for="p in projects.projects"
                  :key="p.id"
                  :value="p.id"
                  class="sched-tab__option"
                >
                  <SelectItemText>{{ p.name }}</SelectItemText>
                </SelectItem>
              </SelectViewport>
            </SelectContent>
          </SelectPortal>
        </SelectRoot>
      </Label>

      <div class="sched-tab__field">
        <span class="sched-tab__label">目标 session</span>
        <!-- 「新建专用 session」仅创建态提供;编辑态换目标走下拉
             (update 不建新 session)。 -->
        <label v-if="!editingId" class="sched-tab__dedicated">
          <CheckboxRoot
            v-model="form.newDedicated"
            class="sched-tab__checkbox"
            data-testid="sched-new-dedicated"
          >
            <CheckboxIndicator class="sched-tab__checkbox-indicator">
              <Icon name="check" :size="12" />
            </CheckboxIndicator>
          </CheckboxRoot>
          新建专用 session(名称同任务名)
        </label>
        <SelectRoot
          v-if="!form.newDedicated"
          :model-value="form.targetSessionId || undefined"
          :disabled="!form.projectId"
          @update:model-value="onPickSession"
        >
          <SelectTrigger
            class="sched-tab__trigger"
            data-testid="sched-session-select"
            aria-label="目标 session"
          >
            <SelectValue
              :placeholder="form.projectId ? '选择 session(仅普通会话)' : '先选择 project'"
            />
            <SelectIcon class="sched-tab__trigger-icon">
              <Icon name="chevron-down" :size="12" />
            </SelectIcon>
          </SelectTrigger>
          <SelectPortal>
            <SelectContent
              class="sched-tab__dropdown"
              position="popper"
              :side-offset="4"
            >
              <SelectViewport class="sched-tab__dropdown-viewport">
                <SelectItem
                  v-for="s in sessionOptions"
                  :key="s.id"
                  :value="s.id"
                  class="sched-tab__option"
                >
                  <SelectItemText>{{ s.title }}</SelectItemText>
                </SelectItem>
              </SelectViewport>
            </SelectContent>
          </SelectPortal>
        </SelectRoot>
      </div>

      <!-- 新建专用 session 时的模型选择(空 = 沿用全局默认);写进新
           session 的 per-session 覆盖列,定时注入的轮次固定用该模型。 -->
      <div v-if="!editingId && form.newDedicated" class="sched-tab__field">
        <span class="sched-tab__label">专用 session 模型</span>
        <SelectRoot
          :model-value="form.modelId || undefined"
          @update:model-value="onPickModel"
        >
          <SelectTrigger
            class="sched-tab__trigger"
            data-testid="sched-model-select"
            aria-label="专用 session 模型"
          >
            <SelectValue placeholder="默认(跟随全局设置)" />
            <SelectIcon class="sched-tab__trigger-icon">
              <Icon name="chevron-down" :size="12" />
            </SelectIcon>
          </SelectTrigger>
          <SelectPortal>
            <SelectContent
              class="sched-tab__dropdown"
              position="popper"
              :side-offset="4"
            >
              <SelectViewport class="sched-tab__dropdown-viewport">
                <SelectItem
                  v-for="m in flatModelOptions"
                  :key="m.id"
                  :value="m.id"
                  class="sched-tab__option"
                >
                  <SelectItemText>{{ m.providerDisplayName }} · {{ m.displayName }}</SelectItemText>
                </SelectItem>
              </SelectViewport>
            </SelectContent>
          </SelectPortal>
        </SelectRoot>
      </div>

      <div class="sched-tab__field">
        <span class="sched-tab__label">触发计划</span>
        <div class="sched-tab__schedule-row">
          <SelectRoot :model-value="form.kind" @update:model-value="onPickKind">
            <SelectTrigger
              class="sched-tab__trigger sched-tab__kind"
              data-testid="sched-kind"
              aria-label="触发档位"
            >
              <SelectValue />
              <SelectIcon class="sched-tab__trigger-icon">
                <Icon name="chevron-down" :size="12" />
              </SelectIcon>
            </SelectTrigger>
            <SelectPortal>
              <SelectContent
                class="sched-tab__dropdown"
                position="popper"
                :side-offset="4"
              >
                <SelectViewport class="sched-tab__dropdown-viewport">
                  <SelectItem
                    v-for="k in KIND_OPTIONS"
                    :key="k.value"
                    :value="k.value"
                    class="sched-tab__option"
                  >
                    <SelectItemText>{{ k.label }}</SelectItemText>
                  </SelectItem>
                </SelectViewport>
              </SelectContent>
            </SelectPortal>
          </SelectRoot>
          <template v-if="form.kind === 'once'">
            <input
              v-model="form.onceAt"
              type="datetime-local"
              class="sched-tab__input sched-tab__datetime"
              data-testid="sched-once-at"
            />
            <span class="sched-tab__unit">(到点触发一次)</span>
          </template>
          <template v-else-if="form.kind === 'hourly'">
            <input
              v-model.number="form.hourlyMinute"
              type="number"
              min="0"
              max="59"
              step="1"
              class="sched-tab__input sched-tab__minutes"
              data-testid="sched-hourly-minute"
            />
            <span class="sched-tab__unit">分钟(每小时第几分钟)</span>
          </template>
          <template v-else-if="form.kind === 'daily'">
            <input v-model="form.at" type="time" class="sched-tab__input sched-tab__time" />
          </template>
          <template v-else-if="form.kind === 'weekdays'">
            <span class="sched-tab__unit">周一至五</span>
            <input v-model="form.at" type="time" class="sched-tab__input sched-tab__time" />
          </template>
          <template v-else-if="form.kind === 'weekly'">
            <SelectRoot v-model="form.weekday">
              <SelectTrigger
                class="sched-tab__trigger sched-tab__weekday"
                aria-label="星期"
              >
                <SelectValue />
                <SelectIcon class="sched-tab__trigger-icon">
                  <Icon name="chevron-down" :size="12" />
                </SelectIcon>
              </SelectTrigger>
              <SelectPortal>
                <SelectContent
                  class="sched-tab__dropdown"
                  position="popper"
                  :side-offset="4"
                >
                  <SelectViewport class="sched-tab__dropdown-viewport">
                    <SelectItem
                      v-for="w in WEEKDAY_OPTIONS"
                      :key="w.value"
                      :value="w.value"
                      class="sched-tab__option"
                    >
                      <SelectItemText>{{ w.label }}</SelectItemText>
                    </SelectItem>
                  </SelectViewport>
                </SelectContent>
              </SelectPortal>
            </SelectRoot>
            <input v-model="form.at" type="time" class="sched-tab__input sched-tab__time" />
          </template>
          <template v-else-if="form.kind === 'monthly'">
            <span class="sched-tab__unit">每月</span>
            <input
              v-model.number="form.monthlyDay"
              type="number"
              min="1"
              max="31"
              step="1"
              class="sched-tab__input sched-tab__minutes"
              data-testid="sched-monthly-day"
            />
            <span class="sched-tab__unit">号</span>
            <input v-model="form.at" type="time" class="sched-tab__input sched-tab__time" />
          </template>
          <template v-else>
            <input
              v-model.number="form.intervalCount"
              type="number"
              min="1"
              step="1"
              class="sched-tab__input sched-tab__minutes"
              data-testid="sched-interval-count"
            />
            <SelectRoot
              :model-value="form.intervalUnit"
              @update:model-value="onPickIntervalUnit"
            >
              <SelectTrigger
                class="sched-tab__trigger sched-tab__weekday"
                data-testid="sched-interval-unit"
                aria-label="频率单位"
              >
                <SelectValue />
                <SelectIcon class="sched-tab__trigger-icon">
                  <Icon name="chevron-down" :size="12" />
                </SelectIcon>
              </SelectTrigger>
              <SelectPortal>
                <SelectContent
                  class="sched-tab__dropdown"
                  position="popper"
                  :side-offset="4"
                >
                  <SelectViewport class="sched-tab__dropdown-viewport">
                    <SelectItem
                      v-for="u in INTERVAL_UNITS"
                      :key="u.value"
                      :value="u.value"
                      class="sched-tab__option"
                    >
                      <SelectItemText>每 {{ u.label }}</SelectItemText>
                    </SelectItem>
                  </SelectViewport>
                </SelectContent>
              </SelectPortal>
            </SelectRoot>
          </template>
        </div>
      </div>

      <!-- F2b 结束条件:固定时间 → 永不/次数;固定频率 → 永不/结束日期
           (prd D10:UI 按类型限定,提交时另一条件显式清空);单次档无
           结束条件,整块不渲染。 -->
      <div v-if="form.kind !== 'once'" class="sched-tab__field">
        <span class="sched-tab__label">结束条件</span>
        <div class="sched-tab__end-row" role="radiogroup" aria-label="结束条件">
          <label class="sched-tab__end-option">
            <input v-model="form.endMode" type="radio" value="never" name="sched-end" />
            永不结束
          </label>
          <label v-if="isFixedTime" class="sched-tab__end-option">
            <input v-model="form.endMode" type="radio" value="count" name="sched-end" />
            限定
            <input
              v-model.number="form.maxRuns"
              type="number"
              min="1"
              step="1"
              class="sched-tab__input sched-tab__count"
              data-testid="sched-max-runs"
              :disabled="form.endMode !== 'count'"
            />
            次
          </label>
          <label v-else class="sched-tab__end-option">
            <input v-model="form.endMode" type="radio" value="date" name="sched-end" />
            结束日期
            <input
              v-model="form.endDate"
              type="date"
              class="sched-tab__input sched-tab__date"
              data-testid="sched-end-date"
              :disabled="form.endMode !== 'date'"
            />
          </label>
        </div>
      </div>

      <Label class="sched-tab__field">
        <span class="sched-tab__label">提示词(每次触发注入的 user 消息)</span>
        <textarea
          v-model="form.prompt"
          class="sched-tab__input sched-tab__prompt"
          rows="4"
          placeholder="如:汇总昨天的工作进展"
        ></textarea>
      </Label>

      <p v-if="softWarning" class="sched-tab__softwarn" data-testid="sched-soft-warning" role="status">
        {{ softWarning }}
      </p>
      <p v-if="formError" class="sched-tab__error" role="alert">{{ formError }}</p>

      <div class="sched-tab__form-actions">
        <button type="button" class="btn btn--muted" @click="cancelForm">取消</button>
        <button
          type="button"
          class="btn btn--primary"
          data-testid="sched-submit"
          :disabled="saving"
          @click="submitForm"
        >
          {{ saving ? "保存中…" : editingId ? "保存修改" : "创建" }}
        </button>
      </div>
    </section>

    <!-- 任务卡片列表 -->
    <section class="sched-tab__list">
      <div class="sched-tab__list-head">
        <h3 class="sched-tab__section-title">任务列表</h3>
        <button
          v-if="!formOpen"
          type="button"
          class="btn btn--primary"
          data-testid="sched-create-btn"
          @click="openCreate"
        >
          <Icon name="plus" :size="12" />
          新建任务
        </button>
      </div>
      <p v-if="store.error" class="sched-tab__error" role="alert">{{ store.error }}</p>
      <p v-if="store.tasks.length === 0 && !store.error" class="sched-tab__empty">
        还没有定时任务。
      </p>
      <ul class="sched-tab__cards">
        <li
          v-for="task in store.tasks"
          :key="task.id"
          class="sched-tab__card"
          :class="{ 'sched-tab__card--disabled': !task.enabled }"
          :data-testid="`sched-card-${task.id}`"
        >
          <div class="sched-tab__card-main">
            <div class="sched-tab__card-head">
              <span class="sched-tab__card-name" :title="task.name">{{ task.name }}</span>
              <!-- 来源徽标(08-29-schedule-task-tool):agent 创建的显式标注,
                   user 创建不标(缺省态零噪音)。 -->
              <span
                v-if="task.created_by === 'agent'"
                class="sched-tab__card-origin"
                title="该任务由 agent 在对话中创建"
              >
                agent
              </span>
              <span
                class="sched-tab__card-state"
                :class="{
                  'sched-tab__card-state--off': !task.enabled && stateLabel(task) === '已停用',
                  'sched-tab__card-state--done':
                    !task.enabled && stateLabel(task) !== '已停用',
                }"
              >
                {{ stateLabel(task) }}
              </span>
            </div>
            <div class="sched-tab__card-meta">
              <Icon name="folder" :size="11" />
              {{ projectNameOf(task) }} · {{ sessionTitleOf(task) }}
            </div>
            <div class="sched-tab__card-meta">
              <Icon name="clock" :size="11" />
              {{ describeSchedule(task.schedule) }}
            </div>
            <div class="sched-tab__card-prompt" :title="task.prompt">
              {{ summarizePrompt(task.prompt) }}
            </div>
            <div class="sched-tab__card-fires">
              <span>上次:{{ formatFireTime(task.last_fired_at) }}</span>
              <span>下次:{{ formatFireTime(displayNextFireAt(task)) }}</span>
              <span v-if="hasEndCondition(task)">{{ cardEndSummary(task) }}</span>
            </div>
          </div>
          <div class="sched-tab__card-actions">
            <button
              type="button"
              role="switch"
              :aria-checked="task.enabled"
              class="sched-tab__switch"
              :class="{ 'sched-tab__switch--on': task.enabled }"
              :disabled="togglingIds.has(task.id)"
              :data-testid="`sched-toggle-${task.id}`"
              :title="task.enabled ? '停用' : '启用'"
              @click="toggleEnabled(task)"
            >
              <span class="sched-tab__switch-knob" />
            </button>
            <button
              type="button"
              class="btn btn--ghost sched-tab__card-btn"
              :data-testid="`sched-edit-${task.id}`"
              @click="openEdit(task)"
            >
              <Icon name="pencil" :size="12" />
              编辑
            </button>
            <button
              type="button"
              class="btn btn--ghost sched-tab__card-btn sched-tab__card-btn--danger"
              :data-testid="`sched-delete-${task.id}`"
              @click="deleteTarget = task"
            >
              <Icon name="x" :size="12" />
              删除
            </button>
          </div>
        </li>
      </ul>
    </section>

    <ConfirmDialog
      :open="deleteTarget !== null"
      title="删除定时任务"
      confirm-text="删除"
      @cancel="deleteTarget = null"
      @confirm="confirmDelete"
    >
      <p>
        确定删除任务「{{ deleteTarget?.name }}」?删除后不再触发,已产生的
        消息保留在目标 session 中。
      </p>
    </ConfirmDialog>
  </div>
</template>

<style scoped>
.sched-tab {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.sched-tab__intro {
  margin: 0 0 4px 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: 1.6;
}

.sched-tab__killwarn {
  display: block;
  margin-top: 2px;
  color: var(--color-status-warn);
}

.sched-tab__section-title {
  margin: 0;
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

/* 列表头:标题 + 新建按钮同行(按钮右对齐),不再让按钮独占一行。 */
.sched-tab__list-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

/* --- 表单(镜像 SearchTab form 容器)--- */

.sched-tab__form {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
}

.sched-tab__field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.sched-tab__label {
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
}

.sched-tab__input {
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  width: 100%;
  box-sizing: border-box;
}

.sched-tab__input:focus {
  outline: none;
  border-color: var(--color-accent);
}

/* --- reka Select(ProvidersTab/ModelForm 同款;trigger 字号与本表单
   input 一致(text-sm),弹层 option 与全局各下拉一致(text-base))--- */

.sched-tab__trigger {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  font-family: inherit;
  width: 100%;
  box-sizing: border-box;
  cursor: pointer;
  transition: border-color var(--duration-base) var(--ease-out);
}

.sched-tab__trigger:hover {
  border-color: var(--color-accent-muted);
}

.sched-tab__trigger[data-state="open"] {
  border-color: var(--color-accent);
}

.sched-tab__trigger[data-disabled] {
  opacity: 0.5;
  cursor: not-allowed;
}

.sched-tab__trigger-icon {
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
}

/* Portal children —— SelectPortal teleport 到 body,规范要求 :deep()
   (reka-ui-usage.md gotcha;宽度对齐 trigger 用 --reka-select-trigger-width)。 */
:deep(.sched-tab__dropdown) {
  position: fixed;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  min-width: var(--reka-select-trigger-width, 240px);
  width: var(--reka-select-trigger-width);
  z-index: var(--z-over-modal) !important;
  overflow: hidden;
}

:deep(.sched-tab__dropdown-viewport) {
  padding: 4px;
}

:deep(.sched-tab__option) {
  display: flex;
  align-items: center;
  padding: 6px 10px;
  font-size: var(--text-base);
  color: var(--color-text-primary);
  border-radius: var(--radius-sm);
  cursor: pointer;
  user-select: none;
}

:deep(.sched-tab__option[data-highlighted]) {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

:deep(.sched-tab__option[data-state="checked"]) {
  color: var(--color-accent-text);
}

.sched-tab__dedicated {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  cursor: pointer;
}

/* reka Checkbox(ModelForm 同款基座)。 */
.sched-tab__checkbox {
  width: 16px;
  height: 16px;
  flex-shrink: 0;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 3px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
}

.sched-tab__checkbox[data-state="checked"] {
  background: var(--color-accent);
  border-color: var(--color-accent);
}

.sched-tab__checkbox-indicator {
  color: #fff;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  line-height: 0;
}

/* 档位行:档位下拉 + 按 kind 的参数控件。桌面一行,控件适度伸展
   (flex-grow + max-width 上限)避免只在行首挤一小撮;窄屏换行铺满。 */
.sched-tab__schedule-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  flex-wrap: wrap;
}

.sched-tab__kind {
  flex: 1 1 96px;
  max-width: 140px;
}

.sched-tab__time {
  flex: 1 1 116px;
  max-width: 180px;
}

/* 单次档的 datetime-local(日期+时刻,比 time 输入宽)。 */
.sched-tab__datetime {
  flex: 1 1 180px;
  max-width: 230px;
}

.sched-tab__minutes {
  flex: 1 1 90px;
  max-width: 140px;
}

.sched-tab__weekday {
  flex: 1 1 92px;
  max-width: 150px;
}

.sched-tab__unit {
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  flex-shrink: 0;
}

/* F2b 结束条件:radiogroup + 内联的条件参数(次数 / 日期)。 */
.sched-tab__end-row {
  display: flex;
  align-items: center;
  gap: 16px;
  min-width: 0;
  flex-wrap: wrap;
}

.sched-tab__end-option {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  cursor: pointer;
  min-height: 44px; /* 触控目标(settings-modal 44px 规则)。 */
}

.sched-tab__end-option input[type="radio"] {
  accent-color: var(--color-accent, currentcolor);
  margin: 0;
}

.sched-tab__end-option input[type="radio"]:focus-visible {
  outline: 2px solid var(--color-accent, currentcolor);
  outline-offset: 2px;
}

.sched-tab__count {
  width: 72px;
  flex: 0 0 72px;
}

.sched-tab__date {
  flex: 1 1 140px;
  max-width: 200px;
}

.sched-tab__prompt {
  resize: vertical;
  min-height: 72px;
  font-family: inherit;
  line-height: 1.5;
}

.sched-tab__form-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}

/* --- 软警示 / 错误 --- */

.sched-tab__softwarn {
  margin: 0;
  font-size: var(--text-xs);
  line-height: 1.5;
  padding: 4px 8px;
  border-radius: var(--radius-sm);
  color: var(--color-status-warn);
  background: color-mix(in srgb, var(--color-status-warn) 8%, transparent);
  border-left: 2px solid var(--color-status-warn);
}

.sched-tab__error {
  margin: 0;
  font-size: var(--text-xs);
  line-height: 1.5;
  padding: 4px 8px;
  border-radius: var(--radius-sm);
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 2px solid var(--color-tool-error);
}

/* --- 列表 --- */

.sched-tab__list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.sched-tab__empty {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--color-text-muted);
}

.sched-tab__cards {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.sched-tab__card {
  display: flex;
  gap: 10px;
  padding: 10px 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
}

/* 停用行灰显(design §7):整卡降不透明度,展示值仍可读。 */
.sched-tab__card--disabled {
  opacity: 0.55;
}

.sched-tab__card-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.sched-tab__card-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.sched-tab__card-name {
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sched-tab__card-state {
  flex-shrink: 0;
  font-size: var(--text-2-xs);
  color: var(--color-tool-write);
}

/* 来源徽标:agent 创建的任务(08-29-schedule-task-tool)。与状态徽章
   同排、更低调(描边式,user 任务不渲染 = 缺省态零噪音)。 */
.sched-tab__card-origin {
  flex-shrink: 0;
  font-size: var(--text-2-xs);
  line-height: 1;
  padding: 2px 5px;
  border: 1px solid var(--color-border-primary);
  border-radius: 999px;
  color: var(--color-text-secondary);
}

.sched-tab__card-state--off {
  color: var(--color-text-muted);
}

/* F2b 完成态(自动停用,区别于手动停用的灰显)。 */
.sched-tab__card-state--done {
  color: var(--color-status-success);
}

.sched-tab__card-meta {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sched-tab__card-prompt {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sched-tab__card-fires {
  display: flex;
  gap: 12px;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-muted);
  flex-wrap: wrap;
}

.sched-tab__card-actions {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 6px;
  flex-shrink: 0;
}

.sched-tab__card-btn {
  padding: 3px 8px;
  font-size: var(--text-xs);
}

.sched-tab__card-btn--danger {
  color: var(--color-tool-error-text);
}

/* 启停 switch:手搓 role="switch"(36×20 药丸 + 滑块),启用态 accent。 */
.sched-tab__switch {
  width: 36px;
  height: 20px;
  border-radius: 999px;
  border: 1px solid var(--color-bg-border-strong);
  background: var(--color-bg-app);
  padding: 0;
  position: relative;
  cursor: pointer;
  transition: background var(--duration-base) var(--ease-out),
    border-color var(--duration-base) var(--ease-out);
}

.sched-tab__switch--on {
  background: var(--color-accent);
  border-color: var(--color-accent);
}

.sched-tab__switch:disabled {
  opacity: 0.5;
  cursor: default;
}

.sched-tab__switch-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--color-text-primary);
  transition: transform var(--duration-base) var(--ease-out);
}

.sched-tab__switch--on .sched-tab__switch-knob {
  transform: translateX(16px);
  background: var(--color-text-on-accent);
}

/* --- S6b 移动端(320-430px 零溢出):表单/卡片纵向堆叠,输入全宽。
   触控目标由 style.css 全局 `.settings-modal button { min-height:44px }`
   覆盖;启停 switch 是次要状态切换(DEC-6"44px 只给主操作"的 chip
   例外),这里显式压回全局 min 规则,用 56×28 药丸保持开关形状。 --- */
@media (max-width: 767px) {
  .sched-tab__card {
    flex-direction: column;
  }

  /* 复选框与 switch 同理(DEC-6 chip 例外):CheckboxRoot 渲染为
     <button>,全局 44px min 规则会把 16px 视觉盒撑成大方块;触控
     目标改由外层整行 <label> 承担 —— checkbox 压回视觉尺寸,
     label 行保 44px 高。 */
  .sched-tab__checkbox {
    min-width: 0;
    min-height: 0;
  }

  .sched-tab__dedicated {
    min-height: 44px;
  }

  .sched-tab__card-actions {
    flex-direction: row;
    align-items: center;
    justify-content: flex-end;
    width: 100%;
  }

  .sched-tab__kind,
  .sched-tab__time,
  .sched-tab__datetime,
  .sched-tab__minutes,
  .sched-tab__weekday {
    flex: 1 1 auto;
    width: auto;
    min-width: 0;
  }

  .sched-tab__form-actions {
    flex-wrap: wrap;
  }

  .sched-tab__switch {
    min-width: 0;
    min-height: 0;
    width: 56px;
    height: 28px;
  }

  .sched-tab__switch-knob {
    top: 3px;
    left: 3px;
    width: 20px;
    height: 20px;
  }

  .sched-tab__switch--on .sched-tab__switch-knob {
    transform: translateX(26px);
  }
}
</style>
