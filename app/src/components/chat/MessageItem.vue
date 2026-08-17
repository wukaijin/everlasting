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

import { computed, watch, onUnmounted } from "vue";
import type { ChatMessage } from "../../stores/chat.types";
import { useChatStore } from "../../stores/chat";
import { useStreamControllerStore } from "../../stores/streamController";
import { askCardPropsFor as askCardPropsResolved } from "./messageCards/askCard";
import { modeChangeCardPropsFor as modeChangeCardPropsResolved } from "./messageCards/modeChangeCard";
import { taskStateTransitionCardPropsFor as taskStateTransitionCardPropsResolved } from "./messageCards/taskStateTransitionCard";
import { buildTimeline, shouldUseTimeline, speakerAccentOf, speakerLabelOf, showSpeakerChipFor } from "./messageTimeline";
import { useMessageEditing } from "./useMessageEditing";
import { getToolResult } from "../../utils/messageFormat";
import { createDebouncedRenderer } from "../../utils/markdown";
import ThinkingBlock from "./ThinkingBlock.vue";
import ToolCallCard from "./ToolCallCard.vue";
import DiscussionSummaryCard from "./DiscussionSummaryCard.vue";
import AskUserQuestionCard from "./AskUserQuestionCard.vue";
import RequestModeChangeCard from "./RequestModeChangeCard.vue";
import RequestTaskStateTransitionCard from "./RequestTaskStateTransitionCard.vue";
import UiCard from "./UiCard.vue";
import FileInjectionsHint from "./FileInjectionsHint.vue";
import MessageImages from "./MessageImages.vue";
import MessageActionsMenu from "./MessageActionsMenu.vue";
import MessageItemEdit from "./MessageItemEdit.vue";
import MessageItemFooter from "./MessageItemFooter.vue";
import Icon from "../Icon.vue";

import { USE_UI_TOOL_NAME } from "./uiCard.types";


const props = defineProps<{
  message: ChatMessage;
  /** D2 (08-17-cross-session-search): read-only render context (the
   *  SearchModal's session preview). Structurally disables the
   *  hover `<MessageActionsMenu>` — NOT via CSS: the menu's
   *  Edit/Resend actions call the CURRENT session's store actions,
   *  which in a preview of another session would fire against the
   *  wrong session. */
  readonly?: boolean;
}>();

const chatStore = useChatStore();
const controller = useStreamControllerStore();

// Group-chat closing tool (08-07-group-chat-role-history-isolation
// follow-up): its `summary` is the discussion's final conclusion — render
// a visible DiscussionSummaryCard instead of a plain ToolCallCard.
const END_DISCUSSION_TOOL_NAME = "end_discussion";

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


// --- 提取模块的组件侧包装(08-07-large-file-splitting) ---
const renderTimeline = computed(() => buildTimeline(props.message, rendered.value));
const useTimeline = computed(() => shouldUseTimeline(props.message));
const speakerLabel = computed(() => speakerLabelOf(props.message));
const speakerAccent = computed(() => speakerAccentOf(props.message));
const showSpeakerChip = computed(() => showSpeakerChipFor(props.message));
const askCardPropsFor = (tc: { id: string; name: string }) => askCardPropsResolved(props.message, tc);
const modeChangeCardPropsFor = (tc: { id: string; name: string }) => modeChangeCardPropsResolved(props.message, tc);
const taskStateTransitionCardPropsFor = (tc: { id: string; name: string }) => taskStateTransitionCardPropsResolved(props.message, tc);
const {
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
} = useMessageEditing(
  () => props.message,
  () => isStreaming.value,
);

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

// --- B1 (2026-08-16) R2a: user-turn attachment thumbnails ---------------
// Map `message.metadata.attachments` into the `MessageImages` entry
// shape. Two producers write the manifest (see `AttachmentView` in
// `chat.types.ts`): the optimistic camelCase form from
// `chatSendActions.send` (`file` + `localUrl` + `mediaType`) and the
// rehydrated snake_case backend form (`file` + `media_type`). We
// accept both — `file` present → the thumbnail resolves via the
// daemon GET route; only-`localUrl` (pre-upload blob) renders the
// blob URL. Entries with neither are dropped (nothing to show).
const messageImages = computed<
  Array<{ file?: string; localUrl?: string; mediaType: string }>
>(() => {
  const m = props.message;
  if (m.role !== "user") return [];
  const raw = m.metadata?.attachments;
  if (!Array.isArray(raw)) return [];
  const out: Array<{ file?: string; localUrl?: string; mediaType: string }> =
    [];
  for (const r of raw) {
    if (!r || typeof r !== "object") continue;
    const o = r as Record<string, unknown>;
    const file =
      typeof o.file === "string" && o.file.length > 0 ? o.file : undefined;
    const localUrl =
      typeof o.localUrl === "string" && o.localUrl.length > 0
        ? o.localUrl
        : undefined;
    if (!file && !localUrl) continue;
    const mediaType =
      typeof o.media_type === "string"
        ? o.media_type
        : typeof o.mediaType === "string"
          ? o.mediaType
          : "";
    out.push({ file, localUrl, mediaType });
  }
  return out;
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
      v-if="message.seq !== undefined && !props.readonly"
      :message-seq="message.seq"
      :session-id="chatStore.currentSessionId ?? ''"
      :content="message.content"
      :role="message.role"
      :is-editing="isEditingThisMessage"
      :is-streaming="isStreaming"
      @edit="onEdit"
      @resend="onResend"
    />

    <!--
      Group chat (07-29-group-chat, Phase 4 Step 4 TODO-F1):
      speaker chip. Renders only when `message.speaker` is set
      AND the row is an assistant message (user rows are human
      by definition in any session type, so no chip). The chip
      follows the assistant bubble — appears at the top of the
      row, above the ThinkingBlock.

      Visual: small pill with the speaker's display name +
      accent color. Moderator gets a neutral color + a fixed
      "主持人" label (so the user can always tell who
      arbitrating). Participants get a hash-derived palette
      color (deterministic — same name = same color across
      reloads + sessions).
    -->
    <div
      v-if="showSpeakerChip"
      class="msg-speaker-chip"
      :class="`msg-speaker-chip--${speakerAccent}`"
      :data-testid="`msg-speaker-chip-${message.seq}`"
      :data-speaker="message.speaker"
    >
      <span class="msg-speaker-chip__dot" aria-hidden="true" />
      <span class="msg-speaker-chip__label">{{ speakerLabel }}</span>
    </div>

    <ThinkingBlock
      v-if="
        message.role === 'assistant' &&
        !useTimeline &&
        message.thinkingBlocks &&
        message.thinkingBlocks.length
      "
      :blocks="message.thinkingBlocks"
      :streaming="message.streaming"
      :show-streaming-hint="showStreamingHint"
      :thinking-duration-ms="message.thinkingDurationMs"
    />

    <!--
      交错思考: contentBlocks 时间轴(reload 后有 contentBlocks 时启用)。
      按 LLM 真实流序渲染 thinking + text 块,思考穿插在文本之间。
      每个 thinking 块独立折叠(ThinkingBlock 接收单块 blocks 数组);
      text 块各自 markdown 渲染。回退路径(useTimeline=false)不进这里,
      走顶部 ThinkingBlock + msg__bubble 的旧行为。
    -->
    <template v-if="useTimeline">
      <template v-for="(item, idx) in renderTimeline" :key="idx">
        <ThinkingBlock
          v-if="item.kind === 'thinking'"
          :blocks="item.blocks"
          :streaming="message.streaming"
          :show-streaming-hint="showStreamingHint"
          :thinking-duration-ms="message.thinkingDurationMs"
        />
        <!--
          交错思考: tool_use 在 timeline 内按真实流序渲染(穿插在 thinking/
          text 之间)。复用 ToolCallCard + 4 个 inline 卡片(resolver 在本
          组件 setup 内,timeline 与原 msg__tools 共享同一套渲染逻辑)。
          每个 tool_use 后面紧跟其 inline 卡片(ask_user_question 等),
          与原 msg__tools 的 `<template v-for tc>` 结构一致。
        -->
        <template v-else-if="item.kind === 'tool_use'">
          <DiscussionSummaryCard
            v-if="item.name === END_DISCUSSION_TOOL_NAME"
            :call="item"
            :result="getToolResult(message, item.id)"
          />
          <ToolCallCard
            v-else
            :call="item"
            :result="getToolResult(message, item.id)"
          />
          <AskUserQuestionCard
            v-if="askCardPropsFor(item) !== undefined"
            v-bind="askCardPropsFor(item)!"
          />
          <RequestModeChangeCard
            v-if="modeChangeCardPropsFor(item) !== undefined"
            v-bind="modeChangeCardPropsFor(item)!"
          />
          <RequestTaskStateTransitionCard
            v-if="taskStateTransitionCardPropsFor(item) !== undefined"
            v-bind="taskStateTransitionCardPropsFor(item)!"
          />
          <UiCard v-if="item.name === USE_UI_TOOL_NAME" :call="item" />
        </template>
        <div v-else class="msg__bubble msg__bubble--timeline">
          <span
            class="msg__markdown"
            v-html="item.html"
          />
        </div>
      </template>
      <!-- 流式 cursor(实时态不进时间轴,这里仅 reload 后的静止态,
           但保留以兼容 useTimeline 为真且仍在 streaming 的边界)。 -->
      <span
        v-if="message.streaming"
        class="msg__cursor"
        aria-hidden="true"
        >▍</span
      >
      <!-- (edited) 标签:useTimeline 时文本已进时间轴,msg__bubble 不渲染,
           所以 edited 标签在这里单独补上(assistant 行)。 -->
      <span
        v-if="showEditedLabel"
        class="msg__edited"
        :title="`最后编辑于 ${editedAt}`"
        data-testid="msg-edited-label"
      >
        (edited)
      </span>
    </template>

    <!--
      A5+ (2026-07-04, R8): transient retry notice. While the
      agent loop's `LlmRetrySink` sleeps between retry attempts
      (Full Jitter backoff or honored retry-after advisory),
      the in-flight assistant placeholder carries a `retrying`
      object. We render a small chip above the bubble so the
      user understands the stream is paused, not dead — without
      this row a multi-second backoff looks identical to a
      frozen UI (the user can't tell whether to wait or ressend).

      Visibility rules:
        - Only when `retrying` is set (the controller clears it
          on the next `start` / `delta` / `done` / `error`, so
          the row naturally disappears the moment the retry
          resolves or fails terminally).
        - Assistant rows only (the field is never attached to
          user bubbles).
        - NOT persisted to DB: `rehydrateMessages` does not
          copy `retrying`, so a session reload drops the chip.

      The text is Chinese (对齐 L3b PR4 风格 — no i18n key, the
      project is single-locale). The arrow ↩ mirrors the chat
      affordance icon the user already knows from MessageActionsMenu.
    -->
    <div
      v-if="message.role === 'assistant' && message.retrying"
      class="msg__retrying"
      data-testid="msg-retrying"
      :title="`重试中 ${message.retrying.attempt}/${message.retrying.maxAttempts},${(message.retrying.waitMs / 1000).toFixed(1)}s 后重发`"
    >
      <Icon name="refresh" :size="12" icon-class="msg__retrying-icon" />
      <span class="msg__retrying-text">
        重试中 {{ message.retrying.attempt }}/{{ message.retrying.maxAttempts }},{{
          (message.retrying.waitMs / 1000).toFixed(1)
        }}s 后重发…({{ message.retrying.reason }})
      </span>
    </div>

    <!--
      08-07-group-chat-review-fixes R2: group-chat orchestrator
      boundary notice. When a group-chat discussion skips a turn or
      ends abnormally, `streamController`'s `done` handler attaches a
      `notice` string to the in-flight placeholder (mirroring the
      `retrying` pattern above). We render a muted row above the bubble
      so the user understands why a turn vanished or why the discussion
      stopped. NOT persisted (the controller / rehydrate both skip it),
      so a session reload drops the row — it is live-orchestration
      state only. Assistant rows only.
    -->
    <div
      v-if="message.role === 'assistant' && message.notice"
      class="msg__notice"
      data-testid="msg-notice"
    >
      <Icon name="info" :size="12" icon-class="msg__notice-icon" />
      <span class="msg__notice-text">{{ message.notice }}</span>
    </div>

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
      v-if="visibleToolCalls.length && !useTimeline"
      class="msg__tools"
    >
      <!--
        2026-06-30 Phase E (R22 / AC11): per-tool dispatch. The
        template iterates `visibleToolCalls` and renders each
        ToolCallCard; for `ask_user_question` blocks, an inline
        `<AskUserQuestionCard>` is mounted directly BELOW the
        matching ToolCallCard (sibling within the same
        `msg__tools` flex column). The card reuses the message
        stream's scroll / render lifecycle — no portal, no
        modal (per design §5.5 UI red line + AC10).

        The fragment (`<template v-for>`) keeps the AskUserQuestionCard
        OUTSIDE the ToolCallCard's DOM tree (so it doesn't
        collide with the card's own click handlers / animation
        state). The two are visually adjacent — the user sees
        one logical "ask" affordance composed of the tool
        metadata header (ToolCallCard) + the question body
        (AskUserQuestionCard).

        We render `<AskUserQuestionCard>` only when
        `resolveAskCardState` returns a non-null tuple —
        defensive guard against the brief window between
        tool_use emit and tool_result arrival where neither
        live pending nor DB result exists yet.
      -->
      <template v-for="tc in visibleToolCalls" :key="tc.id">
        <!--
          08-07-group-chat-role-history-isolation follow-up:
          end_discussion 的 summary 是讨论最终结论,渲染为可见的
          DiscussionSummaryCard;其余工具仍走 ToolCallCard。
        -->
        <DiscussionSummaryCard
          v-if="tc.name === END_DISCUSSION_TOOL_NAME"
          :call="tc"
          :result="getToolResult(message, tc.id)"
        />
        <ToolCallCard
          v-else
          :call="tc"
          :result="getToolResult(message, tc.id)"
        />
        <AskUserQuestionCard
          v-if="askCardPropsFor(tc) !== undefined"
          v-bind="askCardPropsFor(tc)!"
        />
        <!--
          2026-07-07 Phase D (`07-07-07-07-request-mode-change-tool`):
          per-tool dispatch for `request_mode_change`. Inline
          `<RequestModeChangeCard>` mounts directly BELOW the
          matching ToolCallCard (sibling within the same
          `msg__tools` flex column). Reuses the message
          stream's scroll / render lifecycle — no portal, no
          modal (per design §5.5 UI red line + AC10).
        -->
        <RequestModeChangeCard
          v-if="modeChangeCardPropsFor(tc) !== undefined"
          v-bind="modeChangeCardPropsFor(tc)!"
        />
        <!--
          2026-07-09 (`07-09-workflow-transition-card`): per-tool
          dispatch for `request_task_state_transition`. Inline
          `<RequestTaskStateTransitionCard>` mounts directly BELOW
          the matching ToolCallCard (sibling within the same
          `msg__tools` flex column). Same no-portal / no-modal UI
          red line as the two cards above.
        -->
        <RequestTaskStateTransitionCard
          v-if="taskStateTransitionCardPropsFor(tc) !== undefined"
          v-bind="taskStateTransitionCardPropsFor(tc)!"
        />
        <UiCard v-if="tc.name === USE_UI_TOOL_NAME" :call="tc" />
      </template>
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
        :message-seq="message.seq"
        :retry-loading="retryLoading"
        @retry="onRetry"
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

    <div v-else-if="showBubble && !useTimeline" class="msg__bubble">
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
      B1 (2026-08-16) R2a: per-user-turn image attachment strip.
      Renders one 64px thumbnail per image this turn carried (pasted
      uploads render optimistically from the same manifest the
      rehydrate path rewrites). Same level as FileInjectionsHint,
      directly below the bubble. `sessionId` follows the existing
      MessageActionsMenu pattern — the row's session is the chat
      store's current session (message rows are per-session lists).
    -->
    <MessageImages
      v-if="message.role === 'user' && messageImages.length > 0"
      :session-id="chatStore.currentSessionId ?? ''"
      :images="messageImages"
    />

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
      :message-seq="message.seq"
      :retry-loading="retryLoading"
      @retry="onRetry"
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

/* Group chat (07-29-group-chat, Phase 4 Step 4 TODO-F4):
   speaker chip. Small pill at the top of the row, before
   the ThinkingBlock / bubble. The chip pairs a 6px colored
   dot with the speaker's display name. Color comes from one
   of two buckets:
     - "neutral" (moderator): fixed accent, no palette.
     - "palette-N" (participant): one of the 8-color palette
       from `utils/colorTag.ts` (djb2 hash of the speaker
       name → N). Same name = same color across reloads +
       sessions, deterministic without needing a DB lookup.
   The chip is rendered only on assistant rows (the v-if
   guard on the template side skips user rows). */
.msg-speaker-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin: 0 0 6px;
  padding: 3px 10px;
  font-size: 11px;
  font-weight: 500;
  line-height: 1.4;
  border-radius: 999px;
  background: var(--ev-color-bg-input, #2a2a2a);
  color: var(--ev-color-text, #e0e0e0);
  border: 1px solid var(--ev-color-border, #444);
  align-self: flex-start;
}

.msg-speaker-chip__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--ev-color-border, #555);
  flex-shrink: 0;
}

.msg-speaker-chip__label {
  white-space: nowrap;
}

/* Moderator: fixed neutral accent so the role is visually
   distinct from participants (who each get a unique palette
   color). The dot uses the muted accent token. */
.msg-speaker-chip--neutral .msg-speaker-chip__dot {
  background: var(--ev-color-accent, #4a8eff);
}

/* Participant chips: 8 palette buckets matching
   `utils/colorTag.ts::COLOR_PALETTE`. Each bucket sets the
   dot color; the chip background stays neutral so the
   visual weight is dominated by the dot + label, not the
   chip background — keeps the chat readable. */
.msg-speaker-chip--palette-0 .msg-speaker-chip__dot { background: #d4826a; }
.msg-speaker-chip--palette-1 .msg-speaker-chip__dot { background: #6a9e7e; }
.msg-speaker-chip--palette-2 .msg-speaker-chip__dot { background: #6a82b5; }
.msg-speaker-chip--palette-3 .msg-speaker-chip__dot { background: #b56a9e; }
.msg-speaker-chip--palette-4 .msg-speaker-chip__dot { background: #8eb56a; }
.msg-speaker-chip--palette-5 .msg-speaker-chip__dot { background: #6ab5ae; }
.msg-speaker-chip--palette-6 .msg-speaker-chip__dot { background: #b5a06a; }
.msg-speaker-chip--palette-7 .msg-speaker-chip__dot { background: #9e6ab5; }

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

/*
  A5+ (2026-07-04): retry notice row. Visually a small inline chip
  above the bubble — same family as `msg__redacted` (dashed border,
  mono font, muted text) but with an amber/warning tint to signal
  "transient degraded state". Spins the icon via CSS animation so
  the user sees live progress (the icon's rotation period is
  decoupled from the wait_ms — it's just a "this is alive"
  affordance, not a precise countdown).
*/
.msg__retrying {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
  padding: 4px 10px;
  background: var(--color-bg-elevated);
  border: 1px dashed var(--color-status-warn, #f0ad4e);
  border-radius: var(--radius-md);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.msg__retrying-icon {
  flex-shrink: 0;
  color: var(--color-status-warn, #f0ad4e);
  animation: msg__retrying-spin 1.4s linear infinite;
}

@keyframes msg__retrying-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}

.msg__retrying-text {
  /* mono font already set on the parent; keep this span plain so
     the reason text wraps naturally on narrow viewports. */
  white-space: normal;
  word-break: break-word;
}

/* 08-07-group-chat-review-fixes R2: group-chat orchestrator notice.
   Mirrors the retrying chip's layout (inline-flex chip above the
   bubble) but uses a solid neutral border instead of the dashed warn
   orange — a notice is informational, not an in-flight retry. */
.msg__notice {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
  padding: 4px 10px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-border, var(--color-border-subtle));
  border-radius: var(--radius-md);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.msg__notice-icon {
  flex-shrink: 0;
  color: var(--color-text-muted);
}

.msg__notice-text {
  white-space: normal;
  word-break: break-word;
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

/* 交错思考: 时间轴内的 text 块气泡。去掉独立 border + 收紧 margin ——
   一个 turn 里可能有多个 text 块(被 thinking 穿插),每个都套独立气泡
   边框会割裂"连续流动"的观感。改为无边框的连续文本流,与上下 ThinkingBlock
   自然衔接。assistant 主题色继承自 `.msg--assistant .msg__bubble`。 */
.msg__bubble--timeline {
  border: none;
  background: transparent;
  margin-top: 2px;
  margin-bottom: 2px;
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
   match and just become dead code.

   08-14 ux-polish-r1 WP2 2.1(评审 B1/B2"密集感"):正文行高已 1.6
   (--leading-relaxed,research B1 验证结论,不动),密集感来自块级元素
   的垂直节奏 —— p 8px / 列表 6px / li 2px 过紧。本组规则把节奏打开一
   档并全部走 spacing token:
     p 8→12,li 2→4,ul/ol 6→4/12(尾元素清零),h* 12/6→16/4,
     blockquote 8→8/12(内 padding 4/12 走 token)。
   pre/table/hr 保持原值仅 token 化(代码块/表格不是密集感来源)。 */
.msg__markdown {
  display: block;
}

.msg__markdown :deep(p) {
  margin: 0 0 var(--space-3) 0;
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
  margin: var(--space-4) 0 var(--space-1) 0;
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
  margin: var(--space-1) 0 var(--space-3) 0;
  padding-left: var(--space-6);
}

.msg__markdown :deep(ul:last-child),
.msg__markdown :deep(ol:last-child) {
  margin-bottom: 0;
}

.msg__markdown :deep(li) {
  margin: var(--space-1) 0;
}

.msg__markdown :deep(li:last-child) {
  margin-bottom: 0;
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
  margin: var(--space-2) 0;
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
  color: var(--color-accent-text);
  text-decoration: underline;
  text-underline-offset: 2px;
}

.msg__markdown :deep(blockquote) {
  margin: var(--space-2) 0 var(--space-3) 0;
  padding: var(--space-1) var(--space-3);
  border-left: 3px solid var(--color-bg-border);
  color: var(--color-text-secondary);
  font-style: italic;
}

.msg__markdown :deep(hr) {
  border: 0;
  border-top: 1px solid var(--color-bg-border);
  margin: var(--space-3) 0;
}

.msg__markdown :deep(table) {
  border-collapse: collapse;
  margin: var(--space-2) 0;
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

/* S6a 消息气泡移动端微调(08-13-mobile-chat-view)。prd A5/C3:窄屏下大段
   纯文本缺视觉层次 → 气泡内 padding 收紧 + 段落间距/行高保持(正文行高
   已 1.6,见 .msg__bubble line-height:var(--leading-relaxed));气泡宽度上限
   从 75% 放宽到 88%,320px 下不至于太窄不可读。对比度不动设计 token
   (design §3.4:只让"段落间距 + 行高"拉开视觉层次)。桌面块零改动。 */
@media (max-width: 767px) {
  .msg {
    max-width: 88%;
  }
  .msg--user {
    margin-right: 8px;
  }
  .msg__bubble {
    padding: 6px 10px;
    margin-bottom: 4px;
    line-height: var(--leading-relaxed);
  }
  /* 时间轴 text 块是"连续文本流"(无边框、收紧 margin 的设计意图,
     见 .msg__bubble--timeline 注释),不受上面 margin-bottom 4px 影响 ——
     同特异性下源序靠后者胜,这里显式恢复 2px,保持交错思考时段的流动感。 */
  .msg__bubble--timeline {
    margin-bottom: 2px;
  }
}

/* S6a 窄屏再降级(D8/D9):360px 以下气泡 padding 再收紧(design §3.5,
   写在 767px 档之后,后者优先,天然覆盖)。 */
@media (max-width: 359px) {
  .msg__bubble {
    padding: 5px 8px;
  }
}
</style>
