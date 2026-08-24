<script setup lang="ts">
// ConfirmDialog — generic confirmation modal. Extracted from
// DeleteWorktreeConfirm. Used for session deletion and any other
// destructive/confirmable action.

import { onUnmounted, ref, watch } from "vue";
import Icon from "../Icon.vue";

const props = withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    variant?: "danger" | "warning" | "default";
    confirmText?: string;
  }>(),
  {
    variant: "danger",
    confirmText: "确认",
  },
);

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

watch(
  () => props.open,
  (open) => {
    if (open) {
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
      >
        <header class="confirm-modal__header">
          <h2 class="confirm-modal__title">
            <Icon
              v-if="variant === 'danger'"
              name="warn"
              :size="14"
              icon-class="confirm-modal__icon"
            />
            {{ title }}
          </h2>
          <button
            type="button"
            class="confirm-modal__close btn btn--icon btn--ghost"
            aria-label="Close"
            @click="emit('cancel')"
          >
            <Icon name="x" :size="14" />
          </button>
        </header>
        <div class="confirm-modal__body">
          <slot />
        </div>
        <footer class="confirm-modal__actions">
          <button
            type="button"
            class="confirm-modal__btn confirm-modal__btn--cancel btn btn--muted"
            @click="emit('cancel')"
          >
            取消
          </button>
          <button
            ref="confirmButton"
            type="button"
            class="confirm-modal__btn btn"
            :class="[
              variant === 'danger'
                ? 'confirm-modal__btn--danger btn--danger'
                : 'confirm-modal__btn--warning btn--tint',
            ]"
            @click="emit('confirm')"
          >
            {{ confirmText }}
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
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: var(--z-confirm);
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
  box-shadow: var(--shadow-xl);
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
  color: var(--color-tool-error-text);
}

/* 按钮样式全部由全局 .btn 家族承载(close = ghost icon / cancel = muted /
 * danger·warning = danger·tint);此处仅保留非按钮元素样式。 */

.confirm-modal__body {
  padding: 16px;
  font-size: var(--text-base);
  line-height: 1.5;
  color: var(--color-text-primary);
}

.confirm-modal__actions {
  display: flex;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--color-bg-border);
  justify-content: flex-end;
}

/* backdrop（Transition 根元素）：opacity 始终 1，无视觉动画；
   transition-duration 仅用于让 Vue Transition 的 enter/leave 计时与下方
   content 动画同步，避免 active class 提前移除而中断 content 过渡
   (07-02-modal-motion-rhythm: mask 不做动画，只 content 做)。 */
.confirm-modal-enter-active {
  transition: opacity var(--duration-modal-in);
}

.confirm-modal-leave-active {
  transition: opacity var(--duration-modal-out);
}

.confirm-modal-enter-active .confirm-modal,
.confirm-modal-leave-active .confirm-modal {
  transition: opacity var(--duration-modal-in) var(--ease-modal-in), transform var(--duration-modal-in) var(--ease-modal-in);
}

.confirm-modal-enter-from .confirm-modal {
  opacity: 0;
  transform: scale(0.1);
}

.confirm-modal-leave-to .confirm-modal {
  opacity: 0;
  transform: scale(0.1);
}

.confirm-modal-leave-active .confirm-modal {
  transition-duration: var(--duration-modal-out);
  transition-timing-function: var(--ease-accelerate);
}
</style>
