<script setup lang="ts">
// ModelsTab — CRUD for LLM models, grouped by provider. Uses
// `useModelsStore().modelsGroupedByProvider` for the grouped display.
// Add/Edit form includes: provider select, model name, display name,
// max tokens (optional), thinking effort (optional), supports thinking,
// context window.
//
// PR5 follow-up: each row now has a "测试" button (right side, in
// `.models-tab__row-actions`) that invokes the new `test_model` IPC
// (catalog-resolved, real `model.model_name` payload). The result is
// rendered inline in the row and persists until either (a) the user
// clicks Test again, or (b) the model row is deleted. Switching
// providers or editing the model fields intentionally does NOT clear
// the result — the test is for the model as a whole, not for the
// form draft.
//
// R1 polish: form controls now use reka-ui `SelectRoot` for the
// provider dropdown, reka-ui `CheckboxRoot` for supportsThinking,
// and themed native inputs (wrapped in reka-ui `Label`) for the
// text fields. Reka-ui 2.x doesn't ship a generic `TextFieldRoot`,
// so for text inputs we keep `<input>` and theme it via the shared
// `.models-tab__input` class to match the rest of the form. The
// contextWindow number input keeps `type="number"` and the v-model
// contract is unchanged.

import { ref, reactive, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
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
  CheckboxRoot,
  CheckboxIndicator,
  Label,
} from "reka-ui";
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
  thinkingEffort: "none" as string, // "none" sentinel = omit (None)
  supportsThinking: false,
  contextWindow: 8192,
});

// --- Test state ----------------------------------------------------------
// Map<modelId, TestState> — one slot per model row. Cleared on
// re-test (entry overwritten) or on model deletion (entry removed
// by `confirmDelete` below).
type TestState =
  | { kind: "running" }
  | { kind: "ok"; latencyMs: number }
  | { kind: "fail"; error: string };

const tests = reactive<Record<string, TestState>>({});

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
    // PR5: drop any cached Test result for the deleted model so
    // a future row with a colliding id doesn't render a stale
    // result.
    delete tests[id];
  } catch (e) {
    console.error("delete model failed:", e);
  }
}

/** PR5: invoke the `test_model` IPC for a specific catalog row.
 *  Renders the result inline in the row (`.models-tab__row-test`).
 *  Per the PR5 spec, the result persists until the user re-clicks
 *  Test on the same row OR the row is deleted. */
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

/** PR5: per-row Test result rendering helpers. Extracted from
 *  the template so the runtime narrowing happens in TypeScript
 *  (the template language doesn't allow `as` casts). */
function testClass(t: TestState | undefined): Record<string, boolean> {
  if (!t) return {};
  return {
    "models-tab__row-test--ok": t.kind === "ok",
    "models-tab__row-test--fail": t.kind === "fail",
    "models-tab__row-test--running": t.kind === "running",
  };
}

function okLatency(t: TestState | undefined): number {
  return t?.kind === "ok" ? t.latencyMs : 0;
}

function failError(t: TestState | undefined): string {
  return t?.kind === "fail" ? t.error : "";
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
        <div v-for="m in group.models" :key="m.id" class="models-tab__row">
          <div class="models-tab__row-info">
            <span class="models-tab__name">{{ m.displayName }}</span>
            <span class="models-tab__model-id">{{ m.modelName }}</span>
            <span v-if="m.supportsThinking" class="models-tab__tag">
              thinking
            </span>
            <span class="models-tab__tag models-tab__tag--muted">
              {{
                m.contextWindow >= 1000
                  ? `${m.contextWindow / 1000}k`
                  : m.contextWindow
              }}
            </span>
            <!-- PR5: per-row Test result, inline. The label
                 appears under the model_id so the row's vertical
                 rhythm is unchanged on the success / never-tested
                 path. -->
            <span
              v-if="tests[m.id]"
              class="models-tab__row-test"
              :class="testClass(tests[m.id])"
            >
              <template v-if="tests[m.id]?.kind === 'running'">
                测试中…
              </template>
              <template v-else-if="tests[m.id]?.kind === 'ok'">
                <Icon name="check" :size="11" />
                通过 ({{ okLatency(tests[m.id]) }}ms)
              </template>
              <template v-else>
                <Icon name="warn" :size="11" />
                {{ failError(tests[m.id]) }}
              </template>
            </span>
          </div>
          <div class="models-tab__row-actions">
            <button
              type="button"
              class="models-tab__btn models-tab__btn--ghost"
              :disabled="tests[m.id]?.kind === 'running'"
              :title="
                tests[m.id]?.kind === 'running'
                  ? '测试中…'
                  : '测试此 model 连通性'
              "
              @click="runTest(m.id)"
            >
              <Icon name="signal" :size="12" />
            </button>
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

      <div v-if="modelsStore.models.length === 0" class="models-tab__empty">
        No models configured. Click "Add Model" to get started.
      </div>
    </div>

    <!-- Add / Edit form -->
    <div v-if="mode !== 'idle'" class="models-tab__form">
      <h4 class="models-tab__form-title">
        {{ mode === "add" ? "Add Model" : "Edit Model" }}
      </h4>

      <Label class="models-tab__field">
        <span class="models-tab__label">Provider</span>
        <SelectRoot v-model="form.providerId">
          <SelectTrigger class="models-tab__trigger" aria-label="Provider">
            <SelectValue placeholder="Select provider" />
            <SelectIcon class="models-tab__trigger-icon">
              <Icon name="chevron-down" :size="12" />
            </SelectIcon>
          </SelectTrigger>
          <SelectPortal>
            <SelectContent
              class="models-tab__content"
              position="popper"
              :side-offset="4"
            >
              <SelectViewport class="models-tab__viewport">
                <SelectItem
                  v-for="p in providersStore.providers"
                  :key="p.id"
                  :value="p.id"
                  class="models-tab__option"
                >
                  <SelectItemText
                    >{{ p.displayName }} ({{ p.protocol }})</SelectItemText
                  >
                </SelectItem>
              </SelectViewport>
            </SelectContent>
          </SelectPortal>
        </SelectRoot>
      </Label>

      <div class="models-tab__row-pair">
        <Label class="models-tab__field">
          <span class="models-tab__label">Model Name</span>
          <input
            v-model="form.modelName"
            type="text"
            class="models-tab__input"
            placeholder="claude-sonnet-4-5"
          />
        </Label>
        <Label class="models-tab__field">
          <span class="models-tab__label">Display Name</span>
          <input
            v-model="form.displayName"
            type="text"
            class="models-tab__input"
            placeholder="Claude Sonnet 4.5"
          />
        </Label>
      </div>

      <div class="models-tab__row-pair">
        <Label class="models-tab__field">
          <span class="models-tab__label">Max Tokens (optional)</span>
          <input
            v-model="form.maxTokens"
            type="text"
            class="models-tab__input"
            placeholder="16384"
          />
        </Label>
        <Label class="models-tab__field">
          <span class="models-tab__label">Context Window</span>
          <input
            v-model.number="form.contextWindow"
            type="number"
            class="models-tab__input"
            min="1"
          />
        </Label>
      </div>

      <div class="models-tab__row-pair">
        <Label class="models-tab__field">
          <span class="models-tab__label">Thinking Effort (optional)</span>
          <SelectRoot v-model="form.thinkingEffort">
            <SelectTrigger
              class="models-tab__trigger"
              aria-label="Thinking effort"
            >
              <SelectValue placeholder="(default: high)" />
              <SelectIcon class="models-tab__trigger-icon">
                <Icon name="chevron-down" :size="12" />
              </SelectIcon>
            </SelectTrigger>
            <SelectPortal>
              <SelectContent
                class="models-tab__content"
                position="popper"
                :side-offset="4"
              >
                <SelectViewport class="models-tab__viewport">
                  <SelectItem value="none" class="models-tab__option">
                    <SelectItemText>(default: high)</SelectItemText>
                  </SelectItem>
                  <SelectItem value="low" class="models-tab__option">
                    <SelectItemText>low</SelectItemText>
                  </SelectItem>
                  <SelectItem value="medium" class="models-tab__option">
                    <SelectItemText>medium</SelectItemText>
                  </SelectItem>
                  <SelectItem value="high" class="models-tab__option">
                    <SelectItemText>high</SelectItemText>
                  </SelectItem>
                  <SelectItem value="xhigh" class="models-tab__option">
                    <SelectItemText>xhigh</SelectItemText>
                  </SelectItem>
                  <SelectItem value="max" class="models-tab__option">
                    <SelectItemText>max</SelectItemText>
                  </SelectItem>
                </SelectViewport>
              </SelectContent>
            </SelectPortal>
          </SelectRoot>
        </Label>
        <div class="models-tab__field models-tab__field--check">
          <span class="models-tab__label">Supports Thinking</span>
          <CheckboxRoot
            v-model="form.supportsThinking"
            class="models-tab__checkbox"
          >
            <CheckboxIndicator class="models-tab__checkbox-indicator">
              <Icon name="check" :size="11" />
            </CheckboxIndicator>
          </CheckboxRoot>
        </div>
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
    <div
      v-if="deleteConfirmId"
      class="models-tab__confirm-overlay"
      @click.self="deleteConfirmId = null"
    >
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

/* PR5: per-row Test result badge. Inline with the model_id so
   the row's vertical rhythm matches the pre-PR5 layout. The
   running state uses the muted text color (it'll resolve to ok
   or fail shortly); the success / fail states use the same
   tool-color tokens as the rest of the settings tabs. */
.models-tab__row-test {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 10px;
  font-family: var(--font-mono);
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.models-tab__row-test--ok {
  color: var(--color-tool-write);
}

.models-tab__row-test--fail {
  color: var(--color-tool-error);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 240px;
}

.models-tab__row-test--running {
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
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 3px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition:
    border-color 0.15s,
    background 0.15s;
}

.models-tab__checkbox:hover {
  border-color: var(--color-accent-muted);
}

.models-tab__checkbox[data-state="checked"] {
  background: var(--color-accent);
  border-color: var(--color-accent);
}

.models-tab__checkbox-indicator {
  color: #fff;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  line-height: 0;
}

/* --- R1: reka-ui Select trigger / content / option theming --- */

.models-tab__trigger {
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

.models-tab__trigger:hover {
  border-color: var(--color-accent-muted);
}

.models-tab__trigger[data-state="open"] {
  border-color: var(--color-accent);
}

.models-tab__trigger-icon {
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
}

:deep(.models-tab__content) {
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

:deep(.models-tab__viewport) {
  padding: 4px;
}

:deep(.models-tab__option) {
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

:deep(.models-tab__option[data-highlighted]) {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

:deep(.models-tab__option[data-state="checked"]) {
  color: var(--color-accent);
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
