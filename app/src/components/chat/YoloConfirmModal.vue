<script setup lang="ts">
// YoloConfirmModal — two-key confirm modal for entering Yolo mode.
//
// PR2 (B7 front-end): the ModeSelect asks "switch to Yolo?"
// before calling `set_session_mode("yolo")` over IPC. This modal
// is the only place where Yolo is offered — it never auto-
// enables and it never appears outside an explicit user click.
//
// Two-key shape (mirrors the spec in
// `.trellis/tasks/06-12-a2-b7-permission-and-mode/research/yolo-safety-design.md`
// §7 "two-click with destructive framing"). Visual + a11y copy
// follows `ConfirmDialog` precedent (`Esc` → cancel, `Enter` →
// confirm, focus moves to confirm button on open) so users with
// muscle memory from session/worktree delete confirms get the
// same affordances.
//
// This is a hand-rolled modal — not `ConfirmDialog` — because:
// 1. The Yolo warning text is longer and risk-flavoured
//    (mentions hard kill list + audit log), so the default
//    `ConfirmDialog` body slot isn't enough.
// 2. We want a distinct "danger" visual (red left border, red
//    confirm button) to signal "this is irreversible /
//    destructive" more strongly than the default `ConfirmDialog`
//    danger variant — Yolo is a session-scope blast radius, not
//    a one-shot delete.
// 3. We need `:disabled` while `streaming` is true to prevent
//    mid-stream entry (matches `ModeSelect`'s `:disabled` and
//    `useKeyboard`'s `enabled()` gate).

import { onUnmounted, ref, watch } from "vue";
import Icon from "../Icon.vue";

const props = defineProps<{
  /** True to render the modal. `v-if`-mounted in the parent
   *  for the leave-animation to fire on close. */
  open: boolean;
  /** Disables both buttons + Esc/Enter handlers while true
   *  (mid-stream is the canonical case — matches the rest
   *  of the B7 UI). */
  disabled?: boolean;
}>();

const emit = defineEmits<{
  cancel: [];
  confirm: [];
}>();

const confirmButton = ref<HTMLButtonElement | null>(null);

/** Esc cancels, Enter confirms. Matches `ConfirmDialog` muscle
 *  memory. Disabled-while-open state mirrors `props.disabled`. */
function onKeyDown(e: KeyboardEvent) {
  if (!props.open || props.disabled) return;
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

/** Focus the confirm button on open so Enter works without a
 *  prior Tab. Mirrors `ConfirmDialog`/`DeleteWorktreeConfirm`.
 *  We deliberately focus "confirm" (not "cancel") because the
 *  entire point of the modal is the second click — defaulting
 *  to the safer "cancel" would force the user to Tab over
 *  before confirming, defeating the purpose. */
watch(
  () => props.open,
  (open) => {
    if (open && !props.disabled) {
      setTimeout(() => confirmButton.value?.focus(), 0);
    }
  },
);
</script>

<template>
  <Transition name="yolo-confirm">
    <div
      v-if="open"
      class="yolo-confirm-backdrop"
      @click.self="!disabled && emit('cancel')"
    >
      <div
        class="yolo-confirm-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="yolo-confirm-title"
      >
        <header class="yolo-confirm-modal__header">
          <h2
            id="yolo-confirm-title"
            class="yolo-confirm-modal__title"
          >
            <Icon
              name="warn"
              :size="14"
              icon-class="yolo-confirm-modal__icon"
            />
            确认进入 Yolo 模式?
          </h2>
          <button
            type="button"
            class="yolo-confirm-modal__close"
            aria-label="Close"
            :disabled="disabled"
            @click="emit('cancel')"
          >
            <Icon name="x" :size="14" />
          </button>
        </header>
        <div class="yolo-confirm-modal__body">
          <p class="yolo-confirm-modal__lead">
            Yolo 模式将允许所有工具调用不再询问，
            硬 kill list 仍然拦截。继续?
          </p>
          <ul class="yolo-confirm-modal__bullets">
            <li>
              <strong>跳过所有用户确认</strong> — LLM 的每一次
              <code>tool_use</code> 都会直接执行，不再弹窗询问。
            </li>
            <li>
              <strong>硬 kill list 仍然拦截</strong> —
              <code>rm -rf /</code>、<code>mkfs</code>、
              <code>git push --force</code> 主分支等破坏性命令
              会被静默拒绝并写入审计日志。
            </li>
            <li>
              <strong>所有 Yolo 操作记入审计</strong> — 可在
              审计日志中追溯本次 session 的每一次工具调用。
            </li>
          </ul>
          <p class="yolo-confirm-modal__hint">
            Yolo 是会话级别的安全姿态，关闭或重启 session 后
            自动失效。
          </p>
        </div>
        <footer class="yolo-confirm-modal__actions">
          <button
            type="button"
            class="yolo-confirm-modal__btn yolo-confirm-modal__btn--cancel"
            :disabled="disabled"
            @click="emit('cancel')"
          >
            取消
          </button>
          <button
            ref="confirmButton"
            type="button"
            class="yolo-confirm-modal__btn yolo-confirm-modal__btn--confirm"
            :disabled="disabled"
            @click="emit('confirm')"
          >
            我已知风险，启用 Yolo
          </button>
        </footer>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.yolo-confirm-backdrop {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1200;
  padding: 24px;
}

/* Danger variant of the centered modal pattern. The 3px red
   left border is the only place in the codebase that uses a
   border thicker than 1px — see audit §4.1 (PR1 review): this
   is an intentional exception to the design-token rule because
   Yolo is the highest-stakes confirmation in the app. The 3px
   width matches `permission-modal__content--critical` in the
   PR3 spec so the two "extreme risk" modals look like cousins. */
.yolo-confirm-modal {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-tool-error);
  border-radius: var(--radius-lg);
  width: 100%;
  max-width: 480px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--shadow-xl);
}

.yolo-confirm-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
}

.yolo-confirm-modal__title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.yolo-confirm-modal__icon {
  color: var(--color-tool-error);
}

.yolo-confirm-modal__close {
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

.yolo-confirm-modal__close:hover:not(:disabled) {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.yolo-confirm-modal__close:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.yolo-confirm-modal__body {
  padding: 16px;
  font-size: var(--text-base);
  line-height: 1.6;
  color: var(--color-text-primary);
}

.yolo-confirm-modal__lead {
  margin: 0 0 12px;
}

.yolo-confirm-modal__bullets {
  margin: 0 0 12px;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.yolo-confirm-modal__bullets li {
  padding-left: 14px;
  position: relative;
  color: var(--color-text-secondary);
  font-size: var(--text-sm);
}

.yolo-confirm-modal__bullets li::before {
  content: "•";
  position: absolute;
  left: 0;
  color: var(--color-text-muted);
}

.yolo-confirm-modal__bullets strong {
  color: var(--color-text-primary);
  font-weight: var(--weight-semibold);
}

.yolo-confirm-modal__bullets code {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  background: var(--color-bg-elevated);
  padding: 1px 4px;
  border-radius: 3px;
  color: var(--color-text-primary);
}

.yolo-confirm-modal__hint {
  margin: 0;
  color: var(--color-text-muted);
  font-size: var(--text-sm);
}

.yolo-confirm-modal__actions {
  display: flex;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--color-bg-border);
  justify-content: flex-end;
}

.yolo-confirm-modal__btn {
  font: inherit;
  font-size: var(--text-sm);
  padding: 6px 14px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  border: 1px solid var(--color-bg-border);
}

.yolo-confirm-modal__btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.yolo-confirm-modal__btn--cancel {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.yolo-confirm-modal__btn--cancel:hover:not(:disabled) {
  border-color: var(--color-accent-muted);
}

.yolo-confirm-modal__btn--confirm {
  background: var(--color-tool-error);
  color: var(--color-text-on-accent);
  border-color: var(--color-tool-error);
}

.yolo-confirm-modal__btn--confirm:hover:not(:disabled) {
  filter: brightness(1.1);
}

/* 150ms fade + scale 0.96→1 enter, 100ms fade-in leave —
   same convention as `DeleteWorktreeConfirm` /
   `ConfirmDialog` so users get a familiar modal feel. */
.yolo-confirm-enter-active,
.yolo-confirm-leave-active {
  transition: opacity var(--duration-base) var(--ease-out);
}

.yolo-confirm-enter-active .yolo-confirm-modal,
.yolo-confirm-leave-active .yolo-confirm-modal {
  transition: opacity var(--duration-base) var(--ease-out), transform var(--duration-base) var(--ease-out);
}

.yolo-confirm-enter-from,
.yolo-confirm-leave-to {
  opacity: 0;
}

.yolo-confirm-enter-from .yolo-confirm-modal,
.yolo-confirm-leave-to .yolo-confirm-modal {
  opacity: 0;
  transform: scale(0.96);
}

.yolo-confirm-leave-active,
.yolo-confirm-leave-active .yolo-confirm-modal {
  transition-duration: 100ms;
  transition-timing-function: ease-in;
}
</style>