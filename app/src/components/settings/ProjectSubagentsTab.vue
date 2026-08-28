<script setup lang="ts">
// ProjectSubagentsTab — Settings「项目」scope → 项目子代理
// (2026-08-29 settings-shell 重构)。
//
// 展示当前选中项目 `<project>/.everlasting/agents/*.md` 下定义的
// 子代理(frontmatter 真项目级),并为每个代理配置模型。数据走
// `useSubagentsStore.fetchForProject(path)`(与全局 SubagentsTab
// 同一 store),这里只过滤 `source === "project"` 的行 —— 全局面
// (内置 / 用户 / 项目混合)仍在「智能体 → Subagents」。
//
// 模型下拉完全镜像 SubagentsTab 的写法:reka-ui SelectRoot +
// INHERIT_SENTINEL 哨兵(reka 2.9.9 禁空串 value)+ 扁平 provider
// 前缀选项(SelectGroup 嵌套会在 Tauri webview 隐身,见
// SubagentsTab 头注 UI fix #2)。写入对 project source 落
// frontmatter `model:` 行,是真项目级配置。

import { computed, onMounted, ref, watch } from "vue";
import {
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
import { useSubagentsStore, type SubagentWithModelRow } from "../../stores/subagents";
import { useModelsStore, type ModelWithProvider } from "../../stores/models";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";
import Icon from "../Icon.vue";

const props = defineProps<{
  /** 项目 scope 选择器当前选中的项目 id(null = 无可见项目)。 */
  projectId: string | null;
}>();

const subagents = useSubagentsStore();
const models = useModelsStore();
const projects = useProjectsStore();

/** 加载失败信息(整页级,区别于 per-row errorByName)。 */
const loadError = ref<string | null>(null);

/** 选中项目(id → ProjectInfo,path 供 fetchForProject)。 */
const project = computed(() =>
  props.projectId ? projects.projectById(props.projectId) : undefined,
);

/** 仅项目级子代理,按名排序(与 SubagentsTab 同款防御性排序)。 */
const projectRows = computed<SubagentWithModelRow[]>(() => {
  return Array.from(subagents.rows.values())
    .filter((r) => r.source === "project")
    .sort((a, b) => a.name.localeCompare(b.name));
});

/** 扁平模型选项(provider 前缀排序),镜像 SubagentsTab。 */
const flatModelOptions = computed<ModelWithProvider[]>(() => {
  const groups = models.modelsGroupedByProvider;
  if (!Array.isArray(groups)) return [];
  const out: ModelWithProvider[] = [];
  for (const g of groups) {
    for (const m of g.models) {
      out.push(m);
    }
  }
  return out.slice().sort((a, b) => {
    const pa = providerLabelFor(a.id);
    const pb = providerLabelFor(b.id);
    if (pa !== pb) return pa.localeCompare(pb);
    return a.displayName.localeCompare(b.displayName);
  });
});

function providerLabelFor(modelId: string): string {
  const groups = models.modelsGroupedByProvider ?? [];
  for (const g of groups) {
    if (g.models.some((m) => m.id === modelId)) {
      return g.provider.displayName;
    }
  }
  return "";
}

async function refresh(): Promise<void> {
  loadError.value = null;
  const path = project.value?.path;
  if (!path) return;
  // 模型目录是下拉依赖,未加载则补拉(SubagentsTab 同款)。
  if (!models.loaded) {
    await models.load().catch(() => {
      /* 下拉为空即可见,不阻塞列表 */
    });
  }
  try {
    await subagents.fetchForProject(path);
  } catch (e) {
    loadError.value = extractErrorMessage(e);
  }
}

onMounted(refresh);
watch(
  () => props.projectId,
  () => {
    refresh();
  },
);

/** reka 2.9.9 禁空串 SelectItem value;null(继承父级)↔ 哨兵。 */
const INHERIT_SENTINEL = "__inherit__";

function selectValue(row: SubagentWithModelRow): string {
  return row.resolvedModelId ?? INHERIT_SENTINEL;
}

function triggerLabel(row: SubagentWithModelRow): string {
  if (row.resolvedModelId === null) return "继承父级 (inherit)";
  if (row.resolvedModelDisplay === null) return row.resolvedModelId;
  const provider = providerLabelFor(row.resolvedModelId);
  return provider ? `${row.resolvedModelDisplay} (${provider})` : row.resolvedModelDisplay;
}

/** Per-row write error(in-flight 状态直接读 store 的 spinner)。 */
const errorByName = ref<Record<string, string>>({});

async function onModelChange(
  row: SubagentWithModelRow,
  newValue: string | string[] | undefined,
) {
  const normalized = Array.isArray(newValue) ? newValue[0] ?? "" : (newValue ?? "");
  const modelId = normalized === INHERIT_SENTINEL ? null : normalized;
  delete errorByName.value[row.name];
  try {
    await subagents.setModel(row.name, "project", modelId);
  } catch (e) {
    const msg = extractErrorMessage(e);
    errorByName.value[row.name] = msg;
    projects.showToast(`设置 ${row.name} 失败：${msg}`, "error");
  }
}

function isLoading(name: string): boolean {
  return subagents.spinnerByName.has(name);
}
</script>

<template>
  <div class="proj-subagents">
    <div v-if="!project" class="proj-subagents__empty">没有可选项目。</div>

    <template v-else>
      <div v-if="loadError" class="proj-subagents__error" role="alert">
        加载失败：{{ loadError }}
      </div>

      <div v-else-if="!subagents.loaded" class="proj-subagents__loading">加载中…</div>

      <p v-else-if="projectRows.length === 0" class="proj-subagents__empty">
        该项目还没有子代理定义。在
        <code class="proj-subagents__code">{{ project.path }}/.everlasting/agents/</code>
        放置 *.md 文件即可创建。
      </p>

      <ul v-else class="proj-subagents__list">
        <li v-for="row in projectRows" :key="row.name" class="proj-subagents__row">
          <div class="proj-subagents__row-header">
            <span class="proj-subagents__name">{{ row.name }}</span>
            <span
              v-if="row.hasDbOverride"
              class="proj-subagents__override-chip"
              title="该 subagent 的模型来自 DB 覆盖表，优先级高于 frontmatter"
            >
              DB override
            </span>
          </div>
          <p v-if="row.description" class="proj-subagents__desc">{{ row.description }}</p>
          <div class="proj-subagents__model-row">
            <SelectRoot
              :model-value="selectValue(row)"
              :disabled="isLoading(row.name)"
              @update:model-value="(v) => onModelChange(row, v)"
            >
              <SelectTrigger
                class="proj-subagents__trigger"
                :class="{
                  'proj-subagents__trigger--invalid':
                    row.resolvedModelId !== null && row.resolvedModelDisplay === null,
                }"
                aria-label="Model"
              >
                <SelectValue>{{ triggerLabel(row) }}</SelectValue>
                <SelectIcon class="proj-subagents__trigger-icon">
                  <Icon name="chevron-down" :size="12" />
                </SelectIcon>
              </SelectTrigger>
              <SelectPortal>
                <SelectContent
                  class="proj-subagents__content"
                  position="popper"
                  :side-offset="4"
                >
                  <SelectViewport class="proj-subagents__viewport">
                    <SelectItem
                      :value="INHERIT_SENTINEL"
                      class="proj-subagents__option proj-subagents__option--inherit"
                    >
                      <SelectItemText>继承父级 (inherit)</SelectItemText>
                    </SelectItem>
                    <SelectItem
                      v-for="m in flatModelOptions"
                      :key="m.id"
                      :value="m.id"
                      class="proj-subagents__option"
                    >
                      <SelectItemText>
                        <span class="proj-subagents__option-provider">
                          {{ providerLabelFor(m.id) }}
                        </span>
                        <span class="proj-subagents__option-name">{{ m.displayName }}</span>
                      </SelectItemText>
                    </SelectItem>
                  </SelectViewport>
                </SelectContent>
              </SelectPortal>
            </SelectRoot>
            <span
              v-if="isLoading(row.name)"
              class="app-spinner proj-subagents__spinner"
              aria-label="保存中"
            />
          </div>
          <p
            v-if="row.resolvedModelId !== null && row.resolvedModelDisplay === null"
            class="proj-subagents__invalid"
            role="alert"
          >
            模型已删除，将降级为父级 (id: {{ row.resolvedModelId }})
          </p>
          <p v-if="errorByName[row.name]" class="proj-subagents__invalid" role="alert">
            {{ errorByName[row.name] }}
          </p>
        </li>
      </ul>
    </template>
  </div>
</template>

<style scoped>
.proj-subagents {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.proj-subagents__empty {
  margin: 0;
  padding: var(--space-4);
  font-size: var(--text-sm);
  color: var(--color-text-muted);
  text-align: center;
  line-height: var(--leading-relaxed);
}

.proj-subagents__code {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  word-break: break-all;
}

.proj-subagents__loading {
  padding: var(--space-3);
  color: var(--color-text-muted);
  font-size: var(--text-sm);
  text-align: center;
}

.proj-subagents__error,
.proj-subagents__invalid {
  margin: 0;
  font-size: var(--text-xs);
  line-height: var(--leading-normal);
  padding: var(--space-1) var(--space-2);
  border-radius: var(--radius-sm);
}

.proj-subagents__error {
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 2px solid var(--color-tool-error);
}

.proj-subagents__invalid {
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 2px solid var(--color-tool-error);
  font-family: var(--font-mono);
}

.proj-subagents__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.proj-subagents__row {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  padding: var(--space-2) var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.proj-subagents__row-header {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.proj-subagents__name {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.proj-subagents__override-chip {
  display: inline-block;
  padding: 1px 6px;
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  border-radius: 999px;
  border: 1px solid var(--color-tool-error);
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  cursor: help;
}

.proj-subagents__desc {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.proj-subagents__model-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-top: var(--space-1);
}

/* --- reka-ui SelectRoot(镜像 SubagentsTab;portal 子元素 :deep()) --- */

.proj-subagents__trigger {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  flex: 1;
  padding: 6px 10px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  font-family: inherit;
  width: 100%;
  min-width: 200px;
  box-sizing: border-box;
  cursor: pointer;
  transition:
    border-color var(--duration-base) var(--ease-out),
    background var(--duration-base) var(--ease-out);
}
.proj-subagents__trigger:hover {
  border-color: var(--color-accent-muted);
}
.proj-subagents__trigger[data-state="open"] {
  border-color: var(--color-accent);
  box-shadow: var(--shadow-ring);
}
.proj-subagents__trigger:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.proj-subagents__trigger--invalid {
  color: var(--color-tool-error-text);
  border-color: var(--color-tool-error);
}

.proj-subagents__trigger-icon {
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
}

:deep(.proj-subagents__content) {
  position: fixed;
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

:deep(.proj-subagents__viewport) {
  padding: 4px;
  max-height: var(--reka-select-content-available-height);
}

:deep(.proj-subagents__option) {
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

:deep(.proj-subagents__option--inherit) {
  color: var(--color-text-secondary);
  font-style: italic;
}

:deep(.proj-subagents__option[data-highlighted]) {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

:deep(.proj-subagents__option[data-state="checked"]) {
  color: var(--color-accent-text);
}

:deep(.proj-subagents__option-provider) {
  display: inline-block;
  font-size: var(--text-2xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
  margin-right: 6px;
  padding: 1px 5px;
  background: var(--color-bg-elevated);
  border-radius: 3px;
}

:deep(.proj-subagents__option-name) {
  color: var(--color-text-primary);
}

/* 形态由全局 .app-spinner 原语提供;类名留作测试/检索钩子 */
.proj-subagents__spinner {
  flex-shrink: 0;
}
</style>
