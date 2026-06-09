<script setup lang="ts">
// ModelForm — Add / Edit form for an LLM model.
//
// Fields:
//   - provider (SelectRoot via reka-ui)
//   - modelName (native <input>, wrapped in reka-ui Label)
//   - displayName (native <input>, wrapped in reka-ui Label)
//   - maxTokens (optional, native <input>)
//   - thinkingEffort (optional, SelectRoot via reka-ui)
//   - supportsThinking (CheckboxRoot via reka-ui)
//   - contextWindow (native <input type="number">)
//
// R1 polish: form controls use reka-ui `SelectRoot` for the
// dropdowns, reka-ui `CheckboxRoot` for supportsThinking, and
// themed native inputs (wrapped in reka-ui `Label`) for the
// text fields. Reka-ui 2.x doesn't ship a generic `TextFieldRoot`,
// so for text inputs we keep `<input>` and theme it via the
// shared `.model-form__input` class to match the rest of the
// form. The contextWindow number input keeps `type="number"`.
//
// IMPORTANT — reka-ui SelectContent portal `:deep()` gotcha:
// any rule styling `.model-form__content`, `.model-form__viewport`,
// or `.model-form__option` MUST be wrapped in `:deep(...)` in
// `<style scoped>`; those elements are rendered to <body> via
// <SelectPortal> and don't get the component's `data-v-xxx`
// attribute, so a plain scoped selector silently drops the rule.
// See `.trellis/spec/frontend/reka-ui-usage.md` for the full
// explanation.

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
import type { ProviderRow } from "../../stores/providers";
import Icon from "../Icon.vue";

export interface ModelFormState {
  providerId: string;
  modelName: string;
  displayName: string;
  /** Empty string = omit (None). */
  maxTokens: string;
  /** "none" sentinel = omit (None). */
  thinkingEffort: string;
  supportsThinking: boolean;
  contextWindow: number;
}

defineProps<{
    /** Drives the form title ("Add Model" vs "Edit Model"). */
    mode: "add" | "edit";
    /** Reactive form state. The parent owns the actual `reactive`
     *  object and watches changes. */
    form: ModelFormState;
    /** Provider list for the dropdown. */
    providers: ProviderRow[];
    /** True while the parent's save() promise is in-flight. */
    saving: boolean;
    /** Whether the Save button should be enabled. */
    canSave: boolean;
}>();

const emit = defineEmits<{
    /** User clicked Save — parent performs the IPC and closes
     *  the form on success. */
    submit: [];
    /** User clicked Cancel — parent resets the form and goes
     *  back to idle. */
    cancel: [];
}>();
</script>

<template>
    <div class="model-form">
        <h4 class="model-form__title">
            {{ mode === "add" ? "Add Model" : "Edit Model" }}
        </h4>

        <Label class="model-form__field">
            <span class="model-form__label">Provider</span>
            <SelectRoot v-model="form.providerId">
                <SelectTrigger class="model-form__trigger" aria-label="Provider">
                    <SelectValue placeholder="Select provider" />
                    <SelectIcon class="model-form__trigger-icon">
                        <Icon name="chevron-down" :size="12" />
                    </SelectIcon>
                </SelectTrigger>
                <SelectPortal>
                    <SelectContent
                        class="model-form__content"
                        position="popper"
                        :side-offset="4"
                    >
                        <SelectViewport class="model-form__viewport">
                            <SelectItem
                                v-for="p in providers"
                                :key="p.id"
                                :value="p.id"
                                class="model-form__option"
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

        <div class="model-form__row-pair">
            <Label class="model-form__field">
                <span class="model-form__label">Model Name</span>
                <input
                    v-model="form.modelName"
                    type="text"
                    class="model-form__input"
                    placeholder="claude-sonnet-4-5"
                />
            </Label>
            <Label class="model-form__field">
                <span class="model-form__label">Display Name</span>
                <input
                    v-model="form.displayName"
                    type="text"
                    class="model-form__input"
                    placeholder="Claude Sonnet 4.5"
                />
            </Label>
        </div>

        <div class="model-form__row-pair">
            <Label class="model-form__field">
                <span class="model-form__label">Max Tokens (optional)</span>
                <input
                    v-model="form.maxTokens"
                    type="text"
                    class="model-form__input"
                    placeholder="16384"
                />
            </Label>
            <Label class="model-form__field">
                <span class="model-form__label">Context Window</span>
                <input
                    v-model.number="form.contextWindow"
                    type="number"
                    class="model-form__input"
                    min="1"
                />
            </Label>
        </div>

        <div class="model-form__row-pair">
            <Label class="model-form__field">
                <span class="model-form__label">Thinking Effort (optional)</span>
                <SelectRoot v-model="form.thinkingEffort">
                    <SelectTrigger
                        class="model-form__trigger"
                        aria-label="Thinking effort"
                    >
                        <SelectValue placeholder="(default: high)" />
                        <SelectIcon class="model-form__trigger-icon">
                            <Icon name="chevron-down" :size="12" />
                        </SelectIcon>
                    </SelectTrigger>
                    <SelectPortal>
                        <SelectContent
                            class="model-form__content"
                            position="popper"
                            :side-offset="4"
                        >
                            <SelectViewport class="model-form__viewport">
                                <SelectItem value="none" class="model-form__option">
                                    <SelectItemText>(default: high)</SelectItemText>
                                </SelectItem>
                                <SelectItem value="low" class="model-form__option">
                                    <SelectItemText>low</SelectItemText>
                                </SelectItem>
                                <SelectItem value="medium" class="model-form__option">
                                    <SelectItemText>medium</SelectItemText>
                                </SelectItem>
                                <SelectItem value="high" class="model-form__option">
                                    <SelectItemText>high</SelectItemText>
                                </SelectItem>
                                <SelectItem value="xhigh" class="model-form__option">
                                    <SelectItemText>xhigh</SelectItemText>
                                </SelectItem>
                                <SelectItem value="max" class="model-form__option">
                                    <SelectItemText>max</SelectItemText>
                                </SelectItem>
                            </SelectViewport>
                        </SelectContent>
                    </SelectPortal>
                </SelectRoot>
            </Label>
            <div class="model-form__field model-form__field--check">
                <span class="model-form__label">Supports Thinking</span>
                <CheckboxRoot
                    v-model="form.supportsThinking"
                    class="model-form__checkbox"
                >
                    <CheckboxIndicator class="model-form__checkbox-indicator">
                        <Icon name="check" :size="11" />
                    </CheckboxIndicator>
                </CheckboxRoot>
            </div>
        </div>

        <div class="model-form__actions">
            <button
                type="button"
                class="model-form__btn model-form__btn--primary"
                :disabled="!canSave"
                @click="emit('submit')"
            >
                {{ saving ? "Saving..." : "Save" }}
            </button>
            <button
                type="button"
                class="model-form__btn model-form__btn--secondary"
                @click="emit('cancel')"
            >
                Cancel
            </button>
        </div>
    </div>
</template>

<style scoped>
.model-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    background: var(--color-bg-elevated);
    border: 1px solid var(--color-bg-border);
    border-radius: 6px;
}

.model-form__title {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--color-text-primary);
}

.model-form__row-pair {
    display: flex;
    gap: 12px;
}

.model-form__row-pair > .model-form__field {
    flex: 1;
}

.model-form__field {
    display: flex;
    flex-direction: column;
    gap: 4px;
}

.model-form__field--check {
    flex-direction: row;
    align-items: center;
    gap: 8px;
    justify-content: flex-end;
    padding-top: 18px;
}

.model-form__label {
    font-size: 11px;
    font-weight: 500;
    color: var(--color-text-secondary);
}

.model-form__input {
    padding: 6px 10px;
    background: var(--color-bg-app);
    border: 1px solid var(--color-bg-border);
    border-radius: 4px;
    color: var(--color-text-primary);
    font-size: 13px;
    width: 100%;
    box-sizing: border-box;
}

.model-form__input:focus {
    outline: none;
    border-color: var(--color-accent);
}

.model-form__checkbox {
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

.model-form__checkbox:hover {
    border-color: var(--color-accent-muted);
}

.model-form__checkbox[data-state="checked"] {
    background: var(--color-accent);
    border-color: var(--color-accent);
}

.model-form__checkbox-indicator {
    color: #fff;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    line-height: 0;
}

/* --- R1: reka-ui Select trigger / content / option theming --- */

.model-form__trigger {
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

.model-form__trigger:hover {
    border-color: var(--color-accent-muted);
}

.model-form__trigger[data-state="open"] {
    border-color: var(--color-accent);
}

.model-form__trigger-icon {
    color: var(--color-text-muted);
    display: inline-flex;
    align-items: center;
}

/* Portal children — MUST be wrapped in :deep() because
 * SelectContent / SelectViewport / SelectItem are rendered
 * to <body> via <SelectPortal> and don't receive the
 * component's data-v-xxx attribute. */
:deep(.model-form__content) {
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

:deep(.model-form__viewport) {
    padding: 4px;
}

:deep(.model-form__option) {
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

:deep(.model-form__option[data-highlighted]) {
    background: var(--color-bg-elevated);
    color: var(--color-text-primary);
}

:deep(.model-form__option[data-state="checked"]) {
    color: var(--color-accent);
}

.model-form__actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
}

.model-form__btn {
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

.model-form__btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
}

.model-form__btn--primary {
    background: var(--color-accent);
    color: #fff;
    border-color: var(--color-accent);
}

.model-form__btn--primary:hover:not(:disabled) {
    background: var(--color-accent-hover);
}

.model-form__btn--secondary {
    background: var(--color-bg-elevated);
    color: var(--color-text-primary);
}

.model-form__btn--secondary:hover:not(:disabled) {
    background: var(--color-bg-border);
}
</style>