<script setup lang="ts">
// ProviderModelsPanel — 单个 provider 卡片内的模型管理区
// (2026-10-19, task `10-19-merge-provider-model-settings`)。
//
// 背景:设置面原本 Providers / Models 两个平行分类 —— 供应商在 A 页配、
// 模型在 B 页配,新增模型要先记住供应商名再回首 select。现在合并为
// 「供应商卡片内嵌模型列表」:每个供应商卡片下直接管理自己的模型,
// 模型表单的 provider 字段被锁定为卡片所属供应商(`providerLabel`)。
//
// 本组件承接原 ModelsTab 的全部模型职责(CRUD + 连通性测试 + 禁用开关
// + 删除确认),并新增:
//   1. **模型探测** — 调供应商的模型列表端点(GET /models /
//      /v1/models),列出可添加的 model name;选中后一键批量导入
//      (批量 = 逐条 `add_model`,失败计数内联报告)。
//   2. **按名称自动匹配能力** — 表单里的按钮查 models.dev 目录,把
//      上下文窗口 / 最大输出 / vision / thinking 填进表单字段(候选
//      多于一档时给「其它候选」小按钮,点了即时切换)。
//
// 状态归属:模型列表在 `useModelsStore`(add/update 后整表刷新),本
// 组件只持有 UI 状态(编辑模式 / 探测结果 / 每行测试结果)。探测与
// 匹配都是「业务失败 200」的 IPC 形状(`success:false` + error),这里
// 统一内联展示,不抛 toast。

import { computed, reactive, ref } from "vue";
import Icon from "../Icon.vue";
import ModelRow, { type TestState } from "./ModelRow.vue";
import ModelForm, { type ModelFormState } from "./ModelForm.vue";
import DeleteModelConfirm from "./DeleteModelConfirm.vue";
import { extractErrorMessage } from "../../utils/useErrorBus";
import { transport } from "../../transport";
import {
  useModelsStore,
  type ModelWithProvider,
} from "../../stores/models";
import type { ProviderRow } from "../../stores/providers";
import {
  capabilitiesFromSpec,
  formatTokens,
  lookupModelMetadata,
  probeProviderModels,
  specTags,
  type ModelSpec,
  type ProbedModel,
} from "../../utils/modelMetadata";

const props = defineProps<{
  /** 卡片所属供应商(模型永远绑在它下面)。 */
  provider: ProviderRow;
}>();

const modelsStore = useModelsStore();

// --- 模型列表 -------------------------------------------------------------

/** 本供应商下的模型(store 已按 updated_at 倒序)。 */
const models = computed<ModelWithProvider[]>(() =>
  modelsStore.models.filter((m) => m.providerId === props.provider.id),
);

// --- 编辑状态(原 ModelsTab 编排逻辑) -------------------------------------

type EditMode = "idle" | "add" | "edit";

const mode = ref<EditMode>("idle");
const editId = ref<string | null>(null);
const deleteConfirmId = ref<string | null>(null);
const saving = ref(false);

const form = reactive<ModelFormState>({
  providerId: "",
  modelName: "",
  displayName: "",
  maxTokens: "128000",
  thinkingEffort: "none",
  supportsThinking: false,
  supportsImages: false,
  contextWindow: 1000000,
});

/** 自动匹配在途 + 结果提示(内联在表单里)。 */
const matching = ref(false);
const matchHint = ref<{ kind: "ok" | "fail" | "info"; text: string } | null>(null);
/** 上次匹配的候选(除首个外的「其它候选」,点了换用)。 */
const alternatives = ref<ModelSpec[]>([]);

/** 传给表单的候选展示项(稳定 key = providerKey:modelId)。 */
const altOptions = computed(() =>
  alternatives.value.map((alt) => ({
    key: `${alt.providerKey}:${alt.modelId}`,
    label: `${alt.providerName} · ${alt.modelId} · ${formatTokens(alt.contextWindow)}`,
  })),
);

/** 表单里点了「其它候选」:按 key 找回条目并重新折算字段。 */
function onApplyAlternative(key: string) {
  const spec = alternatives.value.find((a) => `${a.providerKey}:${a.modelId}` === key);
  if (!spec) return;
  applySpec(spec);
}

const canSave = computed(
  () =>
    form.providerId !== "" &&
    form.modelName.trim() !== "" &&
    form.contextWindow > 0 &&
    !saving.value,
);

function resetForm() {
  form.providerId = props.provider.id;
  form.modelName = "";
  form.displayName = "";
  form.maxTokens = "128000";
  form.thinkingEffort = "none";
  form.supportsThinking = false;
  form.supportsImages = false;
  form.contextWindow = 1000000;
  editId.value = null;
  matchHint.value = null;
  alternatives.value = [];
}

function startAdd() {
  resetForm();
  mode.value = "add";
}

function startEdit(m: ModelWithProvider) {
  mode.value = "edit";
  editId.value = m.id;
  form.providerId = m.providerId;
  form.modelName = m.modelName;
  form.displayName = m.displayName;
  form.maxTokens = m.maxTokens != null ? String(m.maxTokens) : "";
  form.thinkingEffort = m.thinkingEffort ?? "none";
  form.supportsThinking = m.supportsThinking;
  form.supportsImages = m.supportsImages;
  form.contextWindow = m.contextWindow;
  matchHint.value = null;
  alternatives.value = [];
}

function cancelEdit() {
  mode.value = "idle";
  resetForm();
}

/** 表单字段 ← 目录条目(自动匹配 / 候选切换共用)。 */
function applySpec(spec: ModelSpec, sourceLabel?: string) {
  const caps = capabilitiesFromSpec(spec, props.provider.protocol, form.contextWindow || 200_000);
  form.contextWindow = caps.contextWindow;
  form.maxTokens = caps.maxTokens != null ? String(caps.maxTokens) : "";
  form.supportsThinking = caps.supportsThinking;
  form.supportsImages = caps.supportsImages;
  form.thinkingEffort = caps.thinkingEffort ?? "none";
  // 显示名留空才补:用户手填的名字优先(自动匹配不该覆盖用户输入)。
  if (!form.displayName.trim()) form.displayName = spec.name;
  const tags = specTags(spec);
  matchHint.value = {
    kind: "ok",
    text: `已匹配 ${sourceLabel ?? `${spec.providerName} · ${spec.modelId}`}${tags.length ? `(${tags.join(" / ")})` : ""}`,
  };
}

/** 「按名称自动匹配能力」:查 models.dev,用最佳匹配填表。 */
async function onAutoMatch() {
  const name = form.modelName.trim();
  if (!name || matching.value) return;
  matching.value = true;
  matchHint.value = { kind: "info", text: "正在查询 models.dev…" };
  alternatives.value = [];
  try {
    const result = await lookupModelMetadata(name, props.provider.id);
    if (!result.success) {
      matchHint.value = {
        kind: "fail",
        text: `查询失败:${result.error ?? "未知错误"}(可手动填写)`,
      };
      return;
    }
    const [best, ...rest] = result.matches;
    if (!best) {
      matchHint.value = {
        kind: "fail",
        text: `未在 models.dev 找到「${name}」,请手动填写`,
      };
      return;
    }
    applySpec(best);
    alternatives.value = rest.slice(0, 3);
    const staleNote = result.stale ? "(离线缓存,可能不新)" : "";
    if (rest.length > 0) {
      // 计数与下方展示的候选数一致(最多 3 条),避免「另有 7 个」却只列
      // 3 条的口径歧义。
      const shown = alternatives.value.length;
      matchHint.value = {
        kind: "ok",
        text: `${matchHint.value?.text ?? ""}${staleNote} · 另有 ${rest.length} 个候选,下列 ${shown} 个`,
      };
    } else if (staleNote) {
      matchHint.value = {
        kind: "ok",
        text: `${matchHint.value?.text ?? ""}${staleNote}`,
      };
    }
  } catch (e) {
    matchHint.value = { kind: "fail", text: `查询失败:${extractErrorMessage(e)}` };
  } finally {
    matching.value = false;
  }
}

async function save() {
  if (!canSave.value) return;
  saving.value = true;
  try {
    const displayName = form.displayName.trim() || form.modelName.trim();
    const opts: {
      supportsThinking: boolean;
      supportsImages: boolean;
      contextWindow: number;
      maxTokens?: number;
      thinkingEffort?: string;
    } = {
      supportsThinking: form.supportsThinking,
      supportsImages: form.supportsImages,
      contextWindow: form.contextWindow,
    };
    if (form.maxTokens.trim() !== "") {
      const parsed = parseInt(form.maxTokens, 10);
      if (!isNaN(parsed) && parsed > 0) opts.maxTokens = parsed;
    }
    if (form.thinkingEffort !== "none") {
      opts.thinkingEffort = form.thinkingEffort;
    }
    if (mode.value === "add") {
      await modelsStore.add(form.providerId, form.modelName.trim(), displayName, opts);
    } else if (mode.value === "edit" && editId.value) {
      await modelsStore.update(
        editId.value,
        form.providerId,
        form.modelName.trim(),
        displayName,
        opts,
      );
    }
    mode.value = "idle";
    resetForm();
  } catch (e) {
    console.error("save model failed:", e);
    matchHint.value = { kind: "fail", text: `保存失败:${extractErrorMessage(e)}` };
  } finally {
    saving.value = false;
  }
}

async function confirmDelete() {
  const id = deleteConfirmId.value;
  if (!id) return;
  deleteConfirmId.value = null;
  try {
    await modelsStore.remove(id);
    delete tests[id];
  } catch (e) {
    console.error("delete model failed:", e);
  }
}

// --- 每行测试结果 ---------------------------------------------------------

const tests = reactive<Record<string, TestState>>({});

async function runTest(modelId: string) {
  tests[modelId] = { kind: "running" };
  try {
    const result = await transport.invoke<{
      success: boolean;
      latencyMs: number;
      error: string | null;
    }>("test_model", { modelId });
    if (result.success) {
      tests[modelId] = { kind: "ok", latencyMs: result.latencyMs };
    } else {
      tests[modelId] = { kind: "fail", error: result.error ?? "Connection failed" };
    }
  } catch (e) {
    tests[modelId] = { kind: "fail", error: extractErrorMessage(e) };
  }
}

async function toggleDisabled(m: ModelWithProvider) {
  try {
    await modelsStore.setDisabled(m.id, !m.disabled);
  } catch (e) {
    console.error("toggle model disabled failed:", e);
  }
}

// --- 模型探测 -------------------------------------------------------------

const probing = ref(false);
const probeError = ref<string | null>(null);
const metadataWarning = ref<string | null>(null);
const probed = ref<ProbedModel[]>([]);
const probeOpen = ref(false);
const probeFilter = ref("");
const selected = reactive(new Set<string>());
const importing = ref(false);
const importReport = ref<{ added: number; failed: number } | null>(null);

/** 已在目录里的 model name(小写) —— 探测列表据此标「已添加」。 */
const existingNames = computed(
  () => new Set(models.value.map((m) => m.modelName.toLowerCase())),
);

const filteredProbed = computed<ProbedModel[]>(() => {
  const q = probeFilter.value.trim().toLowerCase();
  if (!q) return probed.value;
  return probed.value.filter(
    (p) =>
      p.modelName.toLowerCase().includes(q) || p.displayName.toLowerCase().includes(q),
  );
});

const selectedCount = computed(() => selected.size);

function isAdded(p: ProbedModel): boolean {
  return existingNames.value.has(p.modelName.toLowerCase());
}

async function runProbe() {
  if (probing.value) return;
  probing.value = true;
  probeOpen.value = true;
  probeError.value = null;
  metadataWarning.value = null;
  importReport.value = null;
  selected.clear();
  try {
    const result = await probeProviderModels(props.provider.id);
    if (!result.success) {
      probed.value = [];
      probeError.value = result.error ?? "探测失败";
      return;
    }
    probed.value = result.models;
    metadataWarning.value = result.metadataWarning;
    if (result.truncated) {
      metadataWarning.value = [metadataWarning.value, "列表过大,只显示前 300 个。"]
        .filter(Boolean)
        .join(" / ");
    }
  } catch (e) {
    probed.value = [];
    probeError.value = extractErrorMessage(e);
  } finally {
    probing.value = false;
  }
}

function toggleSelect(name: string) {
  if (selected.has(name)) selected.delete(name);
  else selected.add(name);
}

function selectAllPending() {
  for (const p of filteredProbed.value) {
    if (!isAdded(p)) selected.add(p.modelName);
  }
}

function clearSelection() {
  selected.clear();
}

/** 批量导入选中项:逐条 add_model(失败不中断,末尾汇总)。 */
async function importSelected() {
  if (importing.value) return;
  const targets = probed.value.filter(
    (p) => selected.has(p.modelName) && !isAdded(p),
  );
  if (targets.length === 0) return;
  importing.value = true;
  let added = 0;
  let failed = 0;
  try {
    for (const p of targets) {
      try {
        const opts = p.spec
          ? capabilitiesFromSpec(p.spec, props.provider.protocol)
          : {
              contextWindow: 200_000,
              supportsThinking: false,
              supportsImages: false,
              maxTokens: null,
              thinkingEffort: null,
            };
        await modelsStore.add(props.provider.id, p.modelName, p.displayName || p.modelName, {
          supportsThinking: opts.supportsThinking,
          supportsImages: opts.supportsImages,
          contextWindow: opts.contextWindow,
          ...(opts.maxTokens != null ? { maxTokens: opts.maxTokens } : {}),
          ...(opts.thinkingEffort ? { thinkingEffort: opts.thinkingEffort } : {}),
        });
        added += 1;
        selected.delete(p.modelName);
      } catch (e) {
        console.warn("import model failed:", p.modelName, e);
        failed += 1;
      }
    }
    importReport.value = { added, failed };
  } finally {
    importing.value = false;
  }
}

function closeProbe() {
  probeOpen.value = false;
  probeFilter.value = "";
  importReport.value = null;
}
</script>

<template>
  <div class="provider-models">
    <!-- 工具行:数量 + 探测 + 添加 -->
    <div v-if="mode === 'idle'" class="provider-models__toolbar">
      <span class="provider-models__count">
        <Icon name="circle-stack" :size="12" />
        {{ models.length }} 个模型
      </span>
      <div class="provider-models__toolbar-actions">
        <button
          type="button"
          class="provider-models__btn btn btn--muted"
          :disabled="probing"
          :title="`从 ${provider.displayName} 拉取可用模型列表`"
          :data-testid="`probe-models-${provider.id}`"
          @click="probeOpen ? closeProbe() : runProbe()"
        >
          <Icon :name="probing ? 'refresh' : 'magnifying-glass'" :size="12" :class="{ 'provider-models__spin': probing }" />
          {{ probing ? "探测中…" : probeOpen ? "收起探测" : "探测模型" }}
        </button>
        <button
          type="button"
          class="provider-models__btn btn btn--muted"
          :data-testid="`add-model-${provider.id}`"
          @click="startAdd"
        >
          <Icon name="plus" :size="12" />
          添加模型
        </button>
      </div>
    </div>

    <!-- 模型列表 -->
    <div v-if="mode === 'idle'" class="provider-models__list">
      <ModelRow
        v-for="m in models"
        :key="m.id"
        :model="m"
        :test="tests[m.id]"
        :is-streaming="false"
        :is-default="m.id === modelsStore.defaultModelId"
        @test="runTest(m.id)"
        @edit="startEdit(m)"
        @delete="deleteConfirmId = m.id"
        @toggle-disabled="toggleDisabled(m)"
      />
      <div v-if="models.length === 0" class="provider-models__empty">
        还没有模型 —— 点「探测模型」从供应商拉取,或「添加模型」手填。
      </div>
    </div>

    <!-- 探测面板 -->
    <div
      v-if="probeOpen && mode === 'idle'"
      class="provider-models__probe"
      :data-testid="`probe-panel-${provider.id}`"
    >
      <div class="provider-models__probe-head">
        <span class="provider-models__probe-title">
          供应商可用模型
          <span v-if="probed.length" class="provider-models__probe-count">{{ probed.length }}</span>
        </span>
        <button
          type="button"
          class="provider-models__icon-btn btn btn--icon btn--ghost"
          title="重新探测"
          aria-label="重新探测"
          @click="runProbe"
        >
          <Icon name="refresh" :size="12" :class="{ 'provider-models__spin': probing }" />
        </button>
        <button
          type="button"
          class="provider-models__icon-btn btn btn--icon btn--ghost"
          title="关闭"
          aria-label="关闭探测面板"
          @click="closeProbe"
        >
          <Icon name="x" :size="12" />
        </button>
      </div>

      <p v-if="probeError" class="provider-models__probe-error" :data-testid="`probe-error-${provider.id}`">
        <Icon name="warn" :size="12" />
        {{ probeError }}
      </p>
      <p v-else-if="metadataWarning" class="provider-models__probe-note">
        <Icon name="info" :size="12" />
        能力目录不可用({{ metadataWarning }})—— 导入时按默认能力值(200k / 无 vision)。
      </p>

      <template v-if="probed.length > 0">
        <div class="provider-models__probe-tools">
          <input
            v-model="probeFilter"
            type="text"
            class="provider-models__filter"
            placeholder="过滤模型名…"
            aria-label="过滤模型名"
          />
          <button type="button" class="provider-models__link" @click="selectAllPending">
            全选未添加
          </button>
          <button type="button" class="provider-models__link" @click="clearSelection">
            清空
          </button>
          <span class="provider-models__selected">已选 {{ selectedCount }}</span>
        </div>

        <ul class="provider-models__probe-list">
          <li
            v-for="p in filteredProbed"
            :key="p.modelName"
            class="provider-models__probe-item"
            :class="{ 'provider-models__probe-item--added': isAdded(p) }"
          >
            <label class="provider-models__probe-label">
              <input
                type="checkbox"
                :checked="selected.has(p.modelName) || isAdded(p)"
                :disabled="isAdded(p)"
                @change="toggleSelect(p.modelName)"
              />
              <span class="provider-models__probe-name">{{ p.modelName }}</span>
            </label>
            <span v-if="isAdded(p)" class="provider-models__probe-tag provider-models__probe-tag--added">
              已添加
            </span>
            <template v-else-if="p.spec">
              <span
                v-for="tag in specTags(p.spec)"
                :key="tag"
                class="provider-models__probe-tag"
              >
                {{ tag }}
              </span>
              <span class="provider-models__probe-provider">{{ p.spec.providerName }}</span>
            </template>
            <span v-else class="provider-models__probe-provider">目录无记录</span>
          </li>
          <li v-if="filteredProbed.length === 0" class="provider-models__probe-item">
            没有匹配「{{ probeFilter }}」的模型
          </li>
        </ul>

        <div class="provider-models__probe-actions">
          <button
            type="button"
            class="provider-models__btn btn btn--primary"
            :disabled="selectedCount === 0 || importing"
            :data-testid="`import-models-${provider.id}`"
            @click="importSelected"
          >
            <Icon :name="importing ? 'refresh' : 'plus'" :size="12" :class="{ 'provider-models__spin': importing }" />
            {{ importing ? "导入中…" : `导入 ${selectedCount} 个` }}
          </button>
          <span v-if="importReport" class="provider-models__import-report">
            已添加 {{ importReport.added }}
            <template v-if="importReport.failed"> · 失败 {{ importReport.failed }}</template>
          </span>
        </div>
      </template>
      <p v-else-if="!probeError && !probing" class="provider-models__probe-note">
        <Icon name="info" :size="12" />
        供应商没有返回任何模型(端点可达但列表为空)。
      </p>
    </div>

    <!-- 新增 / 编辑表单(provider 锁定为本卡片) -->
    <ModelForm
      v-if="mode !== 'idle'"
      :mode="mode"
      :form="form"
      :providers="[provider]"
      :provider-label="`${provider.displayName} (${provider.protocol})`"
      :saving="saving"
      :can-save="canSave"
      :matching="matching"
      :match-hint="matchHint"
      :alternatives="altOptions"
      @submit="save"
      @cancel="cancelEdit"
      @auto-match="onAutoMatch"
      @apply-alternative="onApplyAlternative"
    />

    <!-- 删除确认 -->
    <DeleteModelConfirm
      :is-open="deleteConfirmId !== null"
      :model-name="''"
      @confirm="confirmDelete"
      @cancel="deleteConfirmId = null"
    />
  </div>
</template>

<style scoped>
.provider-models {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-left: 20px;
    border-left: 2px solid var(--color-bg-border);
    margin-left: 4px;
    position: relative;
}

.provider-models__toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    flex-wrap: wrap;
}

.provider-models__count {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--text-xs);
    color: var(--color-text-muted);
}

.provider-models__toolbar-actions {
    display: flex;
    gap: 6px;
    align-items: center;
}

.provider-models__btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: var(--text-xs);
    font-weight: var(--weight-medium);
    white-space: nowrap;
}

.provider-models__icon-btn {
    width: 26px;
    height: 26px;
    min-width: 26px;
    min-height: 26px;
    padding: 0;
    justify-content: center;
}

.provider-models__spin {
    animation: provider-models-spin 1s linear infinite;
}

@keyframes provider-models-spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
}

.provider-models__list {
    display: flex;
    flex-direction: column;
    gap: 4px;
}

.provider-models__empty {
    padding: 10px 12px;
    font-size: var(--text-xs);
    color: var(--color-text-muted);
    border: 1px dashed var(--color-bg-border);
    border-radius: var(--radius-sm);
}

/* --- 探测面板 --- */

.provider-models__probe {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px;
    background: var(--color-bg-app);
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-md);
}

.provider-models__probe-head {
    display: flex;
    align-items: center;
    gap: 6px;
}

.provider-models__probe-title {
    flex: 1;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    color: var(--color-text-primary);
}

.provider-models__probe-count {
    font-size: var(--text-2xs);
    font-family: var(--font-mono);
    color: var(--color-text-muted);
    background: var(--color-bg-elevated);
    border-radius: 3px;
    padding: 0 5px;
}

.provider-models__probe-error,
.provider-models__probe-note {
    display: flex;
    align-items: flex-start;
    gap: 4px;
    margin: 0;
    font-size: var(--text-xs);
    line-height: 1.5;
}

.provider-models__probe-error {
    color: var(--color-tool-error-text);
}

.provider-models__probe-note {
    color: var(--color-text-muted);
}

.provider-models__probe-tools {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
}

.provider-models__filter {
    flex: 1;
    min-width: 120px;
    padding: 4px 8px;
    background: var(--color-bg-surface);
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-sm);
    color: var(--color-text-primary);
    font-size: var(--text-xs);
}

.provider-models__filter:focus {
    outline: none;
    border-color: var(--color-accent);
}

.provider-models__link {
    background: none;
    border: none;
    padding: 0;
    font-size: var(--text-xs);
    color: var(--color-accent-text);
    cursor: pointer;
}

.provider-models__link:hover {
    text-decoration: underline;
}

.provider-models__selected {
    font-size: var(--text-2xs);
    font-family: var(--font-mono);
    color: var(--color-text-muted);
}

.provider-models__probe-list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 260px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 1px;
}

.provider-models__probe-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 6px;
    border-radius: var(--radius-sm);
    font-size: var(--text-xs);
    color: var(--color-text-secondary);
}

.provider-models__probe-item:hover {
    background: var(--color-bg-elevated);
}

.provider-models__probe-item--added {
    opacity: 0.55;
}

.provider-models__probe-label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    min-width: 0;
    flex: 1;
}

.provider-models__probe-name {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--color-text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.provider-models__probe-tag {
    flex-shrink: 0;
    font-size: var(--text-2xs);
    font-family: var(--font-mono);
    padding: 0 5px;
    border-radius: 3px;
    background: var(--color-accent-muted);
    color: var(--color-accent-text);
}

.provider-models__probe-tag--added {
    background: var(--color-bg-border);
    color: var(--color-text-muted);
}

.provider-models__probe-provider {
    flex-shrink: 0;
    font-size: var(--text-2xs);
    color: var(--color-text-muted);
}

.provider-models__probe-actions {
    display: flex;
    align-items: center;
    gap: 8px;
}

.provider-models__import-report {
    font-size: var(--text-xs);
    color: var(--color-text-muted);
}

/* --- 移动端:去掉缩进条,操作按钮回 32px(卡片内低频操作) --- */
@media (max-width: 767px) {
    .provider-models {
        padding-left: 10px;
        margin-left: 0;
    }
    .provider-models__toolbar-actions {
        width: 100%;
    }
    .provider-models__toolbar-actions .provider-models__btn {
        flex: 1;
        justify-content: center;
    }
    .provider-models__probe-list {
        max-height: 200px;
    }
}
</style>
