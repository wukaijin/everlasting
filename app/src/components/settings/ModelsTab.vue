<script setup lang="ts">
// ModelsTab — CRUD for LLM models, grouped by provider. Uses
// `useModelsStore().modelsGroupedByProvider` for the grouped display.
// Add/Edit form includes: provider select, model name, display name,
// max tokens (optional), thinking effort (optional), supports thinking,
// context window.

import { ref, reactive, computed } from "vue";
import { useModelsStore, type ModelWithProvider } from "../../stores/models";
import { useProvidersStore } from "../../stores/providers";
import Icon from "../Icon.vue";

const modelsStore = useModelsStore();
const providersStore = useProvidersStore();

// --- Editing state -------------------------------------------------------

type EditMode = "idle" | "add" | "edit";

const mode = ref<EditMode>("idle");
const editId = ref<string | null>(null);
const deleteConfirmId = ref<string | null>(null);
const saving = ref(false);

const form = reactive({
  providerId: "" as string,
  modelName: "",
  displayName: "",
  maxTokens: "" as string, // empty string = omit (None)
  thinkingEffort: "" as string, // empty string = omit (None)
  supportsThinking: false,
  contextWindow: 8192,
});

// --- Computed ------------------------------------------------------------

const grouped = computed(() => modelsStore.modelsGroupedByProvider);

const canSave = computed(
  () =>
    form.providerId !== "" &&
    form.modelName.trim() !== "" &&
    form.displayName.trim() !== "" &&
    form.contextWindow > 0 &&
    !saving.value,
);

// --- Actions -------------------------------------------------------------

function resetForm() {
  form.providerId = providersStore.providers[0]?.id ?? "";
  form.modelName = "";
  form.displayName = "";
  form.maxTokens = "";
  form.thinkingEffort = "";
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
  form.thinkingEffort = m.thinkingEffort ?? "";
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
    if (form.thinkingEffort.trim() !== "") {
      opts.thinkingEffort = form.thinkingEffort.trim();
    }

    if (mode.value === "add") {
      await modelsStore.add(
        form.providerId,
        form.modelName.trim(),
        form.displayName.trim(),
        opts,
      );
    } else if (mode.value === "edit" && editId.value) {
      await modelsStore.update(
        editId.value,
        form.providerId,
        form.modelName.trim(),
        form.displayName.trim(),
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
  } catch (e) {
    console.error("delete model failed:", e);
  }
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
          <span class="models-tab__group-name">{{ group.provider.displayName }}</span>
          <span class="models-tab__group-count">
            {{ group.models.length }}
          </span>
        </div>
        <div
          v-for="m in group.models"
          :key="m.id"
          class="models-tab__row"
        >
          <div class="models-tab__row-info">
            <span class="models-tab__name">{{ m.displayName }}</span>
            <span class="models-tab__model-id">{{ m.modelName }}</span>
            <span v-if="m.supportsThinking" class="models-tab__tag">
              thinking
            </span>
            <span class="models-tab__tag models-tab__tag--muted">
              {{ m.contextWindow >= 1000 ? `${m.contextWindow / 1000}k` : m.contextWindow }}
            </span>
          </div>
          <div class="models-tab__row-actions">
            <button
              type="button"
              class="models-tab__btn models-tab__btn--ghost"
              @click="startEdit(m)"
            >
              <Icon name="pencil" :size="12" />
            </button>
            <button
              type="button"
              class="models-tab__btn models-tab__btn--ghost models-tab__btn--danger"
              @click="deleteConfirmId = m.id"
            >
              <Icon name="trash" :size="12" />
            </button>
          </div>
        </div>
      </div>

      <div
        v-if="modelsStore.models.length === 0"
        class="models-tab__empty"
      >
        No models configured. Click "Add Model" to get started.
      </div>
    </div>

    <!-- Add / Edit form -->
    <div v-if="mode !== 'idle'" class="models-tab__form">
      <h4 class="models-tab__form-title">
        {{ mode === "add" ? "Add Model" : "Edit Model" }}
      </h4>

      <label class="models-tab__field">
        <span class="models-tab__label">Provider</span>
        <select v-model="form.providerId" class="models-tab__input models-tab__select">
          <option
            v-for="p in providersStore.providers"
            :key="p.id"
            :value="p.id"
          >
            {{ p.displayName }} ({{ p.protocol }})
          </option>
        </select>
      </label>

      <div class="models-tab__row-pair">
        <label class="models-tab__field">
          <span class="models-tab__label">Model Name</span>
          <input
            v-model="form.modelName"
            type="text"
            class="models-tab__input"
            placeholder="claude-sonnet-4-5"
          />
        </label>
        <label class="models-tab__field">
          <span class="models-tab__label">Display Name</span>
          <input
            v-model="form.displayName"
            type="text"
            class="models-tab__input"
            placeholder="Claude Sonnet 4.5"
          />
        </label>
      </div>

      <div class="models-tab__row-pair">
        <label class="models-tab__field">
          <span class="models-tab__label">Max Tokens (optional)</span>
          <input
            v-model="form.maxTokens"
            type="text"
            class="models-tab__input"
            placeholder="16384"
          />
        </label>
        <label class="models-tab__field">
          <span class="models-tab__label">Context Window</span>
          <input
            v-model.number="form.contextWindow"
            type="number"
            class="models-tab__input"
            min="1"
          />
        </label>
      </div>

      <div class="models-tab__row-pair">
        <label class="models-tab__field">
          <span class="models-tab__label">Thinking Effort (optional)</span>
          <select v-model="form.thinkingEffort" class="models-tab__input models-tab__select">
            <option value="">(default: high)</option>
            <option value="low">low</option>
            <option value="medium">medium</option>
            <option value="high">high</option>
            <option value="xhigh">xhigh</option>
            <option value="max">max</option>
          </select>
        </label>
        <label class="models-tab__field models-tab__field--check">
          <span class="models-tab__label">Supports Thinking</span>
          <input v-model="form.supportsThinking" type="checkbox" class="models-tab__checkbox" />
        </label>
      </div>

      <!-- Form actions -->
      <div class="models-tab__form-actions">
        <button
          type="button"
          class="models-tab__btn models-tab__btn--primary"
          :disabled="!canSave"
          @click="save"
        >
          {{ saving ? "Saving..." : "Save" }}
        </button>
        <button
          type="button"
          class="models-tab__btn models-tab__btn--secondary"
          @click="cancelEdit"
        >
          Cancel
        </button>
      </div>
    </div>

    <!-- Delete confirmation -->
    <div v-if="deleteConfirmId" class="models-tab__confirm-overlay" @click.self="deleteConfirmId = null">
      <div class="models-tab__confirm">
        <p class="models-tab__confirm-text">
          Delete this model? Sessions referencing this model will fall back to
          the default model.
        </p>
        <div class="models-tab__confirm-actions">
          <button
            type="button"
            class="models-tab__btn models-tab__btn--danger"
            @click="confirmDelete"
          >
            Delete
          </button>
          <button
            type="button"
            class="models-tab__btn models-tab__btn--secondary"
            @click="deleteConfirmId = null"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.models-tab {
  display: flex;
  flex-direction: column;
  gap: 12px;
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

.models-tab__row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 8px 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
}

.models-tab__row-info {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  flex: 1;
}

.models-tab__name {
  font-size: 13px;
  font-weight: 500;
  color: var(--color-text-primary);
}

.models-tab__model-id {
  font-size: 11px;
  color: var(--color-text-muted);
  font-family: var(--font-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.models-tab__tag {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 3px;
  background: var(--color-accent-muted);
  color: var(--color-accent);
  font-family: var(--font-mono);
  flex-shrink: 0;
}

.models-tab__tag--muted {
  background: var(--color-bg-border);
  color: var(--color-text-muted);
}

.models-tab__row-actions {
  display: flex;
  gap: 4px;
  flex-shrink: 0;
}

.models-tab__empty {
  padding: 24px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: 13px;
}

/* --- Form --- */

.models-tab__form {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
}

.models-tab__form-title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.models-tab__row-pair {
  display: flex;
  gap: 12px;
}

.models-tab__row-pair > .models-tab__field {
  flex: 1;
}

.models-tab__field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.models-tab__field--check {
  flex-direction: row;
  align-items: center;
  gap: 8px;
  justify-content: flex-end;
  padding-top: 18px;
}

.models-tab__label {
  font-size: 11px;
  font-weight: 500;
  color: var(--color-text-secondary);
}

.models-tab__input {
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-text-primary);
  font-size: 13px;
  width: 100%;
  box-sizing: border-box;
}

.models-tab__input:focus {
  outline: none;
  border-color: var(--color-accent);
}

.models-tab__select {
  appearance: auto;
}

.models-tab__checkbox {
  width: 16px;
  height: 16px;
  accent-color: var(--color-accent);
}

.models-tab__form-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}

/* --- Buttons --- */

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
  transition: background 0.15s, color 0.15s;
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

.models-tab__btn--secondary {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.models-tab__btn--secondary:hover:not(:disabled) {
  background: var(--color-bg-border);
}

.models-tab__btn--ghost {
  background: transparent;
  border: 0;
  padding: 4px;
  color: var(--color-text-muted);
}

.models-tab__btn--ghost:hover:not(:disabled) {
  color: var(--color-text-primary);
  background: var(--color-bg-border);
}

.models-tab__btn--danger {
  color: var(--color-tool-error);
}

.models-tab__btn--danger:hover:not(:disabled) {
  background: rgba(239, 68, 68, 0.15);
}

/* --- Delete confirm --- */

.models-tab__confirm-overlay {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
  border-radius: 6px;
}

.models-tab__confirm {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  padding: 16px;
  max-width: 360px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.models-tab__confirm-text {
  margin: 0;
  font-size: 13px;
  color: var(--color-text-primary);
  line-height: 1.5;
}

.models-tab__confirm-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}
</style>
