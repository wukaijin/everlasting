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
                    class="delete-model-confirm__btn delete-model-confirm__btn--danger btn btn--danger-soft"
                    @click="emit('confirm')"
                >
                    Delete
                </button>
                <button
                    type="button"
                    class="delete-model-confirm__btn delete-model-confirm__btn--secondary btn btn--muted"
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
    /* 局部层:tab 内确认遮罩,盖本 tab 内容;非全局 modal(那是 --z-modal) */
    z-index: 10;
    border-radius: var(--radius-md);
}

.delete-model-confirm__card {
    background: var(--color-bg-surface);
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-md);
    padding: 16px;
    max-width: 360px;
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.delete-model-confirm__text {
    margin: 0;
    font-size: var(--text-base);
    color: var(--color-text-primary);
    line-height: 1.5;
}

.delete-model-confirm__actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
}

/* 按钮样式由全局 .btn 家族承载(danger = danger-soft / secondary =
   muted);此处仅保留家族不拥有的字重。 */
.delete-model-confirm__btn {
    font-weight: var(--weight-medium);
}
</style>