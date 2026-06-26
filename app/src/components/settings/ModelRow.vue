<script setup lang="ts">
// ModelRow — single row in the Models list. Displays one model's
// name, model_id, tags (thinking / context window), inline test
// result, and three action buttons (test / edit / delete).
//
// PR5 follow-up: the per-row Test result is rendered inline in
// `.models-tab__row-test` (now `.model-row__test` after 8-PR3)
// and persists until either (a) the user clicks Test again, or
// (b) the row is deleted. The test state lives in the parent
// (ModelsTab) so it survives switching tabs — switching
// providers or editing the model fields intentionally does NOT
// clear the result.

import { computed } from "vue";
import type { ModelWithProvider } from "../../stores/models";
import Icon from "../Icon.vue";

export type TestState =
  | { kind: "running" }
  | { kind: "ok"; latencyMs: number }
  | { kind: "fail"; error: string };

const props = defineProps<{
    model: ModelWithProvider;
    /** Per-row test result. Undefined means "never tested". */
    test: TestState | undefined;
    /** Streaming flag for the chat session (currently unused by
     *  this row but kept on the props shape for future
     *  "disable-actions-during-stream" parity with WorktreeChip). */
    isStreaming: boolean;
}>();

const emit = defineEmits<{
    /** User clicked the Test button — invoke the `test_model` IPC
     *  in the parent. */
    test: [];
    /** User clicked the Edit (pencil) button — switch the parent
     *  into `edit` mode and seed the form. */
    edit: [];
    /** User clicked the Delete (trash) button — open the parent's
     *  delete-confirm overlay. */
    delete: [];
}>();

/** PR5: per-row Test result rendering helpers. Extracted from
 *  the template so the runtime narrowing happens in TypeScript
 *  (the template language doesn't allow `as` casts). */
const testClass = computed<Record<string, boolean>>(() => {
    const t = props.test;
    if (!t) return {} as Record<string, boolean>;
    return {
        "model-row__test--ok": t.kind === "ok",
        "model-row__test--fail": t.kind === "fail",
        "model-row__test--running": t.kind === "running",
    } as Record<string, boolean>;
});

const okLatency = computed<number>(() =>
    props.test?.kind === "ok" ? props.test.latencyMs : 0,
);

const failError = computed<string>(() =>
    props.test?.kind === "fail" ? props.test.error : "",
);

const isRunning = computed<boolean>(() => props.test?.kind === "running");

const testTitle = computed<string>(() =>
    isRunning.value ? "测试中…" : "测试此 model 连通性",
);
</script>

<template>
    <div class="model-row">
        <div class="model-row__info">
            <span class="model-row__name">{{ model.displayName }}</span>
            <span class="model-row__model-id">{{ model.modelName }}</span>
            <span v-if="model.supportsThinking" class="model-row__tag">
                thinking
            </span>
            <span class="model-row__tag model-row__tag--muted">
                {{
                    model.contextWindow >= 1000
                        ? `${model.contextWindow / 1000}k`
                        : model.contextWindow
                }}
            </span>
            <!-- PR5: per-row Test result, inline. The label appears
                 under the model_id so the row's vertical rhythm is
                 unchanged on the success / never-tested path. -->
            <span
                v-if="test"
                class="model-row__test"
                :class="testClass"
            >
                <template v-if="test.kind === 'running'">
                    测试中…
                </template>
                <template v-else-if="test.kind === 'ok'">
                    <Icon name="check" :size="12" />
                    通过 ({{ okLatency }}ms)
                </template>
                <template v-else>
                    <Icon name="warn" :size="12" />
                    {{ failError }}
                </template>
            </span>
        </div>
        <div class="model-row__actions">
            <button
                type="button"
                class="model-row__btn model-row__btn--ghost"
                :disabled="isRunning"
                :title="testTitle"
                @click="emit('test')"
            >
                <Icon name="signal" :size="12" />
            </button>
            <button
                type="button"
                class="model-row__btn model-row__btn--ghost"
                @click="emit('edit')"
            >
                <Icon name="pencil" :size="12" />
            </button>
            <button
                type="button"
                class="model-row__btn model-row__btn--ghost model-row__btn--danger"
                @click="emit('delete')"
            >
                <Icon name="trash" :size="12" />
            </button>
        </div>
    </div>
</template>

<style scoped>
.model-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 12px;
    background: var(--color-bg-elevated);
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-md);
}

.model-row__info {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
}

.model-row__name {
    font-size: var(--text-base);
    font-weight: var(--weight-medium);
    color: var(--color-text-primary);
}

.model-row__model-id {
    font-size: var(--text-xs);
    color: var(--color-text-muted);
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.model-row__tag {
    font-size: var(--text-2xs);
    padding: 1px 6px;
    border-radius: 3px;
    background: var(--color-accent-muted);
    color: var(--color-accent);
    font-family: var(--font-mono);
    flex-shrink: 0;
}

.model-row__tag--muted {
    background: var(--color-bg-border);
    color: var(--color-text-muted);
}

/* PR5: per-row Test result badge. Inline with the model_id so
   the row's vertical rhythm matches the pre-PR5 layout. The
   running state uses the muted text color (it'll resolve to ok
   or fail shortly); the success / fail states use the same
   tool-color tokens as the rest of the settings tabs. */
.model-row__test {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--text-2xs);
    font-family: var(--font-mono);
    color: var(--color-text-muted);
    flex-shrink: 0;
}

.model-row__test--ok {
    color: var(--color-tool-write);
}

.model-row__test--fail {
    color: var(--color-tool-error);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 240px;
}

.model-row__test--running {
    color: var(--color-text-muted);
}

.model-row__actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
}

.model-row__btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px;
    border: 0;
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    cursor: pointer;
    background: transparent;
    color: var(--color-text-muted);
    transition:
        background var(--duration-base) var(--ease-out),
        color var(--duration-base) var(--ease-out);
}

.model-row__btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
}

.model-row__btn--ghost:hover:not(:disabled) {
    color: var(--color-text-primary);
    background: var(--color-bg-border);
}

.model-row__btn--danger {
    color: var(--color-tool-error);
}

.model-row__btn--danger:hover:not(:disabled) {
    background: rgba(239, 68, 68, 0.15);
}
</style>