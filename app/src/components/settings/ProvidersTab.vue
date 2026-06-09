<script setup lang="ts">
// ProvidersTab — CRUD for LLM providers. Each provider has a protocol
// (anthropic/openai), display name, base URL, and API key.
//
// PR5 follow-up: the Test button that previously lived here has
// been removed. The user-perceived "Test" flow now lives on the
// Models tab (per-row "测试" button) and runs `test_model` —
// validating that a specific model_name can be reached end-to-end
// is the user-meaningful connectivity check. A provider-level
// protocol-reachability probe (the old `test_provider` IPC) was
// only a subset of that and surfaced confusing results when a
// provider routed to a GLM-style proxy with multiple model names
// where some were 404. `test_provider` is still in the Rust
// registry (`#[allow(dead_code)]`) for future catalog-resolution
// use, but the frontend never calls it.
//
// R1 polish: form controls now use reka-ui `SelectRoot` /
// `SelectTrigger` / `SelectItem` for the protocol dropdown, and
// native `<input>` wrapped in reka-ui `Label` (with project CSS
// vars) for the text fields. Reka-ui 2.x does not ship a generic
// `TextFieldRoot` (that arrived in 3.x), so for text/password
// inputs we keep the native element and theme it via the shared
// `reka-input` class to match the rest of the form. The v-model
// contract is unchanged.

import { ref, reactive, computed } from "vue";
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
  Label,
} from "reka-ui";
import { useProvidersStore, type ProviderRow } from "../../stores/providers";
import { useModelsStore } from "../../stores/models";
import Icon from "../Icon.vue";

const providersStore = useProvidersStore();
const modelsStore = useModelsStore();

// --- Editing state -------------------------------------------------------

type EditMode = "idle" | "add" | "edit";

const mode = ref<EditMode>("idle");
const editId = ref<string | null>(null);

const form = reactive({
  protocol: "anthropic" as string,
  displayName: "",
  baseUrl: "",
  apiKey: "",
});

const saving = ref(false);
const showApiKey = ref(false);
const deleteConfirmId = ref<string | null>(null);

/** PR5: Save is enabled when required fields are filled and the
 *  form isn't mid-save. The pre-PR5 gate also required
 *  `testPassed` — that gate is gone now that the Test button
 *  moved to ModelsTab. */
const canSave = computed(
  () =>
    form.displayName.trim() !== "" &&
    form.baseUrl.trim() !== "" &&
    form.apiKey.trim() !== "" &&
    !saving.value,
);

// --- Actions -------------------------------------------------------------

function resetForm() {
  form.protocol = "anthropic";
  form.displayName = "";
  form.baseUrl = "";
  form.apiKey = "";
  showApiKey.value = false;
  editId.value = null;
}

function startAdd() {
  resetForm();
  mode.value = "add";
}

function startEdit(p: ProviderRow) {
  mode.value = "edit";
  editId.value = p.id;
  form.protocol = p.protocol;
  form.displayName = p.displayName;
  form.baseUrl = p.baseUrl;
  form.apiKey = p.apiKey;
  showApiKey.value = false;
}

function cancelEdit() {
  mode.value = "idle";
  resetForm();
}

async function save() {
  if (!canSave.value) return;
  saving.value = true;
  try {
    if (mode.value === "add") {
      await providersStore.add(
        form.protocol,
        form.displayName,
        form.baseUrl,
        form.apiKey,
      );
    } else if (mode.value === "edit" && editId.value) {
      await providersStore.update(
        editId.value,
        form.protocol,
        form.displayName,
        form.baseUrl,
        form.apiKey,
      );
    }
    // Models may have changed (provider cascade) — reload.
    await modelsStore.load();
    mode.value = "idle";
    resetForm();
  } catch (e) {
    console.error("save failed:", e);
  } finally {
    saving.value = false;
  }
}

async function confirmDelete() {
  const id = deleteConfirmId.value;
  if (!id) return;
  deleteConfirmId.value = null;
  try {
    await providersStore.remove(id);
    // Models may have been cascade-deleted — reload.
    await modelsStore.load();
  } catch (e) {
    console.error("delete failed:", e);
  }
}

/** Mask the api_key for display: show first 6 chars + "****". */
function maskApiKey(key: string): string {
  if (!key) return "(未设置)";
  if (key.length <= 8) return "****";
  return key.slice(0, 6) + "****";
}

/** Protocol badge color. */
function protocolBadgeClass(protocol: string): string {
  return protocol === "openai"
    ? "providers-tab__badge--openai"
    : "providers-tab__badge--anthropic";
}
</script>

<template>
  <div class="providers-tab">
    <!-- Header row -->
    <div class="providers-tab__header">
      <h3 class="providers-tab__heading">Providers</h3>
      <button
        v-if="mode === 'idle'"
        type="button"
        class="providers-tab__btn providers-tab__btn--primary"
        @click="startAdd"
      >
        <Icon name="plus" :size="14" />
        Add Provider
      </button>
    </div>

    <!-- Provider list -->
    <div v-if="mode === 'idle'" class="providers-tab__list">
      <div
        v-for="p in providersStore.providers"
        :key="p.id"
        class="providers-tab__row"
      >
        <div class="providers-tab__row-info">
          <span class="providers-tab__name">{{ p.displayName }}</span>
          <span :class="['providers-tab__badge', protocolBadgeClass(p.protocol)]">
            {{ p.protocol }}
          </span>
          <span class="providers-tab__url">{{ p.baseUrl }}</span>
          <span class="providers-tab__key-hint">
            <Icon name="key" :size="11" />
            {{ maskApiKey(p.apiKey) }}
          </span>
        </div>
        <div class="providers-tab__row-actions">
          <button
            type="button"
            class="providers-tab__btn providers-tab__btn--ghost"
            @click="startEdit(p)"
          >
            <Icon name="pencil" :size="12" />
          </button>
          <button
            type="button"
            class="providers-tab__btn providers-tab__btn--ghost providers-tab__btn--danger"
            @click="deleteConfirmId = p.id"
          >
            <Icon name="trash" :size="12" />
          </button>
        </div>
      </div>

      <div
        v-if="providersStore.providers.length === 0"
        class="providers-tab__empty"
      >
        No providers configured. Click "Add Provider" to get started.
      </div>
    </div>

    <!-- Add / Edit form -->
    <div v-if="mode !== 'idle'" class="providers-tab__form">
      <h4 class="providers-tab__form-title">
        {{ mode === "add" ? "Add Provider" : "Edit Provider" }}
      </h4>

      <Label class="providers-tab__field">
        <span class="providers-tab__label">Protocol</span>
        <SelectRoot v-model="form.protocol">
          <SelectTrigger class="providers-tab__trigger" aria-label="Protocol">
            <SelectValue placeholder="Select protocol" />
            <SelectIcon class="providers-tab__trigger-icon">
              <Icon name="chevron-down" :size="12" />
            </SelectIcon>
          </SelectTrigger>
          <SelectPortal>
            <SelectContent class="providers-tab__content" position="popper" :side-offset="4">
              <SelectViewport class="providers-tab__viewport">
                <SelectItem value="anthropic" class="providers-tab__option">
                  <SelectItemText>Anthropic (Messages API)</SelectItemText>
                </SelectItem>
                <SelectItem value="openai" class="providers-tab__option">
                  <SelectItemText>OpenAI (Chat Completions)</SelectItemText>
                </SelectItem>
              </SelectViewport>
            </SelectContent>
          </SelectPortal>
        </SelectRoot>
      </Label>

      <Label class="providers-tab__field">
        <span class="providers-tab__label">Display Name</span>
        <input
          v-model="form.displayName"
          type="text"
          class="providers-tab__input"
          placeholder="My Provider"
        />
      </Label>

      <Label class="providers-tab__field">
        <span class="providers-tab__label">Base URL</span>
        <input
          v-model="form.baseUrl"
          type="text"
          class="providers-tab__input"
          placeholder="https://api.anthropic.com"
        />
      </Label>

      <Label class="providers-tab__field">
        <span class="providers-tab__label">API Key</span>
        <div class="providers-tab__key-input">
          <input
            v-model="form.apiKey"
            :type="showApiKey ? 'text' : 'password'"
            class="providers-tab__input"
            placeholder="sk-..."
          />
          <button
            type="button"
            class="providers-tab__btn providers-tab__btn--ghost"
            :title="showApiKey ? 'Hide' : 'Show'"
            @click="showApiKey = !showApiKey"
          >
            <Icon :name="showApiKey ? 'eye-slash' : 'eye'" :size="14" />
          </button>
        </div>
      </Label>

      <!-- Form actions -->
      <div class="providers-tab__form-actions">
        <button
          type="button"
          class="providers-tab__btn providers-tab__btn--primary"
          :disabled="!canSave"
          @click="save"
        >
          {{ saving ? "Saving..." : "Save" }}
        </button>
        <button
          type="button"
          class="providers-tab__btn providers-tab__btn--secondary"
          @click="cancelEdit"
        >
          Cancel
        </button>
      </div>
    </div>

    <!-- Delete confirmation -->
    <div v-if="deleteConfirmId" class="providers-tab__confirm-overlay" @click.self="deleteConfirmId = null">
      <div class="providers-tab__confirm">
        <p class="providers-tab__confirm-text">
          Delete this provider? All associated models will also be removed.
          Sessions referencing these models will fall back to the default.
        </p>
        <div class="providers-tab__confirm-actions">
          <button
            type="button"
            class="providers-tab__btn providers-tab__btn--danger"
            @click="confirmDelete"
          >
            Delete
          </button>
          <button
            type="button"
            class="providers-tab__btn providers-tab__btn--secondary"
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
.providers-tab {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.providers-tab__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.providers-tab__heading {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.providers-tab__list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.providers-tab__row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 8px 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
}

.providers-tab__row-info {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  flex: 1;
}

.providers-tab__name {
  font-size: 13px;
  font-weight: 500;
  color: var(--color-text-primary);
}

.providers-tab__badge {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-weight: 500;
}

.providers-tab__badge--anthropic {
  background: var(--color-accent-muted);
  color: var(--color-accent);
}

.providers-tab__badge--openai {
  background: #1a3a2a;
  color: #10b981;
}

.providers-tab__url {
  font-size: 11px;
  color: var(--color-text-muted);
  font-family: var(--font-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.providers-tab__key-hint {
  font-size: 11px;
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.providers-tab__row-actions {
  display: flex;
  gap: 4px;
  flex-shrink: 0;
}

.providers-tab__empty {
  padding: 24px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: 13px;
}

/* --- Form --- */

.providers-tab__form {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
}

.providers-tab__form-title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.providers-tab__field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.providers-tab__label {
  font-size: 11px;
  font-weight: 500;
  color: var(--color-text-secondary);
}

.providers-tab__input {
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-text-primary);
  font-size: 13px;
  width: 100%;
  box-sizing: border-box;
}

.providers-tab__input:focus {
  outline: none;
  border-color: var(--color-accent);
}

.providers-tab__select {
  appearance: auto;
}

/* --- R1: reka-ui Select trigger / content / option theming --- */

.providers-tab__trigger {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-text-primary);
  font-size: 13px;
  font-family: inherit;
  width: 100%;
  box-sizing: border-box;
  cursor: pointer;
  transition: border-color 0.15s;
}

.providers-tab__trigger:hover {
  border-color: var(--color-accent-muted);
}

.providers-tab__trigger[data-state="open"] {
  border-color: var(--color-accent);
}

.providers-tab__trigger-icon {
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
}

:deep(.providers-tab__content) {
  position: fixed;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  min-width: var(--reka-select-trigger-width, 240px);
  width: var(--reka-select-trigger-width);
  z-index: 3000 !important;
  overflow: hidden;
}

:deep(.providers-tab__viewport) {
  padding: 4px;
}

:deep(.providers-tab__option) {
  display: flex;
  align-items: center;
  padding: 6px 10px;
  font-size: 13px;
  color: var(--color-text-primary);
  border-radius: 4px;
  cursor: pointer;
  user-select: none;
  outline: none;
}

:deep(.providers-tab__option[data-highlighted]) {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

:deep(.providers-tab__option[data-state="checked"]) {
  color: var(--color-accent);
}

.providers-tab__key-input {
  display: flex;
  gap: 4px;
  align-items: center;
}

.providers-tab__key-input .providers-tab__input {
  flex: 1;
}

.providers-tab__test-result {
  font-size: 12px;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.providers-tab__test-ok {
  color: var(--color-tool-write);
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.providers-tab__test-fail {
  color: var(--color-tool-error);
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.providers-tab__form-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}

/* --- Buttons --- */

.providers-tab__btn {
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

.providers-tab__btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.providers-tab__btn--primary {
  background: var(--color-accent);
  color: #fff;
  border-color: var(--color-accent);
}

.providers-tab__btn--primary:hover:not(:disabled) {
  background: var(--color-accent-hover);
}

.providers-tab__btn--secondary {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.providers-tab__btn--secondary:hover:not(:disabled) {
  background: var(--color-bg-border);
}

.providers-tab__btn--ghost {
  background: transparent;
  border: 0;
  padding: 4px;
  color: var(--color-text-muted);
}

.providers-tab__btn--ghost:hover:not(:disabled) {
  color: var(--color-text-primary);
  background: var(--color-bg-border);
}

.providers-tab__btn--danger {
  color: var(--color-tool-error);
}

.providers-tab__btn--danger:hover:not(:disabled) {
  background: rgba(239, 68, 68, 0.15);
}

/* --- Delete confirm --- */

.providers-tab__confirm-overlay {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
  border-radius: 6px;
}

.providers-tab__confirm {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  padding: 16px;
  max-width: 360px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.providers-tab__confirm-text {
  margin: 0;
  font-size: 13px;
  color: var(--color-text-primary);
  line-height: 1.5;
}

.providers-tab__confirm-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}
</style>
