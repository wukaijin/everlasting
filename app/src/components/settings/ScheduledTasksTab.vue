<script setup lang="ts">
// ScheduledTasksTab — Settings 第 8 个 tab「定时任务」(F2, task
// `08-28-f2-scheduled-tasks` design §7)。
//
// 一面两区:
//   1. 任务卡片列表 —— 名称 / 目标 session·project / schedule 人话 /
//      prompt 摘要 / 上次·下次触发 / 启停 switch / 删除(ConfirmDialog
//      确认)。停用行灰显,仍展示 schedule 下一到期点(存库展示值)。
//   2. 新建/编辑表单 —— project 下拉 → session 下拉(按 project 过滤,
//      仅 classic;勾选「新建专用 session」则不选)→ 档位(daily 时间 /
//      interval 数字 / weekly 周几+时间)→ prompt textarea。
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
import { transport } from "../../transport";
import { extractErrorMessage } from "../../utils/useErrorBus";
import {
  describeSchedule,
  formatFireTime,
  summarizePrompt,
  WEEKDAY_OPTIONS,
} from "../../utils/scheduledTaskFormat";

const store = useScheduledTasksStore();
const projects = useProjectsStore();
const config = useConfigStore();

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

type FormKind = "daily" | "interval" | "weekly";

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
  if (k === "daily" || k === "interval" || k === "weekly") form.kind = k;
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
  everyMin: 30,
  weekday: "mon",
  prompt: "",
});

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

function resetForm(): void {
  form.name = "";
  form.projectId = projects.currentProjectId ?? "";
  form.newDedicated = false;
  form.targetSessionId = "";
  form.kind = "daily";
  form.at = "09:00";
  form.everyMin = 30;
  form.weekday = "mon";
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
  if (spec?.kind === "daily") {
    form.kind = "daily";
    form.at = spec.at;
  } else if (spec?.kind === "interval") {
    form.kind = "interval";
    form.everyMin = spec.every_min;
  } else if (spec?.kind === "weekly") {
    form.kind = "weekly";
    form.weekday = spec.weekday;
    form.at = spec.at;
  } else {
    form.kind = "daily";
    form.at = "09:00";
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

/** 表单态 → schedule preset 对象。interval 非法(非正整数)返回 null。 */
function buildScheduleSpec(): ScheduleSpec | null {
  switch (form.kind) {
    case "daily":
      return { kind: "daily", at: form.at };
    case "weekly":
      return { kind: "weekly", weekday: form.weekday, at: form.at };
    case "interval": {
      const n = Math.floor(Number(form.everyMin));
      if (!Number.isFinite(n) || n < 1) return null;
      return { kind: "interval", every_min: n };
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
  const spec = buildScheduleSpec();
  if (!spec) {
    formError.value = "interval 分钟数必须为正整数";
    return;
  }
  saving.value = true;
  try {
    const schedule = JSON.stringify(spec);
    if (editingId.value) {
      await store.update(editingId.value, {
        name,
        prompt,
        schedule,
        ...(form.newDedicated ? {} : { targetSessionId: form.targetSessionId }),
      });
      projects.showToast("任务已更新", "info");
    } else {
      await store.create({
        projectId: form.projectId,
        ...(form.newDedicated ? {} : { targetSessionId: form.targetSessionId }),
        name,
        prompt,
        schedule,
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

onMounted(async () => {
  await store.load();
  // 列表展示需要目标 session 的标题(按 project 现取,缓存)。
  const pids = [...new Set(store.tasks.map((t) => t.project_id))];
  if (pids.length > 0) void ensureSessionsFor(pids);
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
                  <SelectItem value="daily" class="sched-tab__option">
                    <SelectItemText>每天</SelectItemText>
                  </SelectItem>
                  <SelectItem value="interval" class="sched-tab__option">
                    <SelectItemText>间隔</SelectItemText>
                  </SelectItem>
                  <SelectItem value="weekly" class="sched-tab__option">
                    <SelectItemText>每周</SelectItemText>
                  </SelectItem>
                </SelectViewport>
              </SelectContent>
            </SelectPortal>
          </SelectRoot>
          <template v-if="form.kind === 'daily'">
            <input v-model="form.at" type="time" class="sched-tab__input sched-tab__time" />
          </template>
          <template v-else-if="form.kind === 'interval'">
            <input
              v-model.number="form.everyMin"
              type="number"
              min="1"
              step="1"
              class="sched-tab__input sched-tab__minutes"
              data-testid="sched-every-min"
            />
            <span class="sched-tab__unit">分钟</span>
          </template>
          <template v-else>
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
              <span
                class="sched-tab__card-state"
                :class="{ 'sched-tab__card-state--off': !task.enabled }"
              >
                {{ task.enabled ? "启用中" : "已停用" }}
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
              <span>下次:{{ formatFireTime(task.next_fire_at) }}</span>
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
  font-size: var(--text-2xs);
  color: var(--color-tool-write);
}

.sched-tab__card-state--off {
  color: var(--color-text-muted);
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
