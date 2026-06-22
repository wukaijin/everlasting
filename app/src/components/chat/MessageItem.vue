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
import {
  TooltipProvider,
  TooltipRoot,
  TooltipTrigger,
  TooltipPortal,
  TooltipContent,
  TooltipArrow,
} from "reka-ui";
import type { ChatMessage } from "../../stores/chat.types";
import { useChatStore } from "../../stores/chat";
import { useProjectsStore } from "../../stores/projects";
import { useStreamControllerStore } from "../../stores/streamController";
import { getToolResult } from "../../utils/messageFormat";
import { createDebouncedRenderer } from "../../utils/markdown";
import { abbreviateDuration } from "../../utils/duration";
import ThinkingBlock from "./ThinkingBlock.vue";
import ToolCallCard from "./ToolCallCard.vue";
import FileInjectionsHint from "./FileInjectionsHint.vue";
import MessageActionsMenu from "./MessageActionsMenu.vue";
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
const isEditingThisMessage = computed<boolean>(
  () =>
    chatStore.editingMessageSeq !== null &&
    chatStore.editingMessageSeq === props.message.seq,
);

// Local textarea buffer. The ref starts with the message's
// current `content` on edit-mode entry and is reset on cancel
// / save-success. We deliberately do NOT bind `v-model` directly
// to `props.message.content` because (a) the prop is read-only
// from the parent's perspective (we don't want to mutate the
// store's reactive ChatMessage from the textarea handler), and
// (b) the controller's `done` / `error` / `delta` handlers
// also mutate the in-memory message's `content` — a live
// v-model would race the streaming text append. The local
// ref + explicit commit on Save sidesteps both.
const editBuffer = ref<string>(props.message.content);
const isSaving = ref<boolean>(false);
const editError = ref<string | null>(null);

// Watch the seq → content transition: when the user opens
// edit mode for THIS message, seed the buffer with the current
// `content`. We watch the message's `content` (not just the
// editing flag) so a streaming turn that ends mid-edit still
// re-seeds the buffer with the final content (otherwise the
// textarea would show stale text from before the stream
// completed). The same watcher also resets `editError` so a
// fresh edit session doesn't carry over the previous error.
watch(
  () => [chatStore.editingMessageSeq, props.message.content] as const,
  ([newSeq, newContent]) => {
    if (newSeq === props.message.seq) {
      editBuffer.value = newContent;
      editError.value = null;
    }
  },
  { immediate: true },
);

// `MessageActionsMenu`'s `edit` emit handler. Routes to the
// chat store so the editing-message-seq flips to this row's
// seq; the local `isEditingThisMessage` then re-evaluates and
// the textarea renders.
function onEdit(messageSeq: number) {
  if (props.message.role !== "user") return;
  if (isStreaming.value) return;
  chatStore.editingMessageSeq = messageSeq;
}

// D3 PR3 (2026-06-17): `MessageActionsMenu`'s `resend` emit
// handler. Re-fires the user message through the chat store,
// which (1) cancels any in-flight stream, (2) re-fires the
// `chat` IPC with the `resendSeq` flag, (3) the backend writes
// a `resend_message` audit row at the user-message persist
// site. We pass `props.message.content` as the user prompt —
// the backend treats the resend as identical to a normal
// send (same content, same history). On error, the
// `chatStore.resendMessage` promise rejects and we surface a
// toast (same pattern as `saveEdit`'s catch path).
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

function cancelEdit() {
  chatStore.editingMessageSeq = null;
  editBuffer.value = props.message.content;
  editError.value = null;
}

async function saveEdit() {
  if (!props.message.seq) {
    editError.value = "消息缺少 seq,无法编辑";
    return;
  }
  if (isSaving.value) return;
  const trimmed = editBuffer.value.trim();
  if (trimmed.length === 0) {
    editError.value = "内容不能为空";
    return;
  }
  if (trimmed === props.message.content) {
    // No-op: same content as before. Close the edit mode so
    // the user doesn't see a textarea that's stuck open.
    cancelEdit();
    return;
  }
  isSaving.value = true;
  editError.value = null;
  try {
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
      throw new Error("editMessage: no current session");
    }
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
    isSaving.value = false;
  }
}

// --- Markdown pipeline ----------------------------------------------------
// `displayContent` is a thin pass-through to `message.content`. The
// leading-whitespace trim that PR7 put here now lives inside
// `renderMarkdown()` (see `app/src/utils/markdown.ts`) so the rendering
// layer owns its own input policy. We keep the named computed because
// (a) the watch below needs a stable dependency reference, and (b) it
// documents intent at the call site ("this is the text we render").
// `editingThisMessage` is read-only here — the markdown pipeline
// pauses while the textarea is open (we don't want a streaming
// delta to clobber the user's edits). The watcher on
// `displayContent` re-schedules on cancel / save so the final
// bubble renders the new content.
const displayContent = computed<string>(() =>
  isEditingThisMessage.value ? "" : props.message.content,
);

const { rendered, schedule, flush, dispose } = createDebouncedRenderer(50);

watch(
  displayContent,
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

// --- F5 latency footer -----------------------------------------------------
// Renders the bottom-right of the assistant bubble with a 1-decimal
// abbreviation of `totalMs` (e.g. "3.2s"). Hover surfaces the three-
// line breakdown (TTFB / 生成 / 端到端). The trigger is the chip
// itself; the tooltip content is a small block with the three rows.
// The display is hidden for:
//   - user-role messages (only assistant turns have a latency)
//   - messages without a `latency` object (pre-F5 rows; UI shows
//     "—" in place of the chip)
//   - messages mid-stream (`streaming` true; the chip is in flux
//     and the user is reading the bubble, not the footer)
const showLatency = computed<boolean>(
  () =>
    props.message.role === "assistant" &&
    !props.message.streaming &&
    !!props.message.latency &&
    typeof props.message.latency.totalMs === "number",
);

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

const latencyTotalLabel = computed<string>(() => {
  const t = props.message.latency?.totalMs;
  if (typeof t !== "number") return "—";
  return abbreviateDuration(t);
});

// The three lines shown in the hover tooltip. Each is omitted (and
// the row hidden) when the value is undefined — the cancel / error
// path leaves ttfbMs / genMs null while totalMs is set, and the UI
// shows only the available rows.
const latencyRows = computed<
  Array<{ label: string; value: string }>
>(() => {
  const lat = props.message.latency;
  if (!lat) return [];
  const rows: Array<{ label: string; value: string }> = [];
  if (typeof lat.ttfbMs === "number") {
    rows.push({ label: "TTFB", value: abbreviateDuration(lat.ttfbMs) });
  }
  if (typeof lat.genMs === "number") {
    rows.push({ label: "生成", value: abbreviateDuration(lat.genMs) });
  }
  if (typeof lat.totalMs === "number") {
    rows.push({ label: "端到端", value: abbreviateDuration(lat.totalMs) });
  }
  return rows;
});
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
    </div>

    <!--
      D3 PR2: inline edit mode for user messages. The bubble
      (markdown render) is hidden and replaced with a <textarea>
      + Save / Cancel buttons. The textarea is autosize-shaped
      (2-20 rows, follows width). Save fires the chat store's
      `editMessage`, which cancels any in-flight stream, calls
      the backend `edit_user_message` IPC, then refreshes the
      in-memory buffer. Failure keeps the edit mode active and
      shows an inline error + toast.

      The edit-mode branch is mutually exclusive with the
      streaming branch (the menu trigger is disabled when
      streaming, so the user can't open edit during a stream),
      but the v-if checks both `isEditingThisMessage` AND the
      absence of streaming as a defensive guard.
    -->
    <div
      v-if="isEditingThisMessage && !isStreaming && message.role === 'user'"
      class="msg__editor"
    >
      <textarea
        v-model="editBuffer"
        class="msg__editor-textarea"
        rows="3"
        :aria-label="`编辑消息 seq ${message.seq}`"
        :disabled="isSaving"
        data-testid="msg-editor-textarea"
      />
      <div
        v-if="editError"
        class="msg__editor-error"
        role="alert"
        data-testid="msg-editor-error"
      >
        <Icon name="warn" :size="12" icon-class="msg__editor-error-icon" />
        {{ editError }}
      </div>
      <div class="msg__editor-actions">
        <button
          type="button"
          class="msg__editor-btn msg__editor-btn--cancel"
          :disabled="isSaving"
          data-testid="msg-editor-cancel"
          @click="cancelEdit"
        >
          取消
        </button>
        <button
          type="button"
          class="msg__editor-btn msg__editor-btn--save"
          :disabled="isSaving || editBuffer.trim().length === 0"
          data-testid="msg-editor-save"
          @click="saveEdit"
        >
          {{ isSaving ? "保存中..." : "保存" }}
        </button>
      </div>
    </div>

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

    <div v-if="message.error" class="msg__error">
      <Icon name="warn" :size="12" icon-class="msg__error-icon" />
      {{ message.error.message }}
    </div>

    <!--
      F5 (LLM Latency Tracking): per-message latency chip. Renders
      the bottom-right of the assistant bubble (the bubble is
      a `max-width: 75%` flex column; the chip is right-aligned
      via `align-self: flex-end`). The chip shows the total
      time in seconds (one decimal) and a hover tooltip breaks
      it down into TTFB / 生成 / 端到端. Pre-F5 / cancel-mid-TTFB
      / no-delta-arrived rows render "—" (handled by the
      `latencyTotalLabel` computed and the conditional
      `v-if="latencyRows.length"`).
    -->
    <TooltipProvider v-if="showLatency">
      <TooltipRoot :delay-duration="150">
        <TooltipTrigger as-child>
          <span class="msg__latency">{{ latencyTotalLabel }}</span>
        </TooltipTrigger>
        <TooltipPortal>
          <TooltipContent
            class="msg__latency-tooltip"
            :side-offset="4"
          >
            <div
              v-for="row in latencyRows"
              :key="row.label"
              class="msg__latency-tooltip-row"
            >
              <span>{{ row.label }}</span>
              <span>{{ row.value }}</span>
            </div>
            <TooltipArrow class="msg__latency-tooltip-arrow" :size="6" />
          </TooltipContent>
        </TooltipPortal>
      </TooltipRoot>
    </TooltipProvider>
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
  border-radius: 8px;
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

.msg__redacted {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
  padding: 4px 10px;
  background: var(--color-bg-elevated);
  border: 1px dashed var(--color-bg-border);
  border-radius: 6px;
  font-size: 11px;
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
  border-radius: 6px;
  /* `white-space: pre-wrap` removed in PR6 — markdown handles its own
     line breaks via `breaks: true` in the marked options, and
     pre-wrap would mangle <pre> code blocks (the leading whitespace
     on each line of code would be preserved literally, fighting the
     monospace font's own rendering). */
  word-break: break-word;
  line-height: 1.6;
  border: 1px solid var(--color-bg-border);

  margin-top: 4px;
  margin-bottom: 4px;
}

.msg--user .msg__bubble {
  background: var(--color-accent);
  color: #ffffff;
  border-color: var(--color-accent);
  border-bottom-right-radius: 2px;
}

.msg--assistant .msg__bubble {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  border-bottom-left-radius: 2px;
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
  font-size: 10px;
  font-family: var(--font-mono);
  color: var(--color-text-muted);
  font-style: italic;
  user-select: none;
}

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

.msg__error {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-top: 4px;
  padding: 0 14px;
  font-size: 12px;
  color: var(--color-tool-error);
}

.msg__error-icon {
  flex-shrink: 0;
}

/* F5 (LLM Latency Tracking): per-message latency chip. Sits
   at the bottom-right of the assistant bubble. The chip
   itself is the TooltipTrigger; the tooltip content is the
   three-row breakdown (TTFB / 生成 / 端到端).

   Visual decisions:
   - 11px mono font to match the existing density (token
     usage chip in ChatInput uses the same).
   - 0.5px muted color so it doesn't fight the bubble for
     attention — the user sees it on glance but isn't
     pulled in.
   - Right-aligned via `align-self: flex-end` (the parent
     `li.msg` is `display: flex; flex-direction: column`,
     so the chip is the rightmost element of the bubble
     column). */
.msg__latency {
  display: inline-flex;
  align-items: center;
  align-self: flex-end;
  margin-top: 4px;
  padding: 0 6px;
  font-size: 11px;
  font-family: var(--font-mono);
  font-weight: 600;
  color: var(--color-text-muted);
  cursor: help;
  border-radius: 4px;
  user-select: none;
}

.msg__latency:hover {
  color: var(--color-text-secondary);
}

/* Tooltip content (reka-ui `TooltipContent` portal to body
   — must use :deep() per `.trellis/spec/frontend/reka-ui-usage.md`
   gotcha). The popover floats above the chip (default
   side is "top"). 11px mono, single-column row layout. */
:deep(.msg__latency-tooltip) {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  padding: 6px 10px;
  min-width: 140px;
  z-index: 3000;
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  animation: msg-latency-tooltip-enter 150ms ease-out;
}

:deep(.msg__latency-tooltip-row) {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  padding: 1px 0;
}

:deep(.msg__latency-tooltip-row span:first-child) {
  color: var(--color-text-secondary);
}

:deep(.msg__latency-tooltip-arrow) {
  fill: var(--color-bg-surface);
  stroke: var(--color-bg-border);
}

@keyframes msg-latency-tooltip-enter {
  from {
    opacity: 0;
    transform: translateY(2px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
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
  font-weight: 600;
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
  font-weight: 600;
}

.msg__markdown :deep(em) {
  font-style: italic;
}

.msg__markdown :deep(code) {
  font-family: var(--font-mono);
  font-size: 0.9em;
  padding: 1px 5px;
  border-radius: 3px;
  background: rgba(255, 255, 255, 0.08);
  border: 1px solid var(--color-bg-border-strong);
}

.msg__markdown :deep(pre) {
  margin: 8px 0;
  padding: 10px 12px;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid var(--color-bg-border-strong);
  border-radius: 6px;
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
  font-weight: 600;
}
</style>
