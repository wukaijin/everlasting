<script setup lang="ts">
// MessageItem — single chat message bubble. Renders (in order):
//   1. Thinking block (if any) — violet left bar, collapsed by default
//   2. Redacted-thinking notice (rare, opaque data preserved for LLM)
//   3. Tool call cards (one per tool_use, with the matching result
//      looked up from the same message's `toolResults`)
//   4. The visible text bubble (with the blinking streaming cursor)
//   5. The error footer (if the turn failed)
//   6. F5 latency footer (right-aligned, hover tooltip with the
//      TTFB / gen / total breakdown)
//
// The "is there a bubble" predicate mirrors the original ChatWindow
// logic: any of {content, toolCalls, toolResults, thinkingBlocks,
// redactedThinkingData} → no bubble. The bubble is the fallback for
// the plain-text-only case.
//
// Markdown rendering (PR6):
//   The bubble text is now `v-html`'d through a debounced marked +
//   DOMPurify pipeline. See `utils/markdown.ts` for the XSS story.
//   The 50ms debounce collapses bursts of SSE deltas into a single
//   re-render; on stream end we flush so the final frame doesn't
//   wait out the timer.
//
// D3 PR2 (2026-06-17): inline message edit (user messages only).
//   On hover, a small ⋯ button appears at the top-right of the
//   <li> via `<MessageActionsMenu>`. Clicking it opens a
//   DropdownMenu with Edit / Resend / Copy; only Edit is wired
//   (Resend is a PR3 placeholder, Copy just hits the clipboard).
//   Edit replaces the bubble with a <textarea> + Save / Cancel
//   buttons; Save fires the chat store's `editMessage` (which
//   cancels any in-flight stream, fires the backend IPC, then
//   refreshes the in-memory buffer). Failure keeps the edit
//   mode active so the user can retry. The streaming state on
//   the parent <li> blocks the menu trigger entirely (defense
//   against mid-stream edits racing the LLM).

import { computed, ref, watch, onUnmounted } from "vue";
import type { ChatMessage } from "../../stores/chat.types";
import { useChatStore } from "../../stores/chat";
import { useProjectsStore } from "../../stores/projects";
import { useStreamControllerStore } from "../../stores/streamController";
import { getToolResult } from "../../utils/messageFormat";
import { createDebouncedRenderer } from "../../utils/markdown";
import ThinkingBlock from "./ThinkingBlock.vue";
import ToolCallCard from "./ToolCallCard.vue";
import FileInjectionsHint from "./FileInjectionsHint.vue";
import MessageActionsMenu from "./MessageActionsMenu.vue";
import MessageItemEdit from "./MessageItemEdit.vue";
import MessageItemFooter from "./MessageItemFooter.vue";
import Icon from "../Icon.vue";

const props = defineProps<{
  message: ChatMessage;
}>();

const chatStore = useChatStore();
const projectsStore = useProjectsStore();
const controller = useStreamControllerStore();

const hasVisibleBubble = computed<boolean>(() => {
  const m = props.message;
  return (
    !!m.content ||
    !!(m.toolCalls && m.toolCalls.length) ||
    !!(m.toolResults && m.toolResults.length) ||
    !!(m.thinkingBlocks && m.thinkingBlocks.length) ||
    !!(m.redactedThinkingData && m.redactedThinkingData.length)
  );
});

const showBubble = computed<boolean>(
  () =>
    !!props.message.content ||
    (!props.message.toolCalls?.length &&
      !props.message.toolResults?.length &&
      !props.message.thinkingBlocks?.length &&
      !props.message.redactedThinkingData?.length),
);

const showStreamingHint = computed<boolean>(
  () => !!props.message.streaming && !props.message.content,
);

// B12 Checklist (PR2 frontend, 2026-06-19): the
// `update_checklist` tool is rendered as a floating
// `<ChecklistCard>` overlay (mounted in ChatPanel), NOT as a
// per-call ToolCallCard in the message stream. Filter the tool
// list so the message bubble doesn't double-render the same
// state. The `use_skill` tool has no special treatment today
// (it renders as a normal ToolCallCard), so this is the first
// "virtual" tool suppression in the codebase. The filter is
// cheap (one linear pass per render); if more virtual tools
// accumulate, extract a `VIRTUAL_TOOLS` constant set.
const VIRTUAL_TOOLS = new Set<string>(["update_checklist"]);
const visibleToolCalls = computed(
  () =>
    props.message.toolCalls?.filter((tc) => !VIRTUAL_TOOLS.has(tc.name)) ?? [],
);

// --- Streaming state ----------------------------------------------------
// D3 PR2: the `MessageActionsMenu` greys out its trigger entirely
// when a stream is in flight on the same session. We read the
// controller's `streamingSessionIds` directly so the menu gets
// a per-session view (other sessions can keep streaming; only
// the current session's edit affordance is locked). The `isAtLeastOne`
// shape avoids subscribing to per-message deltas — we only need a
// boolean per session.
const isStreaming = computed<boolean>(() => {
  if (props.message.streaming) return true;
  // The streaming flag on the placeholder covers the user-sent
  // turn's own assistant message; for the per-session guard we
  // additionally read the controller's set. The two overlap on
  // the placeholder but neither subscribes to the other, so
  // a stale read of one is caught by the other.
  const sid = chatStore.currentSessionId;
  if (!sid) return false;
  return controller.streamingSessionIds.has(sid);
});

// --- D3 PR2: inline edit state -----------------------------------------
// `editingMessageSeq` lives on the chat store so it survives the
// MessageList re-render (Vue key-based remount on session switch
// would lose a local `ref`). A single ref is enough — only one
// row can be in edit mode at a time (opening a second one closes
// the first). The `isEditingThisMessage` computed derives the
// boolean for the current row.
//
// The actual edit UI (textarea + Save / Cancel / inline error)
// lives in `<MessageItemEdit>` (2026-06-23 split). This parent
// keeps three roles:
//   1. `isEditingThisMessage` computed: read-only check used by
//      the v-if gate + the v-bind into the child.
//   2. `editSaving` ref: tracks the in-flight `editMessage` IPC.
//      Passed to the child as the `saving` prop so the Save
//      button can flip to "保存中..." and disable Cancel.
//   3. Three handler functions (`handleSave` / `handleCancel` /
//      `handleResend`): own the store interactions
//      (`chatStore.editMessage` / `chatStore.resendMessage` /
//      `chatStore.editingMessageSeq = null`) and surface
//      toasts on failure. The child only emits intents.
const isEditingThisMessage = computed<boolean>(
  () =>
    chatStore.editingMessageSeq !== null &&
    chatStore.editingMessageSeq === props.message.seq,
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
  if (props.message.role !== "user") return;
  if (isStreaming.value) return;
  chatStore.editingMessageSeq = messageSeq;
}

/** D3 PR3 (2026-06-17): `MessageActionsMenu`'s `resend` emit
 *  handler. Re-fires the user message through the chat store,
 *  which (1) cancels any in-flight stream, (2) re-fires the
 *  `chat` IPC with the `resendSeq` flag, (3) the backend writes
 *  a `resend_message` audit row at the user-message persist
 *  site. We pass `props.message.content` as the user prompt —
 *  the backend treats the resend as identical to a normal
 *  send (same content, same history). On error, the
 *  `chatStore.resendMessage` promise rejects and we surface a
 *  toast (same pattern as `handleSave`'s catch path). */
async function onResend(messageSeq: number) {
  if (props.message.role !== "user") return;
  if (isStreaming.value) return;
  const sid = chatStore.currentSessionId;
  if (!sid) {
    projectsStore.showToast("重发失败: 无当前 session", "error");
    return;
  }
  try {
    await chatStore.resendMessage(sid, messageSeq, props.message.content);
  } catch (e) {
    projectsStore.showToast(
      `重发失败: ${String(e)}`,
      "error",
    );
  }
}

/** `<MessageItemEdit>`'s `save` emit handler. Called with
 *  the trimmed textarea content. Cancels any in-flight
 *  stream, fires the backend `edit_user_message` IPC, then
 *  refreshes the in-memory buffer. On success, closes
 *  edit mode; on failure, surfaces an inline error + a
 *  toast and keeps edit mode active for retry. */
async function handleSave(trimmed: string) {
  if (!props.message.seq) {
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
    await chatStore.editMessage(sid, props.message.seq, trimmed);
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
    editError.value = String(e);
    projectsStore.showToast(
      `编辑失败: ${String(e)}`,
      "error",
    );
  } finally {
    editSaving.value = false;
  }
}

/** `<MessageItemEdit>`'s `cancel` emit handler. Closes
 *  edit mode without saving. Also covers the child-side
 *  "same-content no-op" path (the child emits `cancel`
 *  when the trimmed buffer equals `props.message.content`,
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
  if (props.message.role !== "user") return;
  if (isStreaming.value) return;
  const sid = chatStore.currentSessionId;
  if (!sid) {
    projectsStore.showToast("重发失败: 无当前 session", "error");
    return;
  }
  if (!props.message.seq) {
    projectsStore.showToast("重发失败: 消息缺少 seq", "error");
    return;
  }
  try {
    await chatStore.resendMessage(sid, props.message.seq, props.message.content);
  } catch (e) {
    projectsStore.showToast(
      `重发失败: ${String(e)}`,
      "error",
    );
  }
}

// --- Markdown pipeline ----------------------------------------------------
// `createDebouncedRenderer` collapses the SSE delta stream into
// one render per 50ms quiet window; the `flush()` on stream end
// renders the final frame immediately so the user doesn't see
// a 50ms gap between the last delta and the rendered terminal
// state. The watcher drives the pipeline off `message.content`.
//
// Note: there is no `displayContent` gate here. The pre-split
// `displayContent` computed returned `""` while the row was in
// edit mode, on the theory that a streaming delta could clobber
// the textarea via the markdown render path. The bubble
// template's `v-if="showBubble"` already removes the
// `v-html="rendered"` element when the row is in edit mode
// (the `<MessageItemEdit>` block is the v-if alternative), so
// the markdown output has nowhere to render — the gate is
// redundant. The watcher watches the raw content directly and
// the only side-effect of a streaming delta mid-edit is one
// wasted `schedule()` call (debounced to 50ms, no-op because
// the bubble is unmounted).
const { rendered, schedule, flush, dispose } = createDebouncedRenderer(50);

watch(
  () => props.message.content,
  (next) => {
    schedule(next);
  },
  { immediate: true },
);

// When the stream ends, render the final frame immediately so the user
// doesn't see a 50ms gap between the last delta and the rendered
// terminal state. `streaming` is `true` only while SSE is active.
watch(
  () => props.message.streaming,
  (isStreaming) => {
    if (!isStreaming) flush();
  },
);

onUnmounted(() => {
  dispose();
});

// --- D3 PR3 (2026-06-17): "(edited)" label ----------------------------------
// When the row's metadata carries `edited_at` (written by the
// backend's `edit_user_message` transaction; see
// `.trellis/spec/backend/database-guidelines.md` "Pattern:
// `edit_user_message`"), we render a small grey "(edited)"
// label next to the bubble. The label is intentionally short —
// the user just needs a hint that this row's content was
// edited (vs. an un-edited row); the precise timestamp lives
// in the audit log (the `edit_message` audit row carries
// `edited_at`). Both user AND assistant messages can show the
// label (D3 PR1 in principle only allows user edits, but the
// metadata is read generically — defensive rendering for any
// future edit path). Hidden while the bubble is streaming
// (the placeholder has no metadata until the row is
// persisted) and while the row is in edit mode (the user is
// looking at the editor, not the bubble).
const editedAt = computed<string | null>(() => {
  const meta = props.message.metadata;
  if (!meta || typeof meta !== "object") return null;
  const v = (meta as Record<string, unknown>).edited_at;
  if (typeof v !== "string" || v.length === 0) return null;
  return v;
});

const showEditedLabel = computed<boolean>(
  () =>
    editedAt.value !== null &&
    !props.message.streaming &&
    !isEditingThisMessage.value,
);
</script>

<template>
  <li
    :class="[
      'msg',
      `msg--${message.role}`,
      {
        'msg--err': message.error,
        'msg--editing': isEditingThisMessage,
      },
    ]"
  >
    <!--
      D3 PR2: hover-triggered actions menu. Renders a small ⋯
      button at the top-right of the row (absolute-positioned
      via the .msg-actions class). Hidden when the message is
      being edited or the session is streaming. The hover
      affordance is the parent <li>'s `:hover` so the menu
      stays visible while the cursor moves onto it. See the
      `<MessageActionsMenu>` component for the dropdown shape
      and the disable rules.
    -->
    <MessageActionsMenu
      v-if="message.seq !== undefined"
      :message-seq="message.seq"
      :session-id="chatStore.currentSessionId ?? ''"
      :content="message.content"
      :role="message.role"
      :is-editing="isEditingThisMessage"
      :is-streaming="isStreaming"
      @edit="onEdit"
      @resend="onResend"
    />

    <ThinkingBlock
      v-if="
        message.role === 'assistant' &&
        message.thinkingBlocks &&
        message.thinkingBlocks.length
      "
      :blocks="message.thinkingBlocks"
      :streaming="message.streaming"
      :show-streaming-hint="showStreamingHint"
      :thinking-duration-ms="message.thinkingDurationMs"
    />

    <div
      v-if="message.redactedThinkingData && message.redactedThinkingData.length"
      class="msg__redacted"
      :title="`${message.redactedThinkingData.length} redacted thinking block(s); preserved verbatim for the LLM but not displayable`"
    >
      <Icon name="lock" :size="12" icon-class="msg__redacted-icon" />
      {{ message.redactedThinkingData.length }} redacted thinking block{{
        message.redactedThinkingData.length === 1 ? "" : "s"
      }}
      (preserved for LLM)
    </div>

    <div
      v-if="visibleToolCalls.length"
      class="msg__tools"
    >
      <ToolCallCard
        v-for="tc in visibleToolCalls"
        :key="tc.id"
        :call="tc"
        :result="getToolResult(message, tc.id)"
      />
      <!--
        2026-06-27 polish: when the message has tool calls but no
        text bubble (the common "LLM only emitted tools" turn), the
        F5 latency chip used to render OUTSIDE msg__tools, leaving
        a visually-detached `2.7s` label floating in space below
        the last tool card. Moving the footer INSIDE msg__tools
        attaches the chip to the last tool card visually. When the
        message has a text bubble, the v-if below short-circuits and
        the footer renders in its original bubble-anchored position
        (where the latency is conceptually attached to the LLM's
        prose, not its tool calls).
      -->
      <MessageItemFooter
        v-if="!showBubble && !isEditingThisMessage"
        :role="message.role"
        :streaming="!!message.streaming"
        :latency="message.latency"
        :error="message.error"
      />
    </div>

    <!--
      D3 PR2 (2026-06-17): inline edit mode for user messages.
      2026-06-23 split: the editor UI lives in
      `<MessageItemEdit>` — this parent only handles the
      v-if gate, the store-orchestrating handlers
      (`handleSave` / `handleCancel` / `handleResend`),
      and the IPC state machine (`editSaving` /
      `editError`). The child is a pure presentation layer
      that emits `save(trimmed)` / `cancel` / `resend`;
      no Pinia store import.

      The edit-mode branch is mutually exclusive with
      the streaming branch (the menu trigger is disabled
      when streaming, so the user can't open edit during
      a stream), but the v-if checks both
      `isEditingThisMessage` AND the absence of streaming
      as a defensive guard.
    -->
    <MessageItemEdit
      v-if="isEditingThisMessage && !isStreaming && message.role === 'user'"
      :seq="message.seq ?? 0"
      :content="message.content"
      :is-streaming="isStreaming"
      :current-session-id="chatStore.currentSessionId"
      :is-editing-this-message="isEditingThisMessage"
      :saving="editSaving"
      :error-message="editError"
      @save="handleSave"
      @cancel="handleCancel"
      @resend="handleResend"
    />

    <div v-else-if="showBubble" class="msg__bubble">
      <span
        v-if="hasVisibleBubble || message.content"
        class="msg__markdown"
        v-html="rendered"
      />
      <span v-if="message.streaming" class="msg__cursor" aria-hidden="true"
        >▍</span
      >
      <!--
        D3 PR3 (2026-06-17): "(edited)" label. Renders
        inline at the bottom-right of the bubble when the
        row's metadata has `edited_at`. The label is a small
        grey mono-text chip — visually quiet so it doesn't
        compete with the bubble content. The `title`
        attribute surfaces the precise edit timestamp on
        hover for users who care to look. We keep this
        separate from the F5 latency chip (which renders
        BELOW the bubble in `.msg__latency`) so the two
        never collide when both are present (assistant
        message with both latency + edited_at).
      -->
      <span
        v-if="showEditedLabel"
        class="msg__edited"
        :title="`最后编辑于 ${editedAt}`"
        data-testid="msg-edited-label"
      >
        (edited)
      </span>
    </div>

    <!--
      B2 PR3: per-user-turn `@relpath` injection hint row.
      Renders the agent loop's verdict for every @file
      token the user typed in this message — text
      injections (with line count), image/PDF/Office/
      binary degradations, and out-of-root / missing /
      unreadable skips. Mounted ONLY for user messages
      (the assistant never has @ tokens) and ONLY when
      the `injections` array is non-empty (a no-@ user
      message leaves the field undefined; the
      `v-if` keeps the DOM clean for the common case).
      The component is a thin renderer — see
      `FileInjectionsHint.vue` for the per-row shape.
    -->
    <FileInjectionsHint
      v-if="
        message.role === 'user' &&
        message.injections &&
        message.injections.length > 0
      "
      :injections="message.injections"
    />

    <!--
      2026-06-23 split: error row + F5 latency chip extracted
      into `<MessageItemFooter>`. Per the task's ADR-2
      decision, the (edited) label stays in the parent
      (inside the bubble div) — it is visually distinct
      from the error / latency chips that hang below the
      bubble, and it shares a flex column with the bubble
      text. The footer only handles error + latency.

      The parent passes the raw `error` / `latency` from
      the ChatMessage and the streaming flag (the footer
      reads them through the same v-if gate as before).

      2026-06-27 polish: when the message has tool calls but
      no text bubble, the footer is rendered INSIDE
      `msg__tools` above (so the latency chip attaches to
      the last tool card). The outer footer here only
      renders when there's NO tool-calls/no-bubble mismatch
      (i.e., bubble-only or user-role / system rows). The
      `v-if` gates both: no tools AND no bubble visible.
    -->
    <MessageItemFooter
      v-if="!visibleToolCalls.length || showBubble"
      :role="message.role"
      :streaming="!!message.streaming"
      :latency="message.latency"
      :error="message.error"
    />
  </li>
</template>

<style scoped>
.msg {
  display: flex;
  flex-direction: column;
  max-width: 75%;
  /* Position context for the absolute-positioned
     .msg-actions trigger — see MessageActionsMenu.vue.
     `relative` lets the trigger anchor to the row's
     top-right without flowing inline. */
  position: relative;
}

.msg--user {
  align-self: flex-end;
  margin-right: 16px;
}

.msg--assistant {
  align-self: flex-start;
}

/* D3 PR2: the inline edit mode gets a subtle accent border
   + a tinted background to signal "this row is in
   edit-mode" — analogous to the visual hint the
   .tool-card--pending class gives the tool card. The user
   can still see the surrounding context (no full
   `outline` ring) but the row is clearly demarcated. */
.msg--editing {
  padding: 4px 6px;
  margin: -4px -6px;
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--color-accent) 6%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-accent) 40%, var(--color-bg-border));
}

/* D3 PR2: hover affordance for the .msg-actions trigger.
   The trigger is `position: absolute; top: -8px; right: 4px`
   inside MessageActionsMenu and starts at `opacity: 0`; we
   fade it in when the user hovers the row. `:focus-within`
   keeps it visible while keyboard focus is anywhere inside
   the row (e.g. a Save button after a click). The check for
   `msg--editing` / `msg--err` is handled by the
   MessageActionsMenu's own state classes (they keep
   `pointer-events: none` + `opacity: 0` even when the
   parent is hovered). */
.msg:hover .msg-actions,
.msg:focus-within .msg-actions {
  opacity: 1;
}

/* PR-3a (2026-06-27): whole-row hover tint. A 6% primary-text
   wash on the row tells the user "this is an interactive row"
   (not just the bubble — the row owns the actions menu).
   Excluded for edit/err states (they own their own visual
   treatment via .msg--editing / .msg--err backgrounds). The
   transition keeps the wash smooth and avoids a hard flash
   on rapid mouse passes. */
.msg:not(.msg--editing):not(.msg--err) {
    border-radius: var(--radius-lg);
    transition: background-color var(--duration-fast) var(--ease-out);
}
.msg:not(.msg--editing):not(.msg--err):hover,
.msg:not(.msg--editing):not(.msg--err):focus-within {
    background: var(--color-bg-hover);
}

.msg__redacted {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
  padding: 4px 10px;
  background: var(--color-bg-elevated);
  border: 1px dashed var(--color-bg-border);
  border-radius: var(--radius-md);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.msg__redacted-icon {
  flex-shrink: 0;
  color: var(--color-text-secondary);
}

.msg__tools {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 4px;
  max-width: 100%;
}

.msg__bubble {
  padding: 10px 14px;
  border-radius: var(--radius-lg);
  /* `white-space: pre-wrap` removed in PR6 — markdown handles its own
     line breaks via `breaks: true` in the marked options, and
     pre-wrap would mangle <pre> code blocks (the leading whitespace
     on each line of code would be preserved literally, fighting the
     monospace font's own rendering). */
  word-break: break-word;
  line-height: var(--leading-relaxed);
  border: 1px solid var(--color-bg-border);

  margin-top: 4px;
  margin-bottom: 4px;
}

/* PR-3a (2026-06-27): user bubble lightened.
   Was: accent (#3b5bdb) fill + white text. Too visually heavy for
   a chat where the user message is one of two equally-weighted roles
   in a turn. New: accent-muted (#1e2a5e) fill + primary text
   (cbd5e1). WCAG 8.66:1 contrast — both AA (4.5) and AAA (7) pass.
   Subtle 30% accent border for delineation against chat-panel bg. */
.msg--user .msg__bubble {
  background: var(--color-accent-muted);
  color: var(--color-text-primary);
  border-color: color-mix(in srgb, var(--color-accent) 30%, transparent);
  /* PR5a (2026-06-27, D6 方案A): 3px accent left bar — a visual
     anchor for "this is my input" that distinguishes the user
     bubble from the assistant's elevated-gray bubble at a glance,
     reusing the tool-card left-bar semantic. Inset box-shadow
     (not border-left) so it doesn't perturb the bubble's 1px
     border-width or shift the layout. Assistant bubbles get no
     left bar. */
  box-shadow: inset 3px 0 0 var(--color-accent);
}

.msg--assistant .msg__bubble {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.msg--err .msg__bubble {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}

.msg__cursor {
  display: inline-block;
  margin-left: 2px;
  animation: blink 1s steps(1) infinite;
  color: var(--color-text-muted);
}

@keyframes blink {
  50% {
    opacity: 0;
  }
}

/* D3 PR3 (2026-06-17): "(edited)" label. Sits inline at the
   bottom-right of the bubble when the row's metadata has
   `edited_at`. Visually quiet (small mono grey, no border,
   no padding) so it doesn't compete with the bubble content
   or the F5 latency chip below. The `margin-left: auto`
   pushes it to the right edge of the bubble's flex column;
   for assistant bubbles the chip stays on the bubble's
   right side, matching the bubble's bottom-right
   alignment convention (the F5 latency chip lives
   separately below the bubble). */
.msg__edited {
  display: inline-flex;
  align-self: flex-end;
  margin-top: 2px;
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  color: var(--color-text-muted);
  font-style: italic;
  user-select: none;
}

/* Markdown content (v-html). The HTML lives in a child tree without
   scoped classes, so every selector below uses :deep() to reach into
   the rendered output. Keep the list focused on elements marked
   actually produces — avoid hypothetical selectors that will never
   match and just become dead code. */
.msg__markdown {
  display: block;
}

.msg__markdown :deep(p) {
  margin: 0 0 8px 0;
}

.msg__markdown :deep(p:last-child) {
  margin-bottom: 0;
}

.msg__markdown :deep(h1),
.msg__markdown :deep(h2),
.msg__markdown :deep(h3),
.msg__markdown :deep(h4),
.msg__markdown :deep(h5),
.msg__markdown :deep(h6) {
  margin: 12px 0 6px 0;
  font-weight: var(--weight-semibold);
  line-height: 1.3;
}

.msg__markdown :deep(h1) {
  font-size: 1.4em;
}
.msg__markdown :deep(h2) {
  font-size: 1.25em;
}
.msg__markdown :deep(h3) {
  font-size: 1.1em;
}
.msg__markdown :deep(h4) {
  font-size: 1em;
}

.msg__markdown :deep(h1:first-child),
.msg__markdown :deep(h2:first-child),
.msg__markdown :deep(h3:first-child),
.msg__markdown :deep(h4:first-child) {
  margin-top: 0;
}

.msg__markdown :deep(ul),
.msg__markdown :deep(ol) {
  margin: 6px 0;
  padding-left: 24px;
}

.msg__markdown :deep(li) {
  margin: 2px 0;
}

.msg__markdown :deep(strong) {
  font-weight: var(--weight-semibold);
}

.msg__markdown :deep(em) {
  font-style: italic;
}

.msg__markdown :deep(code) {
  font-family: var(--font-mono);
  font-size: 0.9em;
  padding: 1px 5px;
  border-radius: 3px;
  background: color-mix(in srgb, var(--color-text-primary) 8%, transparent);
  border: 1px solid var(--color-bg-border-strong);
}

.msg__markdown :deep(pre) {
  margin: 8px 0;
  padding: 10px 12px;
  background: color-mix(in srgb, var(--color-text-primary) 6%, transparent);
  border: 1px solid var(--color-bg-border-strong);
  border-radius: var(--radius-md);
  overflow-x: auto;
  line-height: 1.45;
}

.msg__markdown :deep(pre code) {
  padding: 0;
  background: transparent;
  border: 0;
  font-size: 0.9em;
  white-space: pre;
}

.msg__markdown :deep(a) {
  color: var(--color-accent);
  text-decoration: underline;
  text-underline-offset: 2px;
}

.msg__markdown :deep(blockquote) {
  margin: 8px 0;
  padding: 4px 12px;
  border-left: 3px solid var(--color-bg-border);
  color: var(--color-text-secondary);
  font-style: italic;
}

.msg__markdown :deep(hr) {
  border: 0;
  border-top: 1px solid var(--color-bg-border);
  margin: 12px 0;
}

.msg__markdown :deep(table) {
  border-collapse: collapse;
  margin: 8px 0;
  font-size: 0.95em;
}

.msg__markdown :deep(th),
.msg__markdown :deep(td) {
  /* Stronger border color than --color-bg-border because table cells
     sit on --color-bg-elevated (the bubble) and the regular border
     reads as invisible (only 4 luminance units of separation). */
  border: 1px solid var(--color-bg-border-strong);
  padding: 4px 8px;
  text-align: left;
}

.msg__markdown :deep(th) {
  background: var(--color-bg);
  font-weight: var(--weight-semibold);
}
</style>
