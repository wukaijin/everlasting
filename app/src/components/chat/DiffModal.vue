<script setup lang="ts">
// DiffModal — session-level diff overlay.
//
// Step 4 / PR3: triggered by the "diff" chip in the chat panel
// header. Closes on backdrop click, on the close button, or on
// Esc (Esc handling lives in the parent — see ChatPanel's
// onKeyDown — but the backdrop click + close button both fire
// the `close` emit).
//
// Renders the existing `DiffView` component with the session's
// cached diff result. Loading and error states are rendered
// inline in the body; the parent owns the data fetch.

import DiffView from "./DiffView.vue";
import Icon from "../Icon.vue";

defineProps<{
    /** Open/closed state. Driven by parent. */
    isOpen: boolean;
    /** True while the parent is fetching the diff. Renders a
     *  loading placeholder in the body. */
    isLoading: boolean;
    /** Error message from the last fetch. Renders a styled
     *  placeholder in the body when non-null. */
    error: string | null;
    /** Cached diff result. When null and not loading, the body
     *  is empty (parent hasn't fetched yet). */
    result: { files: import("./DiffView.vue").FileDiff[] } | null;
}>();

const emit = defineEmits<{
    close: [];
}>();
</script>

<template>
    <Transition name="diff-modal">
        <div
            v-if="isOpen"
            class="diff-modal-backdrop"
            @click.self="emit('close')"
        >
            <div
                class="diff-modal"
                role="dialog"
                aria-modal="true"
                aria-label="Session diff"
            >
                <header class="diff-modal__header">
                    <h2 class="diff-modal__title">
                        Session diff
                        <span v-if="result" class="diff-modal__count">
                            ({{ result.files.length }}
                            {{ result.files.length === 1 ? "file" : "files" }})
                        </span>
                    </h2>
                    <button
                        type="button"
                        class="diff-modal__close"
                        @click="emit('close')"
                        aria-label="Close"
                    >
                        <Icon name="x" :size="14" />
                    </button>
                </header>
                <div class="diff-modal__body">
                    <div v-if="isLoading" class="diff-modal__loading">
                        Loading diff…
                    </div>
                    <div v-else-if="error" class="diff-modal__error">
                        {{ error }}
                    </div>
                    <DiffView v-else-if="result" :files="result.files" />
                </div>
            </div>
        </div>
    </Transition>
</template>

<style scoped>
/* -----------------------------------------------------------------------
 * Diff modal. Full-viewport overlay; the inner .diff-modal is
 * centered and sized to leave 40px margin on each side. Scrolling
 * happens inside .diff-modal__body so the header + close button
 * stay pinned.
 * -------------------------------------------------------------------- */
.diff-modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    padding: 40px;
}

.diff-modal {
    background: var(--color-bg-surface);
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-lg);
    width: 100%;
    max-width: 1100px;
    max-height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: var(--shadow-xl);
}

.diff-modal__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 16px;
    border-bottom: 1px solid var(--color-bg-border);
    background: var(--color-bg-elevated);
    flex-shrink: 0;
}

.diff-modal__title {
    margin: 0;
    font-size: var(--text-base);
    font-weight: var(--weight-semibold);
    color: var(--color-text-primary);
    display: inline-flex;
    align-items: baseline;
    gap: 8px;
}

.diff-modal__count {
    font-size: var(--text-xs);
    color: var(--color-text-muted);
    font-weight: 400;
}

.diff-modal__close {
    background: transparent;
    border: 0;
    color: var(--color-text-muted);
    cursor: pointer;
    padding: 4px;
    border-radius: var(--radius-sm);
    display: inline-flex;
    align-items: center;
    justify-content: center;
}

.diff-modal__close:hover {
    background: var(--color-bg-border);
    color: var(--color-text-primary);
}

.diff-modal__body {
    flex: 1;
    overflow-y: auto;
    padding: 12px 16px;
    background: var(--color-bg-app);
}

.diff-modal__loading,
.diff-modal__error {
    padding: 24px;
    text-align: center;
    color: var(--color-text-muted);
    font-size: var(--text-base);
}

.diff-modal__error {
    color: var(--color-tool-error);
}

/* R4 popup animation: 仅 content 做 scale 0.96→1 + opacity 过渡；
 * backdrop（Transition 根元素）opacity 始终 1、无视觉动画，transition-
 * duration 仅用于让 Vue Transition 的 enter/leave 计时与 content 同步，
 * 避免 active class 提前移除而中断 content 过渡 (07-02-modal-motion-rhythm)。 */
.diff-modal-enter-active {
    transition: opacity var(--duration-modal-in);
}

.diff-modal-leave-active {
    transition: opacity var(--duration-modal-out);
}

.diff-modal-enter-active .diff-modal,
.diff-modal-leave-active .diff-modal {
    transition: opacity var(--duration-modal-in) var(--ease-modal-in), transform var(--duration-modal-in) var(--ease-modal-in);
}

.diff-modal-enter-from .diff-modal {
    opacity: 0;
    transform: scale(0.1);
}

.diff-modal-leave-to .diff-modal {
    opacity: 0;
    transform: scale(0.1);
}

.diff-modal-leave-active .diff-modal {
    transition-duration: var(--duration-modal-out);
    transition-timing-function: var(--ease-accelerate);
}
</style>