<script setup lang="ts">
// ModelsTab — CRUD for LLM models, grouped by provider. Uses
// `useModelsStore().modelsGroupedByProvider` for the grouped
// display. 8-PR3 split: this file is now a thin orchestration
// layer. The row rendering moved to `ModelRow.vue`, the Add/Edit
// form moved to `ModelForm.vue`, and the delete confirmation
// overlay moved to `DeleteModelConfirm.vue`. This file owns the
// editing state (mode / editId / form / saving), the per-row
// test state (`tests` map), and the action handlers (`startAdd`,
// `startEdit`, `save`, `runTest`, `confirmDelete`).
//
// Stream-controller rule: nothing here mutates the streamController
// directly — the underlying `modelsStore.add / update / remove`
// calls handle the cross-store invalidation. Per
// `.trellis/spec/frontend/state-management.md §Stream Controller
// Pattern`, the test IPC is independent of the chat stream so
// it stays out of the controller's domain.

import { ref, reactive, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useModelsStore, type ModelWithProvider } from "../../stores/models";
import { useProvidersStore } from "../../stores/providers";
import ModelRow, { type TestState } from "./ModelRow.vue";
import ModelForm, { type ModelFormState } from "./ModelForm.vue";
import DeleteModelConfirm from "./DeleteModelConfirm.vue";
import Icon from "../Icon.vue";

const modelsStore = useModelsStore();
const providersStore = useProvidersStore();

// --- Editing state -------------------------------------------------------

type EditMode = "idle" | "add" | "edit";

const mode = ref<EditMode>("idle");
const editId = ref<string | null>(null);
const deleteConfirmId = ref<string | null>(null);
const saving = ref(false);

const form = reactive<ModelFormState>({
  providerId: "",
  modelName: "",
  displayName: "",
  maxTokens: "",
  thinkingEffort: "none",
  supportsThinking: false,
  contextWindow: 8192,
});

// --- Test state ----------------------------------------------------------
// Map<modelId, TestState> — one slot per model row. Cleared on
// re-test (entry overwritten) or on model deletion (entry removed
// by `confirmDelete` below).
const tests = reactive<Record<string, TestState>>({});

// --- Computed ------------------------------------------------------------

const grouped = computed(() => modelsStore.modelsGroupedByProvider);

const canSave = computed(
  () =>
    form.providerId !== "" &&
    form.modelName.trim() !== "" &&
    form.contextWindow > 0 &&
    !saving.value,
);

// --- Actions -------------------------------------------------------------

function resetForm() {
  form.providerId = providersStore.providers[0]?.id ?? "";
  form.modelName = "";
  form.displayName = "";
  form.maxTokens = "";
  form.thinkingEffort = "none";
  form.supportsThinking = false;
  form.contextWindow = 8192;
  editId.value = null;
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
  form.contextWindow = m.contextWindow;
}

function cancelEdit() {
  mode.value = "idle";
  resetForm();
}

async function save() {
  if (!canSave.value) return;
  saving.value = true;
  try {
    // display_name fallback: empty → use model_name
    const displayName = form.displayName.trim() || form.modelName.trim();
    // Build opts: omit undefined fields so Tauri IPC treats them as
    // None (not null). See HACKING-wsl FU-1.
    const opts: {
      supportsThinking: boolean;
      contextWindow: number;
      maxTokens?: number;
      thinkingEffort?: string;
    } = {
      supportsThinking: form.supportsThinking,
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
      await modelsStore.add(
        form.providerId,
        form.modelName.trim(),
        displayName,
        opts,
      );
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
    // PR5: drop any cached Test result for the deleted model so
    // a future row with a colliding id doesn't render a stale
    // result.
    delete tests[id];
  } catch (e) {
    console.error("delete model failed:", e);
  }
}

/** PR5: invoke the `test_model` IPC for a specific catalog row.
 *  Renders the result inline in the row. The result persists
 *  until the user re-clicks Test on the same row OR the row is
 *  deleted. */
async function runTest(modelId: string) {
  tests[modelId] = { kind: "running" };
  try {
    const result = await invoke<{
      success: boolean;
      latencyMs: number;
      error: string | null;
    }>("test_model", { modelId });
    if (result.success) {
      tests[modelId] = { kind: "ok", latencyMs: result.latencyMs };
    } else {
      tests[modelId] = {
        kind: "fail",
        error: result.error ?? "Connection failed",
      };
    }
  } catch (e) {
    tests[modelId] = { kind: "fail", error: String(e) };
  }
}

function openDeleteConfirm(m: ModelWithProvider) {
  deleteConfirmId.value = m.id;
}
</script>

<template>
    <div class="models-tab">
        <!-- Header row -->
        <div class="models-tab__header">
            <h3 class="models-tab__heading">Models</h3>
            <button
                v-if="mode === 'idle'"
                type="button"
                class="models-tab__btn models-tab__btn--primary"
                @click="startAdd"
            >
                <Icon name="plus" :size="14" />
                Add Model
            </button>
        </div>

        <!-- Grouped model list -->
        <div v-if="mode === 'idle'" class="models-tab__list">
            <div
                v-for="group in grouped"
                :key="group.provider.id"
                class="models-tab__group"
            >
                <div class="models-tab__group-header">
                    <Icon name="server" :size="12" />
                    <span class="models-tab__group-name">{{
                        group.provider.displayName
                    }}</span>
                    <span class="models-tab__group-count">
                        {{ group.models.length }}
                    </span>
                </div>
                <ModelRow
                    v-for="m in group.models"
                    :key="m.id"
                    :model="m"
                    :test="tests[m.id]"
                    :is-streaming="false"
                    @test="runTest(m.id)"
                    @edit="startEdit(m)"
                    @delete="openDeleteConfirm(m)"
                />
            </div>

            <div v-if="modelsStore.models.length === 0" class="models-tab__empty">
                No models configured. Click "Add Model" to get started.
            </div>
        </div>

        <!-- Add / Edit form -->
        <ModelForm
            v-else
            :mode="mode"
            :form="form"
            :providers="providersStore.providers"
            :saving="saving"
            :can-save="canSave"
            @submit="save"
            @cancel="cancelEdit"
        />

        <!-- Delete confirmation -->
        <DeleteModelConfirm
            :is-open="deleteConfirmId !== null"
            :model-name="''"
            @confirm="confirmDelete"
            @cancel="deleteConfirmId = null"
        />
    </div>
</template>

<style scoped>
.models-tab {
    display: flex;
    flex-direction: column;
    gap: 12px;
    position: relative;
}

.models-tab__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
}

.models-tab__heading {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--color-text-primary);
}

.models-tab__list {
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.models-tab__group {
    display: flex;
    flex-direction: column;
    gap: 4px;
}

.models-tab__group-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 0;
    color: var(--color-text-muted);
    font-size: 11px;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.04em;
}

.models-tab__group-name {
    color: var(--color-text-secondary);
}

.models-tab__group-count {
    background: var(--color-bg-elevated);
    padding: 0 6px;
    border-radius: 3px;
    font-size: 10px;
    font-family: var(--font-mono);
}

.models-tab__empty {
    padding: 24px;
    text-align: center;
    color: var(--color-text-muted);
    font-size: 13px;
}

/* --- Buttons (header Add Model) --- */

.models-tab__btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 12px;
    border: 1px solid var(--color-bg-border);
    border-radius: 4px;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    background: transparent;
    color: var(--color-text-secondary);
    transition:
        background 0.15s,
        color 0.15s;
}

.models-tab__btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
}

.models-tab__btn--primary {
    background: var(--color-accent);
    color: #fff;
    border-color: var(--color-accent);
}

.models-tab__btn--primary:hover:not(:disabled) {
    background: var(--color-accent-hover);
}
</style>