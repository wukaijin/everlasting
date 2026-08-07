// 内联编辑状态与处理器(拆分自 MessageItem.vue, 08-07-large-file-splitting)。
// composable:入参 message 与 isStreaming 的 getter,保持响应性。

import { computed, ref, watch } from "vue";
import type { ChatMessage } from "../../stores/chat.types";
import { extractErrorMessage } from "../../utils/useErrorBus";
import { categoryRetryable } from "../../utils/error";
import { useChatStore } from "../../stores/chat";
import { useProjectsStore } from "../../stores/projects";
import { useStreamControllerStore } from "../../stores/streamController";

export function useMessageEditing(
  message: () => ChatMessage,
  isStreaming: () => boolean,
) {
  const chatStore = useChatStore();
  const projectsStore = useProjectsStore();
  const controller = useStreamControllerStore();

const isEditingThisMessage = computed<boolean>(
  () =>
    chatStore.editingMessageSeq !== null &&
    chatStore.editingMessageSeq === message().seq,
);

/** True while the `editMessage` IPC is in flight. Disables
 *  the child editor's Save / Cancel buttons and flips the
 *  Save label to "保存中...". Reset to false on success
 *  (edit mode closes) and on failure (caught in the IPC
 *  promise, see `handleSave`). */
const editSaving = ref<boolean>(false);

/** Inline error message shown above the Save / Cancel row
 *  by `<MessageItemEdit>`. Set when the `editMessage` IPC
 *  rejects; cleared on the next edit-mode entry (the
 *  parent flips it to null when `isEditingThisMessage`
 *  flips to true). */
const editError = ref<string | null>(null);

watch(
  () => isEditingThisMessage.value,
  (now) => {
    if (now) {
      // Fresh edit session: clear any stale error from
      // the previous attempt. The save-in-flight flag
      // can't be stale (a previous save would have closed
      // edit mode on success or routed through the catch
      // on failure).
      editError.value = null;
    }
  },
  { immediate: true },
);

/** `MessageActionsMenu`'s `edit` emit handler. Routes to the
 *  chat store so the editing-message-seq flips to this row's
 *  seq; the local `isEditingThisMessage` then re-evaluates and
 *  the textarea renders. */
function onEdit(messageSeq: number) {
  if (message().role !== "user") return;
  if (isStreaming()) return;
  chatStore.editingMessageSeq = messageSeq;
}

/** D3 PR3 (2026-06-17): `MessageActionsMenu`'s `resend` emit
 *  handler. Re-fires the user message through the chat store,
 *  which (1) cancels any in-flight stream, (2) re-fires the
 *  `chat` IPC with the `resendSeq` flag, (3) the backend writes
 *  a `resend_message` audit row at the user-message persist
 *  site. We pass `message().content` as the user prompt —
 *  the backend treats the resend as identical to a normal
 *  send (same content, same history). On error, the
 *  `chatStore.resendMessage` promise rejects and we surface a
 *  toast (same pattern as `handleSave`'s catch path). */
async function onResend(messageSeq: number) {
  if (message().role !== "user") return;
  if (isStreaming()) return;
  const sid = chatStore.currentSessionId;
  if (!sid) {
    projectsStore.showToast("重发失败: 无当前 session", "error");
    return;
  }
  try {
    await chatStore.resendMessage(sid, messageSeq, message().content);
  } catch (e) {
    projectsStore.showToast(
      `重发失败: ${extractErrorMessage(e)}`,
      "error",
    );
  }
}

/** A5 R2 (2026-07-17): `MessageItemFooter`'s `retry` emit
 *  handler. Re-fires the chat stream against the errored
 *  assistant row by calling `chatStore.retryChat` (which
 *  cancels any in-flight stream, mutates the errored row,
 *  strips the `ERROR_MARKER` tail, and starts a new stream
 *  without a `resendSeq` flag — no audit, no content
 *  mutation). Returns gracefully (no throw) so the loading
 *  state in the footer resets regardless of the IPC outcome.
 *
 *  During the retry we set `retryLoading = true` so the
 *  footer button flips to its disabled "重试中..." state;
 *  the watcher on `streamingSessionIds` below clears it when
 *  the new stream ends (`done` / `error` / cancel).
 *
 *  Why guard `categoryRetryable`: defensively prevent a click
 *  that shouldn't be possible (footer only renders the button
 *  when retryable is true) from firing the IPC — keeps the
 *  log quiet on stale UI snapshots. */
const retryLoading = ref(false);

async function onRetry(messageSeq: number) {
  if (typeof messageSeq !== "number") return;
  if (!categoryRetryable(message().error?.category)) return;
  if (isStreaming()) return;
  const sid = chatStore.currentSessionId;
  if (!sid) {
    projectsStore.showToast("重试失败: 无当前 session", "error");
    return;
  }
  retryLoading.value = true;
  try {
    await chatStore.retryChat(sid, messageSeq);
  } catch (e) {
    projectsStore.showToast(
      `重试失败: ${extractErrorMessage(e)}`,
      "error",
    );
    retryLoading.value = false;
  }
  // Success path leaves `retryLoading=true`; the watcher
  // below clears it on the next stream-id transition.
}

/** A5 R2: when this message's session leaves the streaming
 *  set (the retry's `done` / `error` event landed), reset
 *  the footer's `retryLoading` flag so the button goes back
 *  to its non-loading label. Watching `streamingSessionIds`
 *  is cheaper than watching `message.streaming` because
 *  the latter is set true during the retry's placeholder
 *  mutation, which would never go back to false unless we
 *  watched something else too. The streaming set is the
 *  single source of truth for "the session has an in-flight
 *  request" — when it loses this message's session, the
 *  retry chain is over. */
watch(
  () => controller.streamingSessionIds,
  (ids) => {
    const sid = chatStore.currentSessionId;
    if (sid && !ids.has(sid) && retryLoading.value) {
      retryLoading.value = false;
    }
  },
);

/** `<MessageItemEdit>`'s `save` emit handler. Called with
 *  the trimmed textarea content. Cancels any in-flight
 *  stream, fires the backend `edit_user_message` IPC, then
 *  refreshes the in-memory buffer. On success, closes
 *  edit mode; on failure, surfaces an inline error + a
 *  toast and keeps edit mode active for retry. */
async function handleSave(trimmed: string) {
  if (!message().seq) {
    editError.value = "消息缺少 seq,无法编辑";
    return;
  }
  if (editSaving.value) return;
  // The session id we send must be the one this message
  // belongs to. The store's `currentSessionId` is the user's
  // *current* session — for a rehydrated message this is
  // always the same value (MessageList only renders messages
  // for the active session), so this is correct. Defensive:
  // if the user somehow triggers edit on a message from a
  // different session (shouldn't happen, the menu is per-
  // message in the active list), the IPC would error with
  // "session not found" and the catch path surfaces it.
  const sid = chatStore.currentSessionId;
  if (!sid) {
    editError.value = "editMessage: no current session";
    return;
  }
  editSaving.value = true;
  editError.value = null;
  try {
    await chatStore.editMessage(sid, message().seq as number, trimmed);
    // Refresh succeeded. The controller's `refresh` has
    // already replaced the in-memory buffer; we close the
    // edit mode so the bubble re-renders with the new
    // content (the rehydrated message carries the new
    // `text` column).
    chatStore.editingMessageSeq = null;
  } catch (e) {
    // Failure path: keep edit mode active so the user can
    // adjust and retry. The error message is the IPC's
    // `String` rejection (e.g. "edit_user_message: user
    // message at seq 5 not found in session ...") or a
    // generic message for client-side errors.
    editError.value = extractErrorMessage(e);
    projectsStore.showToast(
      `编辑失败: ${extractErrorMessage(e)}`,
      "error",
    );
  } finally {
    editSaving.value = false;
  }
}

/** `<MessageItemEdit>`'s `cancel` emit handler. Closes
 *  edit mode without saving. Also covers the child-side
 *  "same-content no-op" path (the child emits `cancel`
 *  when the trimmed buffer equals `message().content`,
 *  so the user doesn't see a textarea stuck open). */
function handleCancel() {
  chatStore.editingMessageSeq = null;
  editError.value = null;
}

/** `<MessageItemEdit>`'s `resend` emit handler. Re-fires
 *  the user prompt through the chat store. The child
 *  currently does not render a Resend button (the user
 *  has to go through the `<MessageActionsMenu>` to get
 *  there), but the prop+emit is exposed for any future
 *  flow that wants to surface Resend from the editor. */
async function handleResend() {
  if (message().role !== "user") return;
  if (isStreaming()) return;
  const sid = chatStore.currentSessionId;
  if (!sid) {
    projectsStore.showToast("重发失败: 无当前 session", "error");
    return;
  }
  if (!message().seq) {
    projectsStore.showToast("重发失败: 消息缺少 seq", "error");
    return;
  }
  try {
    await chatStore.resendMessage(sid, message().seq as number, message().content);
  } catch (e) {
    projectsStore.showToast(
      `重发失败: ${extractErrorMessage(e)}`,
      "error",
    );
  }
}

// --- Markdown pipeline ----------------------------------------------------
  return {
    isEditingThisMessage,
    editSaving,
    editError,
    onEdit,
    onResend,
    onRetry,
    retryLoading,
    handleSave,
    handleCancel,
    handleResend,
  };
}
