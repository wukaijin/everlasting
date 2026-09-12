<script setup lang="ts">
// GroupChatPresetsTab — Settings「群聊预设」页(GCE-P1, task
// `09-12-gc-preset-settings`;覆盖层 GCE-P1b, task `09-12-gc-preset-override`)。
//
// 一面三区(SubagentsTab 行结构 + ScheduledTasksTab 表单结构拼装):
//   1. 内置四档列表 —— 名称 + 描述 + 徽标 + 覆盖管理:内置预设是
//      scripts/group-chat-presets.json 单一事实源(M1 CLI / MCP 消费 +
//      跨机器可移植性依赖 JSON 原样),但允许本机**覆盖行**原位顶替
//      (GCE-P1b):「覆盖编辑」预填 JSON 定义(模型名经 resolveModelRef
//      全目录解析成 UUID,解析不出留空逼用户重选 —— 正是修复场景),
//      存成带 builtin_key 的 DB 行;已覆盖行显示「已覆盖」徽标 +
//      「编辑覆盖」/「恢复内置」(恢复 = 删除覆盖行,回落 JSON 定义)。
//      覆盖行在用户预设列表区不重复出现(内置区原位管理,无双管理面)。
//   2. 用户预设列表 —— 名称 / 描述 / 主持人 + 参与者摘要,编辑 / 删除
//      (ConfirmDialog 确认),行级 spinner + 行级错误。
//   3. 新增 / 编辑 / 覆盖表单 —— 名称、描述、主持人 Select、参与者
//      2-3 行(名字 input + 模型 Select + persona Select 五档),加减
//      按钮(2 下限禁删 / 3 上限隐藏加,GroupChatConfigModal D5 边界)。
//
// 模型引用:用户预设存 models.id UUID(改名不断链);表单 Select 选项
// = 启用模型 ∪ 本行当前值(SubagentsTab rowModelOptions 模式 —— 禁用
// 模型本行回显不泄漏到别行)。覆盖预填走**全目录**解析(禁用也回显,
// resolveModelRef byId/名双趟)。
//
// 校验:前端预校验镜像 Rust 规则(commands/group_chat_presets.rs
// validate_preset_input,单一事实源在服务端):名称 trim 非空 ≤40、
// 与内置 key / 其它用户行大小写不敏感不重名、描述 ≤200、参与者 2..=3
// 人且名字非空 ≤20 预设内唯一、persona 五 kind 白名单。服务端仍是
// 事实源 —— 服务端错误同样内联 + toast(同 key 重复覆盖 400 亦走此路)。
//
// persona 五档 label 为纯 UI 映射;改 kind 域必须三处同步:
// scripts/group-chat-presets.json / commands group_chat_presets.rs /
// 本表(PERSONA_OPTIONS + 上方 Rust 注释的同步义务同源)。

import { computed, onMounted, reactive, ref } from "vue";
import {
  Label,
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
import {
  useGroupChatPresetsStore,
  type GcPresetInput,
  type GcPresetRow,
} from "../../stores/groupChatPresets";
import { useModelsStore, isModelEffectivelyDisabled, type ModelWithProvider } from "../../stores/models";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";
import { GC_PRESETS, resolveModelRef } from "../../utils/groupChatPresets";

const store = useGroupChatPresetsStore();
const models = useModelsStore();
const projects = useProjectsStore();

/** 内置四档(JSON 声明序:review / fe_review / arch / retro)。静态
 *  JSON 直读 —— 只读展示不经过 store(store 只管用户行)。 */
const builtinEntries = Object.entries(GC_PRESETS.presets);

/** persona 五档(value = kind,label 纯 UI)。 */
const PERSONA_OPTIONS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "arch", label: "架构" },
  { value: "product", label: "产品" },
  { value: "backend", label: "后端" },
  { value: "frontend", label: "前端" },
  { value: "outsider", label: "局外" },
];

/** 参与者边界(GroupChatConfigModal D5 同源)。 */
const MIN_PARTICIPANTS = 2;
const MAX_PARTICIPANTS = 3;

/** 内置覆盖行索引(内置 key → 行,GCE-P1b)。内置区三态展示、
 *  「编辑覆盖」与「恢复内置」动作的数据源。 */
const overrideByKey = computed<Map<string, GcPresetRow>>(() => {
  const m = new Map<string, GcPresetRow>();
  for (const row of store.rows) {
    if (row.builtinKey) m.set(row.builtinKey, row);
  }
  return m;
});

/** 内置区渲染行:def + 该 key 的覆盖行(若有)。模板免 `get(key)!`
 *  反复解引用。 */
const builtinRows = computed(() =>
  builtinEntries.map(([key, def]) => ({
    key,
    def,
    override: overrideByKey.value.get(key) ?? null,
  })),
);

/** 用户列表(后端 ORDER BY name;重排防御同 SubagentsTab sortedRows)。
 *  覆盖行不进此列表 —— 在内置区原位管理,避免双管理面。 */
const sortedRows = computed<GcPresetRow[]>(() =>
  [...store.rows]
    .filter((r) => !r.builtinKey)
    .sort((a, b) => a.name.localeCompare(b.name)),
);

/** 模型 UUID → 「provider · 显示名」(目录缺失回退 UUID 前 8 位 ——
 *  模型被删后行仍可读,ScheduledTasksTab modelDisplayName 同款)。 */
function modelDisplay(modelId: string): string {
  const m = (models.models ?? []).find((x) => x.id === modelId);
  return m ? `${m.providerDisplayName} · ${m.displayName}` : modelId.slice(0, 8);
}

/** 列表行的参与者摘要。 */
function rosterSummary(row: GcPresetRow): string {
  return row.participants
    .map((p) => `${p.name}(${modelDisplay(p.modelId)})`)
    .join("、");
}

// --- 模型选项(启用 ∪ 本字段当前值)---------------------------------------

/** SubagentsTab rowModelOptions 同款:启用模型 ∪ **本字段**当前值 ——
 *  禁用模型不可被新选,但字段已指向它时仍作为选项回显(可见可切走)。 */
function modelOptionsFor(currentId: string | null | undefined): ModelWithProvider[] {
  const pinnedId = currentId ?? "";
  return (models.models ?? [])
    .filter((m) => m.id === pinnedId || !isModelEffectivelyDisabled(m))
    .slice()
    .sort(
      (a, b) =>
        a.providerDisplayName.localeCompare(b.providerDisplayName) ||
        a.displayName.localeCompare(b.displayName),
    );
}

const moderatorOptions = computed(() => modelOptionsFor(form.moderatorModelId));

// --- 表单(新增 / 编辑共用) -----------------------------------------------

const formOpen = ref(false);
const editingId = ref<string | null>(null);
/** 覆盖表单的内置 key(GCE-P1b):非空 = 本次表单是内置档覆盖编辑
 *  (预填态或编辑已有覆盖行)。提交走 create(builtinKey)或
 *  update(覆盖行 id);openCreate / openEdit(普通行)/ cancelForm
 *  三处归零,防普通编辑误挂覆盖语义。 */
const overridingKey = ref<string | null>(null);
const saving = ref(false);
const formError = ref<string | null>(null);

/** 表单标题:覆盖态显示目标内置 key,普通态沿用新增 / 编辑。 */
const formTitle = computed(() => {
  if (overridingKey.value) return `覆盖内置预设:${overridingKey.value}`;
  return editingId.value ? "编辑预设" : "新增预设";
});

/** 参与者草稿行(表单内形状 = store 输入形状)。 */
interface ParticipantDraft {
  name: string;
  modelId: string;
  persona: string;
}

const form = reactive({
  name: "",
  description: "",
  moderatorModelId: "",
  participants: [] as ParticipantDraft[],
});

function resetForm(): void {
  const first = models.enabledModels[0]?.id ?? "";
  form.name = "";
  form.description = "";
  form.moderatorModelId = first;
  form.participants = [
    { name: "", modelId: first, persona: "arch" },
    { name: "", modelId: first, persona: "product" },
  ];
}

function openCreate(): void {
  editingId.value = null;
  overridingKey.value = null;
  formError.value = null;
  resetForm();
  formOpen.value = true;
}

function openEdit(row: GcPresetRow): void {
  editingId.value = row.id;
  // 普通行编辑清掉可能残留的覆盖语义(openOverrideEdit 对已有覆盖行
  // 先走本函数再补设 overridingKey)。
  overridingKey.value = null;
  formError.value = null;
  form.name = row.name;
  form.description = row.description;
  form.moderatorModelId = row.moderatorModelId;
  form.participants = row.participants.map((p) => ({
    name: p.name,
    modelId: p.modelId,
    persona: p.persona,
  }));
  formOpen.value = true;
}

/** 内置档「覆盖编辑」(GCE-P1b):已有覆盖行 → 常规编辑该行(editingId
 *  路径,提交走 update);否则自 JSON def 预填新覆盖行(提交走 create
 *  + builtinKey)。模型引用按**全目录**解析(禁用也回显 —— 表单选项 =
 *  启用 ∪ 当前值既有模式);目录里已删的模型解析不出留空,校验逼用户
 *  重选 —— 这正是覆盖要修复的场景。name 留空(display 名与链接键
 *  分离,覆盖行仍须起一个不撞内置 key 的管理面名字)。 */
function openOverrideEdit(key: string): void {
  const existing = overrideByKey.value.get(key);
  if (existing) {
    openEdit(existing);
    overridingKey.value = key;
    return;
  }
  const def = GC_PRESETS.presets[key];
  if (!def) return; // 防御:按钮只在内置行渲染,key 恒 ∈ JSON。
  editingId.value = null;
  overridingKey.value = key;
  formError.value = null;
  form.name = "";
  form.description = def.description;
  form.moderatorModelId =
    resolveModelRef(models.models ?? [], def.moderator_model) ?? "";
  form.participants = def.participants.map((p) => ({
    name: p.name,
    modelId: resolveModelRef(models.models ?? [], p.model) ?? "",
    persona: p.persona,
  }));
  formOpen.value = true;
}

function cancelForm(): void {
  formOpen.value = false;
  editingId.value = null;
  overridingKey.value = null;
  formError.value = null;
}

function addParticipant(): void {
  if (form.participants.length >= MAX_PARTICIPANTS) return;
  form.participants.push({
    name: "",
    modelId: models.enabledModels[0]?.id ?? "",
    persona: "arch",
  });
}

function removeParticipant(idx: number): void {
  if (form.participants.length <= MIN_PARTICIPANTS) return;
  form.participants.splice(idx, 1);
}

/** reka `update:model-value` 载荷归一化(ScheduledTasksTab 同款)。 */
function normalizeSelectValue(v: unknown): string {
  if (Array.isArray(v)) return typeof v[0] === "string" ? v[0] : "";
  return typeof v === "string" ? v : "";
}

function onPickModerator(v: unknown): void {
  form.moderatorModelId = normalizeSelectValue(v);
}

function onPickParticipantModel(idx: number, v: unknown): void {
  form.participants[idx].modelId = normalizeSelectValue(v);
}

function onPickParticipantPersona(idx: number, v: unknown): void {
  const kind = normalizeSelectValue(v);
  if (PERSONA_OPTIONS.some((o) => o.value === kind)) {
    form.participants[idx].persona = kind;
  }
}

// --- 前端预校验(镜像 Rust validate_preset_input;服务端仍是事实源) ------

/** JS 侧按 Unicode 码点计数(Rust chars().count() 同口径)。 */
function charCount(s: string): number {
  return [...s].length;
}

function validateForm(): string | null {
  const name = form.name.trim();
  if (!name) return "预设名称不能为空";
  if (charCount(name) > 40) return "预设名称过长(最多 40 字符)";
  // 与内置 key 大小写不敏感不撞(Rust BUILTIN_PRESET_KEYS 同源)。
  const nameLc = name.toLowerCase();
  if (builtinEntries.some(([k]) => k.toLowerCase() === nameLc)) {
    return `预设名称「${name}」与内置预设冲突,请换一个名称`;
  }
  // 与其它用户行大小写不敏感不重名(update 排除自身)。
  if (
    sortedRows.value.some(
      (r) => r.id !== editingId.value && r.name.toLowerCase() === nameLc,
    )
  ) {
    return `预设名称「${name}」已存在,请换一个名称`;
  }
  if (charCount(form.description.trim()) > 200) {
    return "预设描述过长(最多 200 字符)";
  }
  if (!form.moderatorModelId) return "请选择主持人模型";
  if (
    form.participants.length < MIN_PARTICIPANTS ||
    form.participants.length > MAX_PARTICIPANTS
  ) {
    return `参与者数量必须是 ${MIN_PARTICIPANTS}~${MAX_PARTICIPANTS} 人,当前 ${form.participants.length} 人`;
  }
  const seen = new Set<string>();
  for (const p of form.participants) {
    const pn = p.name.trim();
    if (!pn) return "参与者名字不能为空";
    if (charCount(pn) > 20) return `参与者名字过长(最多 20 字符):${pn}`;
    if (seen.has(pn)) return `参与者重名:「${pn}」`;
    seen.add(pn);
    if (!PERSONA_OPTIONS.some((o) => o.value === p.persona)) {
      return `参与者「${pn}」的 persona 非法(仅支持 ${PERSONA_OPTIONS.map((o) => o.value).join(" / ")})`;
    }
    if (!p.modelId) return `参与者「${pn}」未选择模型`;
  }
  return null;
}

async function submitForm(): Promise<void> {
  formError.value = null;
  const invalid = validateForm();
  if (invalid) {
    formError.value = invalid;
    return;
  }
  saving.value = true;
  try {
    const input: GcPresetInput = {
      name: form.name.trim(),
      description: form.description.trim(),
      moderatorModelId: form.moderatorModelId,
      participants: form.participants.map((p) => ({
        name: p.name.trim(),
        modelId: p.modelId,
        persona: p.persona,
      })),
    };
    if (editingId.value) {
      await store.update(editingId.value, input);
      projects.showToast(
        overridingKey.value ? "覆盖已更新" : "预设已更新",
        "info",
      );
    } else if (overridingKey.value) {
      // 覆盖行新建:builtin_key 随载荷落库(同 key 二次覆盖由服务端
      // 400 拦截 —— 表单开着,错误内联展示)。
      await store.create({ ...input, builtinKey: overridingKey.value });
      projects.showToast("已覆盖内置预设", "info");
    } else {
      await store.create(input);
      projects.showToast("预设已创建", "info");
    }
    cancelForm();
  } catch (e) {
    const msg = extractErrorMessage(e);
    formError.value = msg;
    projects.showToast(`保存群聊预设失败:${msg}`, "error");
  } finally {
    saving.value = false;
  }
}

// --- 列表行:删除 / 恢复内置(ConfirmDialog)+ 行级错误 ---------------------

const deleteTarget = ref<GcPresetRow | null>(null);

/** 待恢复的内置档覆盖行(GCE-P1b)。恢复 = 删覆盖行(同一后端
 *  delete),回落 JSON 源码定义;单独 ConfirmDialog 与普通删除区分
 *  文案,判别不靠 deleteTarget 猜类型。 */
const restoreTarget = ref<GcPresetRow | null>(null);

/** 行级错误(删除 / 恢复失败等;编辑打开前清掉对应行旧错)。 */
const rowErrors = ref<Record<string, string>>({});

function isRowBusy(id: string): boolean {
  return store.spinnerById.has(id);
}

async function confirmDelete(): Promise<void> {
  const row = deleteTarget.value;
  if (!row) return;
  delete rowErrors.value[row.id];
  try {
    await store.remove(row.id);
    projects.showToast("预设已删除", "info");
    deleteTarget.value = null;
  } catch (e) {
    const msg = extractErrorMessage(e);
    rowErrors.value[row.id] = msg;
    projects.showToast(`删除失败:${msg}`, "error");
  }
}

/** 恢复内置 = 删除覆盖行(store.remove 同一后端 delete)。已建定时
 *  任务不受影响(快照语义);两消费方选中该档即回落 JSON 定义
 *  (mergedPresets 原位顶替随行删除自动解除)。 */
async function confirmRestore(): Promise<void> {
  const row = restoreTarget.value;
  if (!row) return;
  delete rowErrors.value[row.id];
  try {
    await store.remove(row.id);
    projects.showToast(`已恢复内置预设「${row.builtinKey}」`, "info");
    restoreTarget.value = null;
  } catch (e) {
    const msg = extractErrorMessage(e);
    rowErrors.value[row.id] = msg;
    projects.showToast(`恢复失败:${msg}`, "error");
  }
}

// --- 加载 -----------------------------------------------------------------

onMounted(async () => {
  // 模型目录(表单下拉与摘要显示的数据源;可能没逛过 Models tab)。
  if (!models.loaded) {
    await models.load().catch(() => {
      // 失败静默:下拉为空 + 摘要回退 UUID 前 8 位,store.loaded 保持
      // false,后续交互重试。
    });
  }
  try {
    await store.load();
  } catch (e) {
    projects.showToast(`加载群聊预设失败:${extractErrorMessage(e)}`, "error");
  }
});
</script>

<template>
  <div class="gcp-tab">
    <p class="gcp-tab__intro">
      用户群聊预设存本机数据库,模型引用按 UUID 记录(模型改名不断链)。
      定时任务与建群弹窗的「审议预设」会叠加在内置四档之后。内置四档仍是
      scripts/group-chat-presets.json 单一事实源,但可被本机覆盖行原位覆盖
      (「覆盖编辑」修阵容;删除覆盖行即恢复源码定义);群聊脚本(M1/MCP)
      仍只认源码 JSON,不消费覆盖行与用户预设。
    </p>

    <!-- 内置四档:源码定义 + 本机覆盖管理(GCE-P1b)。 -->
    <section class="gcp-tab__builtin" data-testid="gcp-builtin-list">
      <h3 class="gcp-tab__section-title">内置预设(源码定义 · 可覆盖)</h3>
      <ul class="gcp-tab__builtin-list">
        <li
          v-for="b in builtinRows"
          :key="b.key"
          class="gcp-tab__builtin-row"
          :data-testid="`gcp-builtin-${b.key}`"
        >
          <div class="gcp-tab__row-header">
            <span class="gcp-tab__name">{{ b.key }}</span>
            <span
              v-if="b.override"
              class="gcp-tab__source-chip gcp-tab__source-chip--override"
              data-testid="gcp-overridden-chip"
            >已覆盖</span>
            <span v-else class="gcp-tab__source-chip">内置</span>
          </div>
          <p class="gcp-tab__description">
            {{ b.override ? b.override.description : b.def.description }}
          </p>
          <!-- 已覆盖时阵容摘要来自覆盖行(UUID 显示名同用户行)。 -->
          <p
            v-if="b.override"
            class="gcp-tab__roster"
            :title="rosterSummary(b.override)"
          >
            主持人 {{ modelDisplay(b.override.moderatorModelId) }} · 参与
            {{ rosterSummary(b.override) }}
          </p>
          <div class="gcp-tab__row-actions">
            <button
              type="button"
              class="btn btn--ghost gcp-tab__row-btn"
              :data-testid="`gcp-override-${b.key}`"
              @click="openOverrideEdit(b.key)"
            >
              <Icon name="pencil" :size="12" />
              {{ b.override ? "编辑覆盖" : "覆盖编辑" }}
            </button>
            <button
              v-if="b.override"
              type="button"
              class="btn btn--ghost gcp-tab__row-btn gcp-tab__row-btn--danger"
              :data-testid="`gcp-restore-${b.key}`"
              @click="restoreTarget = b.override"
            >
              <Icon name="refresh" :size="12" />
              恢复内置
            </button>
            <span
              v-if="b.override && isRowBusy(b.override.id)"
              class="app-spinner gcp-tab__spinner"
              aria-label="处理中"
            />
          </div>
          <p
            v-if="b.override && rowErrors[b.override.id]"
            class="gcp-tab__error"
            role="alert"
          >
            {{ rowErrors[b.override.id] }}
          </p>
        </li>
      </ul>
    </section>

    <!-- 用户预设列表 -->
    <section class="gcp-tab__user">
      <div class="gcp-tab__list-head">
        <h3 class="gcp-tab__section-title">用户预设</h3>
        <button
          v-if="!formOpen"
          type="button"
          class="btn btn--primary"
          data-testid="gcp-create-btn"
          @click="openCreate"
        >
          <Icon name="plus" :size="12" />
          新增预设
        </button>
      </div>

      <p v-if="!store.loaded" class="gcp-tab__loading">加载中…</p>
      <p
        v-else-if="sortedRows.length === 0"
        class="gcp-tab__empty"
        data-testid="gcp-empty"
      >
        还没有用户预设。点「新增预设」建一个自己的审议阵容。
      </p>
      <ul v-else class="gcp-tab__list">
        <li
          v-for="row in sortedRows"
          :key="row.id"
          class="gcp-tab__row"
          :data-testid="`gcp-row-${row.id}`"
        >
          <div class="gcp-tab__row-header">
            <span class="gcp-tab__name">{{ row.name }}</span>
            <span class="gcp-tab__source-chip gcp-tab__source-chip--user">自定义</span>
          </div>
          <p v-if="row.description" class="gcp-tab__description">
            {{ row.description }}
          </p>
          <p class="gcp-tab__roster" :title="rosterSummary(row)">
            主持人 {{ modelDisplay(row.moderatorModelId) }} · 参与
            {{ rosterSummary(row) }}
          </p>
          <div class="gcp-tab__row-actions">
            <button
              type="button"
              class="btn btn--ghost gcp-tab__row-btn"
              :data-testid="`gcp-edit-${row.id}`"
              @click="openEdit(row)"
            >
              <Icon name="pencil" :size="12" />
              编辑
            </button>
            <button
              type="button"
              class="btn btn--ghost gcp-tab__row-btn gcp-tab__row-btn--danger"
              :data-testid="`gcp-delete-${row.id}`"
              @click="deleteTarget = row"
            >
              <Icon name="x" :size="12" />
              删除
            </button>
            <span
              v-if="isRowBusy(row.id)"
              class="app-spinner gcp-tab__spinner"
              aria-label="处理中"
            />
          </div>
          <p
            v-if="rowErrors[row.id]"
            class="gcp-tab__error"
            role="alert"
          >
            {{ rowErrors[row.id] }}
          </p>
        </li>
      </ul>
    </section>

    <!-- 新增 / 编辑 / 覆盖表单 -->
    <section v-if="formOpen" class="gcp-tab__form" data-testid="gcp-form">
      <h3 class="gcp-tab__section-title" data-testid="gcp-form-title">
        {{ formTitle }}
      </h3>
      <p v-if="overridingKey" class="gcp-tab__form-note">
        该覆盖仅本机 GUI 生效(M1/MCP 仍读源码 JSON);恢复内置 =
        删除覆盖行。
      </p>

      <Label class="gcp-tab__field">
        <span class="gcp-tab__label">预设名称</span>
        <input
          v-model="form.name"
          type="text"
          class="gcp-tab__input"
          placeholder="如:我的评审团(≤40 字符,不与内置档重名)"
          data-testid="gcp-name"
        />
      </Label>

      <Label class="gcp-tab__field">
        <span class="gcp-tab__label">描述(可选)</span>
        <input
          v-model="form.description"
          type="text"
          class="gcp-tab__input"
          placeholder="一句话说明这套阵容适合什么场景(≤200 字符)"
          data-testid="gcp-desc"
        />
      </Label>

      <div class="gcp-tab__field">
        <span class="gcp-tab__label">主持人模型</span>
        <SelectRoot
          :model-value="form.moderatorModelId || undefined"
          @update:model-value="onPickModerator"
        >
          <SelectTrigger
            class="gcp-tab__trigger"
            data-testid="gcp-moderator"
            aria-label="主持人模型"
          >
            <SelectValue placeholder="选择主持人模型" />
            <SelectIcon class="gcp-tab__trigger-icon">
              <Icon name="chevron-down" :size="12" />
            </SelectIcon>
          </SelectTrigger>
          <SelectPortal>
            <SelectContent
              class="gcp-tab__dropdown"
              position="popper"
              :side-offset="4"
            >
              <SelectViewport class="gcp-tab__dropdown-viewport">
                <SelectItem
                  v-for="m in moderatorOptions"
                  :key="m.id"
                  :value="m.id"
                  class="gcp-tab__option"
                >
                  <SelectItemText>
                    {{ m.providerDisplayName }} · {{ m.displayName }}
                  </SelectItemText>
                </SelectItem>
              </SelectViewport>
            </SelectContent>
          </SelectPortal>
        </SelectRoot>
      </div>

      <div class="gcp-tab__field">
        <span class="gcp-tab__label">
          参与者(2-3 人,不含主持人)
        </span>
        <div
          v-for="(_, idx) in form.participants"
          :key="idx"
          class="gcp-tab__participant"
          :data-testid="`gcp-participant-${idx}`"
        >
          <div class="gcp-tab__participant-head">
            <span class="gcp-tab__participant-title">
              参与者 #{{ idx + 1 }}
            </span>
            <button
              v-if="form.participants.length > MIN_PARTICIPANTS"
              type="button"
              class="btn btn--icon btn--danger-soft gcp-tab__participant-remove"
              :data-testid="`gcp-p-remove-${idx}`"
              :aria-label="`删除参与者 ${idx + 1}`"
              @click="removeParticipant(idx)"
            >
              <Icon name="x" :size="12" />
            </button>
          </div>
          <Label class="gcp-tab__field">
            <span class="gcp-tab__label">名字</span>
            <input
              v-model="form.participants[idx].name"
              type="text"
              class="gcp-tab__input"
              placeholder="如:架构(≤20 字符,预设内唯一)"
              :data-testid="`gcp-p-name-${idx}`"
            />
          </Label>
          <div class="gcp-tab__participant-row">
            <Label class="gcp-tab__field">
              <span class="gcp-tab__label">模型</span>
              <SelectRoot
                :model-value="form.participants[idx].modelId || undefined"
                @update:model-value="(v: unknown) => onPickParticipantModel(idx, v)"
              >
                <SelectTrigger
                  class="gcp-tab__trigger"
                  :data-testid="`gcp-p-model-${idx}`"
                  aria-label="参与者模型"
                >
                  <SelectValue placeholder="选择模型" />
                  <SelectIcon class="gcp-tab__trigger-icon">
                    <Icon name="chevron-down" :size="12" />
                  </SelectIcon>
                </SelectTrigger>
                <SelectPortal>
                  <SelectContent
                    class="gcp-tab__dropdown"
                    position="popper"
                    :side-offset="4"
                  >
                    <SelectViewport class="gcp-tab__dropdown-viewport">
                      <SelectItem
                        v-for="m in modelOptionsFor(form.participants[idx].modelId)"
                        :key="m.id"
                        :value="m.id"
                        class="gcp-tab__option"
                      >
                        <SelectItemText>
                          {{ m.providerDisplayName }} · {{ m.displayName }}
                        </SelectItemText>
                      </SelectItem>
                    </SelectViewport>
                  </SelectContent>
                </SelectPortal>
              </SelectRoot>
            </Label>
            <Label class="gcp-tab__field">
              <span class="gcp-tab__label">人设</span>
              <SelectRoot
                :model-value="form.participants[idx].persona"
                @update:model-value="(v: unknown) => onPickParticipantPersona(idx, v)"
              >
                <SelectTrigger
                  class="gcp-tab__trigger"
                  :data-testid="`gcp-p-persona-${idx}`"
                  aria-label="参与者人设"
                >
                  <SelectValue />
                  <SelectIcon class="gcp-tab__trigger-icon">
                    <Icon name="chevron-down" :size="12" />
                  </SelectIcon>
                </SelectTrigger>
                <SelectPortal>
                  <SelectContent
                    class="gcp-tab__dropdown"
                    position="popper"
                    :side-offset="4"
                  >
                    <SelectViewport class="gcp-tab__dropdown-viewport">
                      <SelectItem
                        v-for="o in PERSONA_OPTIONS"
                        :key="o.value"
                        :value="o.value"
                        class="gcp-tab__option"
                      >
                        <SelectItemText>{{ o.label }}({{ o.value }})</SelectItemText>
                      </SelectItem>
                    </SelectViewport>
                  </SelectContent>
                </SelectPortal>
              </SelectRoot>
            </Label>
          </div>
        </div>
        <button
          v-if="form.participants.length < MAX_PARTICIPANTS"
          type="button"
          class="btn btn--muted"
          data-testid="gcp-add-participant"
          @click="addParticipant"
        >
          <Icon name="plus" :size="12" />
          添加参与者
        </button>
      </div>

      <p v-if="formError" class="gcp-tab__error" role="alert" data-testid="gcp-form-error">
        {{ formError }}
      </p>

      <div class="gcp-tab__form-actions">
        <button type="button" class="btn btn--muted" data-testid="gcp-cancel" @click="cancelForm">
          取消
        </button>
        <button
          type="button"
          class="btn btn--primary"
          data-testid="gcp-submit"
          :disabled="saving"
          @click="submitForm"
        >
          {{
            saving
              ? "保存中…"
              : editingId
                ? "保存修改"
                : overridingKey
                  ? "覆盖"
                  : "创建"
          }}
        </button>
      </div>
    </section>

    <ConfirmDialog
      :open="deleteTarget !== null"
      title="删除群聊预设"
      confirm-text="删除"
      @cancel="deleteTarget = null"
      @confirm="confirmDelete"
    >
      <p>
        确定删除预设「{{ deleteTarget?.name }}」?已建定时任务的配置是创建时
        展开的快照,不受影响。
      </p>
    </ConfirmDialog>

    <!-- 恢复内置(GCE-P1b):删覆盖行回落 JSON 源码定义。 -->
    <ConfirmDialog
      :open="restoreTarget !== null"
      title="恢复内置预设"
      confirm-text="恢复内置"
      @cancel="restoreTarget = null"
      @confirm="confirmRestore"
    >
      <p>
        恢复内置预设「{{ restoreTarget?.builtinKey }}」?将丢弃覆盖行,回落
        scripts/group-chat-presets.json 源码定义;已建定时任务不受影响
        (快照)。
      </p>
    </ConfirmDialog>
  </div>
</template>

<style scoped>
.gcp-tab {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.gcp-tab__intro {
  margin: 0 0 4px 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: 1.6;
}

.gcp-tab__section-title {
  margin: 0;
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.gcp-tab__list-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.gcp-tab__loading,
.gcp-tab__empty {
  margin: 0;
  padding: 12px;
  color: var(--color-text-muted);
  font-size: var(--text-sm);
  text-align: center;
}

/* 内置只读区:压缩密度(比用户行少一层动作区)。 */
.gcp-tab__builtin-list,
.gcp-tab__list {
  list-style: none;
  margin: 8px 0 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.gcp-tab__builtin-row,
.gcp-tab__row {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.gcp-tab__row-header {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.gcp-tab__name {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.gcp-tab__source-chip {
  display: inline-block;
  padding: 1px 6px;
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  border-radius: 999px;
  border: 1px solid var(--color-bg-border);
  color: var(--color-text-muted);
  background: var(--color-bg-surface);
}

.gcp-tab__source-chip--user,
.gcp-tab__source-chip--override {
  border-color: var(--color-accent);
  color: var(--color-accent-text);
}

/* 覆盖表单的语义说明行(标题下的一句;区别于错误条)。 */
.gcp-tab__form-note {
  margin: -4px 0 0 0;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  line-height: 1.5;
}

.gcp-tab__description {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  line-height: 1.5;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.gcp-tab__roster {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  line-height: 1.5;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.gcp-tab__row-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.gcp-tab__row-btn--danger {
  color: var(--color-tool-error-text);
}

.gcp-tab__spinner {
  flex-shrink: 0;
}

/* 形态由全局 .app-spinner 原语提供(style.css);此处类名留作测试/检索钩子 */

/* --- 表单(镜像 ScheduledTasksTab form 容器)--- */

.gcp-tab__form {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
}

.gcp-tab__field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.gcp-tab__label {
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
}

.gcp-tab__input {
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  width: 100%;
  box-sizing: border-box;
}

.gcp-tab__input:focus {
  outline: none;
  border-color: var(--color-accent);
}

.gcp-tab__participant {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg-app);
}

.gcp-tab__participant + .gcp-tab__participant {
  margin-top: 8px;
}

.gcp-tab__participant-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.gcp-tab__participant-title {
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
}

.gcp-tab__participant-row {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
}

/* --- reka Select(ScheduledTasksTab/SubagentsTab 同款)--- */

.gcp-tab__trigger {
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

.gcp-tab__trigger:hover {
  border-color: var(--color-accent-muted);
}

.gcp-tab__trigger[data-state="open"] {
  border-color: var(--color-accent);
}

.gcp-tab__trigger[data-disabled] {
  opacity: 0.5;
  cursor: not-allowed;
}

.gcp-tab__trigger-icon {
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
}

/* Portal children —— SelectPortal teleport 到 body,规范要求 :deep()
   (reka-ui-usage.md gotcha;宽度对齐 trigger 用 --reka-select-trigger-width)。 */
:deep(.gcp-tab__dropdown) {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  min-width: var(--reka-select-trigger-width, 240px);
  width: var(--reka-select-trigger-width);
  max-height: var(--reka-select-content-available-height);
  z-index: var(--z-over-modal) !important;
  overflow: hidden;
}

:deep(.gcp-tab__dropdown-viewport) {
  padding: 4px;
}

:deep(.gcp-tab__option) {
  display: flex;
  align-items: center;
  padding: 6px 10px;
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  border-radius: var(--radius-sm);
  cursor: pointer;
  user-select: none;
  line-height: 1.4;
}

:deep(.gcp-tab__option[data-highlighted]) {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

:deep(.gcp-tab__option[data-state="checked"]) {
  color: var(--color-accent-text);
}

.gcp-tab__error {
  margin: 0;
  font-size: var(--text-xs);
  line-height: 1.5;
  padding: 4px 8px;
  border-radius: var(--radius-sm);
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 2px solid var(--color-tool-error);
}

.gcp-tab__form-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>
