<script setup lang="ts">
// MessageItemEdit — the inline edit-mode UI for a user message
// (D3 PR2, 2026-06-17). Renders a <textarea> + Save / Cancel
// buttons in place of the message bubble. The component is a
// pure presentation layer: it does NOT touch the Pinia stores
// directly — every save / cancel / resend action is emitted
// to the parent (`MessageItem.vue`) which owns the store
// orchestration (cancel any in-flight stream → fire IPC →
// refresh the in-memory buffer). All transient state (saving
// flag, error message) is driven by props the parent passes in.
//
// Why pure presentation (no store import):
//   - Single source of truth: only `MessageItem.vue` knows the
//     chat store, so refactors to `chatStore.editMessage` /
//     `resendMessage` touch exactly one file.
//   - Testable in isolation: vitest can drive this component
//     with hand-built props and assert on the emitted events
//     without spinning up Pinia. See
//     `app/src/components/chat/MessageItemEdit.test.ts`.
//   - Mirrors the `<MessageActionsMenu>` convention (parent
//     orchestrates, child renders).
//
// Buffer seeding (the `editBuffer` ref):
//   We deliberately do NOT bind `v-model` to the parent's
//   `content` prop because (a) the prop is read-only from the
//   child's perspective (we don't want to mutate the parent's
//   reactive ChatMessage from a textarea handler), and (b)
//   the controller's `done` / `error` / `delta` handlers also
//   mutate the in-memory message's `content` — a live v-model
//   would race the streaming text append. The local ref + a
//   `watch` on `[isEditingThisMessage, content]` re-seeds the
//   buffer on edit-mode entry AND on content churn so a
//   streaming turn that ends mid-edit lands the final content
//   in the textarea (not the stale pre-stream text).

import { ref, watch } from "vue";
import Icon from "../Icon.vue";

const props = withDefaults(
  defineProps<{
    /** The row's seq (used for the `aria-label` only — the
     *  parent already knows the seq when it routes the
     *  `save` emit). The textarea is identified by seq so
     *  screen readers can disambiguate when multiple
     *  edit-mode rows could theoretically be in flight. */
    seq: number;
    /** The message's current content; seeds `editBuffer`
     *  on edit-mode entry. Re-seeded whenever this changes
     *  while the row is in edit mode (covers the
     *  "stream ends mid-edit" race). */
    content: string;
    /** True while a chat stream is in-flight on the
     *  session. While true, the editor is fully disabled
     *  and Save / Cancel / Resend cannot fire (a defensive
     *  guard; the parent already gates the v-if on this). */
    isStreaming: boolean;
    /** Active session id. Used for the `aria-label` so
     *  screen readers can scope the editor. The save
     *  IPC itself uses the parent's `chatStore.currentSessionId`
     *  — the child never invokes the store. */
    currentSessionId: string | null;
    /** True when this row is the one the user opened
     *  edit mode on. The watcher re-seeds the buffer
     *  whenever this flips to true. */
    isEditingThisMessage: boolean;
    /** True while the parent's `editMessage` IPC is
     *  in flight. Disables the Save / Cancel buttons
     *  and flips the Save label to "保存中...". The
     *  parent owns the flag because it owns the
     *  try/catch. */
    saving: boolean;
    /** Inline error message. Set by the parent when
     *  the `editMessage` IPC rejects; cleared on the
     *  next edit-mode entry (parent flips to null). */
    errorMessage: string | null;
  }>(),
  {
    isStreaming: false,
    isEditingThisMessage: false,
    saving: false,
    errorMessage: null,
    currentSessionId: null,
  },
);

const emit = defineEmits<{
  /** Parent should call `chatStore.editMessage(sessionId,
   *  seq, trimmed)` to persist the new content. The
   *  trimmed string is the post-`String.prototype.trim`
   *  value the user typed. The parent handles the
   *  in-flight stream cancel + IPC + buffer refresh. */
  save: [trimmed: string];
  /** Parent should clear `chatStore.editingMessageSeq`
   *  to leave edit mode without saving. */
  cancel: [];
  /** Parent should call `chatStore.resendMessage(sessionId,
   *  seq, content)` to re-fire the user prompt. (D3 PR3;
   *  the button is not currently rendered but the emit
   *  is exposed for future flows.) */
  resend: [];
}>();

/** Local textarea buffer. Seeded with the message's current
 *  `content` on edit-mode entry; reset on cancel / save. */
const editBuffer = ref<string>(props.content);

/** Watch the seq → content transition: when edit mode
 *  opens for THIS message, seed the buffer with the
 *  current `content`. We also watch `content` (not just
 *  the editing flag) so a streaming turn that ends
 *  mid-edit still re-seeds the buffer with the final
 *  content (otherwise the textarea would show stale
 *  text from before the stream completed). */
watch(
  () => [props.isEditingThisMessage, props.content] as const,
  ([editing, newContent]) => {
    if (editing) {
      editBuffer.value = newContent;
    }
  },
  { immediate: true },
);

function onCancel() {
  if (props.saving || props.isStreaming) return;
  emit("cancel");
}

function onSave() {
  if (props.saving || props.isStreaming) return;
  const trimmed = editBuffer.value.trim();
  if (trimmed.length === 0) {
    // Don't emit; just return. The parent doesn't set
    // errorMessage for client-side validation (it'd
    //  flicker on every keystroke during the empty-
    //  period between typing & the user pressing save).
    // The disabled state of the Save button already
    //  covers this case (button is disabled when the
    //  buffer is empty post-trim), so reaching here
    //  means the user clicked the button before the
    //  disabled state was re-evaluated — defensive no-op.
    return;
  }
  if (trimmed === props.content) {
    // No-op: same content as before. Bubble a `cancel`
    // so the parent leaves edit mode (the user gets
    // the same "click cancel" affordance without an
    // extra button).
    emit("cancel");
    return;
  }
  emit("save", trimmed);
}
</script>

<template>
  <div class="msg__editor">
    <textarea
      v-model="editBuffer"
      class="msg__editor-textarea"
      rows="3"
      :aria-label="`编辑消息 seq ${seq}`"
      :disabled="saving || isStreaming"
      data-testid="msg-editor-textarea"
    />
    <div
      v-if="errorMessage"
      class="msg__editor-error"
      role="alert"
      data-testid="msg-editor-error"
    >
      <Icon name="warn" :size="12" icon-class="msg__editor-error-icon" />
      {{ errorMessage }}
    </div>
    <div class="msg__editor-actions">
      <button
        type="button"
        class="msg__editor-btn msg__editor-btn--cancel"
        :disabled="saving || isStreaming"
        data-testid="msg-editor-cancel"
        @click="onCancel"
      >
        取消
      </button>
      <button
        type="button"
        class="msg__editor-btn msg__editor-btn--save"
        :disabled="saving || isStreaming || editBuffer.trim().length === 0"
        data-testid="msg-editor-save"
        @click="onSave"
      >
        {{ saving ? "保存中..." : "保存" }}
      </button>
    </div>
  </div>
</template>

<style scoped>
/* D3 PR2: inline edit mode (user messages only). The
   bubble is replaced with a <textarea> + Save / Cancel.
   The textarea visually echoes the bubble's padding /
   radius so the edit-mode "row" feels like an in-place
   mutation of the bubble, not a totally different UI. */
.msg__editor {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 14px;
  border-radius: 6px;
  border: 1px solid color-mix(in srgb, var(--color-accent) 60%, var(--color-bg-border));
  background: var(--color-bg-elevated);
  /* `max-width: 100%` so the editor never overflows the
     parent <li> (which is itself `max-width: 75%`); the
     75% cap comes from the .msg rule. */
  max-width: 100%;
  margin-top: 4px;
  margin-bottom: 4px;
}

.msg__editor-textarea {
  width: 100%;
  min-height: 60px;
  max-height: 320px;
  padding: 6px 8px;
  background: var(--color-bg);
  color: var(--color-text-primary);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  font-family: inherit;
  font-size: 13px;
  line-height: 1.5;
  resize: vertical;
  outline: none;
  transition: border-color 0.12s, box-shadow 0.12s;
  box-sizing: border-box;
}

.msg__editor-textarea:focus {
  border-color: var(--color-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent);
}

.msg__editor-textarea:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.msg__editor-error {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--color-tool-error);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-tool-error) 40%, transparent);
  border-radius: 4px;
  padding: 4px 8px;
}

.msg__editor-error-icon {
  flex-shrink: 0;
}

.msg__editor-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.msg__editor-btn {
  padding: 4px 12px;
  border-radius: 4px;
  font-size: 12px;
  font-weight: 500;
  font-family: inherit;
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  cursor: pointer;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}

.msg__editor-btn:hover:not(:disabled) {
  background: var(--color-bg-surface);
  border-color: var(--color-text-muted);
}

.msg__editor-btn:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.msg__editor-btn--save {
  background: var(--color-accent);
  color: #ffffff;
  border-color: var(--color-accent);
}

.msg__editor-btn--save:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-accent) 85%, #000);
  border-color: color-mix(in srgb, var(--color-accent) 85%, #000);
}

.msg__editor-btn--save:disabled {
  background: color-mix(in srgb, var(--color-accent) 50%, transparent);
  border-color: transparent;
  color: #ffffff;
}
</style>
