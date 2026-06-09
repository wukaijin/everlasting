<script setup lang="ts">
// DeleteModelConfirm — overlay confirmation for deleting a model.
// 8-PR3: extracted from ModelsTab.vue. Renders a backdrop + a
// small confirmation card with a Delete (danger) and Cancel
// (secondary) button pair. The parent owns the actual deletion
// IPC.

defineProps<{
    /** Open/closed state. */
    isOpen: boolean;
    /** Display name of the model being deleted (currently shown
     *  via the generic copy in the parent, but kept on the props
     *  for future per-model copy). */
    modelName: string;
}>();

const emit = defineEmits<{
    confirm: [];
    cancel: [];
}>();
</script>

<template>
    <div
        v-if="isOpen"
        class="delete-model-confirm"
        @click.self="emit('cancel')"
    >
        <div class="delete-model-confirm__card">
            <p class="delete-model-confirm__text">
                Delete this model? Sessions referencing this model will fall back to
                the default model.
            </p>
            <div class="delete-model-confirm__actions">
                <button
                    type="button"
                    class="delete-model-confirm__btn delete-model-confirm__btn--danger"
                    @click="emit('confirm')"
                >
                    Delete
                </button>
                <button
                    type="button"
                    class="delete-model-confirm__btn delete-model-confirm__btn--secondary"
                    @click="emit('cancel')"
                >
                    Cancel
                </button>
            </div>
        </div>
    </div>
</template>

<style scoped>
.delete-model-confirm {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 10;
    border-radius: 6px;
}

.delete-model-confirm__card {
    background: var(--color-bg-surface);
    border: 1px solid var(--color-bg-border);
    border-radius: 6px;
    padding: 16px;
    max-width: 360px;
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.delete-model-confirm__text {
    margin: 0;
    font-size: 13px;
    color: var(--color-text-primary);
    line-height: 1.5;
}

.delete-model-confirm__actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
}

.delete-model-confirm__btn {
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

.delete-model-confirm__btn--danger {
    color: var(--color-tool-error);
}

.delete-model-confirm__btn--danger:hover:not(:disabled) {
    background: rgba(239, 68, 68, 0.15);
}

.delete-model-confirm__btn--secondary {
    background: var(--color-bg-elevated);
    color: var(--color-text-primary);
}

.delete-model-confirm__btn--secondary:hover:not(:disabled) {
    background: var(--color-bg-border);
}
</style>