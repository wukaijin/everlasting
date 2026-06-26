<script setup lang="ts">
// DeleteWorktreeConfirm — destructive confirmation modal for
// `delete_worktree`. Shown only when (a) the session is `active`
// AND (b) the worktree has changes (the diff cache has at least
// one file). When the worktree is clean, or the session is
// `detached`, the parent invokes `deleteWorktree` directly with
// no prompt — the modal is purely a safety net for the "I have
// uncommitted work I might forget about" case.
//
// Two-button layout (cancel / confirm) with the cancel as the
// default focus for keyboard users.

import { onUnmounted, ref, watch } from "vue";
import Icon from "../Icon.vue";

const props = defineProps<{
  open: boolean;
  fileCount: number;
}>();

const emit = defineEmits<{
  cancel: [];
  confirm: [];
}>();

const confirmButton = ref<HTMLButtonElement | null>(null);

function onKeyDown(e: KeyboardEvent) {
  if (!props.open) return;
  if (e.key === "Escape") {
    e.preventDefault();
    emit("cancel");
  } else if (e.key === "Enter") {
    e.preventDefault();
    emit("confirm");
  }
}

if (typeof window !== "undefined") {
  window.addEventListener("keydown", onKeyDown);
  onUnmounted(() => window.removeEventListener("keydown", onKeyDown));
}

// Focus the confirm button on open so Enter doesn't have to be
// pressed twice (once to focus, once to confirm).
watch(
  () => props.open,
  (open) => {
    if (open) {
      // Defer to next tick so the button exists in the DOM.
      setTimeout(() => confirmButton.value?.focus(), 0);
    }
  },
);
</script>

<template>
  <Transition name="confirm-modal">
    <div
      v-if="open"
      class="confirm-backdrop"
      @click.self="emit('cancel')"
    >
      <div
        class="confirm-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Delete worktree confirmation"
      >
        <header class="confirm-modal__header">
          <h2 class="confirm-modal__title">
            <Icon name="warn" :size="14" icon-class="confirm-modal__icon" />
            确认删除 worktree?
          </h2>
          <button
            type="button"
            class="confirm-modal__close"
            aria-label="Close"
            @click="emit('cancel')"
          >
            <Icon name="x" :size="14" />
          </button>
        </header>
        <div class="confirm-modal__body">
          <p>
            {{ fileCount }} 个文件
            会被销毁，worktree 目录和 <code>session/&lt;id&gt;</code>
            分支也会被永久删除。
          </p>
          <p class="confirm-modal__hint">
            无法撤销。如需保留工作内容，请先 commit 或 detach。
          </p>
        </div>
        <footer class="confirm-modal__actions">
          <button
            type="button"
            class="confirm-modal__btn confirm-modal__btn--cancel"
            @click="emit('cancel')"
          >
            取消
          </button>
          <button
            ref="confirmButton"
            type="button"
            class="confirm-modal__btn confirm-modal__btn--danger"
            @click="emit('confirm')"
          >
            确认删除
          </button>
        </footer>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.confirm-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1100;
  padding: 24px;
}

.confirm-modal {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  width: 100%;
  max-width: 460px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
}

.confirm-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
}

.confirm-modal__title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.confirm-modal__icon {
  color: var(--color-tool-error);
}

.confirm-modal__close {
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

.confirm-modal__close:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.confirm-modal__body {
  padding: 16px;
  font-size: var(--text-base);
  line-height: 1.5;
  color: var(--color-text-primary);
}

.confirm-modal__body code {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  background: var(--color-bg-elevated);
  padding: 1px 4px;
  border-radius: 3px;
}

.confirm-modal__hint {
  color: var(--color-text-muted);
  font-size: var(--text-sm);
  margin-top: 8px;
}

.confirm-modal__actions {
  display: flex;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--color-bg-border);
  justify-content: flex-end;
}

.confirm-modal__btn {
  font: inherit;
  font-size: var(--text-sm);
  padding: 6px 14px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  border: 1px solid var(--color-bg-border);
}

.confirm-modal__btn--cancel {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.confirm-modal__btn--cancel:hover {
  border-color: var(--color-accent-muted);
}

.confirm-modal__btn--danger {
  background: var(--color-tool-error);
  color: #ffffff;
  border-color: var(--color-tool-error);
}

.confirm-modal__btn--danger:hover {
  filter: brightness(1.1);
}

/* R4 modal animation: fade + scale 0.96→1 from center. 150ms
   ease-out on enter, var(--duration-fast) ease-in on leave. Matches the
   SettingsModal treatment for visual consistency. */
.confirm-modal-enter-active,
.confirm-modal-leave-active {
  transition: opacity var(--duration-base) var(--ease-out);
}

.confirm-modal-enter-active .confirm-modal,
.confirm-modal-leave-active .confirm-modal {
  transition: opacity var(--duration-base) var(--ease-out), transform var(--duration-base) var(--ease-out);
}

.confirm-modal-enter-from,
.confirm-modal-leave-to {
  opacity: 0;
}

.confirm-modal-enter-from .confirm-modal,
.confirm-modal-leave-to .confirm-modal {
  opacity: 0;
  transform: scale(0.96);
}

.confirm-modal-leave-active,
.confirm-modal-leave-active .confirm-modal {
  transition-duration: 100ms;
  transition-timing-function: ease-in;
}
</style>
