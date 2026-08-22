<script setup lang="ts">
// SubagentsTab — Settings page tab content for per-subagent model
// configuration (2026-07-03, task 07-03-subagent-per-agent-model-ui,
// 阶段 4).
//
// One row per subagent (builtin + user + project), with a per-row
// model dropdown. The dropdown's first option is "继承父级 (inherit
// parent)" — selecting it clears the agent's model configuration
// (DB row DELETE for builtin; frontmatter `model:` line removal
// for user / project). The rest of the options are the models
// from `useModelsStore`, prefixed with their provider name so the
// user can distinguish them at a glance. Builtin agents have a
// "(DB override)" chip when the DB row exists; user / project
// agents show the file-declared model (declaredModelId) when the
// DB override is absent.
//
// Invalid model badges: when `resolvedModelId` is non-null but
// `resolvedModelDisplay` is null (catalog miss — the model was
// deleted out from under the override), the row shows a red
// "模型已删除，将降级" hint so the user can fix it.
//
// UI polish (07-03 UI fix): replaced the native <select> with
// reka-ui `SelectRoot` to mirror the Add Model Provider selector
// in `ModelForm.vue` (consistent with the project's other
// SelectRoot usages). The empty-string value (inherited parent)
// is mapped to/from `null` in the change handler so the rest of
// the data flow stays `string | null`.
//
// 07-03 UI fix #2: flattened the option list (no SelectGroup /
// SelectLabel / SelectSeparator nesting) — those primitives
// rendered the popover contents invisible in the Tauri webview.
// The provider name is shown as a small prefix on each
// SelectItem instead, which keeps the list visually grouped
// without nesting reka-ui groups.

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
import { useChatStore } from "../../stores/chat";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";
import Icon from "../Icon.vue";

const subagents = useSubagentsStore();
const models = useModelsStore();
const chat = useChatStore();
const projects = useProjectsStore();

/** Per-row error message (rendered inline below the row's
 *  dropdown). Cleared on a successful set. */
const errorByName = ref<Record<string, string>>({});

/** Sorted, de-duplicated list of all subagents (the store
 *  map preserves insertion order, but the backend already
 *  sorts by name — we re-derive the sort defensively so a
 *  future backend reorder doesn't reshuffle the UI). */
const sortedRows = computed<SubagentWithModelRow[]>(() => {
  return Array.from(subagents.rows.values()).sort((a, b) =>
    a.name.localeCompare(b.name),
  );
});

/** Flat list of all models with their provider context, sorted
 *  by provider display name then model display name. Mirrors
 *  `models.modelsGroupedByProvider` but flattened for the
 *  reka-ui SelectItem loop (grouping via SelectGroup / SelectLabel
 *  was dropping the popover contents in the Tauri webview). */
const flatModelOptions = computed<ModelWithProvider[]>(() => {
  const groups = models.modelsGroupedByProvider;
  if (!Array.isArray(groups)) return [];
  const out: ModelWithProvider[] = [];
  for (const g of groups) {
    for (const m of g.models) {
      out.push(m);
    }
  }
  // Stable sort by provider display name (resolved via the
  // grouped store) then model display name, so the dropdown's
  // order is deterministic across re-renders.
  return out.slice().sort((a, b) => {
    const pa =
      groups.find((g) => g.models.some((mm) => mm.id === a.id))
        ?.provider.displayName ?? "";
    const pb =
      groups.find((g) => g.models.some((mm) => mm.id === b.id))
        ?.provider.displayName ?? "";
    if (pa !== pb) return pa.localeCompare(pb);
    return a.displayName.localeCompare(b.displayName);
  });
});

/** Resolves a model id to its provider display name for the
 *  per-row SelectItem prefix label. */
function providerLabelFor(modelId: string): string {
  const groups = models.modelsGroupedByProvider ?? [];
  for (const g of groups) {
    if (g.models.some((m) => m.id === modelId)) {
      return g.provider.displayName;
    }
  }
  return "";
}

/** True when at least one row's model is invalid (catalog
 *  miss → DB override points at a deleted model). Drives the
 *  top-of-tab warning banner. */
const hasAnyInvalid = computed<boolean>(() =>
  sortedRows.value.some(
    (r) =>
      r.resolvedModelId !== null &&
      r.resolvedModelDisplay === null,
  ),
);

/** Map `name → boolean` for the dropdown's disabled state.
 *  True while a write is in flight for that agent. */
function isLoading(name: string): boolean {
  return subagents.spinnerByName.has(name);
}

async function refresh() {
  // Ensure the models catalog is loaded (the dropdown depends
  // on it). The SettingsModal opens the tab on demand; the
  // user may not have visited the Models tab yet.
  if (!models.loaded) {
    await models.load().catch(() => {
      // The toast below catches the same error; the store
      // keeps `loaded=false` so a later interaction retries.
    });
  }
  // Use the current session's cwd as the project path. The
  // backend canonicalizes via `resolve_project_path` in
  // `commands::panel::list_subagents` (same helper).
  const projectPath = chat.currentCwd || "";
  try {
    await subagents.fetchForProject(projectPath);
  } catch (e) {
    projects.showToast(`加载 subagent 列表失败：${extractErrorMessage(e)}`, "error");
  }
}

onMounted(refresh);

// Re-fetch when the user switches projects while the modal
// is open (canonical `currentCwd` change). The Settings modal
// opens for a single "snapshot" (per MVP), but a user-initiated
// project switch in another pane is still possible.
watch(
  () => chat.currentCwd,
  () => {
    refresh();
  },
);

/** Sentinel value used for the "inherit parent" SelectItem.
 *  reka-ui 2.9.9 forbids empty-string SelectItem values
 *  ("A <SelectItem /> must have a value prop that is not an
 *  empty string"), so we map `null` ↔ this sentinel at the
 *  SelectRoot boundary and only emit `null` to the IPC. */
const INHERIT_SENTINEL = "__inherit__";

/** Maps the row's resolved model to the SelectRoot's string
 *  value (`null` resolvedModel → INHERIT_SENTINEL, since reka-ui
 *  forbids empty-string SelectItem values). */
function selectValue(row: SubagentWithModelRow): string {
  return row.resolvedModelId ?? INHERIT_SENTINEL;
}

/** Closed-trigger label. reka-ui's SelectValue flattens the selected
 *  SelectItemText to plain text, so the provider chip span inside the
 *  option loses its chip styling + margin and the trigger reads
 *  "ProviderModel" as one mashed word. Render the trigger's label
 *  explicitly instead (same pattern as GroupChatConfigModal's
 *  `modelLabel`); the open dropdown keeps the chip look. */
function triggerLabel(row: SubagentWithModelRow): string {
  if (row.resolvedModelId === null) return "继承父级 (inherit)";
  // Invalid model (catalog miss): show the id verbatim so the user
  // can see what the override points at (matches the red-trigger
  // state and the comment on the invalid-model hint below).
  if (row.resolvedModelDisplay === null) return row.resolvedModelId;
  const provider = providerLabelFor(row.resolvedModelId);
  return provider
    ? `${row.resolvedModelDisplay} (${provider})`
    : row.resolvedModelDisplay;
}

async function onModelChange(
  row: SubagentWithModelRow,
  newValue: string | string[] | undefined,
) {
  // reka-ui's `update:model-value` payload is `string |
  // string[] | undefined`. We use a single-select, so collapse
  // the array branch defensively (it shouldn't fire here, but
  // typing forces us to handle it).
  const normalized =
    Array.isArray(newValue) ? newValue[0] ?? "" : (newValue ?? "");
  const modelId = normalized === INHERIT_SENTINEL ? null : normalized;
  // Clear the row's prior error before firing the new write
  // (so the red banner disappears on the first successful set).
  delete errorByName.value[row.name];
  try {
    await subagents.setModel(row.name, row.source, modelId);
  } catch (e) {
    const msg = extractErrorMessage(e);
    errorByName.value[row.name] = msg;
    projects.showToast(`设置 ${row.name} 失败：${msg}`, "error");
  }
}

function sourceLabel(source: SubagentWithModelRow["source"]): string {
  switch (source) {
    case "builtin":
      return "内置";
    case "user":
      return "用户";
    case "project":
      return "项目";
  }
}
</script>

<template>
  <div class="subagents-tab">
    <p class="subagents-tab__intro">
      为每个 subagent（内置 + 用户 + 项目）配置默认模型。下拉显示 model
      display_name，写入时自动映射为 catalog id。优先级：DB 覆盖 &gt;
      frontmatter &gt; 继承父级。
    </p>

    <!--
      Top-of-tab invalid-model warning. Shows when at least one
      row's DB override points at a deleted model. Click to
      re-fetch (the user may have just re-added the model in the
      Models tab).
    -->
    <div v-if="hasAnyInvalid" class="subagents-tab__warn" role="alert">
      <span>
        有 subagent 指向已删除的 model（将自动降级为父级）。在 Models
        标签里重新创建该 model，或在下拉里换一个有效的 model。
      </span>
    </div>

    <div v-if="!subagents.loaded" class="subagents-tab__loading">
      加载中…
    </div>

    <ul v-else class="subagents-tab__list">
      <li
        v-for="row in sortedRows"
        :key="row.name"
        class="subagents-tab__row"
        :data-source="row.source"
      >
        <div class="subagents-tab__row-header">
          <span class="subagents-tab__name">{{ row.name }}</span>
          <span class="subagents-tab__source-chip" :data-source="row.source">
            {{ sourceLabel(row.source) }}
          </span>
          <span
            v-if="row.hasDbOverride"
            class="subagents-tab__override-chip"
            title="该 subagent 的模型来自 DB 覆盖表，优先级高于 frontmatter"
          >
            DB override
          </span>
        </div>
        <p
          v-if="row.description"
          class="subagents-tab__description"
        >
          {{ row.description }}
        </p>
        <div class="subagents-tab__model-row">
          <!--
            reka-ui SelectRoot — mirrors the Add Model Provider
            selector in ModelForm.vue. Empty string = inherit
            parent (reka-ui SelectItem can't take null/boolean).
            The closed trigger renders `triggerLabel(row)` itself
            (SelectValue would flatten the option's provider chip
            span into mashed plain text); the open list keeps the
            `<provider>` chip prefix inside each SelectItemText
            (no SelectGroup / SelectLabel nesting — see header
            note on UI fix #2).
          -->
          <SelectRoot
            :model-value="selectValue(row)"
            :disabled="isLoading(row.name)"
            @update:model-value="(v) => onModelChange(row, v)"
          >
            <SelectTrigger
              class="subagents-tab__trigger"
              :class="{
                'subagents-tab__trigger--invalid':
                  row.resolvedModelId !== null &&
                  row.resolvedModelDisplay === null,
              }"
              aria-label="Model"
            >
              <SelectValue>{{ triggerLabel(row) }}</SelectValue>
              <SelectIcon class="subagents-tab__trigger-icon">
                <Icon name="chevron-down" :size="12" />
              </SelectIcon>
            </SelectTrigger>
            <SelectPortal>
              <SelectContent
                class="subagents-tab__content"
                position="popper"
                :side-offset="4"
              >
                <SelectViewport class="subagents-tab__viewport">
                  <!-- Inherit parent — always first.
                       Value is the INHERIT_SENTINEL sentinel
                       (reka-ui 2.9.9 forbids empty-string
                       SelectItem values); onModelChange maps
                       it back to `null` for the IPC. -->
                  <SelectItem
                    :value="INHERIT_SENTINEL"
                    class="subagents-tab__option subagents-tab__option--inherit"
                  >
                    <SelectItemText>继承父级 (inherit)</SelectItemText>
                  </SelectItem>
                  <!-- Provider-grouped model list, flattened
                       (see UI fix #2 note). -->
                  <SelectItem
                    v-for="m in flatModelOptions"
                    :key="m.id"
                    :value="m.id"
                    class="subagents-tab__option"
                  >
                    <SelectItemText>
                      <span class="subagents-tab__option-provider">
                        {{ providerLabelFor(m.id) }}
                      </span>
                      <span class="subagents-tab__option-name">
                        {{ m.displayName }}
                      </span>
                    </SelectItemText>
                  </SelectItem>
                </SelectViewport>
              </SelectContent>
            </SelectPortal>
          </SelectRoot>
          <span
            v-if="isLoading(row.name)"
            class="subagents-tab__spinner"
            aria-label="保存中"
          />
        </div>
        <!--
          Invalid-model hint: resolvedModelId is set (DB override
          or frontmatter declared) but the model is gone from the
          catalog. Render a red "model 已删除，将降级" warning. The
          dropdown still shows the id verbatim so the user can
          change it.
        -->
        <p
          v-if="
            row.resolvedModelId !== null && row.resolvedModelDisplay === null
          "
          class="subagents-tab__invalid"
          role="alert"
        >
          模型已删除，将降级为父级 (id: {{ row.resolvedModelId }})
        </p>
        <p
          v-if="errorByName[row.name]"
          class="subagents-tab__error"
          role="alert"
        >
          {{ errorByName[row.name] }}
        </p>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.subagents-tab {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.subagents-tab__intro {
  margin: 0 0 4px 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: 1.6;
}

.subagents-tab__warn {
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 3px solid var(--color-tool-error);
  padding: 6px 10px;
  font-size: var(--text-xs);
  color: var(--color-text-primary);
  border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
}

.subagents-tab__loading {
  padding: 12px;
  color: var(--color-text-muted);
  font-size: var(--text-sm);
  text-align: center;
}

.subagents-tab__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.subagents-tab__row {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.subagents-tab__row-header {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.subagents-tab__name {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.subagents-tab__source-chip {
  display: inline-block;
  padding: 1px 6px;
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  border-radius: 999px;
  border: 1px solid var(--color-bg-border);
  color: var(--color-text-muted);
  background: var(--color-bg-surface);
}

.subagents-tab__source-chip[data-source="builtin"] {
  border-color: var(--color-tool-write);
  color: var(--color-tool-write);
}
.subagents-tab__source-chip[data-source="user"] {
  border-color: var(--color-accent);
  color: var(--color-accent-text);
}
.subagents-tab__source-chip[data-source="project"] {
  border-color: var(--color-tool-shell);
  color: var(--color-tool-shell);
}

.subagents-tab__override-chip {
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

.subagents-tab__description {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  line-height: 1.5;
  /* Truncate long descriptions to 2 lines (UI density). */
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.subagents-tab__model-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

/* --- reka-ui SelectRoot (mirrors ModelForm.vue's Provider selector) --- */

.subagents-tab__trigger {
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
.subagents-tab__trigger:hover {
  border-color: var(--color-accent-muted);
}
.subagents-tab__trigger[data-state="open"] {
  border-color: var(--color-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent);
}
.subagents-tab__trigger:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.subagents-tab__trigger--invalid {
  color: var(--color-tool-error-text);
  border-color: var(--color-tool-error);
}

.subagents-tab__trigger-icon {
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
}

/* Portal children — MUST be wrapped in :deep() because
 * SelectContent / SelectViewport / SelectItem are rendered
 * to <body> via <SelectPortal> and don't receive the
 * component's data-v-xxx attribute.
 * See `.trellis/spec/frontend/reka-ui-usage.md` for the
 * full explanation. */
:deep(.subagents-tab__content) {
  position: fixed;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  min-width: var(--reka-select-trigger-width, 240px);
  width: var(--reka-select-trigger-width);
  max-height: var(--reka-select-content-available-height);
  z-index: 3000 !important;
  overflow: hidden;
}

:deep(.subagents-tab__viewport) {
  padding: 4px;
  max-height: var(--reka-select-content-available-height);
}

:deep(.subagents-tab__option) {
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

:deep(.subagents-tab__option--inherit) {
  color: var(--color-text-secondary);
  font-style: italic;
}

:deep(.subagents-tab__option[data-highlighted]) {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

:deep(.subagents-tab__option[data-state="checked"]) {
  color: var(--color-accent-text);
}

:deep(.subagents-tab__option-provider) {
  display: inline-block;
  font-size: var(--text-2xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
  margin-right: 6px;
  padding: 1px 5px;
  background: var(--color-bg-elevated);
  border-radius: 3px;
}

:deep(.subagents-tab__option-name) {
  color: var(--color-text-primary);
}

.subagents-tab__spinner {
  width: 14px;
  height: 14px;
  border: 2px solid var(--color-bg-border);
  border-top-color: var(--color-accent);
  border-radius: 50%;
  animation: subagents-tab-spin 0.8s linear infinite;
  flex-shrink: 0;
}

@keyframes subagents-tab-spin {
  to { transform: rotate(360deg); }
}

.subagents-tab__invalid,
.subagents-tab__error {
  margin: 0;
  font-size: var(--text-xs);
  line-height: 1.5;
  padding: 4px 8px;
  border-radius: var(--radius-sm);
}

.subagents-tab__invalid {
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 2px solid var(--color-tool-error);
  font-family: var(--font-mono);
}

.subagents-tab__error {
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 2px solid var(--color-tool-error);
}
</style>